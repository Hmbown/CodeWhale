//! Stateful, PTY-backed terminal sessions.
//!
//! Live PTY processes remain deliberately process-local, while a non-secret
//! durable summary records identity, last-known cwd, lifecycle state, and
//! replacement history. A later process reports the shell as stale/lost and
//! starts a new identity; it never claims to reattach from a reused PID.

#[cfg(unix)]
use std::collections::{HashMap, VecDeque};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(unix)]
use std::time::{Duration, Instant};

use async_trait::async_trait;
#[cfg(unix)]
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use uuid::Uuid;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
#[cfg(unix)]
use super::spec::{optional_u64, required_str};

#[cfg(unix)]
const BUFFER_LIMIT: usize = 512 * 1024;
#[cfg(unix)]
const OUTPUT_LIMIT: usize = 12 * 1024;
#[cfg(unix)]
const DEFAULT_TIMEOUT_SECS: u64 = 120;
#[cfg(unix)]
const MAX_TIMEOUT_SECS: u64 = 600;
#[cfg(unix)]
const CANCEL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const CANCEL_SENTINEL_RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(unix)]
struct TerminalSession {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
    output: Arc<Mutex<OutputBuffer>>,
    read_cursor: u64,
    command: Option<CommandState>,
    durable: DurableTerminalRecord,
    durable_path: PathBuf,
}

#[cfg(unix)]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct DurableTerminalRecord {
    schema_version: u32,
    session_id: String,
    runtime_nonce: String,
    process_id: u32,
    workspace: PathBuf,
    name: String,
    shell: String,
    last_known_cwd: PathBuf,
    environment_summary: EnvironmentSummary,
    state: DurableTerminalState,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<Box<DurableTerminalRecord>>,
}

#[cfg(unix)]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct EnvironmentSummary {
    term: Option<String>,
    virtual_env_active: bool,
    conda_env_active: bool,
    nix_shell_active: bool,
    note: String,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DurableTerminalState {
    Running,
    Idle,
    Canceled,
    Failed,
    StaleLost,
    Reset,
}

#[cfg(unix)]
struct CommandState {
    marker: String,
}

#[cfg(unix)]
#[derive(Default)]
struct OutputBuffer {
    bytes: VecDeque<u8>,
    total: u64,
}

#[cfg(unix)]
impl OutputBuffer {
    fn append(&mut self, data: &[u8]) {
        self.total = self.total.saturating_add(data.len() as u64);
        self.bytes.extend(data);
        while self.bytes.len() > BUFFER_LIMIT {
            let _ = self.bytes.pop_front();
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<_>>()).into_owned()
    }
}

#[cfg(unix)]
type SharedSession = Arc<Mutex<TerminalSession>>;

#[cfg(unix)]
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SessionKey {
    workspace: PathBuf,
    name: String,
}

#[cfg(unix)]
static SESSIONS: OnceLock<Mutex<HashMap<SessionKey, SharedSession>>> = OnceLock::new();

#[cfg(unix)]
static RUNTIME_NONCE: OnceLock<String> = OnceLock::new();

#[cfg(unix)]
fn runtime_nonce() -> &'static str {
    RUNTIME_NONCE.get_or_init(|| Uuid::new_v4().to_string())
}

#[cfg(unix)]
fn sessions() -> &'static Mutex<HashMap<SessionKey, SharedSession>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(unix)]
fn session_key(name: &str, workspace: &Path) -> SessionKey {
    SessionKey {
        workspace: workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf()),
        name: name.to_string(),
    }
}

#[cfg(unix)]
fn durable_path(name: &str, workspace: &Path) -> Result<PathBuf, String> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(name.as_bytes());
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    #[cfg(test)]
    let state_dir = workspace.join(".codewhale-test-terminal-sessions");
    #[cfg(not(test))]
    let state_dir = codewhale_config::ensure_state_dir("terminal-sessions")
        .map_err(|error| format!("failed to resolve terminal session state directory: {error}"))?;
    std::fs::create_dir_all(&state_dir)
        .map_err(|error| format!("failed to create terminal session state directory: {error}"))?;
    Ok(state_dir.join(format!("{}.json", &digest[..32])))
}

