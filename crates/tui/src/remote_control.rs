//! Account-owned remote control for the active TUI session.
//!
//! This is deliberately a typed relay, not a remote shell. The control plane
//! may send prompts, approval decisions, and run-control requests for the exact
//! enrolled target. Provider credentials, paths, environment variables, and
//! arbitrary command strings never cross this boundary.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::Url;
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    core::events::{Event as EngineEvent, TurnOutcomeStatus},
    models::{ContentBlock, Message},
};

const PRODUCTION_CONTROL_PLANE: &str = "https://api.codewhale.net/";
const ENROLLMENT_SECRET_SLOT: &str = "cwc-remote-control-enrollment-v1";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const SYNC_INTERVAL: Duration = Duration::from_millis(1_200);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RUNS: usize = 64;
const MAX_COMMANDS: usize = 128;
const CAPABILITIES: &[&str] = &["evidence-ledger", "fim", "git", "shell"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlAction {
    Start,
    Stop,
}

#[derive(Debug, Clone)]
pub struct RemoteStart {
    pub workspace_label: String,
    pub target_ref: String,
    pub session_id: String,
    pub runtime_version: String,
    pub runtime_commit: String,
}

#[derive(Debug, Clone)]
pub enum RemoteEvent {
    Notice(String),
    Connected {
        account_ref: String,
        runner_id: String,
        target_ref: String,
    },
    Command {
        run_id: String,
        seq: u64,
        command: RemoteCommand,
    },
    Failed(String),
    Stopped,
    OwnershipRestored {
        approvals: Vec<PendingRemoteApproval>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    Prompt {
        turn_id: String,
        prompt: String,
    },
    Approval {
        gate: String,
        approved: bool,
    },
    Control {
        action: RemoteControlRequest,
        turn_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRequest {
    Interrupt,
    Cancel,
}

#[derive(Debug, Clone)]
enum WorkerCommand {
    Upload {
        run_id: String,
        acknowledgements: Vec<CommandAcknowledgement>,
        envelopes: Vec<Value>,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandAcknowledgement {
    command_seq: u64,
    command_type: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedEnrollment {
    schema_version: u64,
    control_plane_base: String,
    runner_enrollment_id: String,
    account_ref: String,
    device_id: String,
    target_ref: String,
    target_grant_ref: String,
    runtime_version: String,
    runtime_commit: String,
    bootstrap_secret: String,
}

#[derive(Debug, Clone)]
struct LiveEnrollment {
    persisted: PersistedEnrollment,
    access_token: String,
}

#[derive(Debug, Clone)]
struct ActiveRelayRun {
    run_id: String,
    turn_id: String,
}

#[derive(Debug, Clone)]
pub struct PendingRemoteApproval {
    pub tool_id: String,
    pub tool_name: String,
    pub description: String,
    pub input: Value,
    pub approval_key: String,
    pub intent_summary: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Status {
    #[default]
    Off,
    Connecting,
    Connected,
    Stopping,
    Failed,
}

/// UI-thread owner for remote-control state and typed transport channels.
pub struct RemoteControlController {
    status: Status,
    status_detail: String,
    account_ref: Option<String>,
    target_ref: Option<String>,
    active_run: Option<ActiveRelayRun>,
    event_seq: HashMap<String, u64>,
    uploaded_snapshots: HashSet<String>,
    pending_approvals: HashMap<String, PendingRemoteApproval>,
    command_fingerprints: HashMap<(String, u64), String>,
    worker_tx: Option<mpsc::UnboundedSender<WorkerCommand>>,
    event_rx: Option<mpsc::UnboundedReceiver<RemoteEvent>>,
    worker: Option<tokio::task::JoinHandle<()>>,
    applying_remote_command: bool,
    ownership_blocked_until: Option<Instant>,
}

impl Default for RemoteControlController {
    fn default() -> Self {
        Self {
            status: Status::Off,
            status_detail: "off".to_string(),
            account_ref: None,
            target_ref: None,
            active_run: None,
            event_seq: HashMap::new(),
            uploaded_snapshots: HashSet::new(),
            pending_approvals: HashMap::new(),
            command_fingerprints: HashMap::new(),
            worker_tx: None,
            event_rx: None,
            worker: None,
            applying_remote_command: false,
            ownership_blocked_until: None,
        }
    }
}

impl RemoteControlController {
    pub fn start(&mut self, start: RemoteStart) -> Result<(), String> {
        if matches!(
            self.status,
            Status::Connecting | Status::Connected | Status::Stopping
        ) {
            return Err("Remote control is already active.".to_string());
        }
        if self.status == Status::Failed
            && self
                .ownership_blocked_until
                .is_some_and(|deadline| Instant::now() < deadline)
        {
            return Err(
                "The previous remote lease may still be active; wait for ownership to return before reconnecting."
                    .to_string(),
            );
        }
        if !valid_runtime_version(&start.runtime_version)
            || !valid_runtime_commit(&start.runtime_commit)
            || !valid_opaque_ref(&start.target_ref)
            || !valid_opaque_ref(&start.session_id)
        {
            return Err("This build or session does not have an enrollable identity.".to_string());
        }
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.stop_worker();
        self.status = Status::Connecting;
        self.status_detail = "waiting for account authorization".to_string();
        self.target_ref = Some(start.target_ref.clone());
        self.worker_tx = Some(worker_tx);
        self.event_rx = Some(event_rx);
        self.worker = Some(tokio::spawn(async move {
            if let Err(error) = relay_worker(start, worker_rx, event_tx.clone()).await {
                let _ = event_tx.send(RemoteEvent::Failed(error));
            }
        }));
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.status == Status::Connecting {
            // The worker may have completed its server-side connect just before
            // the UI consumed RemoteEvent::Connected. Aborting it cannot prove
            // that no lease exists, so retain the ownership lock through the
            // server expiry instead of returning local input immediately.
            self.stop_worker();
            self.status = Status::Failed;
            self.status_detail =
                "authorization cancelled; waiting for any server lease to expire safely"
                    .to_string();
            self.ownership_blocked_until = Some(Instant::now() + Duration::from_secs(95));
        } else if self.status == Status::Connected {
            let queued = self
                .worker_tx
                .as_ref()
                .is_some_and(|tx| tx.send(WorkerCommand::Stop).is_ok());
            self.worker_tx = None;
            if queued {
                self.status = Status::Stopping;
                self.status_detail = "confirming the runner is offline".to_string();
            } else {
                self.status = Status::Failed;
                self.status_detail =
                    "waiting for the last server lease to expire safely".to_string();
                self.ownership_blocked_until = Some(Instant::now() + Duration::from_secs(95));
            }
        }
        if self.status == Status::Off {
            self.account_ref = None;
            self.active_run = None;
            self.pending_approvals.clear();
            self.command_fingerprints.clear();
            self.ownership_blocked_until = None;
        }
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        self.worker_tx = None;
        self.event_rx = None;
    }

    pub fn try_next_event(&mut self) -> Option<RemoteEvent> {
        if self.status == Status::Failed
            && self
                .ownership_blocked_until
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            let approvals = self
                .pending_approvals
                .drain()
                .map(|(_, value)| value)
                .collect();
            self.stop_worker();
            self.status = Status::Off;
            self.status_detail = "off".to_string();
            self.ownership_blocked_until = None;
            return Some(RemoteEvent::OwnershipRestored { approvals });
        }
        let event = self.event_rx.as_mut()?.try_recv().ok()?;
        match &event {
            RemoteEvent::Connected {
                account_ref,
                target_ref,
                ..
            } => {
                self.status = Status::Connected;
                self.status_detail = "web owns prompts and approvals".to_string();
                self.ownership_blocked_until = None;
                self.account_ref = Some(account_ref.clone());
                self.target_ref = Some(target_ref.clone());
            }
            RemoteEvent::Failed(reason) => {
                self.status = Status::Failed;
                self.status_detail =
                    format!("{reason}; waiting for the last server lease to expire safely");
                self.ownership_blocked_until = Some(Instant::now() + Duration::from_secs(95));
                self.active_run = None;
                // The worker may have failed after the UI queued a snapshot but before the
                // control plane durably accepted it. A later attachment must be allowed to
                // send a fresh bounded snapshot for that run.
                self.uploaded_snapshots.clear();
            }
            RemoteEvent::Stopped => {
                self.status = Status::Off;
                self.status_detail = "off".to_string();
                self.active_run = None;
                self.ownership_blocked_until = None;
                if !self.pending_approvals.is_empty() {
                    let approvals = self
                        .pending_approvals
                        .drain()
                        .map(|(_, value)| value)
                        .collect();
                    return Some(RemoteEvent::OwnershipRestored { approvals });
                }
            }
            RemoteEvent::Notice(_)
            | RemoteEvent::Command { .. }
            | RemoteEvent::OwnershipRestored { .. } => {}
        }
        Some(event)
    }

    pub fn status_line(&self) -> String {
        match self.status {
            Status::Off => "Remote control: off".to_string(),
            Status::Connecting => format!("Remote control: connecting · {}", self.status_detail),
            Status::Connected => format!(
                "Remote control: connected · account {} · {}",
                self.account_ref.as_deref().unwrap_or("account"),
                self.status_detail
            ),
            Status::Stopping => {
                "Remote control: stopping · confirming the runner is offline".to_string()
            }
            Status::Failed => format!("Remote control: disconnected · {}", self.status_detail),
        }
    }

    pub fn blocks_local_input(&self) -> bool {
        let server_may_still_own = match self.status {
            Status::Connecting | Status::Connected | Status::Stopping => true,
            Status::Failed => self
                .ownership_blocked_until
                .is_some_and(|deadline| Instant::now() < deadline),
            Status::Off => false,
        };
        server_may_still_own && !self.applying_remote_command
    }

    pub fn set_applying_remote_command(&mut self, value: bool) {
        self.applying_remote_command = value;
    }

    pub fn claim_command(
        &mut self,
        run_id: &str,
        seq: u64,
        command: &RemoteCommand,
    ) -> Result<bool, String> {
        let fingerprint = command_fingerprint(command);
        let key = (run_id.to_string(), seq);
        if let Some(existing) = self.command_fingerprints.get(&key) {
            if existing == &fingerprint {
                return Ok(false);
            }
            return Err(
                "The control plane reused a command sequence with different content.".to_string(),
            );
        }
        self.command_fingerprints.insert(key, fingerprint);
        Ok(true)
    }

    pub fn activate_prompt(&mut self, run_id: &str, turn_id: &str) {
        self.active_run = Some(ActiveRelayRun {
            run_id: run_id.to_string(),
            turn_id: turn_id.to_string(),
        });
    }

    pub fn active_run_matches(&self, run_id: &str) -> bool {
        self.active_run
            .as_ref()
            .is_some_and(|active| active.run_id == run_id)
    }

    pub fn upload_snapshot(&mut self, run_id: &str, messages: &[Message]) {
        if !self.uploaded_snapshots.insert(run_id.to_string()) {
            return;
        }
        let projected = project_session_messages(messages);
        self.upload_envelope(
            run_id,
            "session.snapshot",
            None,
            json!({ "messages": projected }),
        );
    }

    pub fn acknowledge(
        &self,
        run_id: &str,
        seq: u64,
        command: &RemoteCommand,
        status: &str,
        error: Option<String>,
    ) {
        let Some(tx) = &self.worker_tx else {
            return;
        };
        let _ = tx.send(WorkerCommand::Upload {
            run_id: run_id.to_string(),
            acknowledgements: vec![CommandAcknowledgement {
                command_seq: seq,
                command_type: command.kind().to_string(),
                status: status.to_string(),
                turn_id: command.turn_id().map(ToString::to_string),
                error: error.map(|value| value.chars().take(800).collect()),
            }],
            envelopes: Vec::new(),
        });
    }

    pub fn record_remote_approval(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        description: &str,
        input: &Value,
        approval_key: &str,
        intent_summary: Option<&str>,
    ) -> String {
        let gate = projected_approval_id(tool_id);
        self.pending_approvals.insert(
            gate.clone(),
            PendingRemoteApproval {
                tool_id: tool_id.to_string(),
                tool_name: tool_name.to_string(),
                description: description.to_string(),
                input: input.clone(),
                approval_key: approval_key.to_string(),
                intent_summary: intent_summary.map(ToString::to_string),
            },
        );
        if let Some(active) = self.active_run.clone() {
            self.upload_envelope(
                &active.run_id,
                "approval.required",
                Some(&active.turn_id),
                json!({
                    "id": gate,
                    "approval_id": gate,
                    "tool_name": tool_name,
                    "description": description,
                }),
            );
        }
        gate
    }

    pub fn take_pending_approval(&mut self, gate: &str) -> Option<String> {
        self.pending_approvals
            .remove(gate)
            .map(|approval| approval.tool_id)
    }

    pub fn observe_engine_event(&mut self, event: &EngineEvent) {
        let Some(active) = self.active_run.clone() else {
            return;
        };
        match event {
            EngineEvent::MessageDelta { content, .. } => self.upload_envelope(
                &active.run_id,
                "item.delta",
                Some(&active.turn_id),
                json!({ "kind": "agent_message", "delta": content }),
            ),
            EngineEvent::ToolCallStarted { id, name, .. } => self.upload_envelope(
                &active.run_id,
                "item.started",
                Some(&active.turn_id),
                json!({ "tool": { "id": id, "name": name, "input": {} } }),
            ),
            EngineEvent::ToolCallComplete { id, result, .. } => {
                let (event_name, status) = if result.is_ok() {
                    ("item.completed", "completed")
                } else {
                    ("item.failed", "failed")
                };
                self.upload_envelope(
                    &active.run_id,
                    event_name,
                    Some(&active.turn_id),
                    json!({
                        "item": {
                            "id": id,
                            "kind": "tool_call",
                            "status": status,
                            "summary": "",
                            "detail": "",
                        }
                    }),
                );
            }
            EngineEvent::TurnStarted { turn_id, route, .. } => {
                self.active_run = Some(ActiveRelayRun {
                    run_id: active.run_id.clone(),
                    turn_id: turn_id.clone(),
                });
                self.upload_envelope(
                    &active.run_id,
                    "turn.started",
                    Some(turn_id),
                    json!({
                        "turn": {
                            "model": route.as_ref().map(|value| value.model.as_str()).unwrap_or(""),
                            "mode": "",
                        }
                    }),
                );
            }
            EngineEvent::TurnComplete { usage, status, .. } => {
                let status = match status {
                    TurnOutcomeStatus::Completed => "completed",
                    TurnOutcomeStatus::Interrupted => "interrupted",
                    TurnOutcomeStatus::Failed => "failed",
                };
                self.upload_envelope(
                    &active.run_id,
                    "turn.completed",
                    Some(&active.turn_id),
                    json!({ "turn": { "status": status, "usage": usage } }),
                );
                self.active_run = None;
            }
            _ => {}
        }
    }

    fn upload_envelope(
        &mut self,
        run_id: &str,
        event: &str,
        turn_id: Option<&str>,
        payload: Value,
    ) {
        let seq = self.event_seq.entry(run_id.to_string()).or_insert(0);
        *seq = seq.saturating_add(1);
        let Some(tx) = &self.worker_tx else {
            return;
        };
        let _ = tx.send(WorkerCommand::Upload {
            run_id: run_id.to_string(),
            acknowledgements: Vec::new(),
            envelopes: vec![json!({
                "schema_version": 1,
                "seq": *seq,
                "event": event,
                "kind": event,
                "turn_id": turn_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "payload": payload,
            })],
        });
    }
}

impl Drop for RemoteControlController {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl RemoteCommand {
    fn kind(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt.request",
            Self::Approval { .. } => "approval.decision",
            Self::Control { .. } => "run.control",
        }
    }

    fn turn_id(&self) -> Option<&str> {
        match self {
            Self::Prompt { turn_id, .. } => Some(turn_id),
            Self::Control { turn_id, .. } => turn_id.as_deref(),
            Self::Approval { .. } => None,
        }
    }
}

pub fn target_ref(workspace: &Path, session_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(session_id.as_bytes());
    format!("target_{}", &bytes_to_hex(&hasher.finalize())[..32])
}

fn project_session_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "user" => "user",
                "assistant" => "assistant",
                _ => return None,
            };
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then(|| {
                json!({
                    "role": role,
                    "text": text.chars().take(16 * 1024).collect::<String>(),
                })
            })
        })
        .rev()
        .take(64)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn projected_approval_id(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"local-runtime:approval\0");
    hasher.update(raw.as_bytes());
    format!("local_approval_{}", &bytes_to_hex(&hasher.finalize())[..24])
}

fn command_fingerprint(command: &RemoteCommand) -> String {
    let canonical = format!("{command:?}");
    bytes_to_hex(&Sha256::digest(canonical.as_bytes()))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn relay_worker(
    start: RemoteStart,
    mut worker_rx: mpsc::UnboundedReceiver<WorkerCommand>,
    event_tx: mpsc::UnboundedSender<RemoteEvent>,
) -> Result<(), String> {
    let base = runner_control_plane_base()?;
    let client = Client::builder()
        .https_only(!cfg!(debug_assertions))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Remote control could not initialize secure networking.".to_string())?;

    let mut enrollment = match load_persisted_enrollment()? {
        Some(saved) if saved.matches(&start, &base) => {
            match refresh_enrollment(&client, saved).await {
                Ok(enrollment) => enrollment,
                Err(error) if error == "runner_enrollment_revoked" => {
                    delete_persisted_enrollment();
                    enroll_device(&client, &base, &start, &event_tx).await?
                }
                Err(error) => return Err(error),
            }
        }
        Some(_) => {
            delete_persisted_enrollment();
            enroll_device(&client, &base, &start, &event_tx).await?
        }
        None => enroll_device(&client, &base, &start, &event_tx).await?,
    };

    let mut runner_id = connect_runner(&client, &enrollment, &start).await?;
    let _ = event_tx.send(RemoteEvent::Connected {
        account_ref: enrollment.persisted.account_ref.clone(),
        runner_id: runner_id.clone(),
        target_ref: start.target_ref.clone(),
    });
    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut command_cursor: HashMap<String, u64> = HashMap::new();
    let mut delivered: HashMap<(String, u64), String> = HashMap::new();

    loop {
        tokio::select! {
            command = worker_rx.recv() => {
                match command {
                    Some(WorkerCommand::Upload { run_id, acknowledgements, envelopes }) => {
                        runner_request(
                            &client,
                            &enrollment,
                            Method::POST,
                            &["api", "local-runners", &runner_id, "runs", &run_id, "events"],
                            &[],
                            Some(json!({ "acknowledgements": acknowledgements, "envelopes": envelopes })),
                        ).await?;
                    }
                    Some(WorkerCommand::Stop) | None => {
                        // Do not return local input until the control plane has
                        // durably released this lease. If the confirmation
                        // cannot be delivered, the UI keeps ownership locked
                        // through the server-side lease expiry instead.
                        post_heartbeat(&client, &enrollment, &runner_id, &start, "offline").await?;
                        let _ = event_tx.send(RemoteEvent::Stopped);
                        return Ok(());
                    }
                }
            }
            () = tokio::time::sleep(SYNC_INTERVAL) => {
                if enrollment_needs_refresh(&enrollment) {
                    enrollment = refresh_enrollment(&client, enrollment.persisted.clone()).await?;
                    runner_id = connect_runner(&client, &enrollment, &start).await?;
                }
                if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                    post_heartbeat(&client, &enrollment, &runner_id, &start, "active").await?;
                    last_heartbeat = Instant::now();
                }
                let runs = list_runs(&client, &enrollment, &runner_id).await?;
                for run_id in runs {
                    let since = command_cursor.get(&run_id).copied().unwrap_or(0);
                    for listed in list_commands(&client, &enrollment, &runner_id, &run_id, since).await? {
                        let seq = listed.seq;
                        if !listed.ack_status.is_empty() {
                            if listed.ack_status == "accepted" {
                                recover_run(
                                    &client,
                                    &enrollment,
                                    &runner_id,
                                    &run_id,
                                    "accepted command has no terminal acknowledgement after runner restart",
                                ).await?;
                            }
                            command_cursor.insert(run_id.clone(), seq);
                            continue;
                        }
                        let command = parse_remote_command(&listed.command, &run_id)?;
                        let fingerprint = command_fingerprint(&command);
                        let key = (run_id.clone(), seq);
                        if let Some(existing) = delivered.get(&key) {
                            if existing != &fingerprint {
                                return Err("The control plane replayed a changed command sequence.".to_string());
                            }
                        } else {
                            delivered.insert(key, fingerprint);
                            upload_command_accepted(
                                &client,
                                &enrollment,
                                &runner_id,
                                &run_id,
                                seq,
                                &command,
                            ).await?;
                            event_tx.send(RemoteEvent::Command {
                                run_id: run_id.clone(),
                                seq,
                                command,
                            }).map_err(|_| "The terminal remote-control owner stopped.".to_string())?;
                        }
                        command_cursor.insert(run_id.clone(), seq.max(since));
                    }
                }
            }
        }
    }
}

impl PersistedEnrollment {
    fn matches(&self, start: &RemoteStart, base: &str) -> bool {
        self.schema_version == 1
            && self.control_plane_base == base
            && self.target_ref == start.target_ref
            && self.runtime_version == start.runtime_version
            && self.runtime_commit == start.runtime_commit
            && valid_opaque_ref(&self.runner_enrollment_id)
            && valid_opaque_ref(&self.account_ref)
            && valid_opaque_ref(&self.device_id)
            && valid_opaque_ref(&self.target_grant_ref)
            && valid_secret(&self.bootstrap_secret)
    }
}

async fn enroll_device(
    client: &Client,
    base: &str,
    start: &RemoteStart,
    event_tx: &mpsc::UnboundedSender<RemoteEvent>,
) -> Result<LiveEnrollment, String> {
    let device_id = format!("device_{}", uuid::Uuid::new_v4().simple());
    let value = public_request(
        client,
        Method::POST,
        control_plane_url(base, &["api", "runner", "device", "start"], &[])?,
        json!({
            "deviceId": device_id,
            "deviceLabel": "Codewhale terminal",
            "targetRef": start.target_ref,
            "targetLabel": start.workspace_label,
            "runtimeVersion": start.runtime_version,
            "runtimeCommit": start.runtime_commit,
            "capabilities": CAPABILITIES,
        }),
    )
    .await?;
    let device_code = secret_field(&value, "deviceCode")?;
    let user_code = string_field(&value, "userCode")?;
    let verification_uri = string_field(&value, "verificationUriComplete")?;
    let interval = value
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|value| (1..=30).contains(value))
        .ok_or_else(|| {
            "Codewhale returned an invalid device authorization interval.".to_string()
        })?;
    let expires_in = value
        .get("expiresIn")
        .and_then(Value::as_u64)
        .filter(|value| (60..=1800).contains(value))
        .ok_or_else(|| "Codewhale returned an invalid device authorization expiry.".to_string())?;
    validate_authorization_url(&verification_uri, &user_code)?;
    let _ = event_tx.send(RemoteEvent::Notice(format!(
        "Authorize this terminal at {verification_uri} (code {user_code})."
    )));
    let _ = webbrowser::open(&verification_uri);
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    loop {
        if Instant::now() >= deadline {
            return Err("Remote-control authorization expired; run /rc again.".to_string());
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let response = client
            .post(control_plane_url(
                base,
                &["api", "runner", "device", "token"],
                &[],
            )?)
            .json(&json!({ "deviceCode": device_code }))
            .send()
            .await
            .map_err(|_| "Remote-control authorization could not reach Codewhale.".to_string())?;
        if response.status() == StatusCode::ACCEPTED {
            continue;
        }
        if !response.status().is_success() {
            return Err("Remote-control authorization was rejected.".to_string());
        }
        let exchange = read_bounded_json(response).await?;
        let enrollment = enrollment_from_exchange(exchange, base, &device_id, start)?;
        save_persisted_enrollment(&enrollment.persisted)?;
        return Ok(enrollment);
    }
}

fn enrollment_from_exchange(
    value: Value,
    base: &str,
    device_id: &str,
    start: &RemoteStart,
) -> Result<LiveEnrollment, String> {
    if value.get("status").and_then(Value::as_str) != Some("approved") {
        return Err("Codewhale returned an invalid runner credential.".to_string());
    }
    let record = value
        .get("enrollment")
        .filter(|value| value.is_object())
        .ok_or_else(|| "Codewhale returned an invalid runner credential.".to_string())?;
    let enrollment_id = opaque_field(record, "id")?;
    let account_ref = opaque_field(record, "userId")?;
    let returned_device = opaque_field(record, "deviceId")?;
    if returned_device != device_id
        || record.get("runtimeVersion").and_then(Value::as_str)
            != Some(start.runtime_version.as_str())
        || record.get("runtimeCommit").and_then(Value::as_str)
            != Some(start.runtime_commit.as_str())
        || !exact_capabilities(record.get("capabilities"))
    {
        return Err("The runner credential does not match this terminal.".to_string());
    }
    let target_grant_ref = record
        .get("targetGrants")
        .and_then(Value::as_array)
        .and_then(|grants| {
            grants.iter().find(|grant| {
                grant.get("targetRef").and_then(Value::as_str) == Some(start.target_ref.as_str())
                    && grant
                        .get("revokedAt")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty()
            })
        })
        .and_then(|grant| grant.get("grantId"))
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .ok_or_else(|| "Codewhale returned no grant for this session.".to_string())?
        .to_string();
    Ok(LiveEnrollment {
        persisted: PersistedEnrollment {
            schema_version: 1,
            control_plane_base: base.to_string(),
            runner_enrollment_id: enrollment_id,
            account_ref,
            device_id: returned_device,
            target_ref: start.target_ref.clone(),
            target_grant_ref,
            runtime_version: start.runtime_version.clone(),
            runtime_commit: start.runtime_commit.clone(),
            bootstrap_secret: secret_field(&value, "bootstrapSecret")?,
        },
        access_token: access_token(&value)?,
    })
}

async fn refresh_enrollment(
    client: &Client,
    persisted: PersistedEnrollment,
) -> Result<LiveEnrollment, String> {
    let url = control_plane_url(
        &persisted.control_plane_base,
        &["api", "runner", "enrollments", "token"],
        &[],
    )?;
    let response = client
        .post(url)
        .json(&json!({
            "enrollmentId": persisted.runner_enrollment_id,
            "bootstrapSecret": persisted.bootstrap_secret,
        }))
        .send()
        .await
        .map_err(|_| "Remote-control credential refresh could not reach Codewhale.".to_string())?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err("runner_enrollment_revoked".to_string());
    }
    if !response.status().is_success() {
        return Err("Remote-control credential refresh was rejected.".to_string());
    }
    let value = read_bounded_json(response).await?;
    let record = value
        .get("enrollment")
        .filter(|value| value.is_object())
        .ok_or_else(|| "Codewhale returned an invalid refreshed credential.".to_string())?;
    if record.get("id").and_then(Value::as_str) != Some(persisted.runner_enrollment_id.as_str())
        || record.get("userId").and_then(Value::as_str) != Some(persisted.account_ref.as_str())
        || record.get("deviceId").and_then(Value::as_str) != Some(persisted.device_id.as_str())
        || record.get("runtimeVersion").and_then(Value::as_str)
            != Some(persisted.runtime_version.as_str())
        || record.get("runtimeCommit").and_then(Value::as_str)
            != Some(persisted.runtime_commit.as_str())
        || !exact_capabilities(record.get("capabilities"))
    {
        return Err("Codewhale returned a mismatched refreshed credential.".to_string());
    }
    Ok(LiveEnrollment {
        persisted,
        access_token: access_token(&value)?,
    })
}

async fn connect_runner(
    client: &Client,
    enrollment: &LiveEnrollment,
    start: &RemoteStart,
) -> Result<String, String> {
    let value = runner_request(
        client,
        enrollment,
        Method::POST,
        &["api", "local-runners", "connect"],
        &[],
        Some(json!({
            "deviceId": enrollment.persisted.device_id,
            "targetRef": start.target_ref,
            "displayLabel": start.workspace_label,
            "runtimeVersion": start.runtime_version,
            "runtimeCommit": start.runtime_commit,
            "capabilities": CAPABILITIES,
            "status": "active",
        })),
    )
    .await?;
    value
        .get("runner")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .map(ToString::to_string)
        .ok_or_else(|| "Codewhale returned an invalid runner lease.".to_string())
}

async fn post_heartbeat(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
    start: &RemoteStart,
    status: &str,
) -> Result<(), String> {
    runner_request(
        client,
        enrollment,
        Method::POST,
        &["api", "local-runners", runner_id, "heartbeat"],
        &[],
        Some(json!({
            "runtimeVersion": start.runtime_version,
            "runtimeCommit": start.runtime_commit,
            "capabilities": CAPABILITIES,
            "status": status,
        })),
    )
    .await
    .map(|_| ())
}

async fn list_runs(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
) -> Result<Vec<String>, String> {
    let value = runner_request(
        client,
        enrollment,
        Method::GET,
        &["api", "local-runners", runner_id, "runs"],
        &[],
        None,
    )
    .await?;
    let runs = value
        .get("runs")
        .and_then(Value::as_array)
        .filter(|runs| runs.len() <= MAX_RUNS)
        .ok_or_else(|| "Codewhale returned an invalid runner run list.".to_string())?;
    runs.iter()
        .map(|run| {
            run.get("id")
                .and_then(Value::as_str)
                .filter(|value| valid_opaque_ref(value))
                .map(ToString::to_string)
                .ok_or_else(|| "Codewhale returned an invalid runner run.".to_string())
        })
        .collect()
}

async fn list_commands(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
    run_id: &str,
    since: u64,
) -> Result<Vec<ListedCommand>, String> {
    let value = runner_request(
        client,
        enrollment,
        Method::GET,
        &[
            "api",
            "local-runners",
            runner_id,
            "runs",
            run_id,
            "commands",
        ],
        &[
            ("since_seq", since.to_string()),
            ("include_accepted", "1".to_string()),
        ],
        None,
    )
    .await?;
    let commands = value
        .get("commands")
        .and_then(Value::as_array)
        .filter(|commands| commands.len() <= MAX_COMMANDS)
        .ok_or_else(|| "Codewhale returned an invalid command list.".to_string())?;
    commands
        .iter()
        .map(|item| {
            let seq = item
                .get("seq")
                .and_then(Value::as_u64)
                .filter(|value| *value > since)
                .ok_or_else(|| "Codewhale returned an invalid command sequence.".to_string())?;
            let command = item
                .get("command")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or_else(|| "Codewhale returned an invalid typed command.".to_string())?;
            Ok(ListedCommand {
                seq,
                command,
                ack_status: item
                    .get("ackStatus")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

struct ListedCommand {
    seq: u64,
    command: Value,
    ack_status: String,
}

async fn upload_command_accepted(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
    run_id: &str,
    seq: u64,
    command: &RemoteCommand,
) -> Result<(), String> {
    runner_request(
        client,
        enrollment,
        Method::POST,
        &["api", "local-runners", runner_id, "runs", run_id, "events"],
        &[],
        Some(json!({
            "acknowledgements": [{
                "commandSeq": seq,
                "commandType": command.kind(),
                "status": "accepted",
                "turnId": command.turn_id(),
            }],
            "envelopes": [],
        })),
    )
    .await
    .map(|_| ())
}

async fn recover_run(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
    run_id: &str,
    reason: &str,
) -> Result<(), String> {
    runner_request(
        client,
        enrollment,
        Method::POST,
        &[
            "api",
            "local-runners",
            runner_id,
            "runs",
            run_id,
            "recovery",
        ],
        &[],
        Some(json!({ "reason": reason })),
    )
    .await
    .map(|_| ())
}

fn parse_remote_command(value: &Value, expected_run_id: &str) -> Result<RemoteCommand, String> {
    if value.get("runId").and_then(Value::as_str) != Some(expected_run_id) {
        return Err("A remote command targeted a different run.".to_string());
    }
    match value.get("type").and_then(Value::as_str) {
        Some("prompt.request") => {
            let turn_id = value
                .get("turnId")
                .and_then(Value::as_str)
                .filter(|value| valid_opaque_ref(value))
                .ok_or_else(|| "A remote prompt had no valid turn id.".to_string())?;
            let prompt = value
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 128 * 1024)
                .ok_or_else(|| "A remote prompt was empty or oversized.".to_string())?;
            Ok(RemoteCommand::Prompt {
                turn_id: turn_id.to_string(),
                prompt: prompt.to_string(),
            })
        }
        Some("approval.decision") => {
            let gate = value
                .get("gate")
                .and_then(Value::as_str)
                .filter(|value| valid_opaque_ref(value))
                .ok_or_else(|| "A remote approval had no valid gate id.".to_string())?;
            let approved = match value.get("decision").and_then(Value::as_str) {
                Some("approved") => true,
                Some("denied") => false,
                _ => return Err("A remote approval had an invalid decision.".to_string()),
            };
            Ok(RemoteCommand::Approval {
                gate: gate.to_string(),
                approved,
            })
        }
        Some("run.control") => {
            let action = match value.get("action").and_then(Value::as_str) {
                Some("interrupt") => RemoteControlRequest::Interrupt,
                Some("cancel") => RemoteControlRequest::Cancel,
                _ => return Err("A remote run-control command had an invalid action.".to_string()),
            };
            let turn_id = value
                .get("turnId")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            Ok(RemoteCommand::Control { action, turn_id })
        }
        _ => Err("Codewhale sent an unsupported remote command.".to_string()),
    }
}

async fn runner_request(
    client: &Client,
    enrollment: &LiveEnrollment,
    method: Method,
    segments: &[&str],
    query: &[(&str, String)],
    body: Option<Value>,
) -> Result<Value, String> {
    let url = control_plane_url(&enrollment.persisted.control_plane_base, segments, query)?;
    let mut request = client
        .request(method, url)
        .bearer_auth(&enrollment.access_token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Remote control lost its secure connection.".to_string())?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        delete_persisted_enrollment();
        return Err("The remote-control enrollment was revoked.".to_string());
    }
    if !response.status().is_success() {
        return Err(format!(
            "The remote-control server rejected a request ({}).",
            response.status()
        ));
    }
    read_bounded_json(response).await
}

async fn public_request(
    client: &Client,
    method: Method,
    url: Url,
    body: Value,
) -> Result<Value, String> {
    let response = client
        .request(method, url)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Remote control could not reach Codewhale.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Codewhale rejected remote-control enrollment ({}).",
            response.status()
        ));
    }
    read_bounded_json(response).await
}

async fn read_bounded_json(response: reqwest::Response) -> Result<Value, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("Codewhale returned an oversized remote-control response.".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Codewhale returned an unreadable response.".to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("Codewhale returned an oversized remote-control response.".to_string());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "Codewhale returned an invalid remote-control response.".to_string())
}

fn runner_control_plane_base() -> Result<String, String> {
    if cfg!(debug_assertions)
        && let Ok(value) = std::env::var("CWC_RUNNER_CONTROL_PLANE_BASE")
    {
        let parsed =
            Url::parse(&value).map_err(|_| "The runner control plane is invalid.".to_string())?;
        let loopback = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none();
        if loopback {
            return Ok(parsed.to_string());
        }
        return Err(
            "Debug remote control only accepts an explicit loopback control plane.".to_string(),
        );
    }
    Ok(PRODUCTION_CONTROL_PLANE.to_string())
}

fn control_plane_url(
    base: &str,
    segments: &[&str],
    query: &[(&str, String)],
) -> Result<Url, String> {
    let mut url =
        Url::parse(base).map_err(|_| "The runner control plane is invalid.".to_string())?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "The runner control plane is invalid.".to_string())?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn load_persisted_enrollment() -> Result<Option<PersistedEnrollment>, String> {
    let Some(raw) = codewhale_secrets::Secrets::auto_detect()
        .get(ENROLLMENT_SECRET_SLOT)
        .map_err(|error| format!("Could not read the saved remote-control enrollment: {error}"))?
    else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|_| "The saved remote-control enrollment is invalid.".to_string())
}

fn save_persisted_enrollment(enrollment: &PersistedEnrollment) -> Result<(), String> {
    let raw = serde_json::to_string(enrollment)
        .map_err(|_| "Could not encode the remote-control enrollment.".to_string())?;
    codewhale_secrets::Secrets::auto_detect()
        .set(ENROLLMENT_SECRET_SLOT, &raw)
        .map_err(|error| format!("Could not securely save the remote-control enrollment: {error}"))
}

fn delete_persisted_enrollment() {
    if let Err(error) = codewhale_secrets::Secrets::auto_detect().delete(ENROLLMENT_SECRET_SLOT) {
        tracing::warn!("could not delete revoked remote-control enrollment: {error}");
    }
}

fn enrollment_needs_refresh(enrollment: &LiveEnrollment) -> bool {
    jwt_expiry(&enrollment.access_token)
        .is_none_or(|expiry| expiry <= epoch_seconds().saturating_add(60))
}

fn jwt_expiry(token: &str) -> Option<u64> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let payload = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice::<Value>(&payload)
        .ok()?
        .get("exp")?
        .as_u64()
}

fn access_token(value: &Value) -> Result<String, String> {
    let token = value
        .get("credential")
        .and_then(|value| value.get("accessToken"))
        .and_then(Value::as_str)
        .filter(|value| {
            (64..=8192).contains(&value.len()) && !value.chars().any(char::is_whitespace)
        })
        .ok_or_else(|| "Codewhale returned an invalid runner access token.".to_string())?
        .to_string();
    if jwt_expiry(&token).is_none_or(|expiry| expiry <= epoch_seconds()) {
        return Err("Codewhale returned an expired runner access token.".to_string());
    }
    Ok(token)
}

fn exact_capabilities(value: Option<&Value>) -> bool {
    let Some(items) = value.and_then(Value::as_array) else {
        return false;
    };
    let mut actual = items.iter().filter_map(Value::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    actual == CAPABILITIES
}

fn validate_authorization_url(value: &str, user_code: &str) -> Result<(), String> {
    let url = Url::parse(value)
        .map_err(|_| "Codewhale returned an invalid authorization URL.".to_string())?;
    let pairs = url.query_pairs().collect::<Vec<_>>();
    if url.scheme() != "https"
        || url.host_str() != Some("app.codewhale.net")
        || url.path() != "/runner/authorize"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || pairs.len() != 1
        || pairs[0].0 != "user_code"
        || pairs[0].1 != user_code
    {
        return Err("Codewhale returned an invalid authorization URL.".to_string());
    }
    Ok(())
}

fn string_field(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 2048)
        .map(ToString::to_string)
        .ok_or_else(|| format!("Codewhale returned an invalid {field}."))
}

fn secret_field(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| valid_secret(value))
        .map(ToString::to_string)
        .ok_or_else(|| format!("Codewhale returned an invalid {field}."))
}

