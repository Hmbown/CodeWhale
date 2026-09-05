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
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wait_timeout::ChildExt;

#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::os::windows::io::FromRawHandle;
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
use crate::tools::resource_admission::{
    CommandExpense, HeavyCommandPermit, MemoryPressure, acquire_heavy_command_permit,
    infer_command_expense,
};
use crate::work_graph::{
    EvidenceKind, EvidenceRef, OperationIntent, OperationOwnerSnapshot, OwnerState,
    SharedWorkRuntime,
};
use crate::worker_profile::ShellPolicy;
use output::{
    BoundedOutputAccumulator, BoundedOutputSnapshot, RAW_STREAM_SETTLED_TAIL_BYTES,
    RawOutputBuffer, SharedRawOutput, new_shared_raw_output, tail_from_buffer, tail_text,
    take_delta_from_buffer,
};

const READONLY_ENV_MARKER: &str = "CODEWHALE_INTERNAL_READONLY_ARGV";

#[cfg(unix)]
static PENDING_PERSISTENT_PROCESS_GROUPS: std::sync::OnceLock<
    Mutex<std::collections::HashSet<u32>>,
> = std::sync::OnceLock::new();

#[cfg(unix)]
fn pending_persistent_process_groups() -> &'static Mutex<std::collections::HashSet<u32>> {
    PENDING_PERSISTENT_PROCESS_GROUPS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

#[cfg(unix)]
fn register_pending_persistent_process_group(process_group_id: u32) {
    let mut groups = pending_persistent_process_groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    groups.insert(process_group_id);
}

#[cfg(unix)]
fn unregister_pending_persistent_process_group(process_group_id: u32) {
    let mut groups = pending_persistent_process_groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    groups.remove(&process_group_id);
}

/// Kill services that were staged for ownership transfer but have not yet
/// been released. The process-wide signal path calls this immediately before
/// `process::exit`, where Rust destructors cannot run.
#[cfg(unix)]
pub(crate) fn abort_pending_persistent_process_groups_for_exit() {
    let groups = {
        let mut groups = pending_persistent_process_groups()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        groups.drain().collect::<Vec<_>>()
    };
    for process_group_id in groups {
        if let Ok(process_group_id) = i32::try_from(process_group_id) {
            // SAFETY: the id was captured from a child spawned with
            // `process_group(0)`. A negative pid targets that child's process
            // group, never Codewhale's own group.
            unsafe {
                libc::kill(-process_group_id, libc::SIGKILL);
            }
        }
    }
}

fn validate_shell_working_dir(path: &Path, inherited_session_workspace: bool) -> Result<()> {
    let metadata = std::fs::metadata(path).with_context(|| {
        let source = if inherited_session_workspace {
            "saved session workspace"
        } else {
            "requested working directory"
        };
        format!(
            "{source} is unavailable: {}. Restore or remap that directory, resume/fork the session from an existing workspace, or pass an explicit `working_dir`/`cwd` to exec_shell",
            path.display()
        )
    })?;
    if !metadata.is_dir() {
        let source = if inherited_session_workspace {
            "saved session workspace"
        } else {
            "requested working directory"
        };
        return Err(anyhow!(
            "{source} is not a directory: {}. Resume/fork from an existing workspace or pass an explicit `working_dir`/`cwd`",
            path.display()
        ));
    }
    Ok(())
}

/// Status of a shell process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShellStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

/// Result from a shell command execution.
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
    /// Immutable root session that launched the job. Empty legacy records are
    /// intentionally hidden from session-scoped completion drains.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_session_id: String,
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
    #[serde(default)]
    pub stdout_len: usize,
    #[serde(default)]
    pub stderr_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    pub linked_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_session_id: String,
}

/// Byte evidence captured alongside a bounded completion event. Exact unless
/// the stream exceeded the in-memory retention ceiling, in which case the
/// omission is declared per stream rather than presented as complete.
#[derive(Debug, Clone)]
pub(crate) struct ShellCompletionEvidence {
    pub event: ShellCompletionEvent,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_omitted: usize,
    stderr_omitted: usize,
}

impl ShellCompletionEvidence {
    /// Encode each stream losslessly. UTF-8 remains readable; arbitrary bytes
    /// use base64 so `retrieve_tool_result` can still recover exact output.
    pub(crate) fn artifact_bytes(&self) -> Vec<u8> {
        fn stream(bytes: &[u8], omitted: usize) -> serde_json::Value {
            let mut value = match std::str::from_utf8(bytes) {
                Ok(content) => serde_json::json!({
                    "encoding": "utf-8",
                    "byte_length": bytes.len(),
                    "content": content,
                }),
                Err(_) => serde_json::json!({
                    "encoding": "base64",
                    "byte_length": bytes.len(),
                    "content": base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
            };
            // Additive and only present when something was actually dropped, so
            // the common case stays byte-identical to the v1 artifact readers
            // already parse.
            if omitted > 0
                && let Some(object) = value.as_object_mut()
            {
                object.insert("leading_bytes_omitted".into(), omitted.into());
                object.insert(
                    "total_byte_length".into(),
                    bytes.len().saturating_add(omitted).into(),
                );
            }
            value
        }

        serde_json::json!({
            "schema": "codewhale.shell_completion.evidence.v1",
            "task_id": self.event.task_id,
            "command": self.event.command,
            "status": format!("{:?}", self.event.status),
            "exit_code": self.event.exit_code,
            "duration_ms": self.event.duration_ms,
            "stdout": stream(&self.stdout, self.stdout_omitted),
            "stderr": stream(&self.stderr, self.stderr_omitted),
        })
        .to_string()
        .into_bytes()
    }
}

// Keep the two inline streams at a 2 KiB combined hard ceiling. The durable
// artifact carries the exact bytes beyond these diagnostic tails.
const SHELL_COMPLETION_TAIL_BYTES: usize = 1_024;

/// How long a finished shell record stays listed in `/jobs`.
const FINISHED_SHELL_MAX_AGE: Duration = Duration::from_secs(3600);
/// Ceiling on finished records kept for the jobs panel. A long automation run
/// makes hundreds of `Bash` calls per hour; the panel is only useful for the
/// recent ones (#5472).
const MAX_FINISHED_SHELL_RECORDS: usize = 128;
/// Ceiling on bytes still held across all finished records. Each settled record
/// releases down to a 64 KiB tail, so this only binds when many large outputs
/// finish inside the same window.
const MAX_FINISHED_SHELL_BYTES: usize = 8 * 1024 * 1024;

fn bounded_completion_tail(buffer: &SharedRawOutput, max_bytes: usize) -> (usize, String) {
    let (total, candidate) = tail_from_buffer(buffer, max_bytes);
    if candidate.len() <= max_bytes {
        return (total, candidate);
    }
    let content_budget = max_bytes.saturating_sub(3);
    let mut start = candidate.len().saturating_sub(content_budget);
    while start < candidate.len() && !candidate.is_char_boundary(start) {
        start += 1;
    }
    (total, format!("...{}", &candidate[start..]))
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
impl ShellChild {
    fn process_id(&self) -> Option<u32> {
        match self {
            Self::Process(child) => Some(child.id()),
            #[cfg(not(target_env = "ohos"))]
            Self::Pty(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellOwnership {
    Managed,
    PersistPending,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentServiceReceipt {
    pub task_id: String,
    pub pid: u32,
    pub process_group_id: u32,
    pub ownership: String,
}

#[cfg(unix)]
fn signal_child_process_group(child: &Child, signal: libc::c_int) -> std::io::Result<()> {
    let pgid = child.id() as libc::pid_t;
    if pgid <= 0 {
        return Ok(());
    }

    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            // The group is already gone (or never formed); nothing to signal.
            Ok(())
        } else {
            Err(err)
        }
    }
}

#[cfg(unix)]
fn kill_child_process_group(child: &mut Child) -> std::io::Result<()> {
    let pgid = child.id() as libc::pid_t;
    if pgid <= 0 {
        return child.kill();
    }

    signal_child_process_group(child, libc::SIGKILL).or_else(|_| child.kill())
}

/// Bounded wait for the direct child to exit. Returns true once the child was
/// reaped (or the wait errored), false when the grace elapsed first. Unlike
/// `Child::wait`, this can never wedge the caller behind a child stuck in
/// uninterruptible sleep.
#[cfg(unix)]
fn wait_child_bounded(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Terminate a shell's whole process group with a bounded SIGTERM → SIGKILL
/// escalation (#52). The previous kill path SIGKILLed only the direct child
/// and then joined output-reader threads with no timeout, so the tool
/// returned whenever the command's descendants felt like exiting — observed
/// as a 120s foreground timeout returning after 300s. Every step here is
/// bounded: the tool returns at ~timeout + grace.
#[cfg(unix)]
fn terminate_child_process_group(child: &mut Child) -> std::io::Result<()> {
    // Cooperative stop first so shells and their children can run traps and
    // clean up; bounded so a SIGTERM-ignoring command cannot stall the caller.
    let _ = signal_child_process_group(child, libc::SIGTERM);
    if wait_child_bounded(child, KILL_TERM_GRACE) {
        // The leader exited on SIGTERM; descendants may linger, so SIGKILL
        // the rest of the group (ESRCH when it is already empty).
        kill_child_process_group(child)?;
        return Ok(());
    }
    kill_child_process_group(child)?;
    let _ = wait_child_bounded(child, KILL_REAP_GRACE);
    Ok(())
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
    {
        let _ = kill_child_process_group(child);
        let _ = wait_child_bounded(child, KILL_REAP_GRACE);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
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
    buffer: SharedRawOutput,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    // `RawOutputBuffer::append` enforces the in-flight ceiling
                    // here, at the only writer, so a chatty command cannot grow
                    // the process without bound while it runs (#5472). It
                    // returns false once the stream has been abandoned, which
                    // is this thread's only exit when a descendant holds the
                    // pipe open and EOF never arrives.
                    let keep_reading = buffer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .append(&chunk[..n]);
                    if !keep_reading {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn spawn_bounded_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
    output: Arc<Mutex<BoundedOutputAccumulator>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut guard = output.lock().unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = guard.append(&chunk[..n]) {
                        guard.record_error(&error);
                        return;
                    }
                }
                Err(error) => {
                    output
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .record_error(&error);
                    return;
                }
            }
        }
        let mut guard = output.lock().unwrap_or_else(|error| error.into_inner());
        if let Err(error) = guard.finish() {
            guard.record_error(&error);
        }
    })
}

#[cfg(unix)]
fn shared_output_pipe() -> io::Result<(File, File, File)> {
    let mut descriptors = [0; 2];
    // SAFETY: `pipe` initializes both descriptors on success. Each descriptor
    // is immediately transferred into exactly one owned `File`.
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful `pipe` returned two live, uniquely owned descriptors.
    let reader = unsafe { File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    let stderr_writer = writer.try_clone()?;
    Ok((reader, writer, stderr_writer))
}

#[cfg(windows)]
fn shared_output_pipe() -> io::Result<(File, File, File)> {
    let mut read_handle = std::ptr::null_mut();
    let mut write_handle = std::ptr::null_mut();
    // SAFETY: CreatePipe initializes both handles on success; ownership is
    // transferred to `File` immediately below.
    if unsafe {
        windows_sys::Win32::System::Pipes::CreatePipe(
            &mut read_handle,
            &mut write_handle,
            std::ptr::null(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful CreatePipe returned two live, uniquely owned handles.
    let reader = unsafe { File::from_raw_handle(read_handle.cast()) };
    let writer = unsafe { File::from_raw_handle(write_handle.cast()) };
    let stderr_writer = writer.try_clone()?;
    Ok((reader, writer, stderr_writer))
}

const SYNC_READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_NO_OUTPUT_AFTER: Duration = Duration::from_secs(60);

/// Grace between SIGTERM and SIGKILL on the shell kill path (timeout,
/// cancel, drop). Bounded so a SIGTERM-ignoring command is force-killed
/// instead of stalling the tool (#52).
#[cfg(unix)]
const KILL_TERM_GRACE: Duration = Duration::from_millis(500);
/// Bounded reap wait after SIGKILL; a child stuck in uninterruptible sleep
/// must not wedge the caller behind an unbounded `wait`.
#[cfg(unix)]
const KILL_REAP_GRACE: Duration = Duration::from_millis(1_000);
/// Bounded join for output-reader threads after the process group is killed.
/// A descendant that escaped the group (its own session/process group) keeps
/// its inherited pipe write-end open, so the reader cannot see EOF until that
/// descendant exits on its own — an unbounded join held the shell-manager
/// lock for minutes and overshot the tool timeout (#52).
const READER_JOIN_GRACE: Duration = Duration::from_millis(2_000);

fn spawn_sync_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Bounded, unlike the `read_to_end` this replaces (#5472 finding 2).
        // `recv_sync_reader_output` gives up after 5 s, but the thread lives as
        // long as the pipe does — an interactive command that keeps printing
        // grew this Vec without limit, for a result nobody was still waiting
        // for. The tail is what the caller renders, so keep the tail.
        let mut buf = RawOutputBuffer::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if !buf.append(&chunk[..n]) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        tx.send(buf.retained().to_vec()).ok();
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
    /// When the job reached a terminal status. A finished job reports the
    /// duration it finished with; without this, `started_at.elapsed()` kept
    /// growing and `/jobs` showed "2m 07s" for a 12-second command (#5478).
    finished_at: Option<Instant>,
    last_output_at: Instant,
    last_observed_output_len: usize,
    pub sandbox_type: SandboxType,
    pub linked_task_id: Option<String>,
    pub owner_agent: Option<ShellJobOwner>,
    owner_session_id: String,
    ownership: ShellOwnership,
    stdout_buffer: SharedRawOutput,
    stderr_buffer: Option<SharedRawOutput>,
    /// Lowercase `bash` streams one combined process pipe through a bounded
    /// small-contract-compatible accumulator while persisting the complete output.
    bounded_output: Option<Arc<Mutex<BoundedOutputAccumulator>>>,
    heavy_permit: Option<HeavyCommandPermit>,
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
    owner_session_id: String,
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
    /// Wall time to report: elapsed while running, frozen once finished.
    fn wall_duration(&self) -> Duration {
        self.finished_at
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started_at)
    }

    fn wall_millis(&self) -> u64 {
        u64::try_from(self.wall_duration().as_millis()).unwrap_or(u64::MAX)
    }

    /// Stamp the finish instant the first time a terminal status is observed.
    /// Idempotent: a later poll must not restate when the job ended.
    fn mark_finished(&mut self) {
        if self.finished_at.is_none() && self.status != ShellStatus::Running {
            self.finished_at = Some(Instant::now());
        }
    }

    /// Check if the process has completed and update status
    fn poll(&mut self) -> bool {
        self.refresh_output_activity();
        if self.status != ShellStatus::Running {
            self.mark_finished();
            self.publish_lifecycle_best_effort();
            return true;
        }

        #[cfg(unix)]
        let pending_process_group = (self.ownership == ShellOwnership::PersistPending)
            .then(|| self.child.as_ref().and_then(ShellChild::process_id))
            .flatten();
        let completed = if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = status.code;
                    self.status = if status.success {
                        ShellStatus::Completed
                    } else {
                        ShellStatus::Failed
                    };
                    self.heavy_permit.take();
                    self.collect_output();
                    true
                }
                Ok(None) => false, // Still running
                Err(_) => {
                    self.status = ShellStatus::Failed;
                    self.heavy_permit.take();
                    self.collect_output();
                    true
                }
            }
        } else {
            true
        };
        #[cfg(unix)]
        if completed && let Some(process_group_id) = pending_process_group {
            unregister_pending_persistent_process_group(process_group_id);
        }
        self.mark_finished();
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
        if let Some(output) = self.bounded_output.as_ref() {
            return output
                .lock()
                .map(|output| output.total_bytes())
                .unwrap_or(0);
        }
        let stdout_len = self
            .stdout_buffer
            .lock()
            .map(|data| data.total_len())
            .unwrap_or(0);
        let stderr_len = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.total_len()))
            .unwrap_or(0);
        stdout_len.saturating_add(stderr_len)
    }

    /// Drop everything but a bounded tail of both raw streams.
    ///
    /// Called only once a job is terminal **and** its bytes have already been
    /// delivered — either returned as the foreground tool result, or written to
    /// its durable completion artifact. Before that the full bytes are a
    /// contract; after it they are pure residency for the up-to-1 h the
    /// finished record stays listed, which is what took the owner's host to
    /// 11 GB of swap (#5472 finding 1).
    fn release_delivered_output(&mut self) {
        if self.status == ShellStatus::Running {
            return;
        }
        for buffer in [Some(&self.stdout_buffer), self.stderr_buffer.as_ref()]
            .into_iter()
            .flatten()
        {
            buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .release_to_tail(RAW_STREAM_SETTLED_TAIL_BYTES);
        }
    }

    /// Bytes still held in memory for this job, for the eviction accounting in
    /// [`ShellManager::cleanup`] and for tests that assert the bound.
    fn retained_output_bytes(&self) -> usize {
        let stdout = self
            .stdout_buffer
            .lock()
            .map(|data| data.retained().len())
            .unwrap_or(0);
        let stderr = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.retained().len()))
            .unwrap_or(0);
        stdout.saturating_add(stderr)
    }