#[cfg(unix)]
fn load_durable(path: &Path) -> Option<DurableTerminalRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(unix)]
fn persist_durable(path: &Path, record: &DurableTerminalRecord) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(record)
        .map_err(|error| format!("failed to encode terminal session state: {error}"))?;
    crate::utils::write_atomic(path, &payload)
        .map_err(|error| format!("failed to persist terminal session state: {error}"))
}

#[cfg(unix)]
fn create_session(name: &str, workspace: &std::path::Path) -> Result<SharedSession, String> {
    // Unit tests exercise the persistent-PTY contract, not a developer's
    // interactive shell startup files. Keep their deadlines deterministic and
    // avoid racing process-global HOME/SHELL overrides from parallel tests.
    #[cfg(test)]
    let shell = "/bin/sh".to_string();
    #[cfg(not(test))]
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let durable_path = durable_path(name, &workspace)?;
    let previous = load_durable(&durable_path).map(|mut record| {
        // A persisted shell is historical evidence only. A random per-process
        // nonce makes PID reuse irrelevant and deliberately forbids reattach.
        record.state = DurableTerminalState::StaleLost;
        record.updated_at = chrono::Utc::now().to_rfc3339();
        Box::new(record)
    });
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open PTY: {e}"))?;

    let mut command = portable_pty::CommandBuilder::new(&shell);
    command.arg("-i");
    command.cwd(&workspace);
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("failed to start shell {shell}: {e}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to read PTY: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to write PTY: {e}"))?;
    let output = Arc::new(Mutex::new(OutputBuffer::default()));
    let reader_output = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut output) = reader_output.lock() {
                        output.append(&buf[..n]);
                    }
                }
            }
        }
    });

    let durable = DurableTerminalRecord {
        schema_version: 1,
        session_id: Uuid::new_v4().to_string(),
        runtime_nonce: runtime_nonce().to_string(),
        process_id: std::process::id(),
        workspace: workspace.clone(),
        name: name.to_string(),
        shell,
        last_known_cwd: workspace,
        environment_summary: EnvironmentSummary {
            term: std::env::var("TERM").ok().filter(|value| !value.trim().is_empty()),
            virtual_env_active: std::env::var_os("VIRTUAL_ENV").is_some(),
            conda_env_active: std::env::var_os("CONDA_PREFIX").is_some(),
            nix_shell_active: std::env::var_os("IN_NIX_SHELL").is_some(),
            note: "Values and secrets are not persisted; in-shell environment changes are process-local."
                .to_string(),
        },
        state: DurableTerminalState::Idle,
        updated_at: chrono::Utc::now().to_rfc3339(),
        previous,
    };
    persist_durable(&durable_path, &durable)?;

    Ok(Arc::new(Mutex::new(TerminalSession {
        writer: Arc::new(Mutex::new(writer)),
        child,
        output,
        read_cursor: 0,
        command: None,
        durable,
        durable_path,
    })))
}

#[cfg(unix)]
fn get_or_create(name: &str, workspace: &std::path::Path) -> Result<SharedSession, String> {
    let key = session_key(name, workspace);
    let mut registry = sessions()
        .lock()
        .map_err(|_| "terminal session registry lock poisoned".to_string())?;
    if let Some(session) = registry.get(&key) {
        return Ok(Arc::clone(session));
    }
    let session = create_session(name, workspace)?;
    registry.insert(key, Arc::clone(&session));
    Ok(session)
}