fn opaque_field(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .map(ToString::to_string)
        .ok_or_else(|| format!("Codewhale returned an invalid {field}."))
}

fn valid_opaque_ref(value: &str) -> bool {
    (3..=160).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_secret(value: &str) -> bool {
    (32..=8192).contains(&value.len()) && !value.chars().any(char::is_whitespace)
}

fn valid_runtime_version(value: &str) -> bool {
    semver::Version::parse(value).is_ok() && value.len() <= 64
}

fn valid_runtime_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path, query_param},
    };

    #[test]
    fn target_identity_is_stable_without_exposing_the_path() {
        let target = target_ref(Path::new("/Users/alice/private/project"), "session-123");
        assert!(target.starts_with("target_"));
        assert_eq!(target.len(), 39);
        assert!(!target.contains("alice"));
        assert_eq!(
            target,
            target_ref(Path::new("/Users/alice/private/project"), "session-123")
        );
    }

    #[test]
    fn typed_command_parser_rejects_shell_and_cross_run_content() {
        let prompt = parse_remote_command(
            &json!({
                "type": "prompt.request",
                "runId": "run-1",
                "turnId": "turn-1",
                "prompt": "Continue",
            }),
            "run-1",
        )
        .unwrap();
        assert_eq!(
            prompt,
            RemoteCommand::Prompt {
                turn_id: "turn-1".to_string(),
                prompt: "Continue".to_string(),
            }
        );
        assert!(
            parse_remote_command(
                &json!({
                    "type": "shell",
                    "runId": "run-1",
                    "command": "rm -rf /",
                }),
                "run-1"
            )
            .is_err()
        );
        assert!(
            parse_remote_command(
                &json!({
                    "type": "prompt.request",
                    "runId": "run-other",
                    "turnId": "turn-1",
                    "prompt": "Continue",
                }),
                "run-1"
            )
            .is_err()
        );
    }

    #[test]
    fn approval_projection_matches_control_plane_namespace() {
        assert_eq!(projected_approval_id("tool-call-1").len(), 39);
        assert!(projected_approval_id("tool-call-1").starts_with("local_approval_"));
        assert_ne!(
            projected_approval_id("tool-call-1"),
            projected_approval_id("tool-call-2")
        );
    }

    #[test]
    fn authorization_url_is_exact_and_cannot_redirect_or_add_parameters() {
        assert!(
            validate_authorization_url(
                "https://app.codewhale.net/runner/authorize?user_code=ABCD-EFGH-JKLM",
                "ABCD-EFGH-JKLM",
            )
            .is_ok()
        );
        for spoofed in [
            "http://app.codewhale.net/runner/authorize?user_code=ABCD-EFGH-JKLM",
            "https://app.codewhale.net.evil.example/runner/authorize?user_code=ABCD-EFGH-JKLM",
            "https://app.codewhale.net/runner/authorize?user_code=ABCD-EFGH-JKLM&next=https://evil.example",
            "https://app.codewhale.net/runner/authorize?user_code=WRONG-CODE",
        ] {
            assert!(validate_authorization_url(spoofed, "ABCD-EFGH-JKLM").is_err());
        }
    }

    #[test]
    fn command_sequences_are_content_bound_and_replay_safe() {
        let mut controller = RemoteControlController::default();
        let prompt = RemoteCommand::Prompt {
            turn_id: "turn-1".to_string(),
            prompt: "Continue".to_string(),
        };
        assert_eq!(controller.claim_command("run-1", 1, &prompt), Ok(true));
        assert_eq!(controller.claim_command("run-1", 1, &prompt), Ok(false));
        assert!(
            controller
                .claim_command(
                    "run-1",
                    1,
                    &RemoteCommand::Prompt {
                        turn_id: "turn-1".to_string(),
                        prompt: "Changed".to_string(),
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn disconnected_remote_owner_keeps_local_input_locked_until_lease_expiry() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Failed;
        controller.ownership_blocked_until = Some(Instant::now() + Duration::from_secs(90));
        assert!(controller.blocks_local_input());
        controller.ownership_blocked_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!controller.blocks_local_input());
    }

    #[test]
    fn stop_after_lease_expiry_preserves_pending_approvals_for_restoration() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Failed;
        controller.ownership_blocked_until = Some(Instant::now() - Duration::from_secs(1));
        controller.pending_approvals.insert(
            "approval_fixture".to_string(),
            PendingRemoteApproval {
                tool_id: "tool_fixture".to_string(),
                tool_name: "edit".to_string(),
                description: "Edit fixture".to_string(),
                input: Value::Null,
                approval_key: "approval_fixture".to_string(),
                intent_summary: Some("fixture".to_string()),
            },
        );

        controller.stop();
        assert_eq!(controller.status, Status::Failed);
        assert_eq!(controller.pending_approvals.len(), 1);

        let event = controller.try_next_event();
        assert!(matches!(
            event,
            Some(RemoteEvent::OwnershipRestored { approvals })
                if approvals.len() == 1
                    && approvals[0].approval_key == "approval_fixture"
                    && approvals[0].tool_id == "tool_fixture"
        ));
        assert_eq!(controller.status, Status::Off);
        assert!(controller.pending_approvals.is_empty());
    }

    #[test]
    fn cancelling_a_connect_keeps_input_locked_and_reconnect_blocked() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Connecting;
        controller.stop();

        assert_eq!(controller.status, Status::Failed);
        assert!(controller.blocks_local_input());
        let result = controller.start(RemoteStart {
            workspace_label: "fixture".to_string(),
            target_ref: "target_fixture".to_string(),
            session_id: "session_fixture".to_string(),
            runtime_version: "0.9.1".to_string(),
            runtime_commit: "a".repeat(40),
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("previous remote lease"));
    }

    #[test]
    fn failed_worker_allows_snapshot_retry_without_releasing_ownership() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Connected;
        controller.uploaded_snapshots.insert("run-1".to_string());
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Failed("fixture disconnect".to_string()))
            .unwrap();

        assert!(matches!(
            controller.try_next_event(),
            Some(RemoteEvent::Failed(_))
        ));
        assert!(controller.uploaded_snapshots.is_empty());
        assert!(controller.blocks_local_input());
    }

    #[tokio::test]
    async fn cwc_runner_wire_contract_preserves_pending_and_recovery_commands() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/local-runners/runner-1/runs/run-1/commands"))
            .and(query_param("since_seq", "0"))
            .and(query_param("include_accepted", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "commands": [{
                    "seq": 1,
                    "deliveryStatus": "pending",
                    "ackStatus": "",
                    "command": {
                        "type": "prompt.request",
                        "runId": "run-1",
                        "turnId": "turn-1",
                        "prompt": "Continue from the web."
                    }
                }, {
                    "seq": 2,
                    "deliveryStatus": "acknowledged",
                    "ackStatus": "accepted",
                    "command": {
                        "type": "run.control",
                        "runId": "run-1",
                        "action": "interrupt"
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/local-runners/runner-1/runs/run-1/events"))
            .and(body_json(json!({
                "acknowledgements": [{
                    "commandSeq": 1,
                    "commandType": "prompt.request",
                    "status": "accepted",
                    "turnId": "turn-1"
                }],
                "envelopes": []
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "accepted": [],
                "count": 1,
                "cursor": 0
            })))
            .expect(1)
            .mount(&server)
            .await;

        let enrollment = LiveEnrollment {
            persisted: PersistedEnrollment {
                schema_version: 1,
                control_plane_base: format!("{}/", server.uri()),
                runner_enrollment_id: "enrollment-1".to_string(),
                account_ref: "account-1".to_string(),
                device_id: "device-1".to_string(),
                target_ref: "target-1".to_string(),
                target_grant_ref: "grant-1".to_string(),
                runtime_version: "0.9.1".to_string(),
                runtime_commit: "a".repeat(40),
                bootstrap_secret: "b".repeat(43),
            },
            access_token: "fixture-runner-access-token".to_string(),
        };
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("fixture client");

        let listed = list_commands(&client, &enrollment, "runner-1", "run-1", 0)
            .await
            .expect("CWC command list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].ack_status, "");
        assert_eq!(listed[1].ack_status, "accepted");
        let prompt =
            parse_remote_command(&listed[0].command, "run-1").expect("typed prompt command");
        upload_command_accepted(
            &client,
            &enrollment,
            "runner-1",
            "run-1",
            listed[0].seq,
            &prompt,
        )
        .await
        .expect("durable accepted acknowledgement");
    }
}