    /// Collect output from the background threads
    fn collect_output(&mut self) {
        // Kill the whole process group before joining reader threads.
        // When the shell spawned persistent background jobs (e.g. `nohup curl`),
        // those subprocesses keep the pipe write-ends open after the shell exits.
        // Without this kill, the reader join would block until the descendant
        // exits, freezing the UI event loop that calls list_jobs() → poll() →
        // collect_output(). The joins themselves are additionally bounded
        // (READER_JOIN_GRACE) because a descendant in its own session/process
        // group escapes even the group kill (#52).
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
            finish_background_reader(handle, &self.status, Some(&self.stdout_buffer));
        }
        if let Some(handle) = self.stderr_thread.take() {
            finish_background_reader(handle, &self.status, self.stderr_buffer.as_ref());
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
        if let Some(snapshot) = self.bounded_output_snapshot(false).ok().flatten() {
            return (snapshot.content, String::new(), snapshot.total_bytes, 0);
        }
        let (stdout_bytes, stderr_bytes, stdout_omitted, stderr_omitted) =
            self.retained_output_bytes_with_omissions();
        // Report what the stream produced, not what is still held.
        let stdout_len = stdout_bytes.len().saturating_add(stdout_omitted);
        let stderr_len = stderr_bytes.len().saturating_add(stderr_omitted);

        (
            String::from_utf8_lossy(&stdout_bytes).to_string(),
            String::from_utf8_lossy(&stderr_bytes).to_string(),
            stdout_len,
            stderr_len,
        )
    }

