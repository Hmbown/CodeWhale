//! Fleet worker host adapters.
//!
//! Adapters own process boundaries for worker hosts. The manager can lease and
//! observe work through this trait without knowing whether the worker is a
//! local child process or an SSH-backed remote command.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use codewhale_protocol::fleet::FleetHostSpec;
use thiserror::Error;

const DEFAULT_LOG_LIMIT_BYTES: usize = 64 * 1024;
const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 10;

pub type FleetHostResult<T> = Result<T, FleetHostError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetHostErrorKind {
    Retryable,
    Terminal,
    Configuration,
}

#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct FleetHostError {
    pub kind: FleetHostErrorKind,
    pub message: String,
}

impl FleetHostError {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: FleetHostErrorKind::Retryable,
            message: message.into(),
        }
    }

    fn terminal(message: impl Into<String>) -> Self {
        Self {
            kind: FleetHostErrorKind::Terminal,
            message: message.into(),
        }
    }

    fn configuration(message: impl Into<String>) -> Self {
        Self {
            kind: FleetHostErrorKind::Configuration,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetWorkerCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl FleetWorkerCommand {
    pub fn new<S, I, A>(program: S, args: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FleetWorkerStartRequest {
    pub worker_id: String,
    pub command: FleetWorkerCommand,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub env_allowlist: BTreeSet<String>,
    pub log_limit_bytes: usize,
}

impl FleetWorkerStartRequest {
    pub fn new(worker_id: impl Into<String>, command: FleetWorkerCommand) -> Self {
        Self {
            worker_id: worker_id.into(),
            command,
            cwd: None,
            env: BTreeMap::new(),
            env_allowlist: BTreeSet::new(),
            log_limit_bytes: DEFAULT_LOG_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetWorkerHandle {
    pub worker_id: String,
    pub host_kind: FleetHostKind,
    pub pid: Option<u32>,
    pub log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetHostKind {
    LocalProcess,
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetHostWorkerState {
    Running,
    Exited,
    Failed,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetHostWorkerStatus {
    pub worker_id: String,
    pub state: FleetHostWorkerState,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub memory_mb: Option<u64>,
    pub retryable: bool,
}

pub trait FleetHostAdapter {
    fn start_worker(
        &mut self,
        request: FleetWorkerStartRequest,
    ) -> FleetHostResult<FleetWorkerHandle>;
    fn read_status(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus>;
    fn read_logs(&self, worker_id: &str, max_bytes: usize) -> FleetHostResult<String>;
    fn interrupt_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus>;
    fn restart_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetWorkerHandle>;
    fn stop_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus>;
    fn cleanup_worker(&mut self, worker_id: &str) -> FleetHostResult<()>;
}

#[derive(Debug)]
pub struct LocalProcessFleetHostAdapter {
    workspace: PathBuf,
    processes: BTreeMap<String, LocalWorkerProcess>,
}

#[derive(Debug)]
struct LocalWorkerProcess {
    request: FleetWorkerStartRequest,
    child: Child,
    host_kind: FleetHostKind,
    log_path: PathBuf,
    stopped: bool,
    last_exit: Option<ExitStatus>,
    last_memory_mb: Option<u64>,
}

impl LocalProcessFleetHostAdapter {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            processes: BTreeMap::new(),
        }
    }

    fn start_with_kind(
        &mut self,
        request: FleetWorkerStartRequest,
        host_kind: FleetHostKind,
    ) -> FleetHostResult<FleetWorkerHandle> {
        validate_worker_id(&request.worker_id)?;
        if self.processes.contains_key(&request.worker_id) {
            let status = self.read_status(&request.worker_id)?;
            if matches!(status.state, FleetHostWorkerState::Running) {
                return Err(FleetHostError::terminal(format!(
                    "worker {} is already running",
                    request.worker_id
                )));
            }
            self.processes.remove(&request.worker_id);
        }

        let mut env = process_base_env();
        env.extend(filtered_env(&request.env, &request.env_allowlist)?);
        let log_path = self.log_path_for(&request.worker_id, host_kind);
        let log = open_worker_log(&log_path)?;
        let stderr = log
            .try_clone()
            .map_err(|err| FleetHostError::retryable(format!("cloning worker log: {err}")))?;

        let mut command = Command::new(&request.command.program);
        command
            .args(&request.command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .env_clear()
            .envs(env);
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }

        let child = command.spawn().map_err(|err| {
            classify_spawn_error(err, format!("starting worker {}", request.worker_id))
        })?;
        let pid = child.id();
        let handle = FleetWorkerHandle {
            worker_id: request.worker_id.clone(),
            host_kind,
            pid: Some(pid),
            log_path: log_path.clone(),
        };
        self.processes.insert(
            request.worker_id.clone(),
            LocalWorkerProcess {
                request,
                child,
                host_kind,
                log_path,
                stopped: false,
                last_exit: None,
                last_memory_mb: None,
            },
        );
        Ok(handle)
    }

    fn log_path_for(&self, worker_id: &str, host_kind: FleetHostKind) -> PathBuf {
        let host_dir = match host_kind {
            FleetHostKind::LocalProcess => "local",
            FleetHostKind::Ssh => "ssh",
        };
        self.workspace
            .join(".codewhale")
            .join("fleet-host")
            .join(host_dir)
            .join(format!("{}.log", safe_path_segment(worker_id)))
    }
}

impl FleetHostAdapter for LocalProcessFleetHostAdapter {
    fn start_worker(
        &mut self,
        request: FleetWorkerStartRequest,
    ) -> FleetHostResult<FleetWorkerHandle> {
        self.start_with_kind(request, FleetHostKind::LocalProcess)
    }

    fn read_status(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        let process = self
            .processes
            .get_mut(worker_id)
            .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
        if let Some(status) = process.last_exit {
            return Ok(status_from_exit(
                worker_id,
                Some(process.child.id()),
                status,
                process.stopped,
                process.last_memory_mb,
            ));
        }
        match process.child.try_wait() {
            Ok(None) => {
                let pid = process.child.id();
                let memory_mb = if process.host_kind == FleetHostKind::LocalProcess {
                    sample_process_memory_mb(pid)
                } else {
                    None
                };
                process.last_memory_mb = memory_mb.or(process.last_memory_mb);
                Ok(FleetHostWorkerStatus {
                    worker_id: worker_id.to_string(),
                    state: FleetHostWorkerState::Running,
                    pid: Some(pid),
                    exit_code: None,
                    // Report the retained value, not the raw sample: a
                    // transient ps failure must not flicker a live worker's
                    // memory to None (the Exited arm already does this).
                    memory_mb: process.last_memory_mb,
                    retryable: false,
                })
            }
            Ok(Some(status)) => {
                process.last_exit = Some(status);
                Ok(status_from_exit(
                    worker_id,
                    Some(process.child.id()),
                    status,
                    process.stopped,
                    process.last_memory_mb,
                ))
            }
            Err(err) => Err(FleetHostError::retryable(format!(
                "reading worker {worker_id} status: {err}"
            ))),
        }
    }

    fn read_logs(&self, worker_id: &str, max_bytes: usize) -> FleetHostResult<String> {
        let process = self
            .processes
            .get(worker_id)
            .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
        let max_bytes = max_bytes.min(process.request.log_limit_bytes.max(1));
        read_bounded_log(&process.log_path, max_bytes)
    }

    fn interrupt_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        {
            let process = self
                .processes
                .get_mut(worker_id)
                .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
            if process.last_exit.is_some() {
                return self.read_status(worker_id);
            }
            interrupt_child(&mut process.child)?;
        }
        wait_for_exit(self, worker_id, Duration::from_millis(750))
    }

    fn restart_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetWorkerHandle> {
        let request = self
            .processes
            .get(worker_id)
            .map(|process| process.request.clone())
            .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
        let _ = self.stop_worker(worker_id);
        self.processes.remove(worker_id);
        self.start_worker(request)
    }

    fn stop_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        {
            let process = self
                .processes
                .get_mut(worker_id)
                .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
            process.stopped = true;
            if process.last_exit.is_none() {
                match process.child.try_wait() {
                    Ok(Some(status)) => {
                        process.last_exit = Some(status);
                    }
                    Ok(None) => {
                        process.child.kill().map_err(|err| {
                            FleetHostError::retryable(format!("stopping worker {worker_id}: {err}"))
                        })?;
                        let status = process.child.wait().map_err(|err| {
                            FleetHostError::retryable(format!(
                                "waiting for worker {worker_id}: {err}"
                            ))
                        })?;
                        process.last_exit = Some(status);
                    }
                    Err(err) => {
                        return Err(FleetHostError::retryable(format!(
                            "reading worker {worker_id} status before stop: {err}"
                        )));
                    }
                }
            }
        }
        self.read_status(worker_id)
    }

    fn cleanup_worker(&mut self, worker_id: &str) -> FleetHostResult<()> {
        if matches!(
            self.read_status(worker_id).map(|status| status.state),
            Ok(FleetHostWorkerState::Running)
        ) {
            let _ = self.stop_worker(worker_id)?;
        }
        self.processes.remove(worker_id);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SshFleetHostConfig {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity: Option<PathBuf>,
    pub known_hosts: Option<PathBuf>,
    pub host_key_fingerprint: Option<String>,
    pub working_directory: PathBuf,
    pub env_allowlist: BTreeSet<String>,
    pub codewhale_binary: String,
    pub ssh_binary: String,
    pub connect_timeout_seconds: u64,
}

impl SshFleetHostConfig {
    pub fn new(host: impl Into<String>, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
            identity: None,
            known_hosts: None,
            host_key_fingerprint: None,
            working_directory: working_directory.into(),
            env_allowlist: BTreeSet::new(),
            codewhale_binary: "codewhale".to_string(),
            ssh_binary: "ssh".to_string(),
            connect_timeout_seconds: DEFAULT_CONNECT_TIMEOUT_SECONDS,
        }
    }

    pub fn from_host_spec(spec: &FleetHostSpec) -> FleetHostResult<Self> {
        let FleetHostSpec::Ssh {
            host,
            port,
            user,
            identity,
            known_hosts,
            host_key_fingerprint,
            working_directory,
            env_allowlist,
            codewhale_binary,
        } = spec
        else {
            return Err(FleetHostError::configuration(
                "expected SSH fleet host spec",
            ));
        };
        let working_directory = working_directory.clone().ok_or_else(|| {
            FleetHostError::configuration("SSH fleet host spec requires working_directory")
        })?;
        let codewhale_binary = codewhale_binary.clone().ok_or_else(|| {
            FleetHostError::configuration("SSH fleet host spec requires codewhale_binary")
        })?;
        let mut config = Self::new(host.clone(), working_directory);
        config.port = *port;
        config.user = user.clone();
        config.identity = identity.clone();
        config.known_hosts = known_hosts.clone();
        config.host_key_fingerprint = host_key_fingerprint.clone();
        config.env_allowlist = env_allowlist.iter().cloned().collect();
        config.codewhale_binary = codewhale_binary;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> FleetHostResult<()> {
        if self.host.trim().is_empty() {
            return Err(FleetHostError::configuration(
                "SSH fleet host requires an explicit host",
            ));
        }
        if self.codewhale_binary.trim().is_empty() {
            return Err(FleetHostError::configuration(
                "SSH fleet host requires an explicit codewhale binary path",
            ));
        }
        if self.working_directory.as_os_str().is_empty() {
            return Err(FleetHostError::configuration(
                "SSH fleet host requires an explicit working directory",
            ));
        }
        validate_env_allowlist(&self.env_allowlist)
    }

    fn target(&self) -> String {
        self.user
            .as_ref()
            .filter(|user| !user.trim().is_empty())
            .map(|user| format!("{user}@{}", self.host))
            .unwrap_or_else(|| self.host.clone())
    }
}

#[derive(Debug)]
pub struct SshFleetHostAdapter {
    config: SshFleetHostConfig,
    local: LocalProcessFleetHostAdapter,
}

impl SshFleetHostAdapter {
    pub fn new(workspace: impl AsRef<Path>, config: SshFleetHostConfig) -> FleetHostResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            local: LocalProcessFleetHostAdapter::new(workspace),
        })
    }

    pub fn build_ssh_command(
        &self,
        request: &FleetWorkerStartRequest,
    ) -> FleetHostResult<FleetWorkerCommand> {
        self.config.validate()?;
        let env = filtered_env(&request.env, &self.config.env_allowlist)?;
        let mut args = vec![
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            format!("ConnectTimeout={}", self.config.connect_timeout_seconds),
        ];
        for key in env.keys() {
            args.push("-o".to_string());
            args.push(format!("SendEnv={key}"));
        }
        if let Some(port) = self.config.port {
            args.push("-p".to_string());
            args.push(port.to_string());
        }
        if let Some(identity) = &self.config.identity {
            args.push("-i".to_string());
            args.push(identity.display().to_string());
        }
        args.push(self.config.target());
        args.push(self.remote_command(request));
        Ok(FleetWorkerCommand::new(
            self.config.ssh_binary.clone(),
            args,
        ))
    }

    fn ssh_start_request(
        &self,
        request: FleetWorkerStartRequest,
    ) -> FleetHostResult<FleetWorkerStartRequest> {
        let command = self.build_ssh_command(&request)?;
        let mut env = ssh_client_env();
        env.extend(filtered_env(&request.env, &self.config.env_allowlist)?);
        let env_allowlist = env.keys().cloned().collect();
        Ok(FleetWorkerStartRequest {
            worker_id: request.worker_id,
            command,
            cwd: None,
            env,
            env_allowlist,
            log_limit_bytes: request.log_limit_bytes,
        })
    }

    fn remote_command(&self, request: &FleetWorkerStartRequest) -> String {
        let mut parts = vec![
            "cd".to_string(),
            shell_quote(&self.config.working_directory.display().to_string()),
            "&&".to_string(),
            "exec".to_string(),
            shell_quote(&self.config.codewhale_binary),
        ];
        parts.extend(request.command.args.iter().map(|arg| shell_quote(arg)));
        parts.join(" ")
    }
}

impl FleetHostAdapter for SshFleetHostAdapter {
    fn start_worker(
        &mut self,
        request: FleetWorkerStartRequest,
    ) -> FleetHostResult<FleetWorkerHandle> {
        let request = self.ssh_start_request(request)?;
        self.local.start_with_kind(request, FleetHostKind::Ssh)
    }

    fn read_status(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        self.local.read_status(worker_id)
    }

    fn read_logs(&self, worker_id: &str, max_bytes: usize) -> FleetHostResult<String> {
        self.local.read_logs(worker_id, max_bytes)
    }

    fn interrupt_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        self.local.interrupt_worker(worker_id)
    }

    fn restart_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetWorkerHandle> {
        let request = self
            .local
            .processes
            .get(worker_id)
            .map(|process| process.request.clone())
            .ok_or_else(|| FleetHostError::terminal(format!("unknown worker {worker_id}")))?;
        let _ = self.stop_worker(worker_id);
        self.local.processes.remove(worker_id);
        self.local.start_with_kind(request, FleetHostKind::Ssh)
    }

    fn stop_worker(&mut self, worker_id: &str) -> FleetHostResult<FleetHostWorkerStatus> {
        self.local.stop_worker(worker_id)
    }

    fn cleanup_worker(&mut self, worker_id: &str) -> FleetHostResult<()> {
        self.local.cleanup_worker(worker_id)
    }
}

fn open_worker_log(path: &Path) -> FleetHostResult<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            FleetHostError::retryable(format!(
                "creating worker log dir {}: {err}",
                parent.display()
            ))
        })?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|err| FleetHostError::retryable(format!("opening worker log: {err}")))
}

fn read_bounded_log(path: &Path, max_bytes: usize) -> FleetHostResult<String> {
    let mut file = File::open(path).map_err(|err| {
        FleetHostError::retryable(format!("opening worker log {}: {err}", path.display()))
    })?;
    let len = file
        .metadata()
        .map_err(|err| FleetHostError::retryable(format!("reading worker log metadata: {err}")))?
        .len();
    let max_bytes = max_bytes.max(1) as u64;
    if len > max_bytes {
        file.seek(SeekFrom::Start(len - max_bytes))
            .map_err(|err| FleetHostError::retryable(format!("seeking worker log: {err}")))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| FleetHostError::retryable(format!("reading worker log: {err}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn status_from_exit(
    worker_id: &str,
    pid: Option<u32>,
    status: ExitStatus,
    stopped: bool,
    memory_mb: Option<u64>,
) -> FleetHostWorkerStatus {
    let success = status.success();
    FleetHostWorkerStatus {
        worker_id: worker_id.to_string(),
        state: if stopped {
            FleetHostWorkerState::Stopped
        } else if success {
            FleetHostWorkerState::Exited
        } else {
            FleetHostWorkerState::Failed
        },
        pid,
        exit_code: status.code(),
        memory_mb,
        retryable: !success && !stopped,
    }
}

#[cfg(unix)]
fn sample_process_memory_mb(pid: u32) -> Option<u64> {
    // Resolve `ps` via PATH like every other external command in the
    // codebase: /bin/ps does not exist on NixOS and some minimal containers,
    // which would silently report permanent None for live workers.
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rss_kb = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    (rss_kb > 0).then_some(rss_kb.div_ceil(1024))
}

#[cfg(not(unix))]
fn sample_process_memory_mb(_pid: u32) -> Option<u64> {
    None
}

fn classify_spawn_error(err: std::io::Error, context: String) -> FleetHostError {
    match err.kind() {
        std::io::ErrorKind::NotFound => FleetHostError::configuration(format!("{context}: {err}")),
        std::io::ErrorKind::PermissionDenied => {
            FleetHostError::terminal(format!("{context}: {err}"))
        }
        _ => FleetHostError::retryable(format!("{context}: {err}")),
    }
}

fn wait_for_exit(
    adapter: &mut LocalProcessFleetHostAdapter,
    worker_id: &str,
    timeout: Duration,
) -> FleetHostResult<FleetHostWorkerStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = adapter.read_status(worker_id)?;
        if !matches!(status.state, FleetHostWorkerState::Running) {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn interrupt_child(child: &mut Child) -> FleetHostResult<()> {
    let pid = child.id() as libc::pid_t;
    let rc = unsafe { libc::kill(pid, libc::SIGINT) };
    if rc == 0 {
        Ok(())
    } else {
        Err(FleetHostError::retryable(format!(
            "interrupting worker pid {}: {}",
            child.id(),
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(not(unix))]
fn interrupt_child(child: &mut Child) -> FleetHostResult<()> {
    child
        .kill()
        .map_err(|err| FleetHostError::retryable(format!("interrupting worker: {err}")))
}

fn filtered_env(
    env: &BTreeMap<String, String>,
    allowlist: &BTreeSet<String>,
) -> FleetHostResult<BTreeMap<String, String>> {
    validate_env_allowlist(allowlist)?;
    Ok(env
        .iter()
        .filter(|(key, _)| allowlist.contains(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

fn validate_env_allowlist(allowlist: &BTreeSet<String>) -> FleetHostResult<()> {
    for key in allowlist {
        if !is_safe_env_key(key) {
            return Err(FleetHostError::configuration(format!(
                "fleet host env allowlist key {key} looks secret-bearing; pass secrets through config providers, not worker argv/env"
            )));
        }
    }
    Ok(())
}

fn is_safe_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ![
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "CREDENTIAL",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

fn ssh_client_env() -> BTreeMap<String, String> {
    ["HOME", "PATH", "SSH_AUTH_SOCK"]
        .into_iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| (key.to_string(), value))
        })
        .collect()
}

fn process_base_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "HOME",
        "PATH",
        "SYSTEMROOT",
        "SystemRoot",
        "COMSPEC",
        "ComSpec",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    env
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn validate_worker_id(worker_id: &str) -> FleetHostResult<()> {
    if worker_id.trim().is_empty() {
        return Err(FleetHostError::configuration("worker id cannot be empty"));
    }
    Ok(())
}

fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn sample_process_memory_reports_nonzero_for_self() {
        // The current test process is alive, so its RSS must sample to Some(>0).
        let mb = sample_process_memory_mb(std::process::id());
        assert!(
            matches!(mb, Some(v) if v > 0),
            "expected Some(>0) MB for the live self process, got {mb:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sample_process_memory_is_none_for_dead_pid() {
        // Use a PID beyond every mainstream kernel's default pid ceiling
        // (Linux pid_max default 4M/32k, macOS ~99998, BSDs 99999): PID 0 is
        // kernel_task on macOS and semantically special to `ps -p`, so it is
        // not a portable "no such process" probe.
        assert_eq!(sample_process_memory_mb(999_999_999), None);
    }

    fn shell_command(script: &str) -> FleetWorkerCommand {
        if cfg!(windows) {
            FleetWorkerCommand::new("cmd", ["/C", script])
        } else {
            FleetWorkerCommand::new("sh", ["-c", script])
        }
    }

    fn wait_for_log(
        adapter: &LocalProcessFleetHostAdapter,
        worker_id: &str,
        needle: &str,
    ) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let logs = adapter.read_logs(worker_id, 4096).unwrap();
            if logs.contains(needle) || Instant::now() > deadline {
                return logs;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn fleet_host_local_adapter_starts_reads_bounded_logs_and_stops() {
        let tmp = TempDir::new().unwrap();
        let mut adapter = LocalProcessFleetHostAdapter::new(tmp.path());
        let script = if cfg!(windows) {
            "echo 0123456789abcdef& ping -n 30 127.0.0.1 >NUL"
        } else {
            "printf 0123456789abcdef; sleep 30"
        };
        let mut request = FleetWorkerStartRequest::new("local-1", shell_command(script));
        let line_ending_bytes = if cfg!(windows) { 2 } else { 0 };
        request.log_limit_bytes = 16 + line_ending_bytes;

        let handle = adapter.start_worker(request).unwrap();
        assert_eq!(handle.host_kind, FleetHostKind::LocalProcess);
        assert!(handle.pid.is_some());
        let status = adapter.read_status("local-1").unwrap();
        assert_eq!(status.state, FleetHostWorkerState::Running);

        let logs = wait_for_log(&adapter, "local-1", "abcdef");
        let logs = logs.trim_end_matches(&['\r', '\n'][..]);
        assert!(logs.ends_with("0123456789abcdef"), "{logs:?}");
        let bounded = adapter.read_logs("local-1", 6 + line_ending_bytes).unwrap();
        let bounded = bounded.trim_end_matches(&['\r', '\n'][..]);
        assert!(bounded.ends_with("abcdef"), "{bounded:?}");

        let status = adapter.stop_worker("local-1").unwrap();
        assert_eq!(status.state, FleetHostWorkerState::Stopped);
        adapter.cleanup_worker("local-1").unwrap();
        assert_eq!(
            adapter.read_status("local-1").unwrap_err().kind,
            FleetHostErrorKind::Terminal
        );
    }

    #[test]
    fn fleet_host_local_adapter_restarts_worker_with_same_request() {
        let tmp = TempDir::new().unwrap();
        let mut adapter = LocalProcessFleetHostAdapter::new(tmp.path());
        let script = if cfg!(windows) {
            "echo restart-ready & ping -n 30 127.0.0.1 >NUL"
        } else {
            "printf restart-ready; sleep 30"
        };
        let request = FleetWorkerStartRequest::new("local-restart", shell_command(script));
        let first = adapter.start_worker(request).unwrap();
        let restarted = adapter.restart_worker("local-restart").unwrap();

        assert_eq!(restarted.worker_id, first.worker_id);
        assert_eq!(restarted.host_kind, FleetHostKind::LocalProcess);
        assert_ne!(restarted.pid, first.pid);
        let logs = wait_for_log(&adapter, "local-restart", "restart-ready");
        assert!(logs.contains("restart-ready"));
        adapter.stop_worker("local-restart").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fleet_host_local_adapter_reports_running_worker_memory_usage() {
        let tmp = TempDir::new().unwrap();
        let mut adapter = LocalProcessFleetHostAdapter::new(tmp.path());
        let request =
            FleetWorkerStartRequest::new("local-memory", shell_command("printf ready; sleep 30"));

        adapter.start_worker(request).unwrap();
        let _ = wait_for_log(&adapter, "local-memory", "ready");

        let status = adapter.read_status("local-memory").unwrap();

        assert_eq!(status.state, FleetHostWorkerState::Running);
        assert!(
            status.memory_mb.is_some_and(|memory_mb| memory_mb > 0),
            "running local worker status should include RSS memory_mb, got {status:?}"
        );

        adapter.stop_worker("local-memory").unwrap();
    }

    #[test]
    fn fleet_host_ssh_kind_does_not_report_local_process_memory() {
        let tmp = TempDir::new().unwrap();
        let mut adapter = LocalProcessFleetHostAdapter::new(tmp.path());
        let script = if cfg!(windows) {
            "echo ready & ping -n 30 127.0.0.1 >NUL"
        } else {
            "printf ready; sleep 30"
        };
        let request = FleetWorkerStartRequest::new("ssh-memory", shell_command(script));

        adapter
            .start_with_kind(request, FleetHostKind::Ssh)
            .unwrap();
        let _ = wait_for_log(&adapter, "ssh-memory", "ready");

        let status = adapter.read_status("ssh-memory").unwrap();

        assert_eq!(status.state, FleetHostWorkerState::Running);
        assert_eq!(status.memory_mb, None);

        adapter.stop_worker("ssh-memory").unwrap();
    }

    #[test]
    fn fleet_host_rejects_secret_like_env_allowlist_keys() {
        let mut env = BTreeMap::new();
        env.insert("DEEPSEEK_API_KEY".to_string(), "secret".to_string());
        let allowlist = BTreeSet::from(["DEEPSEEK_API_KEY".to_string()]);

        let err = filtered_env(&env, &allowlist).unwrap_err();

        assert_eq!(err.kind, FleetHostErrorKind::Configuration);
        assert!(err.message.contains("looks secret-bearing"));
    }

    #[test]
    fn fleet_host_ssh_command_uses_sendenv_without_argv_secret_values() {
        let tmp = TempDir::new().unwrap();
        let mut config = SshFleetHostConfig::new("builder.example.test", "/srv/codewhale");
        config.user = Some("fleet".to_string());
        config.port = Some(2222);
        config.identity = Some(PathBuf::from("/tmp/fleet_id"));
        config.codewhale_binary = "/usr/local/bin/codewhale".to_string();
        config.env_allowlist = BTreeSet::from(["FLEET_PROFILE".to_string()]);
        let adapter = SshFleetHostAdapter::new(tmp.path(), config).unwrap();
        let mut request = FleetWorkerStartRequest::new(
            "ssh-1",
            FleetWorkerCommand::new("codewhale", ["fleet-worker", "noop"]),
        );
        request.env.insert(
            "FLEET_PROFILE".to_string(),
            "super-secret-profile-value".to_string(),
        );

        let command = adapter.build_ssh_command(&request).unwrap();
        let argv = command.args.join(" ");

        assert_eq!(command.program, "ssh");
        assert!(argv.contains("BatchMode=yes"));
        assert!(argv.contains("SendEnv=FLEET_PROFILE"));
        assert!(argv.contains("fleet@builder.example.test"));
        assert!(argv.contains("/usr/local/bin/codewhale"));
        assert!(argv.contains("fleet-worker"));
        assert!(!argv.contains("super-secret-profile-value"));
    }

    #[test]
    fn fleet_host_ssh_config_requires_explicit_safe_fields() {
        let tmp = TempDir::new().unwrap();
        let mut config = SshFleetHostConfig::new("", "/srv/codewhale");
        config.env_allowlist = BTreeSet::from(["SAFE_FLAG".to_string()]);

        let err = SshFleetHostAdapter::new(tmp.path(), config).unwrap_err();

        assert_eq!(err.kind, FleetHostErrorKind::Configuration);
        assert!(err.message.contains("explicit host"));
    }

    #[test]
    fn fleet_host_ssh_config_maps_from_protocol_host_spec() {
        let spec = FleetHostSpec::Ssh {
            host: "builder.example.test".to_string(),
            port: Some(2222),
            user: Some("fleet".to_string()),
            identity: Some(PathBuf::from("/tmp/fleet_id")),
            known_hosts: None,
            host_key_fingerprint: None,
            working_directory: Some(PathBuf::from("/srv/codewhale")),
            env_allowlist: vec!["FLEET_PROFILE".to_string()],
            codewhale_binary: Some("/usr/local/bin/codewhale".to_string()),
        };

        let config = SshFleetHostConfig::from_host_spec(&spec).unwrap();

        assert_eq!(config.host, "builder.example.test");
        assert_eq!(config.port, Some(2222));
        assert_eq!(config.user.as_deref(), Some("fleet"));
        assert_eq!(config.working_directory, PathBuf::from("/srv/codewhale"));
        assert!(config.env_allowlist.contains("FLEET_PROFILE"));
        assert_eq!(config.codewhale_binary, "/usr/local/bin/codewhale");
    }
}