#[cfg(unix)]
fn find(name: &str, workspace: &Path) -> Result<SharedSession, String> {
    let live = sessions()
        .lock()
        .map_err(|_| "terminal session registry lock poisoned".to_string())?
        .get(&session_key(name, workspace))
        .cloned();
    if let Some(live) = live {
        return Ok(live);
    }
    if let Ok(path) = durable_path(name, workspace)
        && let Some(mut record) = load_durable(&path)
    {
        record.state = DurableTerminalState::StaleLost;
        record.updated_at = chrono::Utc::now().to_rfc3339();
        let _ = persist_durable(&path, &record);
        return Err(format!(
            "terminal session '{name}' is stale/lost after restart (last cwd: {}); run terminal/run with this name to start a replacement while preserving the historical summary",
            record.last_known_cwd.display()
        ));
    }
    Err(format!(
        "terminal session '{name}' does not exist in workspace {}",
        workspace.display()
    ))
}

#[cfg(unix)]
fn write_bytes(session: &TerminalSession, bytes: &[u8]) -> Result<(), String> {
    let mut writer = session
        .writer
        .lock()
        .map_err(|_| "terminal PTY writer lock poisoned".to_string())?;
    writer
        .write_all(bytes)
        .map_err(|e| format!("PTY write failed: {e}"))?;
    writer.flush().map_err(|e| format!("PTY flush failed: {e}"))
}

#[cfg(unix)]
fn output_snapshot(session: &TerminalSession) -> String {
    session
        .output
        .lock()
        .map(|output| output.text())
        .unwrap_or_default()
}

#[cfg(unix)]
fn take_output(session: &mut TerminalSession) -> String {
    let Ok(output) = session.output.lock() else {
        return String::new();
    };
    let retained_start = output.total.saturating_sub(output.bytes.len() as u64);
    let start = session.read_cursor.max(retained_start);
    let skip = usize::try_from(start.saturating_sub(retained_start)).unwrap_or(usize::MAX);
    let bytes = output.bytes.iter().skip(skip).copied().collect::<Vec<_>>();
    session.read_cursor = output.total;
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(unix)]
fn prune_output(input: &str) -> String {
    if input.len() <= OUTPUT_LIMIT {
        return input.to_string();
    }
    let head = OUTPUT_LIMIT / 3;
    let tail = OUTPUT_LIMIT - head;
    let head_end = input
        .char_indices()
        .find(|(index, _)| *index >= head)
        .map_or(input.len(), |(index, _)| index);
    let tail_start = input
        .char_indices()
        .rev()
        .find(|(index, _)| input.len() - *index <= tail)
        .map_or(0, |(index, _)| index);
    format!(
        "{}\n… [output truncated: {} bytes omitted] …\n{}",
        &input[..head_end],
        input.len() - OUTPUT_LIMIT,
        &input[tail_start..]
    )
}

#[cfg(unix)]
fn completion(session: &TerminalSession) -> Option<(i32, String)> {
    let state = session.command.as_ref()?;
    let output = output_snapshot(session);
    let marker = format!("\n{}:", state.marker);
    let line = output
        .rsplit(&marker)
        .next()
        .and_then(|tail| tail.lines().next())?;
    let (status, cwd) = line.split_once(':')?;
    Some((status.parse().ok()?, cwd.to_string()))
}

#[cfg(unix)]
fn start_command(session: &mut TerminalSession, command: &str) -> Result<(), String> {
    if session.command.is_some() && completion(session).is_none() {
        return Err("terminal session already has a running foreground command".to_string());
    }
    let marker = format!("__CODEWHALE_TERM_{}__", Uuid::new_v4().simple());
    // The command must run in the CURRENT shell — a subshell would discard
    // exactly the state (cd, exports, functions, activated envs) this tool
    // exists to preserve (EXEC-001). The sentinel line is typed after the
    // command; the tty line discipline holds it until the foreground command
    // finishes reading input.
    let wrapped = format!(
        "{command}\n__cw_status=$?; printf '\\n{marker}:%s:%s\\n' \"$__cw_status\" \"$PWD\"\n"
    );
    write_bytes(session, wrapped.as_bytes())?;
    session.command = Some(CommandState { marker });
    session.durable.state = DurableTerminalState::Running;
    session.durable.updated_at = chrono::Utc::now().to_rfc3339();
    persist_durable(&session.durable_path, &session.durable)?;
    Ok(())
}