    /// Retained bytes for both streams plus how many leading bytes the memory
    /// bound discarded. Callers that publish these bytes as evidence must
    /// declare the omission rather than presenting a clipped stream as exact.
    fn retained_output_bytes_with_omissions(&self) -> (Vec<u8>, Vec<u8>, usize, usize) {
        if let Some(snapshot) = self.bounded_output_snapshot(false).ok().flatten() {
            let omitted = snapshot.total_bytes.saturating_sub(snapshot.retained_bytes);
            return (snapshot.content.into_bytes(), Vec::new(), omitted, 0);
        }
        let (stdout_bytes, stdout_omitted) = self
            .stdout_buffer
            .lock()
            .map(|data| (data.retained().to_vec(), data.dropped()))
            .unwrap_or_default();
        let (stderr_bytes, stderr_omitted) = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| {
                buffer
                    .lock()
                    .ok()
                    .map(|data| (data.retained().to_vec(), data.dropped()))
            })
            .unwrap_or_default();
        (stdout_bytes, stderr_bytes, stdout_omitted, stderr_omitted)
    }

    fn take_delta(&mut self) -> (String, String, usize, usize, usize, usize) {
        if let Some(snapshot) = self.bounded_output_snapshot(false).ok().flatten() {
            let changed = snapshot.total_bytes != self.stdout_cursor;
            self.stdout_cursor = snapshot.total_bytes;
            if changed {
                self.last_output_at = Instant::now();
                self.last_observed_output_len = snapshot.total_bytes;
                let delta_len = snapshot.content.len();
                return (
                    snapshot.content,
                    String::new(),
                    delta_len,
                    0,
                    snapshot.total_bytes,
                    0,
                );
            }
            return (String::new(), String::new(), 0, 0, snapshot.total_bytes, 0);
        }
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
        #[cfg(unix)]
        if self.ownership == ShellOwnership::PersistPending
            && let Some(process_group_id) = self.child.as_ref().and_then(ShellChild::process_id)
        {
            unregister_pending_persistent_process_group(process_group_id);
        }
        if let Some(ref mut child) = self.child {
            match child {
                ShellChild::Process(proc) => {
                    #[cfg(windows)]
                    {
                        terminate_windows_job(self.windows_job.as_ref(), proc)
                            .context("Failed to kill process tree")?;
                        let _ = proc.wait();
                    }
                    #[cfg(all(not(windows), unix))]
                    {
                        // Bounded SIGTERM → SIGKILL escalation against the
                        // whole process group; returns within ~grace even if
                        // the command ignores SIGTERM (#52).
                        terminate_child_process_group(proc).context("Failed to kill process")?;
                    }
                    #[cfg(all(not(windows), not(unix)))]
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
        self.mark_finished();
        self.heavy_permit.take();
        self.collect_output();
        self.publish_lifecycle_best_effort();
        Ok(())
    }

    /// Get a snapshot of the current state
    #[allow(dead_code)]
    pub fn snapshot(&self) -> Result<ShellResult> {
        let sandboxed = !matches!(self.sandbox_type, SandboxType::None);
        if let Some(snapshot) = self.bounded_output_snapshot(self.status != ShellStatus::Running)? {
            return Ok(ShellResult {
                task_id: Some(self.id.clone()),
                status: self.status.clone(),
                exit_code: self.exit_code,
                stdout: snapshot.content,
                stderr: String::new(),
                duration_ms: self.wall_millis(),
                stdout_len: snapshot.total_bytes,
                stderr_len: 0,
                stdout_omitted: snapshot.total_bytes.saturating_sub(snapshot.retained_bytes),
                stderr_omitted: 0,
                stdout_truncated: snapshot.truncated,
                stderr_truncated: false,
                sandboxed,
                sandbox_type: sandboxed.then(|| self.sandbox_type.to_string()),
                sandbox_denied: false,
            });
        }
        let (stdout_full, stderr_full, stdout_total, stderr_total) = self.full_output();
        let (stdout, stdout_meta) = truncate_with_meta(&stdout_full);
        let (stderr, stderr_meta) = truncate_with_meta(&stderr_full);
        // `truncate_with_meta` can only see the bytes still held. Fold in what
        // the in-memory bound dropped so a >16 MiB stream reports its real
        // length and its real omission instead of silently shrinking (#5472).
        let stdout_dropped = stdout_total.saturating_sub(stdout_meta.original_len);
        let stderr_dropped = stderr_total.saturating_sub(stderr_meta.original_len);
        Ok(ShellResult {
            task_id: Some(self.id.clone()),
            status: self.status.clone(),
            exit_code: self.exit_code,
            stdout,
            stderr,
            duration_ms: self.wall_millis(),
            stdout_len: stdout_total,
            stderr_len: stderr_total,
            stdout_omitted: stdout_meta.omitted.saturating_add(stdout_dropped),
            stderr_omitted: stderr_meta.omitted.saturating_add(stderr_dropped),
            stdout_truncated: stdout_meta.truncated || stdout_dropped > 0,
            stderr_truncated: stderr_meta.truncated || stderr_dropped > 0,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(self.sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: self.sandbox_denied(),
        })
    }

    fn bounded_output_snapshot(&self, finalize: bool) -> Result<Option<BoundedOutputSnapshot>> {
        self.bounded_output
            .as_ref()
            .map(|output| {
                output
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .snapshot(finalize)
                    .map_err(anyhow::Error::from)
            })
            .transpose()
    }

    fn job_snapshot(&self) -> ShellJobSnapshot {
        // Use tail_from_buffer instead of full_output so we never clone the
        // entire accumulated stdout/stderr for display purposes.  full_output
        // is O(total_bytes_written), which caused the ShellManager mutex to be
        // held for an arbitrarily long time during list_jobs() calls from the
        // TUI event loop — freezing input handling on long automation runs.
        let (stdout_len, stdout_tail) =
            if let Some(snapshot) = self.bounded_output_snapshot(false).ok().flatten() {
                (snapshot.total_bytes, tail_text(&snapshot.content, 1_200))
            } else {
                tail_from_buffer(&self.stdout_buffer, 1200)
            };
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
            elapsed_ms: self.wall_millis(),
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
            owner_session_id: self.owner_session_id.clone(),
        }
    }

    fn completion_event(&self) -> ShellCompletionEvent {
        let snapshot = self.job_snapshot();
        let (stdout_len, stdout_tail) =
            if let Some(output) = self.bounded_output_snapshot(false).ok().flatten() {
                (
                    output.total_bytes,
                    tail_text(&output.content, SHELL_COMPLETION_TAIL_BYTES),
                )
            } else {
                bounded_completion_tail(&self.stdout_buffer, SHELL_COMPLETION_TAIL_BYTES)
            };
        let (stderr_len, stderr_tail) = self
            .stderr_buffer
            .as_ref()
            .map(|buffer| bounded_completion_tail(buffer, SHELL_COMPLETION_TAIL_BYTES))
            .unwrap_or((0, String::new()));
        ShellCompletionEvent {
            task_id: snapshot.id,
            command: snapshot.command,
            status: snapshot.status,
            exit_code: snapshot.exit_code,
            duration_ms: snapshot.elapsed_ms,
            stdout_tail,
            stderr_tail,
            stdout_len,
            stderr_len,
            evidence_ref: None,
            linked_task_id: snapshot.linked_task_id,
            owner_agent_id: snapshot.owner_agent_id,
            owner_agent_name: snapshot.owner_agent_name,
            owner_session_id: snapshot.owner_session_id,
        }
    }

    fn completion_evidence(&self) -> ShellCompletionEvidence {
        let event = self.completion_event();
        let (stdout, stderr, stdout_omitted, stderr_omitted) =
            self.retained_output_bytes_with_omissions();
        ShellCompletionEvidence {
            event,
            stdout,
            stderr,
            stdout_omitted,
            stderr_omitted,
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

fn finish_background_reader(
    handle: std::thread::JoinHandle<()>,
    status: &ShellStatus,
    buffer: Option<&SharedRawOutput>,
) {
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

    // Bounded join (#52): after the process group is killed the reader
    // normally sees EOF immediately, but a descendant that escaped the group
    // (its own session/process group) keeps its inherited pipe write-end
    // open, so the reader stays blocked until that descendant exits on its
    // own. Joining unboundedly froze the foreground shell — and, through the
    // shell-manager lock, every other shell — for minutes. On timeout the
    // join is handed to a helper thread and we return; the reader thread
    // still finishes on its own once the pipe finally closes.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    if done_rx.recv_timeout(READER_JOIN_GRACE).is_ok() {
        return;
    }
    // The reader is still blocked in `read()` on a pipe a descendant refuses to
    // close. Previously both it and the helper thread above stayed alive for the
    // life of the process, the reader still appending into a buffer nobody would
    // ever read (#5472 finding 2). Abandoning the stream releases what it holds
    // and gives the reader an exit on its next wakeup, which also lets the
    // helper's `join` return.
    if let Some(buffer) = buffer {
        buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .abandon();
    }
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.ownership == ShellOwnership::PersistPending
            && let Some(process_group_id) = self.child.as_ref().and_then(ShellChild::process_id)
        {
            unregister_pending_persistent_process_group(process_group_id);
        }
        if self.ownership != ShellOwnership::Released
            && self.status == ShellStatus::Running
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
            #[cfg(all(not(windows), unix))]
            {
                let _ = child.kill();
                match child {
                    ShellChild::Process(proc) => {
                        let _ = wait_child_bounded(proc, KILL_REAP_GRACE);
                    }
                    #[cfg(not(target_env = "ohos"))]
                    ShellChild::Pty(child) => {
                        let _ = child.wait();
                    }
                }
            }
            #[cfg(all(not(windows), not(unix)))]
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(all(unix, not(target_env = "ohos")))]
pub(crate) fn inherited_interactive_terminal_refusal() -> Option<&'static str> {
    Some(
        "Inherited interactive terminal takeover is unavailable on Unix because foreground TTY \
         ownership cannot be transferred safely. Use Bash with `background: true, tty: true`, \
         then continue it with `action: \"interact\"` and the returned `task_id`; alternatively \
         use `terminal/run` and `terminal/send`, or launch the command in a new terminal. For \
         non-interactive work, omit `interactive: true`.",
    )
}

#[cfg(all(unix, target_env = "ohos"))]
pub(crate) fn inherited_interactive_terminal_refusal() -> Option<&'static str> {
    Some(
        "Inherited interactive terminal takeover is unavailable on Unix because foreground TTY \
         ownership cannot be transferred safely. Launch the command in a new terminal, or omit \
         `interactive: true` for non-interactive work.",
    )
}

#[cfg(not(unix))]
pub(crate) fn inherited_interactive_terminal_refusal() -> Option<&'static str> {
    None
}

/// Manages background shell processes with optional sandboxing.
pub struct ShellManager {
    processes: HashMap<String, BackgroundShell>,
    stale_jobs: HashMap<String, ShellJobSnapshot>,
    default_workspace: PathBuf,
    sandbox_manager: SandboxManager,
    sandbox_policy: ExecutionSandboxPolicy,
    foreground_background_requested: bool,
    /// Directory for lowercase-`bash` complete-output spill files
    /// (`None` = process temp dir). Overridable so tests can fault-inject a
    /// missing/unwritable spill location.
    output_spill_dir: Option<PathBuf>,
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
    fn require_session_owner(&self, task_id: &str, active_session_id: &str) -> Result<()> {
        let owned = self.processes.get(task_id).is_some_and(|shell| {
            !active_session_id.is_empty() && shell.owner_session_id == active_session_id
        }) || self.stale_jobs.get(task_id).is_some_and(|job| {
            !active_session_id.is_empty() && job.owner_session_id == active_session_id
        });
        if owned {
            Ok(())
        } else {
            // Do not disclose whether the id exists in another session.
            Err(anyhow!("Job {task_id} not found"))
        }
    }

    /// Create a new `ShellManager` with default (no sandbox) policy.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            processes: HashMap::new(),
            stale_jobs: HashMap::new(),
            default_workspace: workspace,
            sandbox_manager: SandboxManager::new(),
            sandbox_policy: ExecutionSandboxPolicy::default(),
            foreground_background_requested: false,
            output_spill_dir: None,
        }
    }

    /// Point lowercase-`bash` complete-output spill files at `dir` instead of
    /// the process temp dir. Tests use a nonexistent dir to simulate a full or
    /// broken temp volume. (unix-only: the regression test that uses it drives
    /// a POSIX shell loop.)
    #[cfg(all(test, unix))]
    pub(crate) fn set_output_spill_dir_for_test(&mut self, dir: Option<PathBuf>) {
        self.output_spill_dir = dir;
    }

    /// Insert a finished job without spawning. Count-bound tests would
    /// otherwise pay for 100+ live shells.
    #[cfg(test)]
    pub(crate) fn seed_finished_record_for_test(&mut self, id: impl Into<String>, age: Duration) {
        let now = Instant::now();
        let started_at = now.checked_sub(age).unwrap_or(now);
        let id = id.into();
        self.processes.insert(
            id.clone(),
            BackgroundShell {
                id,
                command: String::new(),
                working_dir: self.default_workspace.clone(),
                status: ShellStatus::Completed,
                exit_code: Some(0),
                started_at,
                finished_at: Some(now),
                last_output_at: now,
                last_observed_output_len: 0,
                sandbox_type: SandboxType::None,
                linked_task_id: None,
                owner_agent: None,
                owner_session_id: String::new(),
                ownership: ShellOwnership::Managed,
                stdout_buffer: new_shared_raw_output(),
                stderr_buffer: Some(new_shared_raw_output()),
                bounded_output: None,
                heavy_permit: None,
                stdout_cursor: 0,
                stderr_cursor: 0,
                completion_reported: false,
                stdin: None,
                child: None,
                #[cfg(windows)]
                windows_job: None,
                stdout_thread: None,
                stderr_thread: None,
                work_lifecycle: None,
                lifecycle_seq: 0,
                last_lifecycle_status: None,
                last_lifecycle_bytes: 0,
            },
        );
    }

    /// Test-only observation of the workspace selected by runtime rebuilds.
    #[cfg(test)]
    pub(crate) fn default_workspace(&self) -> &Path {
        &self.default_workspace
    }

    /// Enable or disable bubblewrap passthrough (#2184).
    ///
    /// When enabled and `/usr/bin/bwrap` is executable on Linux, exec_shell
    /// commands are routed through bubblewrap for filesystem isolation.
    pub fn set_prefer_bwrap(&mut self, prefer: bool) {
        self.sandbox_manager.set_prefer_bwrap(prefer);
    }

    /// Set user-configured bwrap mount extensions (#5410): extra read-only
    /// roots and writable device nodes such as `/dev/null`.
    pub fn set_bwrap_extensions(&mut self, extensions: crate::sandbox::BwrapMountExtensions) {
        self.sandbox_manager.set_bwrap_extensions(extensions);
    }

    /// Forward the opt-in sandbox read deny-list (S1, #5568) to the sandbox
    /// manager; `~` prefixes expand there.
    pub fn set_denied_read_subpaths(&mut self, paths: Vec<std::path::PathBuf>) {
        self.sandbox_manager.set_denied_read_subpaths(paths);
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

    #[cfg(test)]
    pub(crate) fn foreground_background_requested_for_test(&self) -> bool {
        self.foreground_background_requested
    }

    fn clear_foreground_background_request(&mut self) {
        self.foreground_background_requested = false;
    }

    fn take_foreground_background_request(&mut self) -> bool {
        let requested = self.foreground_background_requested;
        self.foreground_background_requested = false;
        requested
    }

    /// Execute a shell command with stdin/TTY options plus an extra env-var map
    /// that is merged into the spawned process environment. Used by the
    /// `shell_env` hook injection path (#456).
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
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

    /// Launch a parent-owned job stamped to the immutable root session.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options_env_for_session(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
        owner_session_id: &str,
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
            None,
            owner_session_id.to_string(),
            None,
            None,
            false,
            (1_000, 600_000),
        )
    }

    /// Same as `execute_with_options_env`, with optional background-job owner
    /// attribution for sub-agent launched jobs.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
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
            String::new(),
            None,
            None,
            false,
            (1_000, 600_000),
        )
    }

    /// Test-only owner-aware launch with an explicit immutable session owner.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub fn execute_with_options_env_for_owner_and_session(
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
        owner_session_id: &str,
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
            owner_session_id.to_string(),
            None,
            None,
            false,
            (1_000, 600_000),
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
        owner_session_id: String,
        work_lifecycle: Option<ShellWorkLifecycle>,
        readonly_workspace: Option<&std::path::Path>,
        persist_pending: bool,
        timeout_bounds_ms: (u64, u64),
    ) -> Result<ShellResult> {
        // Log execution via ShellDispatcher when SHELL_DISPATCHER_LOG is set.
        crate::shell_dispatcher::ShellDispatcher::log_exec(command);

        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);
        validate_shell_working_dir(&work_dir, working_dir.is_none())?;

        let timeout_ms = timeout_ms.clamp(timeout_bounds_ms.0, timeout_bounds_ms.1);

        // Use override policy if provided, otherwise use the manager's policy
        let policy = policy_override.unwrap_or_else(|| self.sandbox_policy.clone());

        // Create command spec and prepare sandboxed environment
        let spec = if let Some(workspace) = readonly_workspace {
            if command.contains('|') {
                // An agent read-only pipeline: every segment was admitted by
                // `is_agent_readonly_shell_command` (no separators, redirects,
                // expansions, or subshells — only `|` between validated
                // segments), so a shell is needed solely to bind the segments
                // and report a failed stage through pipefail.
                let piped = format!("set -o pipefail; {command}");
                CommandSpec::shell(&piped, work_dir.clone(), Duration::from_millis(timeout_ms))
            } else {
                let (program, args) = hardened_readonly_argv(command)?;
                let program = resolve_readonly_program(&program, workspace)?;
                CommandSpec::program(
                    program
                        .to_str()
                        .ok_or_else(|| anyhow!("read-only executable path is not valid UTF-8"))?,
                    args,
                    work_dir.clone(),
                    Duration::from_millis(timeout_ms),
                )
            }
        } else {
            CommandSpec::shell(command, work_dir.clone(), Duration::from_millis(timeout_ms))
        };
        let spec = spec.with_policy(policy).with_env(extra_env);
        let exec_env = self.sandbox_manager.prepare(&spec);

        if background {
            let bounded_output = timeout_bounds_ms == (1, BASH_MAX_TIMEOUT_MS);
            self.spawn_background_sandboxed(
                command,
                &work_dir,
                &exec_env,
                None,
                stdin_data,
                tty,
                ShellSpawnContext {
                    owner_agent,
                    owner_session_id,
                    work_lifecycle,
                },
                persist_pending,
                bounded_output,
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

        // A new Unix process group that inherits the terminal is not its
        // foreground owner. Letting it read stdin triggers SIGTTIN; sharing
        // Codewhale's group instead would make cooked-mode Ctrl+C terminate
        // both parent and child. Until this path owns a complete POSIX job-
        // control lease, fail closed before spawning. Persistent PTY tools
        // already provide a safe interactive lane without taking over the
        // operator's live terminal.
        if let Some(message) = inherited_interactive_terminal_refusal() {
            return Err(anyhow!(message));
        }

        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);
        validate_shell_working_dir(&work_dir, working_dir.is_none())?;

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
        remove_readonly_redirect_env(&mut cmd, &exec_env.env);

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
    #[allow(clippy::too_many_arguments)]
    fn spawn_background_sandboxed(
        &mut self,
        original_command: &str,
        working_dir: &std::path::Path,
        exec_env: &ExecEnv,
        heavy_permit: Option<HeavyCommandPermit>,
        stdin_data: Option<&str>,
        tty: bool,
        spawn_context: ShellSpawnContext,
        persist_pending: bool,
        small_contract_mode: bool,
    ) -> Result<ShellResult> {
        let ShellSpawnContext {
            owner_agent,
            owner_session_id,
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

        let stdout_buffer = new_shared_raw_output();
        let stderr_buffer = if tty || persist_pending || small_contract_mode {
            None
        } else {
            Some(new_shared_raw_output())
        };
        // The spill file is best-effort: a full disk or exhausted descriptor
        // table must not make `echo ok` unrunnable (that is exactly how the
        // owner's session got wedged under swap exhaustion).
        let bounded_output = small_contract_mode.then(|| {
            Arc::new(Mutex::new(BoundedOutputAccumulator::new_in(
                self.output_spill_dir.as_deref(),
            )))
        });

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
        } else if persist_pending {
            let mut cmd = Command::new(program);
            crate::utils::suppress_console_window(&mut cmd);
            push_shell_args(&mut cmd, program, args);
            cmd.current_dir(working_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));
            remove_readonly_redirect_env(&mut cmd, &exec_env.env);

            let child = cmd.spawn().with_context(|| {
                format!("Failed to spawn persistent service: {original_command}")
            })?;
            (ShellChild::Process(child), None, None, None)
        } else {
            let mut cmd = Command::new(program);
            crate::utils::suppress_console_window(&mut cmd);
            push_shell_args(&mut cmd, program, args);
            cmd.current_dir(working_dir).stdin(Stdio::piped());
            let combined_reader = if small_contract_mode {
                let (reader, stdout, stderr) =
                    shared_output_pipe().context("Failed to create combined shell output pipe")?;
                cmd.stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
                Some(reader)
            } else {
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
                None
            };
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));
            remove_readonly_redirect_env(&mut cmd, &exec_env.env);

            let mut child = cmd
                .spawn()
                .with_context(|| format!("Failed to spawn background: {original_command}"))?;
            #[cfg(windows)]
            {
                windows_job = attach_windows_job(&child, original_command);
            }

            let stdin_handle = child.stdin.take().map(StdinWriter::Pipe);

            let (stdout_thread, stderr_thread) =
                if let (Some(reader), Some(output)) = (combined_reader, bounded_output.as_ref()) {
                    (
                        Some(spawn_bounded_reader_thread(reader, Arc::clone(output))),
                        None,
                    )
                } else {
                    let stdout_handle = child.stdout.take().ok_or_else(|| {
                        #[cfg(windows)]
                        terminate_unregistered_process(&mut child, windows_job.as_ref());
                        #[cfg(not(windows))]
                        terminate_unregistered_process(&mut child);
                        anyhow!("Failed to capture stdout")
                    })?;
                    let stderr_handle = child.stderr.take().ok_or_else(|| {
                        #[cfg(windows)]
                        terminate_unregistered_process(&mut child, windows_job.as_ref());
                        #[cfg(not(windows))]
                        terminate_unregistered_process(&mut child);
                        anyhow!("Failed to capture stderr")
                    })?;
                    (
                        Some(spawn_reader_thread(
                            stdout_handle,
                            Arc::clone(&stdout_buffer),
                        )),
                        stderr_buffer
                            .as_ref()
                            .map(|buffer| spawn_reader_thread(stderr_handle, Arc::clone(buffer))),
                    )
                };

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
            finished_at: None,
            last_output_at: started,
            last_observed_output_len: 0,
            sandbox_type,
            linked_task_id: None,
            owner_agent,
            owner_session_id,
            ownership: if persist_pending {
                ShellOwnership::PersistPending
            } else {
                ShellOwnership::Managed
            },
            stdout_buffer,
            stderr_buffer,
            bounded_output,
            heavy_permit,
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

        #[cfg(unix)]
        if persist_pending {
            let process_group_id = bg_shell
                .child
                .as_ref()
                .and_then(ShellChild::process_id)
                .ok_or_else(|| anyhow!("Persistent service has no process group id"))?;
            register_pending_persistent_process_group(process_group_id);
        }

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
        // Evict here, not only from `list_jobs()`: retention must not depend on
        // the user opening the jobs panel, and every spawn is exactly the moment
        // the previous calls' records became one call staler (#5472).
        self.cleanup(FINISHED_SHELL_MAX_AGE);

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
                return shell.snapshot();
            }
        } else {
            shell.poll();
        }

        shell.snapshot()
    }

    /// Poll a job and return only its status.
    ///
    /// The foreground wait loop ticks every 100 ms and discards the snapshot
    /// unless the job is terminal, but `get_output` → `snapshot` clones both
    /// raw buffers to build it. On a command printing 50 MB that was ~1.5 GB of
    /// allocate-and-drop churn per wait, all of it thrown away (#5472 finding 1,
    /// the transient term). Status is what the loop actually needs.
    fn poll_status(&mut self, task_id: &str) -> Result<ShellStatus> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.poll();
        Ok(shell.status.clone())
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

    pub fn write_stdin_for_session(
        &mut self,
        active_session_id: &str,
        task_id: &str,
        input: &str,
        close: bool,
    ) -> Result<()> {
        self.require_session_owner(task_id, active_session_id)?;
        self.write_stdin(task_id, input, close)
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

    fn attach_heavy_permit(&mut self, task_id: &str, permit: HeavyCommandPermit) -> Result<()> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.heavy_permit = Some(permit);
        Ok(())
    }

    /// Kill a running background process
    pub fn kill(&mut self, task_id: &str) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        shell.kill()?;
        shell.snapshot()
    }

    pub fn kill_for_session(
        &mut self,
        active_session_id: &str,
        task_id: &str,
    ) -> Result<ShellResult> {
        self.require_session_owner(task_id, active_session_id)?;
        self.kill(task_id)
    }

    /// Kill every currently running background shell process.
    #[cfg(test)]
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

    pub fn kill_running_for_session(
        &mut self,
        active_session_id: &str,
    ) -> Result<Vec<ShellResult>> {
        let ids = self
            .processes
            .iter()
            .filter(|(_, shell)| {
                shell.status == ShellStatus::Running
                    && !active_session_id.is_empty()
                    && shell.owner_session_id == active_session_id
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            results.push(self.kill(&id)?);
        }
        Ok(results)
    }

    /// Transfer every still-running `persist:true` process out of Codewhale's
    /// ownership. This is called only by the real headless exec host after the
    /// enclosing turn has completed successfully.
    #[cfg(unix)]
    pub fn commit_persistent_services(&mut self) -> Result<Vec<PersistentServiceReceipt>> {
        let mut ids = self
            .processes
            .iter()
            .filter(|(_, shell)| shell.ownership == ShellOwnership::PersistPending)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.sort();

        for id in &ids {
            let shell = self
                .processes
                .get_mut(id)
                .ok_or_else(|| anyhow!("Persistent service {id} disappeared before commit"))?;
            shell.poll();
            if shell.status != ShellStatus::Running {
                return Err(anyhow!(
                    "Persistent service {id} exited before ownership transfer (status {:?}, exit code {:?})",
                    shell.status,
                    shell.exit_code
                ));
            }
            if shell
                .child
                .as_ref()
                .and_then(ShellChild::process_id)
                .is_none()
            {
                return Err(anyhow!(
                    "Persistent service {id} has no releasable process id"
                ));
            }
        }

        let mut receipts = Vec::with_capacity(ids.len());
        for id in ids {
            let mut shell = self
                .processes
                .remove(&id)
                .ok_or_else(|| anyhow!("Persistent service {id} disappeared during commit"))?;
            let pid = shell
                .child
                .as_ref()
                .and_then(ShellChild::process_id)
                .ok_or_else(|| anyhow!("Persistent service {id} lost its process id"))?;
            unregister_pending_persistent_process_group(pid);
            shell.ownership = ShellOwnership::Released;
            shell.stdin = None;
            shell.heavy_permit.take();
            shell.work_lifecycle = None;
            receipts.push(PersistentServiceReceipt {
                task_id: id,
                pid,
                process_group_id: pid,
                ownership: "external".to_string(),
            });
        }
        Ok(receipts)
    }

    /// Kill only services waiting for a successful exec ownership transfer.
    /// Ordinary background jobs retain their existing manager lifetime.
    pub fn abort_persistent_services(&mut self) {
        let ids = self
            .processes
            .iter()
            .filter(|(_, shell)| shell.ownership == ShellOwnership::PersistPending)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            if let Err(error) = self.kill(&id) {
                tracing::warn!(shell_id = %id, %error, "failed to abort pending persistent service");
            }
        }
    }

    /// Poll a background process and return incremental output.
    #[cfg(test)]
    pub fn poll_delta(
        &mut self,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        self.get_output_delta(task_id, wait, timeout_ms)
    }

    pub fn poll_delta_for_session(
        &mut self,
        active_session_id: &str,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        self.require_session_owner(task_id, active_session_id)?;
        self.get_output_delta(task_id, wait, timeout_ms)
    }

    fn get_output_delta_for_session(
        &mut self,
        active_session_id: &str,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        self.require_session_owner(task_id, active_session_id)?;
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
        Err(anyhow!("Job {task_id} not found"))
    }

    pub fn inspect_job_for_session(
        &mut self,
        active_session_id: &str,
        task_id: &str,
    ) -> Result<ShellJobDetail> {
        self.require_session_owner(task_id, active_session_id)?;
        self.inspect_job(task_id)
    }

    /// List all live and known-stale background shell jobs for the TUI.
    pub fn list_jobs(&mut self) -> Vec<ShellJobSnapshot> {
        for shell in self.processes.values_mut() {
            shell.poll();
        }
        // Evict completed processes older than 1 hour to bound memory growth.
        self.cleanup(FINISHED_SHELL_MAX_AGE);

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

    pub fn list_jobs_for_session(&mut self, active_session_id: &str) -> Vec<ShellJobSnapshot> {
        if active_session_id.is_empty() {
            return Vec::new();
        }
        self.list_jobs()
            .into_iter()
            .filter(|job| job.owner_session_id == active_session_id)
            .collect()
    }

    /// Whether a finished parent-owned job's completion is waiting to be
    /// claimed. Unlike
    /// [`Self::may_have_undelivered_completion`] this polls, so it reports
    /// readiness the moment the process exits; the engine's idle shell wake
    /// uses it to fire exactly when evidence exists.
    #[cfg(test)]
    pub(crate) fn has_finished_unreported_jobs(&mut self) -> bool {
        self.processes.values_mut().any(|shell| {
            shell.poll();
            shell.owner_agent.is_none()
                && shell.status != ShellStatus::Running
                && !shell.completion_reported
        })
    }

    pub(crate) fn has_finished_unreported_jobs_for_session(
        &mut self,
        active_session_id: &str,
    ) -> bool {
        !active_session_id.is_empty()
            && self.processes.values_mut().any(|shell| {
                shell.poll();
                shell.owner_session_id == active_session_id
                    && shell.owner_agent.is_none()
                    && shell.status != ShellStatus::Running
                    && !shell.completion_reported
            })
    }

    /// Drain once-only completion events together with lossless stream bytes.
    /// The engine publishes the bytes outside this manager's mutex and puts
    /// only the bounded event plus resulting handle into model context.
    #[cfg(test)]
    pub(crate) fn drain_finished_jobs_with_evidence(&mut self) -> Vec<ShellCompletionEvidence> {
        self.drain_finished_jobs_with_evidence_inner(None)
    }

    pub(crate) fn drain_finished_jobs_with_evidence_for_session(
        &mut self,
        active_session_id: &str,
    ) -> Vec<ShellCompletionEvidence> {
        self.drain_finished_jobs_with_evidence_inner(Some(active_session_id))
    }

    fn drain_finished_jobs_with_evidence_inner(
        &mut self,
        active_session_id: Option<&str>,
    ) -> Vec<ShellCompletionEvidence> {
        let mut completions = Vec::new();
        for shell in self.processes.values_mut() {
            shell.poll();
            let owned = active_session_id.is_none_or(|session_id| {
                !session_id.is_empty() && shell.owner_session_id == session_id
            });
            if owned && shell.status != ShellStatus::Running && !shell.completion_reported {
                shell.completion_reported = true;
                completions.push(shell.completion_evidence());
                // The bytes are now in the caller's hands (they become a durable
                // session artifact). Holding a second copy here for the rest of
                // the retention hour is what #5472 measured.
                shell.release_delivered_output();
            }
        }
        completions.sort_by(|a, b| a.event.task_id.cmp(&b.event.task_id));
        completions
    }

    /// A terminal foreground result is already returned as the tool result;
    /// do not emit it again through the background-completion channel.
    fn acknowledge_foreground_completion(&mut self, task_id: &str) {
        if let Some(shell) = self.processes.get_mut(task_id) {
            shell.completion_reported = true;
            // The caller already holds this job's `ShellResult`; the record only
            // stays listed so `/jobs` can show it. A 1,200-char tail is all any
            // remaining consumer reads, so the rest is released now instead of
            // at the 1 h `cleanup` (#5472 finding 1 — the dominant term: every
            // uppercase `Bash` call, foreground included, went through here).
            shell.release_delivered_output();
        }
    }

    /// Whether the next production turn may inject a parent-owned shell
    /// completion event.
    ///
    /// This deliberately does not poll processes or flip
    /// `completion_reported`: preview is read-only. A running job counts as
    /// pending because it can finish before production drains completions; in
    /// that race an exact request body cannot be proved without mutation.
    #[cfg(test)]
    pub fn may_have_undelivered_completion(&self) -> bool {
        self.processes
            .values()
            .any(|shell| shell.owner_agent.is_none() && !shell.completion_reported)
    }

    pub fn may_have_undelivered_completion_for_session(&self, active_session_id: &str) -> bool {
        !active_session_id.is_empty()
            && self.processes.values().any(|shell| {
                shell.owner_session_id == active_session_id
                    && shell.owner_agent.is_none()
                    && !shell.completion_reported
            })
    }

    /// Return agent owners whose tracked shell work is still running. The
    /// engine uses this to keep a worker's heartbeat alive while its only
    /// pending work is an explicitly tracked background shell task.
    #[cfg(test)]
    pub fn running_owner_agent_ids(&mut self) -> Vec<String> {
        self.running_owner_agent_ids_inner(None)
    }

    pub fn running_owner_agent_ids_for_session(&mut self, active_session_id: &str) -> Vec<String> {
        self.running_owner_agent_ids_inner(Some(active_session_id))
    }

    fn running_owner_agent_ids_inner(&mut self, active_session_id: Option<&str>) -> Vec<String> {
        let mut owners = self
            .processes
            .values_mut()
            .filter_map(|shell| {
                shell.poll();
                (shell.status == ShellStatus::Running
                    && active_session_id.is_none_or(|session_id| {
                        !session_id.is_empty() && shell.owner_session_id == session_id
                    }))
                .then(|| {
                    shell
                        .owner_agent
                        .as_ref()
                        .map(|owner| owner.agent_id.clone())
                })
                .flatten()
            })
            .collect::<Vec<_>>();
        owners.sort();
        owners.dedup();
        owners
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
                owner_session_id: String::new(),
            },
        );
    }

    /// Clean up completed processes older than the given duration, then enforce
    /// the count and byte ceilings on what is left.
    ///
    /// Age alone is not a bound: it only fired from `list_jobs()`, so a session
    /// that never opened the jobs panel evicted nothing, and 500 finished
    /// records inside one hour were all retained regardless of size (#5472).
    pub fn cleanup(&mut self, max_age: Duration) {
        self.processes.retain(|_, shell| {
            if shell.status == ShellStatus::Running {
                true
            } else {
                shell.started_at.elapsed() < max_age
            }
        });
        self.enforce_finished_job_bounds();
    }

    /// Bytes still held across every tracked job. The retention bound in #5472
    /// is stated in these terms, so the tests assert on them directly.
    #[cfg(all(test, unix))]
    pub(crate) fn retained_output_bytes_total(&self) -> usize {
        self.processes
            .values()
            .map(BackgroundShell::retained_output_bytes)
            .fold(0, usize::saturating_add)
    }

    #[cfg(test)]
    pub(crate) fn tracked_job_count(&self) -> usize {
        self.processes.len()
    }

    /// Drop the oldest finished records once they exceed either ceiling.
    /// Running jobs are never evicted — killing the handle would orphan a live
    /// process — and a job whose completion has not been delivered yet is
    /// evicted last, since dropping it loses the only copy of its result.
    fn enforce_finished_job_bounds(&mut self) {
        let mut finished = self
            .processes
            .iter()
            .filter(|(_, shell)| shell.status != ShellStatus::Running)
            .map(|(id, shell)| {
                (
                    id.clone(),
                    shell.completion_reported,
                    shell.started_at,
                    shell.retained_output_bytes(),
                )
            })
            .collect::<Vec<_>>();
        let total_bytes: usize = finished
            .iter()
            .map(|(_, _, _, bytes)| *bytes)
            .fold(0, usize::saturating_add);
        if finished.len() <= MAX_FINISHED_SHELL_RECORDS && total_bytes <= MAX_FINISHED_SHELL_BYTES {
            return;
        }
        // Undelivered completions last, then oldest first.
        finished.sort_by(|a, b| a.1.cmp(&b.1).reverse().then_with(|| a.2.cmp(&b.2)));
        let mut remaining_count = finished.len();
        let mut remaining_bytes = total_bytes;
        for (id, _, _, bytes) in finished {
            if remaining_count <= MAX_FINISHED_SHELL_RECORDS
                && remaining_bytes <= MAX_FINISHED_SHELL_BYTES
            {
                break;
            }
            self.processes.remove(&id);
            remaining_count -= 1;
            remaining_bytes = remaining_bytes.saturating_sub(bytes);
        }
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
    SafetyLevel, analyze_command, extract_primary_command, is_agent_readonly_shell_command,
    is_github_readonly_command, is_parallel_readonly_command, normalize_windows_command_paths,
};
use crate::execpolicy::{ExecPolicyDecision, load_default_policy};
use crate::features::Feature;
use crate::tools::cargo_failure_summary::summarize_cargo_failure;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64, required_str, type_mismatch,
};
use async_trait::async_trait;
use serde_json::json;

const FOREGROUND_TIMEOUT_RECOVERY_HINT: &str = "Foreground Bash is for bounded commands. \
The timed-out process was killed; rerun long work as Bash action=\"run\" background=true, \
then poll with Bash action=\"wait\" task_id=\"<id>\".";

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

/// Coaching line when the execution sandbox denied a command and the
/// Plan-mode network hint did not already explain it. Most often a write under
/// a read-only posture: name the effective posture and the Ask-only retry shape
/// so other postures do not mistake it for autonomous authority.
fn shell_sandbox_denied_hint(context: &ToolContext, result: &ShellResult) -> Option<String> {
    if !result.sandbox_denied {
        return None;
    }
    let policy = context.elevated_sandbox_policy.as_ref()?;
    Some(format!(
        "The execution sandbox blocked this command. Effective sandbox posture: {}. [sandbox: Ask-only escalation — retry this exact command once with sandbox_permissions (the narrowest wider mode that suffices) + justification; the approval prompt asks the user]",
        policy.posture_label()
    ))
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

fn enforce_readonly_github_network_policy(
    command: &str,
    context: &ToolContext,
) -> Result<(), ToolError> {
    if !is_github_readonly_command(command) {
        return Ok(());
    }
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };

    use crate::network_policy::Decision;
    match decider.evaluate("api.github.com", "Bash") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(
            "Read-only GitHub CLI access to 'api.github.com' is blocked by the active network policy."
                .to_string(),
        )),
        Decision::Prompt => Err(ToolError::permission_denied(
            "Read-only GitHub CLI access to 'api.github.com' requires network approval; allow that host in the parent session or network policy before dispatching the scout."
                .to_string(),
        )),
    }
}