#[cfg(unix)]
fn write_completion_sentinel(
    session: &TerminalSession,
    marker: &str,
    status: i32,
) -> Result<(), String> {
    let sentinel =
        format!("__cw_status={status}; printf '\\n{marker}:%s:%s\\n' \"$__cw_status\" \"$PWD\"\n");
    write_bytes(session, sentinel.as_bytes())
}

#[cfg(unix)]
#[cfg(test)]
fn wait_session(session: &mut TerminalSession, timeout: Duration) -> (Option<(i32, String)>, bool) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(done) = completion(session) {
            return (Some(done), false);
        }
        if Instant::now() >= deadline {
            return (None, true);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Wait without monopolizing the per-session lock. `terminal/send` and
/// `terminal/cancel` must be able to acquire the lock while a foreground
/// command is active; otherwise interactive input and cancellation deadlock
/// behind the waiter.
#[cfg(unix)]
fn wait_shared_session(
    session: &SharedSession,
    timeout: Duration,
) -> Result<(Option<(i32, String)>, bool), ToolError> {
    let deadline = Instant::now() + timeout;
    loop {
        let done = {
            let session = session
                .lock()
                .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
            completion(&session)
        };
        if let Some(done) = done {
            return Ok((Some(done), false));
        }
        if Instant::now() >= deadline {
            return Ok((None, true));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Interrupt a foreground command and wait until the persistent shell confirms
/// that it is ready again.
///
/// Canonical PTYs may flush queued input when they process ETX. Resubmit the
/// completion sentinel until the shell acknowledges it so a busy host cannot
/// strand the session merely because the one post-interrupt write raced that
/// flush.
#[cfg(unix)]
fn cancel_shared_session(session: &SharedSession) -> Result<(i32, String), ToolError> {
    let marker = {
        let session = session
            .lock()
            .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
        if session.command.is_none() || completion(&session).is_some() {
            return Err(ToolError::execution_failed(
                "terminal session has no running foreground command",
            ));
        }
        write_bytes(&session, &[3]).map_err(ToolError::execution_failed)?;
        session
            .command
            .as_ref()
            .expect("running command checked above")
            .marker
            .clone()
    };
    let deadline = Instant::now() + CANCEL_CONFIRM_TIMEOUT;

    loop {
        std::thread::sleep(CANCEL_SENTINEL_RETRY_INTERVAL);
        let session = session
            .lock()
            .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
        if let Some(done) = completion(&session) {
            return Ok(done);
        }
        if Instant::now() >= deadline {
            return Err(ToolError::execution_failed(
                "terminal interrupt was sent, but cancellation was not confirmed within 2 seconds",
            ));
        }
        write_completion_sentinel(&session, &marker, 130).map_err(ToolError::execution_failed)?;
    }
}

#[cfg(unix)]
fn session_result(
    session: &mut TerminalSession,
    done: Option<(i32, String)>,
    timed_out: bool,
) -> ToolResult {
    let output = prune_output(&take_output(session));
    let finished = done.is_some();
    let (exit_code, cwd) = done.map_or((None, String::new()), |(code, cwd)| (Some(code), cwd));
    let status = if timed_out {
        "timed_out"
    } else if finished {
        "completed"
    } else {
        "running"
    };
    if !cwd.is_empty() {
        session.durable.last_known_cwd = PathBuf::from(&cwd);
    }
    session.durable.state = if timed_out || !finished {
        DurableTerminalState::Running
    } else if exit_code == Some(0) {
        DurableTerminalState::Idle
    } else if exit_code == Some(130) {
        DurableTerminalState::Canceled
    } else {
        DurableTerminalState::Failed
    };
    session.durable.updated_at = chrono::Utc::now().to_rfc3339();
    let persistence_error = persist_durable(&session.durable_path, &session.durable).err();
    let previous = session.durable.previous.as_deref().map(|record| {
        json!({
            "session_id": record.session_id,
            "state": record.state,
            "last_known_cwd": record.last_known_cwd,
            "updated_at": record.updated_at,
        })
    });
    ToolResult {
        content: output,
        // A successful send while the command is still running is itself a
        // successful tool operation. Completed commands still report their
        // real exit status, and timeouts remain unsuccessful.
        success: !timed_out && (!finished || exit_code == Some(0)),
        metadata: Some(json!({
            "status": status,
            "exit_code": exit_code,
            "cwd": cwd,
            "session_persistent": true,
            "durability": "live shell is process-local; identity and last-known summary persist",
            "terminal_session_id": session.durable.session_id,
            "terminal_state": session.durable.state,
            "state_path": session.durable_path,
            "previous_session": previous,
            "persistence_error": persistence_error,
        })),
    }
}

#[cfg(unix)]
fn session_name(input: &serde_json::Value, required: bool) -> Result<&str, ToolError> {
    match input.get("session").and_then(serde_json::Value::as_str) {
        Some(name) if !name.is_empty() => Ok(name),
        Some(_) => Err(ToolError::execution_failed("session must not be empty")),
        None if required => Err(ToolError::missing_field("session")),
        None => Ok("term-1"),
    }
}

#[cfg(unix)]
fn timeout_secs(input: &serde_json::Value, key: &str) -> Result<Duration, ToolError> {
    Ok(Duration::from_secs(
        optional_u64(input, key, DEFAULT_TIMEOUT_SECS)?.clamp(1, MAX_TIMEOUT_SECS),
    ))
}

fn shell_allowed(context: &ToolContext) -> Result<(), ToolError> {
    if context.shell_policy.allows_shell() {
        Ok(())
    } else {
        Err(ToolError::execution_failed(
            "Shell tools are disabled by the active permission profile.",
        ))
    }
}

#[cfg(not(unix))]
fn unsupported() -> ToolResult {
    ToolResult::error("Stateful terminal sessions are currently supported on Unix only.")
}

macro_rules! terminal_tool_common {
    ($name:literal, $description:literal) => {
        fn name(&self) -> &'static str {
            $name
        }
        fn description(&self) -> &'static str {
            $description
        }
        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ]
        }
        fn approval_requirement(&self) -> ApprovalRequirement {
            ApprovalRequirement::Required
        }
    };
}