/// `exec_shell_input_is_parallel_readonly` with the agent-posture classifier:
/// same input-shape restrictions (run action only, no background/tty/stdin),
/// but commands are judged by [`is_agent_readonly_shell_command`] so
/// `ShellPolicy::ReadOnly` agents keep a usable inspection surface
/// (pipelines, globs, `git -C`, `find`, `sed -n`, `npm view`).
fn exec_shell_input_agent_readonly(input: &serde_json::Value) -> bool {
    if !exec_shell_input_is_parallel_readonly_shape(input) {
        return false;
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .expect("shape check established a command string");
    is_agent_readonly_shell_command(command)
}

/// `exec_shell_input_agent_readonly` is also the gate-side predicate for the
/// subagent posture check (#5426): the catalog carve-out that admits
/// canonical `bash` to Scout/Reviewer/Planner must judge the same call the
/// `BashTool::execute` `ShellPolicy::ReadOnly` branch will judge, so the
/// posture gate can admit a proven-readonly call without ever widening past
/// the execute-time refusal.
pub(crate) fn agent_readonly_bash_input(input: &serde_json::Value) -> bool {
    // Canonical lowercase `bash` advertises `timeout` in seconds, while the
    // internal executor contract uses `timeout_ms`. Normalize through the same
    // translator the concrete tool uses so posture, session approval, envelope,
    // and execute judge one input (#5595). Legacy/internal shapes fall back to
    // their existing direct classification.
    let translated = contract_bash_legacy_input(input).unwrap_or_else(|_| input.clone());
    exec_shell_input_agent_readonly(&translated)
}

fn exec_shell_input_is_parallel_readonly(input: &serde_json::Value) -> bool {
    if !exec_shell_input_is_parallel_readonly_shape(input) {
        return false;
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .expect("shape check established a command string");
    is_parallel_readonly_command(command)
}

fn exec_shell_input_is_parallel_readonly_shape(input: &serde_json::Value) -> bool {
    let Some(fields) = input.as_object() else {
        return false;
    };
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "action" | "command" | "cwd" | "timeout_ms"))
    {
        return false;
    }
    match input.get("action") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::String(action)) if action == "run" => {}
        Some(_) => return false,
    }
    if ["background", "interactive", "tty", "combined_output"]
        .iter()
        .any(|key| {
            !matches!(
                input.get(*key),
                None | Some(serde_json::Value::Null | serde_json::Value::Bool(false))
            )
        })
    {
        return false;
    }
    if ["stdin", "input", "data"]
        .iter()
        .any(|key| input.get(*key).is_some())
    {
        return false;
    }
    if ["task_id", "id", "wait", "block", "close_stdin", "all"]
        .iter()
        .any(|key| input.get(*key).is_some())
    {
        return false;
    }

    input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some()
}

fn hardened_readonly_argv(command: &str) -> Result<(String, Vec<String>)> {
    let mut argv = shell_words::split(&normalize_windows_command_paths(command))
        .map_err(|error| anyhow!("could not parse classifier-approved read command: {error}"))?;
    if argv.is_empty() {
        return Err(anyhow!("classifier-approved read command was empty"));
    }

    // Even when repository/user configuration names a diff or signature
    // helper, these flags make Git keep the read inside its own process.
    if argv.first().is_some_and(|program| program == "git") {
        // The agent read-only classifier admits `git -C <dir>` and
        // `git --no-pager` before the subcommand; keep the preamble but
        // locate the subcommand after it so the hardening flags splice in
        // the right place. `-C` targets were already workspace-checked by
        // `enforce_readonly_workspace_operands`.
        let mut subcommand_index = 1;
        while let Some(flag) = argv.get(subcommand_index) {
            match flag.as_str() {
                "--no-pager" => subcommand_index += 1,
                "-C" => subcommand_index += 2,
                _ => break,
            }
        }
        let subcommand = argv
            .get(subcommand_index)
            .map(String::as_str)
            .ok_or_else(|| {
                anyhow!("classifier-approved Git read was missing its literal subcommand")
            })?;
        match subcommand {
            "diff" => {
                let at = subcommand_index + 1;
                argv.splice(
                    at..at,
                    ["--no-ext-diff".to_string(), "--no-textconv".to_string()],
                );
            }
            "log" | "show" => {
                let at = subcommand_index + 1;
                argv.splice(
                    at..at,
                    [
                        "--no-ext-diff".to_string(),
                        "--no-textconv".to_string(),
                        "--no-show-signature".to_string(),
                    ],
                );
            }
            "status" | "ls-files" | "blame" | "grep" => {}
            _ => {
                return Err(anyhow!(
                    "classifier-approved Git read did not keep its subcommand in argv[1]"
                ));
            }
        }
    }

    let program = argv.remove(0);
    Ok((program, argv))
}

fn enforce_readonly_workspace_operands(
    command: &str,
    workspace: &std::path::Path,
    effective_cwd: &std::path::Path,
) -> Result<(), ToolError> {
    let argv = shell_words::split(&normalize_windows_command_paths(command)).map_err(|error| {
        ToolError::invalid_input(format!(
            "Could not parse read-only command arguments: {error}"
        ))
    })?;
    if argv.first().is_some_and(|program| program == "gh") {
        // High-level gh reads do not consume local path operands. Their host
        // is pinned and evaluated separately by the network-policy guard.
        return Ok(());
    }
    let workspace = workspace.canonicalize().map_err(|error| {
        ToolError::execution_failed(format!(
            "Could not resolve the Scout workspace before shell dispatch: {error}"
        ))
    })?;
    let effective_cwd = effective_cwd.canonicalize().map_err(|error| {
        ToolError::permission_denied(format!(
            "Could not prove the read-only shell working directory stays in the workspace: {error}"
        ))
    })?;
    if !effective_cwd.starts_with(&workspace) {
        return Err(ToolError::permission_denied(
            "[shell.readonly.cwd.outside_workspace] Read-only Scout shell working directory resolves outside the workspace.",
        ));
    }

    for token in argv.iter().skip(1) {
        if token.starts_with('-') && (token.contains('/') || token.contains('\\')) {
            return Err(ToolError::permission_denied(format!(
                "[shell.readonly.option.attached_path] Read-only Scout shell options may not carry attached paths; refused {token:?}. Use the bounded File read/search actions for project evidence."
            )));
        }
        let value = token
            .split_once('=')
            .map_or(token.as_str(), |(_, value)| value)
            .trim();
        // Windows `Path::canonicalize` returns verbatim device paths
        // (`\\?\C:\...`), which `Path::is_absolute` does not recognize; without
        // stripping the prefix the operand falls into the shape refusal and
        // every canonical absolute read is denied on Windows. The stripped
        // form resolves to the same location, so location-based judgement is
        // unchanged. On unix hosts such spellings stay fail-closed below.
        let value = value
            .strip_prefix(r"\\?\")
            .or_else(|| value.strip_prefix(r"\\.\"))
            .unwrap_or(value);
        if value.is_empty() || value == "-" {
            continue;
        }
        let candidate = std::path::Path::new(value);
        let bytes = value.as_bytes();
        let windows_prefixed =
            (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
                || value.starts_with("\\\\")
                || candidate
                    .components()
                    .any(|component| matches!(component, std::path::Component::Prefix(_)));

        // #5595: an absolute operand is safe by resolved location, not by
        // spelling. This is the canonical `git -C /absolute/workspace log`
        // case. Requiring canonicalization also keeps nonexistent output-like
        // targets fail-closed, while symlinks are judged by their destination.
        if candidate.is_absolute() {
            let resolved = candidate.canonicalize().map_err(|error| {
                ToolError::permission_denied(format!(
                    "[shell.readonly.operand.unresolved] Could not prove absolute read-only operand {value:?} stays inside the workspace because it could not be resolved: {error}"
                ))
            })?;
            if !resolved.starts_with(&workspace) {
                return Err(ToolError::permission_denied(format!(
                    "[shell.readonly.operand.outside_workspace] Read-only Scout shell operand {value:?} resolves outside the workspace. Use the bounded File read/search actions for project evidence."
                )));
            }
            continue;
        }

        if value.starts_with('~')
            || value.contains('\\')
            || windows_prefixed
            || candidate.has_root()
            || candidate
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ToolError::permission_denied(format!(
                "[shell.readonly.operand.shape] Read-only Scout shell operands must stay inside the workspace; refused {value:?}. Use the bounded File read/search actions for project evidence."
            )));
        }

        let joined = effective_cwd.join(candidate);
        if joined.exists() {
            let resolved = joined.canonicalize().map_err(|error| {
                ToolError::permission_denied(format!(
                    "[shell.readonly.operand.unresolved] Could not prove read-only operand {value:?} stays in the workspace: {error}"
                ))
            })?;
            if !resolved.starts_with(&workspace) {
                return Err(ToolError::permission_denied(format!(
                    "[shell.readonly.operand.outside_workspace] Read-only Scout shell operand {value:?} resolves outside the workspace. Use the bounded File read/search actions for project evidence."
                )));
            }
        }
    }
    Ok(())
}

fn readonly_sanitized_path_from(
    workspace: &std::path::Path,
    path: &std::ffi::OsStr,
) -> Option<std::ffi::OsString> {
    let workspace = workspace.canonicalize().ok()?;
    let safe = std::env::split_paths(path).filter_map(|entry| {
        if !entry.is_absolute() {
            return None;
        }
        let resolved = entry.canonicalize().ok()?;
        (!resolved.starts_with(&workspace)).then_some(resolved)
    });
    std::env::join_paths(safe).ok()
}

fn readonly_sanitized_path(workspace: &std::path::Path) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    readonly_sanitized_path_from(workspace, &path).map(|value| value.to_string_lossy().into_owned())
}

fn resolve_readonly_program(program: &str, workspace: &std::path::Path) -> Result<PathBuf> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| anyhow!("no executable search path is configured"))?;
    resolve_readonly_program_from_path(program, workspace, &path)
}

fn resolve_readonly_program_from_path(
    program: &str,
    workspace: &std::path::Path,
    path: &std::ffi::OsStr,
) -> Result<PathBuf> {
    let workspace = workspace.canonicalize()?;
    if std::path::Path::new(program).components().count() != 1 {
        return Err(anyhow!(
            "read-only command must name a bare allowlisted executable"
        ));
    }
    let safe_path = readonly_sanitized_path_from(&workspace, path).ok_or_else(|| {
        anyhow!("no trusted executable search path remains outside the workspace")
    })?;
    let names = if cfg!(windows) {
        vec![format!("{program}.exe"), format!("{program}.com")]
    } else {
        vec![program.to_string()]
    };
    for directory in std::env::split_paths(&safe_path) {
        for name in &names {
            let candidate = directory.join(name);
            if !candidate.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if candidate.metadata()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            let resolved = candidate.canonicalize()?;
            if resolved.is_absolute() && !resolved.starts_with(&workspace) {
                return Ok(resolved);
            }
        }
    }
    Err(anyhow!(
        "allowlisted read-only executable {program:?} was not found at a canonical path outside the workspace"
    ))
}

fn remove_readonly_redirect_env(cmd: &mut Command, env: &HashMap<String, String>) {
    if env.get(READONLY_ENV_MARKER).map(String::as_str) != Some("1") {
        return;
    }
    cmd.env_remove(READONLY_ENV_MARKER);
    let removals = cmd
        .get_envs()
        .filter_map(|(key, _)| {
            let upper = key.to_string_lossy().to_ascii_uppercase();
            let guarded = upper.starts_with("GIT_")
                || upper.starts_with("GH_")
                || upper.starts_with("GITHUB_");
            let safe = matches!(
                upper.as_str(),
                "GIT_OPTIONAL_LOCKS"
                    | "GIT_NO_LAZY_FETCH"
                    | "GIT_PAGER"
                    | "GIT_CONFIG_NOSYSTEM"
                    | "GIT_CONFIG_GLOBAL"
                    | "GIT_CONFIG_PARAMETERS"
                    | "GIT_EXTERNAL_DIFF"
                    | "GIT_ATTR_NOSYSTEM"
                    | "GIT_CONFIG_COUNT"
                    | "GH_PAGER"
                    | "GH_PROMPT_DISABLED"
                    | "GH_NO_UPDATE_NOTIFIER"
                    | "GH_HOST"
                    | "GH_REPO"
            ) || upper.starts_with("GIT_CONFIG_KEY_")
                || upper.starts_with("GIT_CONFIG_VALUE_");
            (guarded && !safe).then(|| key.to_os_string())
        })
        .collect::<Vec<_>>();
    for key in removals {
        cmd.env_remove(key);
    }
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

fn persistent_services_enabled_for(context: &ToolContext) -> bool {
    #[cfg(unix)]
    {
        context.persist_services_enabled
            && context.owner_agent_id.is_none()
            && context.tool_authority.is_none()
            && context.sandbox_backend.is_none()
            && matches!(context.shell_policy, ShellPolicy::Full)
            && matches!(
                context.elevated_sandbox_policy,
                Some(ExecutionSandboxPolicy::DangerFullAccess)
            )
    }
    #[cfg(not(unix))]
    {
        let _ = context;
        false
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_foreground_via_background(
    context: &ToolContext,
    command: &str,
    heavy_permit: Option<HeavyCommandPermit>,
    working_dir: Option<String>,
    timeout_ms: Option<u64>,
    stdin_data: Option<&str>,
    tty: bool,
    policy_override: Option<ExecutionSandboxPolicy>,
    extra_env: HashMap<String, String>,
    direct_argv: bool,
    timeout_bounds_ms: (u64, u64),
) -> Result<ShellResult> {
    let timeout_ms =
        timeout_ms.map(|timeout| timeout.clamp(timeout_bounds_ms.0, timeout_bounds_ms.1));
    let spawn_timeout_ms = timeout_ms.unwrap_or(timeout_bounds_ms.1);
    let spawned = {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.clear_foreground_background_request();
        let owner = shell_job_owner_from_context(context);
        let lifecycle = shell_work_lifecycle_from_context(context);
        manager.execute_with_options_env_for_owner_and_work(
            command,
            working_dir.as_deref(),
            spawn_timeout_ms,
            true,
            stdin_data,
            tty,
            policy_override,
            extra_env,
            owner,
            context.state_namespace.clone(),
            lifecycle,
            direct_argv.then_some(context.workspace.as_path()),
            false,
            timeout_bounds_ms,
        )?
    };
    let task_id = spawned
        .task_id
        .ok_or_else(|| anyhow!("foreground shell did not return a process id"))?;
    if let Some(permit) = heavy_permit {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.attach_heavy_permit(&task_id, permit)?;
    }

    if stdin_data.is_some() {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.write_stdin(&task_id, "", true)?;
    }

    let deadline = timeout_ms.map(|timeout| Instant::now() + Duration::from_millis(timeout));
    // Adaptive poll cadence: fast commands (the common case — grep, wc, echo)
    // finish in single-digit milliseconds, and a fixed 100ms tick made every
    // foreground call pay that full quantum before completion was noticed.
    // Start fine-grained and back off to the 100ms cap for long-running work.
    let mut poll_tick_ms: u64 = FOREGROUND_POLL_INITIAL_MS;
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
            let result = manager.kill(&task_id);
            if result.is_ok() {
                manager.acknowledge_foreground_completion(&task_id);
            }
            return result;
        }

        // Poll status only. The snapshot — and the buffer clones behind it — is
        // built once, when there is actually a result to return (#5472). Both
        // happen under one lock acquisition so the record cannot be evicted
        // between observing that it finished and reading its result.
        let finished = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            if manager.take_foreground_background_request() {
                return manager.get_output(&task_id, false, 0);
            }
            if manager.poll_status(&task_id)? == ShellStatus::Running {
                None
            } else {
                let snapshot = manager.get_output(&task_id, false, 0)?;
                // Ordering matters: the snapshot is taken before the
                // acknowledgement releases the retained bytes.
                manager.acknowledge_foreground_completion(&task_id);
                Some(snapshot)
            }
        };

        if let Some(snapshot) = finished {
            return Ok(snapshot);
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            let mut result = manager.kill(&task_id)?;
            manager.acknowledge_foreground_completion(&task_id);
            result.status = ShellStatus::TimedOut;
            return Ok(result);
        }

        tokio::time::sleep(Duration::from_millis(poll_tick_ms)).await;
        poll_tick_ms = (poll_tick_ms * 2).min(FOREGROUND_POLL_MAX_MS);
    }
}

const BASH_MAX_TIMEOUT_MS: u64 = i32::MAX as u64;

/// Initial cadence for foreground-via-background completion polling. Fast
/// commands dominate real agent traffic; detection latency on `true`-class
/// commands drops from ~100ms to ~10ms, while long commands reach the
/// 100ms cap within one doubling step.
const FOREGROUND_POLL_INITIAL_MS: u64 = 10;
/// Poll-cadence ceiling; matches the previous fixed tick so long-running
/// command overhead is unchanged.
const FOREGROUND_POLL_MAX_MS: u64 = 100;

/// Default foreground lifetime for a contract-`bash` `action=run` that names
/// no `timeout_ms`. Matches the value the tool's own input schema advertises;
/// before this existed the omitted case fell through to
/// `BASH_MAX_TIMEOUT_MS`.
const CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Resolve the lifetime for one `bash` run.
///
/// A foreground contract-`bash` run that names no timeout used to inherit
/// `BASH_MAX_TIMEOUT_MS` (~24.8 days), so a command that blocked on an
/// interactive prompt or a hung network call pinned the turn indefinitely —
/// the tool row just counted seconds while the model waited. The tool's own
/// schema already promises `action=run 120000`, and its description already
/// says foreground is for bounded commands, so honor that: an omitted
/// timeout takes the advertised default, which lets
/// `FOREGROUND_TIMEOUT_RECOVERY_HINT` kill the process and tell the model to
/// rerun with `background=true`.
///
/// An explicit `timeout_ms` is still honored up to the full contract ceiling,
/// and background and interactive runs keep their own lifetimes: their
/// processes are meant to outlive the call, so bounding them here would kill
/// long-lived jobs the model deliberately detached.
fn contract_bash_timeout_ms(
    optional_timeout: bool,
    requested_ms: Option<u64>,
    background: bool,
    interactive: bool,
) -> Option<u64> {
    if optional_timeout && requested_ms.is_none() && !background && !interactive {
        return Some(CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS);
    }
    requested_ms
}

fn contract_bash_error_status(result: &ShellResult, timeout_ms: Option<u64>) -> String {
    match result.status {
        ShellStatus::TimedOut => {
            let millis = timeout_ms.unwrap_or(BASH_MAX_TIMEOUT_MS);
            let seconds = if millis.is_multiple_of(1_000) {
                (millis / 1_000).to_string()
            } else {
                format!("{}", millis as f64 / 1_000.0)
            };
            format!("Command timed out after {seconds} seconds")
        }
        ShellStatus::Killed => "Command aborted".to_string(),
        ShellStatus::Failed | ShellStatus::Completed | ShellStatus::Running => format!(
            "Command exited with code {}",
            result.exit_code.unwrap_or(-1)
        ),
    }
}

fn finish_contract_bash_result(
    result: ShellResult,
    timeout_ms: Option<u64>,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let sandbox_denied_hint = shell_sandbox_denied_hint(context, &result);
    let mut output = result.stdout.clone();
    output.push_str(&result.stderr);
    if let Some(hint) = sandbox_denied_hint {
        output = if output.is_empty() {
            hint
        } else {
            format!("{hint}\n\n{output}")
        };
    }
    let metadata = json!({
        "evidence_routing": "inline", "exit_code": result.exit_code,
        "status": format!("{:?}", result.status), "duration_ms": result.duration_ms,
        "sandboxed": result.sandboxed, "sandbox_type": result.sandbox_type,
        "task_id": result.task_id, "backgrounded": result.status == ShellStatus::Running,
    });
    if result.status == ShellStatus::Running {
        let task_id = result.task_id.as_deref().unwrap_or("unknown");
        let partial = (!output.is_empty()).then(|| format!("\n\nOutput so far:\n{output}"));
        return Ok(ToolResult::success(format!(
            "Foreground shell wait moved to /jobs: {task_id}{}\n\nThe command is still running; completion will appear as a runtime event.",
            partial.as_deref().unwrap_or_default()
        )).with_metadata(metadata));
    }
    if result.status != ShellStatus::Completed {
        let status = contract_bash_error_status(&result, timeout_ms);
        return Err(ToolError::execution_failed(if output.is_empty() {
            status
        } else {
            format!("{output}\n\n{status}")
        }));
    }

    Ok(ToolResult::success(if output.is_empty() {
        "(no output)".to_string()
    } else {
        output
    })
    .with_metadata(metadata))
}

/// Small foreground-only shell surface shown to new model turns.
pub struct LowercaseBashTool;

#[async_trait]
impl ToolSpec for LowercaseBashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the workspace and return stdout and stderr. Output keeps the last 2000 lines or 50KB. An optional timeout is expressed in seconds; when omitted the command is killed after 120 seconds, so pass an explicit timeout for work expected to take longer. In Ask, after a sandbox denial, retry the exact command once with sandbox_permissions (the narrowest wider mode that suffices) and a one-sentence justification; the approval prompt asks the user."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute." },
                "timeout": { "type": "number", "description": "Optional timeout in seconds; when omitted the command is killed after 120 seconds." },
                "sandbox_permissions": {
                    "type": "string",
                    "enum": ["workspace-write", "danger-full-access"],
                    "description": "The wider sandbox mode this exact command needs. Use only as a one-shot retry after a sandbox denial; requires justification and user approval in Ask."
                },
                "justification": {
                    "type": "string",
                    "description": "Required with sandbox_permissions: one sentence explaining why this exact command needs wider access."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        BashTool::contract_delegate().capabilities()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    fn approval_requirement_for(&self, input: &serde_json::Value) -> ApprovalRequirement {
        let translated = contract_bash_legacy_input(input).unwrap_or_else(|_| input.clone());
        BashTool::contract_delegate().approval_requirement_for(&translated)
    }

    fn is_read_only_for(&self, input: &serde_json::Value) -> bool {
        contract_bash_legacy_input(input)
            .is_ok_and(|translated| BashTool::contract_delegate().is_read_only_for(&translated))
    }

    fn supports_parallel_for(&self, input: &serde_json::Value) -> bool {
        self.is_read_only_for(input)
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let translated = contract_bash_legacy_input(&input)?;
        BashTool::contract_delegate()
            .execute(translated, context)
            .await
    }
}

fn contract_bash_legacy_input(input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let object = input
        .as_object()
        .ok_or_else(|| ToolError::invalid_input("bash input must be an object"))?;
    let unexpected = object
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "command" | "timeout" | "sandbox_permissions" | "justification"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(ToolError::invalid_input(format!(
            "unexpected bash parameter(s): {}",
            unexpected.join(", ")
        )));
    }
    let command = required_str(input, "command")?;
    let mut translated = json!({"command": command});
    if let Some(timeout) = input.get("timeout") {
        let seconds = timeout.as_f64().ok_or_else(|| {
            ToolError::invalid_input("Invalid timeout: expected a finite number of seconds")
        })?;
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err(ToolError::invalid_input(
                "Invalid timeout: expected a positive finite number of seconds",
            ));
        }
        let millis = seconds * 1000.0;
        if millis > BASH_MAX_TIMEOUT_MS as f64 {
            return Err(ToolError::invalid_input(format!(
                "Invalid timeout: maximum is {} seconds",
                BASH_MAX_TIMEOUT_MS as f64 / 1000.0
            )));
        }
        translated["timeout_ms"] = json!((millis as u64).max(1));
    }
    for field in ["sandbox_permissions", "justification"] {
        if let Some(value) = input.get(field) {
            translated[field] = value.clone();
        }
    }
    Ok(translated)
}

/// Compatibility shell tool retained for saved v0.9.x transcripts and the
/// background/session control surface. It is hidden from new model catalogs.
pub struct BashTool {
    name: &'static str,
    forced_action: Option<&'static str>,
    read_only: bool,
    optional_timeout: bool,
}

pub(crate) fn readonly_bash_input_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["run"] },
            "command": { "type": "string", "description": "A classifier-approved read command" },
            "cwd": { "type": "string", "description": "Workspace-relative working directory" },
            "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (1000-600000)" }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

impl BashTool {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: false,
            optional_timeout: false,
        }
    }

    pub const fn read_only(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: true,
            optional_timeout: false,
        }
    }

    pub const fn alias(name: &'static str, action: &'static str) -> Self {
        Self {
            name,
            forced_action: Some(action),
            read_only: false,
            optional_timeout: false,
        }
    }

    const fn contract_delegate() -> Self {
        Self {
            name: "bash",
            forced_action: Some("run"),
            read_only: false,
            optional_timeout: true,
        }
    }
}