pub struct TerminalRunTool;
#[async_trait]
impl ToolSpec for TerminalRunTool {
    terminal_tool_common!(
        "terminal/run",
        "Run a command in a persistent PTY shell session. cd, exports, shell functions, and activated environments persist across calls in this process. Identity and a non-secret last-known summary persist across restarts; prior shells are surfaced as stale/lost and are never reattached."
    );
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"command":{"type":"string"},"session":{"type":"string","default":"term-1"},"timeout_secs":{"type":"integer","default":120}},"required":["command"]})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        shell_allowed(context)?;
        #[cfg(unix)]
        {
            let command = required_str(&input, "command")?.to_string();
            let name = session_name(&input, false)?.to_string();
            let session =
                get_or_create(&name, &context.workspace).map_err(ToolError::execution_failed)?;
            let timeout = timeout_secs(&input, "timeout_secs")?;
            return tokio::task::spawn_blocking(move || {
                {
                    let mut session = session.lock().map_err(|_| {
                        ToolError::execution_failed("terminal session lock poisoned")
                    })?;
                    start_command(&mut session, &command).map_err(ToolError::execution_failed)?;
                }
                let (done, timed_out) = wait_shared_session(&session, timeout)?;
                let mut session = session
                    .lock()
                    .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
                Ok(session_result(&mut session, done, timed_out))
            })
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = input;
            Ok(unsupported())
        }
    }
}