#[async_trait]
impl ToolSpec for BashTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        if self.read_only {
            "Inspect the workspace with the bounded read-only command subset. Commands run directly as argv, never through a shell; only action=run plus command, cwd, and timeout_ms are accepted."
        } else {
            "Execute a shell command in the workspace. Action \"run\" (default) executes a command; \"wait\" blocks for a background task until completion or timeout; \"interact\" sends stdin to a background task; \"cancel\" kills a background task. Pass wait=false for a nonblocking task snapshot. Foreground mode is for bounded commands; use background=true for work expected to take >5 seconds. Commands run via the user's login shell ($SHELL); when that shell is zsh, a bare word starting with `=` undergoes `=command` PATH expansion (e.g. `echo ===` fails) — quote such arguments, e.g. `echo '==='`."
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        if self.read_only {
            return readonly_bash_input_schema();
        }
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
                    "description": "Timeout in milliseconds. The default depends on the action: action=run 120000 (the standalone Bash tool caps it at 600000), action=wait 30000, action=interact 1000. A foreground action=run that omits this is bounded by that default and killed with a background-rerun hint; pass an explicit value for longer foreground work, or background=true. For action=wait, `timeout_secs` (seconds) and `timeout` (milliseconds) are accepted aliases."
                },
                "background": {
                    "type": "boolean",
                    "description": "Temporary background; killed at session exit. Surviving headless services need background:true,persist:true."
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Run interactively with terminal IO (default: false)"
                },
                "stdin": {
                    "type": "string",
                    "description": "Stdin data to send (action=run: before waiting; action=interact: to the background task). Also accepted as `input` or `data` — send only one."
                },
                "input": {
                    "type": "string",
                    "description": "Alias for `stdin`."
                },
                "data": {
                    "type": "string",
                    "description": "Alias for `stdin`."
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
                    "description": "Task ID for action=wait/interact/cancel. Also accepted as `id`."
                },
                "task_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "For action=wait: wait on several background task ids at once (alternative to `task_id`)."
                },
                "until": {
                    "type": "string",
                    "enum": ["any", "all"],
                    "description": "For action=wait with `task_ids`: return as soon as any task is terminal (`any`) or when every task is terminal (`all`, default)."
                },
                "id": {
                    "type": "string",
                    "description": "Alias for `task_id`."
                },
                "wait": {
                    "type": "boolean",
                    "description": "For action=wait, block until the task completes or timeout elapses (default: true). Pass false for a nonblocking snapshot; `block` is an accepted alias."
                },
                "close_stdin": {
                    "type": "boolean",
                    "description": "Close stdin after sending (action=interact)"
                },
                "all": {
                    "type": "boolean",
                    "description": "Cancel all running background tasks (action=cancel)"
                },
                "persist": {
                    "type": "boolean",
                    "description": "Keep this background service running after a successful headless exec (default: false). Requires background:true and explicit danger-full-access. Run the service itself in the foreground; do not use nohup or a trailing `&`."
                },
                "sandbox_permissions": {
                    "type": "string",
                    "enum": ["workspace-write", "danger-full-access"],
                    "description": "The wider sandbox mode this exact command needs. Use only as a one-shot retry after a sandbox denial; requires justification and user approval in Ask."
                },
                "justification": {
                    "type": "string",
                    "description": "Required with sandbox_permissions: one sentence explaining why this exact command needs wider access."
                }
            },
            // The schema used to declare nothing required at all, so
            // `Bash{}` was schema-valid for the tool that runs shell
            // commands. What is required is per-action and cannot be spelled
            // as a flat `required` list: `run` needs `command`,
            // `wait`/`interact`/`cancel` need `task_id` (or its `id` alias),
            // and `cancel` needs `all` instead when cancelling everything.
            // A root `anyOf` of `required` groups is how this repo already
            // spells that (`finance`, `apply_patch`), and `schema_sanitize`
            // knows the shape: providers that reject root composition get the
            // groups merged and the constraint restated as a description note
            // (`root_composition_constraint_note`). The cost is that
            // `strict_schema_supported` rejects a root `anyOf`, so `Bash`
            // opts out of DeepSeek strict mode — as `finance` already does on
            // the same default agent surface, which turns strict mode off for
            // the whole tool set regardless.
            "anyOf": [
                { "required": ["command"] },
                { "required": ["task_id"] },
                { "required": ["id"] },
                { "required": ["all"] }
            ]
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
        // `and_then(as_str).unwrap_or("run")` treated *any* non-string
        // `action` as absent and fell through to the branch that runs
        // arbitrary code: `Bash{action: 3, command: "…"}` executed the
        // command. Every sibling family refuses a non-string action
        // (`canonical_action::required_action`), and `Bash` cannot be the
        // lenient one. `optional_str` is the type-strictness lane's extractor:
        // absent or `null` takes the documented `run` default, anything else
        // is a `type_mismatch` naming the field and the type it needed.
        let action = match self.forced_action {
            Some(forced) => forced,
            None => optional_str(&input, "action")?.unwrap_or("run"),
        };
        match action {
            "wait" => return self.execute_wait(&input, context).await,
            "interact" => return self.execute_interact(&input, context).await,
            "cancel" => return self.execute_cancel(&input, context).await,
            "run" => {}
            // Bash was the only action wrapper whose catch-all fell through to
            // its most dangerous branch: `{"action":"kill", "command":…}` ran
            // the command instead of cancelling, and a mis-cased "Cancel" did
            // the same. Every sibling (`File`, `Git`, `Web`, `Run`) already
            // refuses an unknown action; the tool that executes arbitrary code
            // should not be the lenient one.
            other => {
                return Err(ToolError::invalid_input(format!(
                    "Unknown Bash action \"{other}\"; nothing was run. Pass one of: run, wait, interact, cancel."
                )));
            }
        }
        let command = required_str(&input, "command")?;
        match context.shell_policy {
            ShellPolicy::None => {
                return Ok(ToolResult::error(
                    "Shell tools are disabled by the active permission profile.",
                ));
            }
            ShellPolicy::ReadOnly if !exec_shell_input_agent_readonly(&input) => {
                return Ok(ToolResult::error(
                    "Shell command blocked by read-only shell policy. Use a non-mutating, non-background inspection command, or switch to Work mode (`/mode work`) for write-capable shell work.",
                ));
            }
            ShellPolicy::ReadOnly | ShellPolicy::Full => {}
        }
        enforce_readonly_github_network_policy(command, context)?;
        let requested_timeout_ms = if self.optional_timeout {
            input
                .get("timeout_ms")
                .map(|value| {
                    value
                        .as_u64()
                        .ok_or_else(|| type_mismatch("timeout_ms", value, "a positive integer"))
                })
                .transpose()?
        } else {
            Some(optional_u64(&input, "timeout_ms", 120_000)?.min(600_000))
        };
        let background = optional_bool(&input, "background", false)?;
        let interactive = optional_bool(&input, "interactive", false)?;
        let combined_output = optional_bool(&input, "combined_output", false)?;
        let tty = optional_bool(&input, "tty", false)? || (combined_output && background);
        let timeout_ms = contract_bash_timeout_ms(
            self.optional_timeout,
            requested_timeout_ms,
            background,
            interactive,
        );
        let timeout_value_ms = timeout_ms.unwrap_or(BASH_MAX_TIMEOUT_MS);
        // Strict types (2026-08-04 review): a non-string here used to be
        // silently dropped — the command then ran with NO stdin and reported
        // success, the exact silent-drop failure the alias hardening closed
        // for misspelled names. A wrong type is an error, never a no-op.
        let stdin_data = match first_present_field(&input, &["stdin", "input", "data"]) {
            None => None,
            Some((name, value)) => Some(
                value
                    .as_str()
                    .ok_or_else(|| type_mismatch(name, value, "a string"))?
                    .to_string(),
            ),
        };

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

        let persist = optional_bool(&input, "persist", false)?;
        if persist {
            if !background {
                return Err(ToolError::invalid_input(
                    "persist:true requires background:true; a persisted service must be started as a background task.",
                ));
            }
            if interactive || tty {
                return Err(ToolError::invalid_input(
                    "persist:true cannot be combined with interactive or TTY modes.",
                ));
            }
            if stdin_data.is_some() {
                return Err(ToolError::invalid_input(
                    "persist:true spawns the service with null stdio; stdin data is not accepted.",
                ));
            }
            if !persistent_services_enabled_for(context) {
                return Err(ToolError::not_available(
                    "persistent background services (persist:true) are only available on Unix in the real headless `codewhale exec` host under an explicit danger-full-access / full shell authority. They are rejected in interactive sessions, desktop/app-server hosts, Fleet/sub-agents, restricted or external sandboxes, and TTY/interactive/stdin modes.",
                ));
            }
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
        // Strict types: a non-string cwd used to silently run the command in
        // the workspace default instead of erroring (2026-08-04 review).
        let working_dir = match first_present_field(&input, &["cwd", "working_dir"])
            .map(|(name, value)| {
                value
                    .as_str()
                    .ok_or_else(|| type_mismatch(name, value, "a string"))
            })
            .transpose()?
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
        if matches!(context.shell_policy, ShellPolicy::ReadOnly) {
            let effective_cwd = working_dir
                .as_deref()
                .map(std::path::Path::new)
                .unwrap_or(&context.workspace);
            enforce_readonly_workspace_operands(command, &context.workspace, effective_cwd)?;
        }

        // #456 — collect env from any configured `shell_env` hooks. Runs
        // synchronously, captures stdout, parses `KEY=VAL` lines, audit-logs
        // the keys (never the values). Empty / no-op when no hook is
        // configured.
        let read_only_shell = matches!(context.shell_policy, ShellPolicy::ReadOnly);
        let mut extra_env = if read_only_shell {
            // shell_env hooks are arbitrary operator-configured processes.
            // They cannot run inside the evidence-only execution boundary.
            HashMap::new()
        } else if let Some(hook_executor) = &context.runtime.hook_executor {
            let hook_ctx = crate::hooks::HookContext::new()
                .with_tool_name("exec_shell")
                .with_tool_args(&input);
            hook_executor.collect_shell_env(&hook_ctx)
        } else {
            std::collections::HashMap::new()
        };
        if read_only_shell {
            let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
            let inert_git_helper = if cfg!(windows) {
                "cmd.exe /d /c exit 1"
            } else {
                "/usr/bin/false"
            };
            // Read-only Bash is intentionally a small inspection surface. Git
            // can otherwise invoke operator/repository configured helpers
            // while performing nominal reads (a pager or fsmonitor), and it
            // may opportunistically refresh the index. These environment
            // overrides make those reads non-interactive and suppress the
            // optional mutation/extension seams; the command classifier and
            // machine authority gate remain authoritative as well.
            extra_env.insert("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string());
            extra_env.insert("GIT_NO_LAZY_FETCH".to_string(), "1".to_string());
            extra_env.insert("GIT_PAGER".to_string(), String::new());
            extra_env.insert("GH_PAGER".to_string(), String::new());
            extra_env.insert("GH_PROMPT_DISABLED".to_string(), "1".to_string());
            extra_env.insert("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string());
            // The classifier rejects explicit GHES repo/URL targets. Pin the
            // implicit environment side too, so inherited GH_HOST/GH_REPO
            // cannot redirect the call after api.github.com was approved.
            extra_env.insert("GH_HOST".to_string(), "github.com".to_string());
            extra_env.insert("GH_REPO".to_string(), String::new());
            extra_env.insert("PAGER".to_string(), String::new());
            extra_env.insert("ENV".to_string(), String::new());
            extra_env.insert("BASH_ENV".to_string(), String::new());
            extra_env.insert("CDPATH".to_string(), String::new());
            extra_env.insert("RIPGREP_CONFIG_PATH".to_string(), String::new());
            // Ignore user/system Git configuration and replace any repository
            // external diff helper with a fixed inert executable. Repository
            // config and attributes are attacker-controlled evidence inputs;
            // a nominal `git diff/log/show` must not turn them into programs.
            extra_env.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
            extra_env.insert("GIT_CONFIG_GLOBAL".to_string(), null_device.to_string());
            extra_env.insert("GIT_CONFIG_PARAMETERS".to_string(), String::new());
            extra_env.insert(
                "GIT_EXTERNAL_DIFF".to_string(),
                inert_git_helper.to_string(),
            );
            extra_env.insert("GIT_ATTR_NOSYSTEM".to_string(), "1".to_string());
            if let Some(path) = readonly_sanitized_path(&context.workspace) {
                extra_env.insert("PATH".to_string(), path);
            }
            extra_env.insert("GIT_CONFIG_COUNT".to_string(), "3".to_string());
            extra_env.insert("GIT_CONFIG_KEY_0".to_string(), "core.fsmonitor".to_string());
            extra_env.insert("GIT_CONFIG_VALUE_0".to_string(), "false".to_string());
            extra_env.insert("GIT_CONFIG_KEY_1".to_string(), "core.hooksPath".to_string());
            extra_env.insert("GIT_CONFIG_VALUE_1".to_string(), null_device.to_string());
            extra_env.insert(
                "GIT_CONFIG_KEY_2".to_string(),
                "log.showSignature".to_string(),
            );
            extra_env.insert("GIT_CONFIG_VALUE_2".to_string(), "false".to_string());
            extra_env.insert(READONLY_ENV_MARKER.to_string(), "1".to_string());
        }

        let command_expense = infer_command_expense(command);
        let heavy_permit = acquire_heavy_command_permit(command, context.cancel_token.as_ref())
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let admission_wait_ms = heavy_permit
            .as_ref()
            .map(|permit| u64::try_from(permit.queued_for().as_millis()).unwrap_or(u64::MAX));
        let admission_limit = heavy_permit.as_ref().map(HeavyCommandPermit::limit);
        let admission_memory = heavy_permit
            .as_ref()
            .map(HeavyCommandPermit::memory_pressure);

        // Route through external sandbox backend when configured.
        if let Some(backend) = &context.sandbox_backend {
            if self.optional_timeout {
                return Err(ToolError::not_available(
                    "bash is unavailable with this external sandbox backend because it cannot preserve combined streaming output and timeout semantics. Use the native sandbox or search for the backend-specific shell tool.",
                ));
            }
            if matches!(context.shell_policy, ShellPolicy::ReadOnly) {
                return Err(ToolError::permission_denied(
                    "Read-only Scout shell cannot use an external sandbox backend because that interface accepts a raw command string rather than the classifier-approved argv. Use File read/search, or run this Scout without the external backend.",
                ));
            }
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
                "expense_class": match command_expense {
                    CommandExpense::Heavy => "heavy",
                    CommandExpense::Normal => "normal",
                },
                "resource_admission_wait_ms": admission_wait_ms,
                "resource_admission_limit": admission_limit,
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
                timeout_value_ms,
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
            let result = manager.execute_with_options_env_for_owner_and_work(
                command,
                working_dir.as_deref(),
                timeout_value_ms,
                true,
                stdin_data.as_deref(),
                tty,
                policy_override,
                extra_env,
                shell_job_owner_from_context(context),
                context.state_namespace.clone(),
                shell_work_lifecycle_from_context(context),
                None,
                persist,
                (1_000, 600_000),
            );
            if let (Ok(result), Some(permit)) = (&result, heavy_permit)
                && let Some(task_id) = result.task_id.as_deref()
            {
                manager
                    .attach_heavy_permit(task_id, permit)
                    .map_err(|error| ToolError::execution_failed(error.to_string()))?;
            }
            result
        } else {
            execute_foreground_via_background(
                context,
                command,
                heavy_permit,
                working_dir,
                timeout_ms,
                stdin_data.as_deref(),
                combined_output,
                policy_override,
                extra_env,
                matches!(context.shell_policy, ShellPolicy::ReadOnly),
                if self.optional_timeout {
                    (1, BASH_MAX_TIMEOUT_MS)
                } else {
                    (1_000, 600_000)
                },
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
                if self.optional_timeout {
                    return finish_contract_bash_result(result, timeout_ms, context);
                }
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
                let sandbox_denied_hint = if network_restricted_hint.is_none() {
                    shell_sandbox_denied_hint(context, &result)
                } else {
                    None
                };
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
                } else if persist && result.status == ShellStatus::Running {
                    format!(
                        "Persistent service staged: {task_id_str}. Probe readiness with a separate command. Codewhale will transfer ownership only if this exec finishes successfully."
                    )
                } else if result.status == ShellStatus::Running {
                    let completion_contract = if context.owner_agent_id.is_some() {
                        "completion stays in task/status and is not injected into the parent model."
                    } else {
                        "completion is delivered to the model as an internal runtime event and shown in task/status state."
                    };
                    if backgrounded_foreground {
                        format!(
                            "Foreground shell wait moved to /jobs: {task_id_str}\n\nReturns immediately; {completion_contract} Keep working; call Bash action=\"wait\" task_id=\"{task_id_str}\" at a true dependency to block until completion or timeout."
                        )
                    } else {
                        format!(
                            "Background task started: {task_id_str}\n\nReturns immediately; {completion_contract} Codewhale terminates this task when the session exits. If a service must survive a successful headless exec, start it with background=true and persist=true. Keep working; call Bash action=\"wait\" task_id=\"{task_id_str}\" at a true dependency to block until completion or timeout."
                        )
                    }
                } else if result.status == ShellStatus::Killed && was_cancelled {
                    format!(
                        "Command canceled; process killed.\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.stdout, result.stderr
                    )
                } else if result.status == ShellStatus::TimedOut {
                    format!(
                        "Command timed out after {timeout_value_ms}ms; process killed.\n\n{FOREGROUND_TIMEOUT_RECOVERY_HINT}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
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
                if let Some(hint) = sandbox_denied_hint.as_deref() {
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
                    "expense_class": match command_expense {
                        CommandExpense::Heavy => "heavy",
                        CommandExpense::Normal => "normal",
                    },
                    "resource_admission_wait_ms": admission_wait_ms,
                    "resource_admission_limit": admission_limit,
                    "resource_admission_memory": match admission_memory {
                        Some(MemoryPressure::Critical) => "critical",
                        Some(MemoryPressure::Constrained) => "constrained",
                        Some(MemoryPressure::Nominal) | None => "nominal",
                        Some(MemoryPressure::Unknown) => "unknown",
                    },
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
                if persist {
                    metadata["persist_requested"] = json!(true);
                    metadata["ownership"] = json!("managed_pending_exec_success");
                    metadata["background_policy"] = json!("pending_ownership_transfer");
                    metadata["auto_resume_on_completion"] = json!(false);
                    metadata["completion_surface"] = json!("headless_exec_release_receipt");
                } else if background || backgrounded_foreground {
                    let child_owned = context.owner_agent_id.is_some();
                    metadata["auto_resume_on_completion"] = json!(!child_owned);
                    metadata["completion_surface"] = if child_owned {
                        json!("task_status_and_explicit_wait")
                    } else {
                        json!("runtime_event_and_task_status")
                    };
                    metadata["background_policy"] = json!("nonblocking");
                }
                if result.status == ShellStatus::TimedOut && !background && !interactive {
                    metadata["foreground_timeout_recovery"] = json!({
                        "process_killed": true,
                        "hint": FOREGROUND_TIMEOUT_RECOVERY_HINT,
                        "recommended_tools": ["Bash", "task_shell_start", "task_shell_wait"],
                        "rerun_as": {"tool": "Bash", "action": "run", "background": true},
                        "poll_with": [
                            {"tool": "Bash", "action": "wait"},
                            {"tool": "task_shell_wait"}
                        ]
                    });
                }
                if let Some(hint) = network_restricted_hint {
                    metadata["sandbox_network_restricted"] = json!(true);
                    metadata["sandbox_network_denied_hint"] = json!(hint);
                }
                if let Some(hint) = sandbox_denied_hint {
                    metadata["sandbox_denied_hint"] = json!(hint);
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
            Err(e) => Ok(ToolResult::error(shell_execution_failed_message(&e))),
        }
    }
}

/// Render a spawn/stream failure for the model and the user: the full cause
/// chain (an anyhow context alone hides the `ENOSPC`/`EMFILE` underneath) plus,
/// when the innermost error looks like host resource exhaustion, what to do
/// about it. The shell tool keeps no state from a failed spawn, so retrying is
/// always safe.
fn shell_execution_failed_message(error: &anyhow::Error) -> String {
    let hint = error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .find_map(output::resource_exhaustion_hint);
    match hint {
        Some(hint) => format!(
            "Shell execution failed: {error:#}. Likely host resource exhaustion — {hint}. The shell tool itself is still usable; the next call starts fresh."
        ),
        None => format!("Shell execution failed: {error:#}"),
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
        // Multi-task wait (#5549): task_ids + until=any|all. The validated
        // single-task path stays unchanged below when task_ids is absent.
        if let Some(task_ids) = input.get("task_ids") {
            let ids = task_ids
                .as_array()
                .ok_or_else(|| type_mismatch("task_ids", task_ids, "an array of strings"))?;
            let ids = ids
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| type_mismatch("task_ids", value, "strings"))
                        .map(str::to_string)
                })
                .collect::<Result<Vec<_>, _>>()?;
            return self.execute_wait_many(input, context, &ids).await;
        }
        let task_id = required_task_id(input)?;
        let wait = match first_present_field(input, &["wait", "block"]) {
            None => true,
            Some((name, value)) => value
                .as_bool()
                .ok_or_else(|| type_mismatch(name, value, "a boolean"))?,
        };
        let timeout_ms = wait_timeout_ms(input)?;

        let (delta, wait_canceled) = if wait {
            wait_for_shell_delta_cancellable(context, task_id, timeout_ms).await?
        } else {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let delta = manager
                .get_output_delta_for_session(&context.state_namespace, task_id, false, timeout_ms)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            (delta, false)
        };

        let status = delta.result.status.clone();
        let mut result = build_shell_delta_tool_result(delta, context);
        if let Some(metadata) = result.metadata.as_mut()
            && let Some(object) = metadata.as_object_mut()
        {
            object.insert("wait_timeout_ms".to_string(), json!(timeout_ms));
        }
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

    /// Wait on several background tasks at once (#5549).
    ///
    /// `until` selects the completion condition: `any` returns as soon as one
    /// task is terminal, `all` waits for every one. Unknown task ids fail the
    /// call (same contract as the single-task path); a timeout reports the
    /// still-running ids so the caller can cancel or wait again.
    async fn execute_wait_many(
        &self,
        input: &serde_json::Value,
        context: &ToolContext,
        task_ids: &[String],
    ) -> Result<ToolResult, ToolError> {
        let until = match input.get("until").and_then(serde_json::Value::as_str) {
            None | Some("all") => "all",
            Some("any") => "any",
            Some(other) => {
                return Err(ToolError::invalid_input(format!(
                    "until must be \"any\" or \"all\", got {other}"
                )));
            }
        };
        let wait = match first_present_field(input, &["wait", "block"]) {
            None => true,
            Some((name, value)) => value
                .as_bool()
                .ok_or_else(|| type_mismatch(name, value, "a boolean"))?,
        };
        let timeout_ms = wait_timeout_ms(input)?;
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut timed_out = false;
        let mut wait_canceled = false;

        let snapshot =
            |manager: &mut ShellManager| -> Result<Vec<(String, ShellStatus)>, ToolError> {
                task_ids
                    .iter()
                    .map(|id| {
                        let detail = manager
                            .inspect_job(id)
                            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                        Ok((id.clone(), detail.snapshot.status))
                    })
                    .collect()
            };

        let mut poll_tick_ms: u64 = FOREGROUND_POLL_INITIAL_MS;
        let statuses = loop {
            let current = {
                let mut manager = context
                    .shell_manager
                    .lock()
                    .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
                snapshot(&mut manager)?
            };
            if context
                .cancel_token
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                wait_canceled = true;
                break current;
            }
            let running = current
                .iter()
                .filter(|(_, status)| *status == ShellStatus::Running)
                .count();
            let terminal = current.len().saturating_sub(running);
            let satisfied = if until == "any" {
                terminal >= 1
            } else {
                running == 0
            };
            if !wait || satisfied {
                break current;
            }
            if std::time::Instant::now() >= deadline {
                timed_out = true;
                break current;
            }
            tokio::time::sleep(Duration::from_millis(poll_tick_ms)).await;
            poll_tick_ms = (poll_tick_ms * 2).min(FOREGROUND_POLL_MAX_MS);
        };

        let running_after = statuses
            .iter()
            .filter(|(_, status)| *status == ShellStatus::Running)
            .count();
        let settled = statuses.len().saturating_sub(running_after);
        let lines: Vec<String> = statuses
            .iter()
            .map(|(id, status)| format!("{id}: {status:?}"))
            .collect();
        let still_running: Vec<&str> = statuses
            .iter()
            .filter(|(_, status)| *status == ShellStatus::Running)
            .map(|(id, _)| id.as_str())
            .collect();
        let summary = if timed_out {
            format!(
                "wait timed out after {}ms; still running: {}",
                timeout_ms,
                if still_running.is_empty() {
                    "none".to_string()
                } else {
                    still_running.join(", ")
                }
            )
        } else if wait_canceled {
            format!(
                "wait canceled; still running: {}",
                if still_running.is_empty() {
                    "none".to_string()
                } else {
                    still_running.join(", ")
                }
            )
        } else {
            format!(
                "{settled} of {} background command{} settled; remaining running: {}",
                statuses.len(),
                if statuses.len() == 1 { "" } else { "s" },
                if still_running.is_empty() {
                    "none".to_string()
                } else {
                    still_running.join(", ")
                }
            )
        };
        let content = format!("{summary}\n{}\n", lines.join("\n"));
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "statuses".to_string(),
            serde_json::Value::Object(
                statuses
                    .iter()
                    .map(|(id, status)| (id.clone(), serde_json::json!(format!("{status:?}"))))
                    .collect::<serde_json::Map<_, _>>(),
            ),
        );
        metadata.insert("wait_timeout_ms".to_string(), json!(timeout_ms));
        metadata.insert("until".to_string(), json!(until));
        metadata.insert("timed_out".to_string(), json!(timed_out));
        if wait_canceled {
            metadata.insert("wait_canceled".to_string(), json!(true));
        }
        Ok(ToolResult {
            content: content.trim().to_string(),
            success: true,
            metadata: Some(serde_json::Value::Object(metadata)),
        })
    }

    async fn execute_interact(
        &self,
        input: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(input)?;
        let close_stdin = optional_bool(input, "close_stdin", false)?;
        let timeout_ms = optional_u64(input, "timeout_ms", 1_000)?;
        // Same strict-type contract as `run` (2026-08-04): a non-string here
        // was silently dropped, so an `interact` call reported success while
        // writing nothing to the child's stdin. Alias order also matches
        // `run` now — `stdin` first — so the same payload reaches the same
        // place whichever spelling the model uses.
        let interaction_input = match first_present_field(input, &["stdin", "input", "data"]) {
            None => "",
            Some((name, value)) => value
                .as_str()
                .ok_or_else(|| type_mismatch(name, value, "a string"))?,
        };

        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            if !interaction_input.is_empty() || close_stdin {
                manager
                    .write_stdin_for_session(
                        &context.state_namespace,
                        task_id,
                        interaction_input,
                        close_stdin,
                    )
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
                    .get_output_delta_for_session(&context.state_namespace, task_id, false, 0)
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
                    .get_output_delta_for_session(&context.state_namespace, task_id, false, 0)
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
        let cancel_all = optional_bool(input, "all", false)?;
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;

        if cancel_all {
            let results = manager
                .kill_running_for_session(&context.state_namespace)
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
            .kill_for_session(&context.state_namespace, task_id)
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
    // A present-but-non-string task_id is a type error, not a missing field:
    // "missing required field" sends the model's retry in the wrong
    // direction when it already supplied `task_id: 42` (2026-08-04 review).
    match first_present_field(input, &["task_id", "id"]) {
        None => Err(ToolError::missing_field("task_id")),
        Some((name, value)) => value
            .as_str()
            .ok_or_else(|| type_mismatch(name, value, "a string")),
    }
}

/// First PRESENT value among aliased spellings of one field. `null` counts
/// as absent, matching the `is_absent` rule the shared typed helpers use.
fn first_present_field<'a>(
    input: &'a serde_json::Value,
    names: &[&'static str],
) -> Option<(&'static str, &'a serde_json::Value)> {
    names.iter().find_map(|name| match input.get(*name) {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some((*name, value)),
    })
}

/// Effective `action=wait` timeout in milliseconds. `timeout_ms` is
/// canonical; `timeout_secs` (seconds) and bare `timeout` (milliseconds) are
/// honored so a habit formed on other wait tools gets the duration it asked
/// for instead of silently falling back to the 30 s default.
fn wait_timeout_ms(input: &serde_json::Value) -> Result<u64, ToolError> {
    match first_present_field(input, &["timeout_ms", "timeout_secs", "timeout"]) {
        None => Ok(30_000),
        Some(("timeout_secs", value)) => {
            let secs = value
                .as_u64()
                .ok_or_else(|| type_mismatch("timeout_secs", value, "an integer"))?;
            Ok(secs.saturating_mul(1_000))
        }
        Some((name, value)) => value
            .as_u64()
            .ok_or_else(|| type_mismatch(name, value, "an integer")),
    }
}

fn build_shell_delta_tool_result(delta: ShellDeltaResult, context: &ToolContext) -> ToolResult {
    let result = delta.result;
    let network_restricted_hint =
        shell_network_restricted_hint(context, &delta.command, &result).map(str::to_string);
    let sandbox_denied_hint = if network_restricted_hint.is_none() {
        shell_sandbox_denied_hint(context, &result)
    } else {
        None
    };
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
    // The model cannot see metadata, so surface the real elapsed time in the
    // visible content. Without it every wait result looks identical whether
    // the task just started or has been running for minutes, which biases the
    // model into busy-polling short waits and misjudging long ones.
    output = format!("{}\n\n{output}", wait_timing_line(&result));

    if let Some(hint) = network_restricted_hint.as_deref() {
        output = format!("{hint}\n\n{output}");
    }
    if let Some(hint) = sandbox_denied_hint.as_deref() {
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
    if let Some(hint) = sandbox_denied_hint
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("sandbox_denied_hint".to_string(), json!(hint));
    }
    if provenance_hint.is_some()
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("macos_provenance_restricted".to_string(), json!(true));
    }
    tool_result
}

/// Human-readable elapsed time for a shell task ("450 ms", "12.3 s", "2m5s").
fn format_elapsed_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        let secs = ms as f64 / 1_000.0;
        format!("{secs} s")
    } else {
        let total_secs = ms / 1_000;
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    }
}

/// One-line status + elapsed summary for wait/delta results, placed at the top
/// of the visible content so the model can judge how long it actually waited.
fn wait_timing_line(result: &ShellResult) -> String {
    let status_phrase = match result.status {
        ShellStatus::Running => "still running",
        ShellStatus::Completed => "completed",
        ShellStatus::Failed => "failed",
        ShellStatus::Killed => "killed",
        ShellStatus::TimedOut => "timed out",
    };
    let elapsed = format_elapsed_ms(result.duration_ms);
    match result.task_id.as_deref() {
        Some(task_id) => format!("Task {task_id} {status_phrase} after {elapsed}."),
        None => format!("Task {status_phrase} after {elapsed}."),
    }
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

    let mut poll_tick_ms: u64 = FOREGROUND_POLL_INITIAL_MS;
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
                .get_output_delta_for_session(&context.state_namespace, task_id, false, 0)
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
                .get_output_delta_for_session(&context.state_namespace, task_id, false, 0)
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

        tokio::time::sleep(Duration::from_millis(poll_tick_ms)).await;
        poll_tick_ms = (poll_tick_ms * 2).min(FOREGROUND_POLL_MAX_MS);
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