pub struct TerminalSendTool;
#[async_trait]
impl ToolSpec for TerminalSendTool {
    terminal_tool_common!(
        "terminal/send",
        "Send raw input to a live persistent terminal session. Use a literal ETX control byte to interrupt an interactive process. A prior-process shell is reported as stale/lost rather than reattached."
    );
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"session":{"type":"string"},"text":{"type":"string"},"wait_ms":{"type":"integer","default":250}},"required":["session","text"]})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        shell_allowed(context)?;
        #[cfg(unix)]
        {
            let name = session_name(&input, true)?.to_string();
            let text = required_str(&input, "text")?.as_bytes().to_vec();
            let session = find(&name, &context.workspace).map_err(ToolError::execution_failed)?;
            let wait = Duration::from_millis(optional_u64(&input, "wait_ms", 250)?.min(60_000));
            return tokio::task::spawn_blocking(move || {
                let mut session = session
                    .lock()
                    .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
                write_bytes(&session, &text).map_err(ToolError::execution_failed)?;
                std::thread::sleep(wait);
                let done = completion(&session);
                Ok(session_result(&mut session, done, false))
            })
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = input;
            Ok(unsupported())
        }
    }
}

pub struct TerminalWaitTool;
#[async_trait]
impl ToolSpec for TerminalWaitTool {
    terminal_tool_common!(
        "terminal/wait",
        "Wait for the current foreground command in a live persistent terminal session and return buffered output. A prior-process shell is reported as stale/lost rather than reattached."
    );
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"session":{"type":"string"},"timeout_secs":{"type":"integer","default":120}},"required":["session"]})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        shell_allowed(context)?;
        #[cfg(unix)]
        {
            let name = session_name(&input, true)?.to_string();
            let session = find(&name, &context.workspace).map_err(ToolError::execution_failed)?;
            let timeout = timeout_secs(&input, "timeout_secs")?;
            return tokio::task::spawn_blocking(move || {
                let (done, timed_out) = wait_shared_session(&session, timeout)?;
                let mut session = session
                    .lock()
                    .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
                Ok(session_result(&mut session, done, timed_out))
            })
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = input;
            Ok(unsupported())
        }
    }
}

pub struct TerminalCancelTool;
#[async_trait]
impl ToolSpec for TerminalCancelTool {
    terminal_tool_common!(
        "terminal/cancel",
        "Interrupt the running foreground command with ETX. The live terminal session survives and can be reused; its non-secret summary persists."
    );
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"session":{"type":"string"}},"required":["session"]})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        shell_allowed(context)?;
        #[cfg(unix)]
        {
            let name = session_name(&input, true)?.to_string();
            let session = find(&name, &context.workspace).map_err(ToolError::execution_failed)?;
            return tokio::task::spawn_blocking(move || {
                let done = cancel_shared_session(&session)?;
                let mut session = session
                    .lock()
                    .map_err(|_| ToolError::execution_failed("terminal session lock poisoned"))?;
                let mut result = session_result(&mut session, Some(done), false);
                result.success = true;
                if let Some(metadata) = result.metadata.as_mut() {
                    metadata["status"] = json!("canceled");
                    metadata["canceled"] = json!(true);
                }
                Ok(result)
            })
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = input;
            Ok(unsupported())
        }
    }
}

pub struct TerminalResetTool;
#[async_trait]
impl ToolSpec for TerminalResetTool {
    terminal_tool_common!(
        "terminal/reset",
        "Kill and recreate a persistent terminal session with a fresh environment. This loses live cd, exports, functions, activated environments, and running work while retaining the prior historical summary."
    );
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"session":{"type":"string"}},"required":["session"]})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        shell_allowed(context)?;
        #[cfg(unix)]
        {
            let name = session_name(&input, true)?.to_string();
            let old = find(&name, &context.workspace).map_err(ToolError::execution_failed)?;
            let workspace = context.workspace.clone();
            return tokio::task::spawn_blocking(move || {
                if let Ok(mut old) = old.lock() { let _ = old.child.kill(); }
                if let Ok(mut old) = old.lock() {
                    old.durable.state = DurableTerminalState::Reset;
                    old.durable.updated_at = chrono::Utc::now().to_rfc3339();
                    let _ = persist_durable(&old.durable_path, &old.durable);
                }
                let fresh = create_session(&name, &workspace).map_err(ToolError::execution_failed)?;
                sessions().lock().map_err(|_| ToolError::execution_failed("terminal session registry lock poisoned"))?.insert(session_key(&name, &workspace), fresh);
                Ok(ToolResult { content: format!("Reset terminal session '{name}'. Lost shell state and any running command."), success: true, metadata: Some(json!({"session":name,"reset":true,"lost_state":["cwd","environment","functions","activated environments","running command"]})) })
            }).await.map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let _ = input;
            Ok(unsupported())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fresh(name: &str) -> SharedSession {
        let session = get_or_create(name, std::path::Path::new("/tmp")).unwrap();
        let mut session_guard = session.lock().unwrap();
        let _ = session_guard.child.kill();
        drop(session_guard);
        let replacement = create_session(name, std::path::Path::new("/tmp")).unwrap();
        sessions().lock().unwrap().insert(
            session_key(name, Path::new("/tmp")),
            Arc::clone(&replacement),
        );
        replacement
    }

    fn run(session: &SharedSession, command: &str, timeout: Duration) -> ToolResult {
        let mut session = session.lock().unwrap();
        start_command(&mut session, command).unwrap();
        let (done, timed_out) = wait_session(&mut session, timeout);
        session_result(&mut session, done, timed_out)
    }

    #[test]
    #[cfg(unix)]
    fn cd_persists_between_runs() {
        let session = fresh("test-cd");
        let _ = run(
            &session,
            "mkdir -p /tmp/cw-term-cd-proof && cd /tmp/cw-term-cd-proof",
            Duration::from_secs(3),
        );
        // A separate run must still be inside the directory — this is the
        // whole point of the stateful session (EXEC-001).
        let result = run(&session, "pwd", Duration::from_secs(3));
        assert!(result.content.contains("cw-term-cd-proof"), "{}", {
            &result.content
        });
    }

    #[test]
    #[cfg(unix)]
    fn export_persists_and_sessions_are_isolated() {
        let one = fresh("test-env-one");
        let two = fresh("test-env-two");
        let _ = run(&one, "export CW_TERM_TEST=present", Duration::from_secs(3));
        assert!(
            run(&one, "printf %s $CW_TERM_TEST", Duration::from_secs(3))
                .content
                .contains("present")
        );
        assert!(
            !run(
                &two,
                "printf %s ${CW_TERM_TEST-unset}",
                Duration::from_secs(3)
            )
            .content
            .contains("present")
        );
    }

    #[test]
    #[cfg(unix)]
    fn reset_replaces_shell_environment() {
        let session = fresh("test-reset");
        let _ = run(
            &session,
            "export CW_TERM_RESET=present",
            Duration::from_secs(3),
        );
        assert!(
            run(&session, "printf %s $CW_TERM_RESET", Duration::from_secs(3))
                .content
                .contains("present")
        );
        let _ = session.lock().unwrap().child.kill();
        let replacement = create_session("test-reset", std::path::Path::new("/tmp")).unwrap();
        sessions().lock().unwrap().insert(
            session_key("test-reset", Path::new("/tmp")),
            Arc::clone(&replacement),
        );
        assert!(
            !run(
                &replacement,
                "printf %s ${CW_TERM_RESET-unset}",
                Duration::from_secs(3)
            )
            .content
            .contains("present")
        );
    }

    #[test]
    #[cfg(unix)]
    fn timeout_leaves_session_alive() {
        let session = fresh("test-timeout");
        let result = run(
            &session,
            "printf before; sleep 2",
            Duration::from_millis(100),
        );
        assert_eq!(result.metadata.as_ref().unwrap()["status"], "timed_out");
        let result = {
            let mut session_guard = session.lock().unwrap();
            let (done, timed_out) = wait_session(&mut session_guard, Duration::from_secs(3));
            session_result(&mut session_guard, done, timed_out)
        };
        assert_eq!(result.metadata.as_ref().unwrap()["status"], "completed");
        let result = run(&session, "printf after", Duration::from_secs(3));
        assert!(result.content.contains("after"));
    }

    #[test]
    #[cfg(unix)]
    fn cancel_interrupts_sleep_and_session_survives() {
        let session = fresh("test-cancel");
        let worker = Arc::clone(&session);
        {
            let mut guard = worker.lock().unwrap();
            start_command(&mut guard, "sleep 10").unwrap();
        }
        let handle = std::thread::spawn(move || {
            let (done, timed_out) = wait_shared_session(&worker, Duration::from_secs(30)).unwrap();
            let mut guard = worker.lock().unwrap();
            session_result(&mut guard, done, timed_out)
        });
        std::thread::sleep(Duration::from_millis(150));
        let done = cancel_shared_session(&session).unwrap();
        let result = handle.join().unwrap();
        assert_eq!(done.0, 130);
        assert_eq!(result.metadata.as_ref().unwrap()["exit_code"], 130);
        assert!(
            run(&session, "printf alive", Duration::from_secs(3))
                .content
                .contains("alive")
        );
    }

    #[test]
    #[cfg(unix)]
    fn session_names_are_scoped_to_workspace() {
        let first_workspace = tempfile::tempdir().unwrap();
        let second_workspace = tempfile::tempdir().unwrap();
        let first = get_or_create("shared-name", first_workspace.path()).unwrap();
        let second = get_or_create("shared-name", second_workspace.path()).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(find("shared-name", first_workspace.path()).is_ok());
        assert!(find("shared-name", second_workspace.path()).is_ok());
        assert!(find("shared-name", Path::new("/tmp")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn durable_summary_marks_prior_process_shell_stale_and_preserves_history() {
        let workspace = tempfile::tempdir().unwrap();
        let canonical_workspace = workspace.path().canonicalize().unwrap();
        let name = format!("restart-proof-{}", Uuid::new_v4());
        let first = create_session(&name, workspace.path()).unwrap();
        let first_record = first.lock().unwrap().durable.clone();
        assert_eq!(first_record.state, DurableTerminalState::Idle);
        assert_eq!(first_record.last_known_cwd, canonical_workspace);
        assert!(!first_record.session_id.is_empty());

        // Removing only the process-local registry entry models a restart.
        // The durable record remains, but cannot be used to reattach.
        sessions()
            .lock()
            .unwrap()
            .remove(&session_key(&name, workspace.path()));
        let stale = match find(&name, workspace.path()) {
            Ok(_) => panic!("persisted session must not be reattached"),
            Err(error) => error,
        };
        assert!(stale.contains("stale/lost"), "{stale}");
        assert!(stale.contains("start a replacement"), "{stale}");

        let replacement = create_session(&name, workspace.path()).unwrap();
        let replacement = replacement.lock().unwrap();
        assert_ne!(replacement.durable.session_id, first_record.session_id);
        let previous = replacement.durable.previous.as_deref().unwrap();
        assert_eq!(previous.session_id, first_record.session_id);
        assert_eq!(previous.state, DurableTerminalState::StaleLost);
        assert_eq!(previous.last_known_cwd, canonical_workspace);
        assert_ne!(replacement.durable.runtime_nonce, "");
    }

    #[test]
    #[cfg(unix)]
    fn output_is_capped_with_notice() {
        let session = fresh("test-output-cap");
        let result = run(&session, "yes x | head -n 100000", Duration::from_secs(3));
        assert!(result.content.len() <= OUTPUT_LIMIT + 100);
        assert!(result.content.contains("output truncated"));
    }
}
