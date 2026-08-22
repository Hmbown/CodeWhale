//! Account-owned remote control for the active TUI session.
//!
//! This is deliberately a typed relay, not a remote shell. The control plane
//! may send prompts, approval decisions, and run-control requests for the exact
//! enrolled target. Provider credentials, paths, environment variables, and
//! arbitrary command strings never cross this boundary.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
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
/// Machine-stable device identity. It outlives individual enrollments so the
/// control plane can fold every folder enrolled from this terminal into one
/// computer instead of one row per `/rc`.
const DEVICE_IDENTITY_SECRET_SLOT: &str = "cwc-remote-control-device-v1";
/// The only web origin whose session links the terminal will surface or open.
const APP_ORIGIN_HOST: &str = "app.codewhale.net";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const SYNC_INTERVAL: Duration = Duration::from_millis(1_200);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RUNS: usize = 64;
const MAX_COMMANDS: usize = 128;
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_RUNTIME_ENVELOPE_BYTES: usize = 128 * 1024;
const SNAPSHOT_ENVELOPE_BYTE_BUDGET: usize = 120 * 1024;
const MAX_SNAPSHOT_MESSAGES: usize = 64;
const MAX_SNAPSHOT_MESSAGE_CHARS: usize = 128 * 1024;
const MIN_TRUNCATED_MESSAGE_CHARS: usize = 32;
const MAX_REMOTE_ERROR_MESSAGE_BYTES: usize = 4 * 1024;
const RUNTIME_UPLOAD_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const RUNTIME_UPLOAD_MAX_BACKOFF: Duration = Duration::from_secs(5);
const CAPABILITIES: &[&str] = &["evidence-ledger", "fim", "git", "shell"];
/// How long an aborted or failed relay keeps local input locked. Matches the
/// server-side runner lease expiry with margin; local input never returns
/// while the server could still consider a remote owner live.
const OWNERSHIP_LOCK_AFTER_FAILURE: Duration = Duration::from_secs(95);
/// Ceiling for draining unacknowledged runtime events during `/rc stop`.
/// Deliberately below `OWNERSHIP_LOCK_AFTER_FAILURE` so a failed drain still
/// resolves into the ownership-locked path before the lease question is moot.
const STOP_DRAIN_DEADLINE: Duration = Duration::from_secs(45);
const JOURNAL_SCHEMA_VERSION: u64 = 1;
/// Hard bounds for the crash-recoverable unacknowledged-envelope journal.
const MAX_JOURNAL_EVENTS: usize = 256;
const MAX_JOURNAL_ENCODED_BYTES: usize = 4 * 1024 * 1024;
/// Capacity held back exclusively for integrity-critical envelopes (terminal
/// turn state, approvals, failures, resynchronization snapshots). Ordinary
/// deltas may never consume this headroom.
const JOURNAL_RESERVED_INTEGRITY_EVENTS: usize = 64;
const JOURNAL_RESERVED_INTEGRITY_BYTES: usize = 1024 * 1024;
/// A deferred (not yet handed to transport) delta envelope may grow to this
/// encoded size through coalescing before it is forced onto the wire.
const DELTA_COALESCE_BYTE_CAP: usize = 32 * 1024;
const JOURNAL_SETUP_ERROR: &str = "Remote control could not prepare its private delivery journal.";
const JOURNAL_UNTRUSTED_ERROR: &str = "The saved remote-control delivery journal could not be trusted; it was set aside. The account run may show an incomplete turn.";

/// Envelopes whose loss would strand account-side truth: terminal turn state,
/// approval requests, failure records, and resynchronization snapshots. They
/// draw on reserved journal capacity, are never dropped silently, and gate
/// `/rc stop` until the server cursor covers them.
fn integrity_critical_event(event: &str) -> bool {
    matches!(
        event,
        "turn.completed" | "approval.required" | "item.failed" | "session.snapshot"
    )
}

fn runtime_envelope_event(envelope: &Value) -> Option<&str> {
    envelope.get("event").and_then(Value::as_str)
}

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
    /// Directory that holds the crash-recoverable delivery journal. `None`
    /// runs memory-only and is reserved for tests; production callers must
    /// always provide a private directory under the Codewhale home.
    pub journal_dir: Option<PathBuf>,
    /// Observed `owner/name` from `git remote get-url origin`, when the folder
    /// is a Git checkout. This is a display receipt, never a path or GitHub App
    /// grant.
    pub git_remote: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RemoteEvent {
    Notice(String),
    Connected {
        account_ref: String,
        runner_id: String,
        target_ref: String,
        attachment: RemoteAttachment,
        links: RemoteLinks,
    },
    Attachment {
        attachment: RemoteAttachment,
        links: RemoteLinks,
    },
    RuntimeCursor {
        run_id: String,
        cursor: u64,
    },
    Command {
        run_id: String,
        seq: u64,
        command: RemoteCommand,
    },
    Failed(String),
    /// The relay died before any server-confirmed lease existed — during
    /// enrollment, device authorization, or the first connect. No lease can
    /// still be live, so nothing is locked and `/rc` can retry immediately.
    FailedPreLease(String),
    Stopped,
    OwnershipRestored {
        approvals: Vec<PendingRemoteApproval>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAttachment {
    pub run_id: String,
    pub workspace_id: String,
    pub runtime_cursor: u64,
    pub snapshot_present: bool,
}

/// Web links the control plane advertises for the attached session. Both are
/// optional: an older control plane omits them and the terminal then simply
/// shows no link. Links are validated against the Codewhale app origin before
/// they are ever displayed or opened; the terminal never invents one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteLinks {
    /// `https://app.codewhale.net/session?run=<runId>` for the live run.
    pub run_url: Option<String>,
    /// `https://app.codewhale.net/settings?section=workspaces` for this computer.
    pub computer_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunnerConnection {
    runner_id: String,
    attachment: RemoteAttachment,
    links: RemoteLinks,
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

enum RelayPhase {
    /// Everything before the first server-confirmed lease: control-plane base
    /// resolution, enrollment, device authorization, and the first connect.
    Enrolling,
    /// The server confirmed a lease (Connected was emitted). Any failure now
    /// is a lost-after-lease disconnect and stays fail-closed.
    Leased,
}

impl RelayPhase {
    fn lease_confirmed(&self) -> bool {
        matches!(self, Self::Leased)
    }
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

#[derive(Debug, Default)]
struct RuntimeTransportOutbox {
    events: BTreeMap<(String, u64), Value>,
}

/// One unacknowledged runtime envelope owned by the controller.
///
/// `handed_off` records whether the envelope may already have reached the
/// server through the transport worker. Once true the envelope is immutable:
/// ambiguous retries must resend byte-identical JSON.
#[derive(Debug, Clone, PartialEq)]
struct PendingRuntimeEnvelope {
    envelope: Value,
    encoded_len: usize,
    integrity: bool,
    handed_off: bool,
}

/// Crash-recoverable journal of unacknowledged runtime envelopes.
///
/// The file name is hash-derived so nothing about the workspace or session
/// leaks through the path; the directory is private and the file owner-only.
/// Neither the path nor the contents are ever reported to the control plane
/// or written to logs. Acknowledged prefixes are compacted on every persist,
/// and a journal that cannot be verified fails closed at load time.
struct RuntimeEventJournal {
    path: PathBuf,
    session_tag: String,
}

impl RuntimeEventJournal {
    fn open(dir: &Path, session_id: &str) -> Result<Self, String> {
        let mut hasher = Sha256::new();
        hasher.update(b"cwc-remote-control-journal\0");
        hasher.update(session_id.as_bytes());
        let session_tag = bytes_to_hex(&hasher.finalize())[..32].to_string();
        std::fs::create_dir_all(dir).map_err(|_| JOURNAL_SETUP_ERROR.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| JOURNAL_SETUP_ERROR.to_string())?;
        }
        Ok(Self {
            path: dir.join(format!("journal_{session_tag}.json")),
            session_tag,
        })
    }

    /// Loads every journaled envelope, or fails closed when the journal
    /// cannot be trusted (corrupt, oversized, or written for another
    /// session). A missing file is an ordinary empty journal.
    fn load(&self) -> Result<HashMap<String, BTreeMap<u64, Value>>, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(_) => return Err(JOURNAL_UNTRUSTED_ERROR.to_string()),
        };
        if bytes.len() > MAX_JOURNAL_ENCODED_BYTES.saturating_mul(2) {
            return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| JOURNAL_UNTRUSTED_ERROR.to_string())?;
        if value.get("schemaVersion").and_then(Value::as_u64) != Some(JOURNAL_SCHEMA_VERSION)
            || value.get("session").and_then(Value::as_str) != Some(self.session_tag.as_str())
        {
            return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
        }
        let runs = value
            .get("runs")
            .and_then(Value::as_object)
            .ok_or_else(|| JOURNAL_UNTRUSTED_ERROR.to_string())?;
        let mut restored: HashMap<String, BTreeMap<u64, Value>> = HashMap::new();
        let mut total_events = 0usize;
        let mut total_bytes = 0usize;
        for (run_id, envelopes) in runs {
            if !valid_opaque_ref(run_id) {
                return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
            }
            let envelopes = envelopes
                .as_array()
                .ok_or_else(|| JOURNAL_UNTRUSTED_ERROR.to_string())?;
            let mut events = BTreeMap::new();
            for envelope in envelopes {
                let seq = runtime_envelope_seq(envelope)
                    .ok_or_else(|| JOURNAL_UNTRUSTED_ERROR.to_string())?;
                let encoded_len = serde_json::to_vec(envelope)
                    .map(|body| body.len())
                    .unwrap_or(usize::MAX);
                if encoded_len > MAX_RUNTIME_ENVELOPE_BYTES {
                    return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
                }
                total_events += 1;
                total_bytes = total_bytes.saturating_add(encoded_len);
                if total_events > MAX_JOURNAL_EVENTS || total_bytes > MAX_JOURNAL_ENCODED_BYTES {
                    return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
                }
                if events.insert(seq, envelope.clone()).is_some() {
                    return Err(JOURNAL_UNTRUSTED_ERROR.to_string());
                }
            }
            if !events.is_empty() {
                restored.insert(run_id.clone(), events);
            }
        }
        Ok(restored)
    }

    /// Atomically replaces the journal with the current unacknowledged set.
    /// An empty set removes the file entirely (prompt compaction).
    fn persist(
        &self,
        pending: &HashMap<String, BTreeMap<u64, PendingRuntimeEnvelope>>,
    ) -> Result<(), String> {
        if pending.values().all(BTreeMap::is_empty) {
            self.remove();
            return Ok(());
        }
        let mut runs = serde_json::Map::new();
        for (run_id, events) in pending {
            if events.is_empty() {
                continue;
            }
            runs.insert(
                run_id.clone(),
                Value::Array(
                    events
                        .values()
                        .map(|entry| entry.envelope.clone())
                        .collect(),
                ),
            );
        }
        let body = serde_json::to_vec(&json!({
            "schemaVersion": JOURNAL_SCHEMA_VERSION,
            "session": self.session_tag,
            "runs": runs,
        }))
        .map_err(|_| JOURNAL_SETUP_ERROR.to_string())?;
        let tmp = self.path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|_| JOURNAL_SETUP_ERROR.to_string())?;
        file.write_all(&body)
            .and_then(|()| file.sync_all())
            .map_err(|_| JOURNAL_SETUP_ERROR.to_string())?;
        drop(file);
        std::fs::rename(&tmp, &self.path).map_err(|_| JOURNAL_SETUP_ERROR.to_string())
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("tmp"));
    }

    /// Moves an untrusted journal aside so the failure is explicit and a
    /// deliberate later `/rc` start can proceed from a clean slate.
    fn quarantine(&self) {
        let _ = std::fs::rename(&self.path, self.path.with_extension("corrupt"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePostOutcome {
    Accepted(u64),
    Retryable,
    AccessTokenExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeFlushOutcome {
    Idle,
    Accepted { run_id: String, cursor: u64 },
    Retryable,
    AccessTokenExpired,
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

/// The machine-stable device identity persisted independently of any
/// enrollment. Enrollments are deleted and re-created when the target or
/// runtime changes; this record is only ever created once per keychain.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedDeviceIdentity {
    schema_version: u64,
    device_id: String,
}

impl PersistedDeviceIdentity {
    fn valid(&self) -> bool {
        self.schema_version == 1 && valid_opaque_ref(&self.device_id)
    }
}

/// Pick the device id this terminal presents to the control plane. A valid
/// saved identity always wins; otherwise the device id of an existing
/// enrollment is adopted (so upgrading terminals keep their computer row);
/// otherwise a fresh id is minted. Returns the id and whether it must be
/// saved.
fn resolve_device_identity(
    saved: Option<PersistedDeviceIdentity>,
    enrollment_device_id: Option<&str>,
) -> (String, bool) {
    if let Some(saved) = saved.filter(PersistedDeviceIdentity::valid) {
        return (saved.device_id, false);
    }
    if let Some(existing) = enrollment_device_id.filter(|value| valid_opaque_ref(value)) {
        return (existing.to_string(), true);
    }
    (format!("device_{}", uuid::Uuid::new_v4().simple()), true)
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
    links: RemoteLinks,
    /// Latest server-confirmed run attachment. Kept separately from
    /// `active_run`: an attachment can exist while the session is idle, and a
    /// mid-turn `/rc` can bind the already-running local turn to it without
    /// inventing a second prompt.
    attached_run_id: Option<String>,
    active_run: Option<ActiveRelayRun>,
    /// A local dispatch that was already in flight when the web attachment
    /// became ready, but whose typed `TurnStarted` event has not landed yet.
    /// The first such event promotes this exact run into `active_run`.
    pending_local_turn_run: Option<String>,
    event_seq: HashMap<String, u64>,
    uploaded_snapshots: HashSet<String>,
    pending_runtime_events: HashMap<String, BTreeMap<u64, PendingRuntimeEnvelope>>,
    pending_approvals: HashMap<String, PendingRemoteApproval>,
    command_fingerprints: HashMap<(String, u64), String>,
    worker_tx: Option<mpsc::UnboundedSender<WorkerCommand>>,
    event_rx: Option<mpsc::UnboundedReceiver<RemoteEvent>>,
    worker: Option<tokio::task::JoinHandle<()>>,
    applying_remote_command: bool,
    ownership_blocked_until: Option<Instant>,
    journal: Option<RuntimeEventJournal>,
    /// At most one deferred (unsent, still coalescible) delta seq per run.
    deferred_delta: HashMap<String, u64>,
    /// Runs whose deltas were shed under pressure; truth is restored with a
    /// bounded snapshot at the next terminal boundary.
    resync_required: HashSet<String>,
    /// Runs that crossed their terminal boundary with `resync_required` set;
    /// the UI drains these via `take_pending_resync`.
    resync_ready: Vec<String>,
    pending_event_count: usize,
    pending_encoded_bytes: usize,
}

impl Default for RemoteControlController {
    fn default() -> Self {
        Self {
            status: Status::Off,
            status_detail: "off".to_string(),
            account_ref: None,
            target_ref: None,
            links: RemoteLinks::default(),
            attached_run_id: None,
            active_run: None,
            pending_local_turn_run: None,
            event_seq: HashMap::new(),
            uploaded_snapshots: HashSet::new(),
            pending_runtime_events: HashMap::new(),
            pending_approvals: HashMap::new(),
            command_fingerprints: HashMap::new(),
            worker_tx: None,
            event_rx: None,
            worker: None,
            applying_remote_command: false,
            ownership_blocked_until: None,
            journal: None,
            deferred_delta: HashMap::new(),
            resync_required: HashSet::new(),
            resync_ready: Vec::new(),
            pending_event_count: 0,
            pending_encoded_bytes: 0,
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
            || !valid_session_ref(&start.session_id)
        {
            return Err("This build or session does not have an enrollable identity.".to_string());
        }
        match &start.journal_dir {
            Some(dir) => {
                let journal = RuntimeEventJournal::open(dir, &start.session_id)?;
                match journal.load() {
                    Ok(restored) => {
                        self.reset_pending_from(restored);
                        self.journal = Some(journal);
                    }
                    Err(error) => {
                        // Fail closed: an unverifiable journal may hide
                        // undelivered terminal or approval state. Set it
                        // aside explicitly rather than silently discarding.
                        journal.quarantine();
                        return Err(error);
                    }
                }
            }
            None => self.journal = None,
        }
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.stop_worker();
        self.status = Status::Connecting;
        self.status_detail = "waiting for account authorization".to_string();
        self.target_ref = Some(start.target_ref.clone());
        self.attached_run_id = None;
        self.active_run = None;
        self.pending_local_turn_run = None;
        self.worker_tx = Some(worker_tx);
        self.event_rx = Some(event_rx);
        self.worker = Some(tokio::spawn(async move {
            let mut phase = RelayPhase::Enrolling;
            if let Err(error) = relay_worker(start, worker_rx, event_tx.clone(), &mut phase).await {
                let _ = if phase.lease_confirmed() {
                    event_tx.send(RemoteEvent::Failed(error))
                } else {
                    event_tx.send(RemoteEvent::FailedPreLease(error))
                };
            }
        }));
        Ok(())
    }

    /// Why `/rc stop` must currently be refused, if any reason exists.
    ///
    /// Stopping is only safe once no remote turn is active and every
    /// integrity-critical envelope (terminal turn state, approvals, failures,
    /// resynchronization snapshots) is behind the server-confirmed cursor.
    pub fn stop_refusal(&self) -> Option<String> {
        if self.has_active_run() {
            return Some(
                "Finish or interrupt the active remote turn before stopping remote control."
                    .to_string(),
            );
        }
        if self.has_unacknowledged_integrity_events() {
            return Some(
                "The server has not yet acknowledged this session's terminal or approval events; try /rc stop again in a moment."
                    .to_string(),
            );
        }
        None
    }

    fn has_unacknowledged_integrity_events(&self) -> bool {
        self.pending_runtime_events
            .values()
            .flat_map(BTreeMap::values)
            .any(|entry| entry.integrity)
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
            self.ownership_blocked_until = Some(Instant::now() + OWNERSHIP_LOCK_AFTER_FAILURE);
        } else if self.status == Status::Connected {
            // Hand every deferred delta to the transport first so the worker's
            // pre-heartbeat drain covers the complete unacknowledged set.
            self.hand_off_all_deferred();
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
                self.ownership_blocked_until = Some(Instant::now() + OWNERSHIP_LOCK_AFTER_FAILURE);
            }
        }
        if self.status == Status::Failed && self.ownership_blocked_until.is_none() {
            // A pre-lease failure holds no lease; stopping is an ordinary
            // reset, not a drain confirmation.
            self.status = Status::Off;
        }
        if self.status == Status::Off {
            self.account_ref = None;
            self.links = RemoteLinks::default();
            self.attached_run_id = None;
            self.active_run = None;
            self.pending_local_turn_run = None;
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
        // Coalescing ends at the next UI poll: hand any deferred delta to the
        // transport so live viewers never wait more than one tick.
        self.hand_off_all_deferred();
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
                attachment,
                links,
                ..
            } => {
                self.apply_attachment(attachment);
                self.links = links.clone();
                // Journal recovery may hold unacknowledged envelopes for runs
                // beyond this attachment; resend every pending run now.
                self.flush_all_pending();
                self.status = Status::Connected;
                self.status_detail = "web mirror connected".to_string();
                self.ownership_blocked_until = None;
                self.account_ref = Some(account_ref.clone());
                self.target_ref = Some(target_ref.clone());
            }
            RemoteEvent::Attachment { attachment, links } => {
                self.apply_attachment(attachment);
                self.links = links.clone();
            }
            RemoteEvent::RuntimeCursor { run_id, cursor } => {
                self.reconcile_runtime_cursor(run_id, *cursor);
            }
            RemoteEvent::FailedPreLease(reason) => {
                // No server-confirmed lease ever existed, so nothing is
                // locked: no reconnect blackout, no approval handoff to
                // undo, and `/rc` can retry immediately.
                self.status = Status::Failed;
                self.status_detail = reason.clone();
                self.ownership_blocked_until = None;
                self.active_run = None;
                self.pending_local_turn_run = None;
            }
            RemoteEvent::Failed(reason) => {
                self.status = Status::Failed;
                self.status_detail =
                    format!("{reason}; waiting for the last server lease to expire safely");
                self.ownership_blocked_until = Some(Instant::now() + OWNERSHIP_LOCK_AFTER_FAILURE);
                self.active_run = None;
                self.pending_local_turn_run = None;
                // Exact unacknowledged runtime envelopes remain owned by this
                // controller (and its journal). A later worker reconnect
                // resends them unchanged.
            }
            RemoteEvent::Stopped => {
                self.status = Status::Off;
                self.status_detail = "off".to_string();
                self.active_run = None;
                self.pending_local_turn_run = None;
                self.attached_run_id = None;
                self.links = RemoteLinks::default();
                self.ownership_blocked_until = None;
                // The worker only reports Stopped after draining through the
                // server-confirmed cursor and posting the offline heartbeat,
                // so an empty pending set means the journal is spent.
                if self.pending_runtime_events.values().all(BTreeMap::is_empty)
                    && let Some(journal) = &self.journal
                {
                    journal.remove();
                }
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

    /// The validated web link for the live session, once the control plane
    /// has advertised one.
    pub fn run_url(&self) -> Option<&str> {
        if matches!(self.status, Status::Connected | Status::Stopping) {
            self.links.run_url.as_deref()
        } else {
            None
        }
    }

    /// The validated web link for this computer's settings row, if advertised.
    pub fn computer_url(&self) -> Option<&str> {
        if matches!(self.status, Status::Connected | Status::Stopping) {
            self.links.computer_url.as_deref()
        } else {
            None
        }
    }

    pub fn status_line(&self) -> String {
        match self.status {
            Status::Off => "Remote control: off".to_string(),
            Status::Connecting => format!("Remote control: connecting · {}", self.status_detail),
            Status::Connected => match self.links.run_url.as_deref() {
                Some(url) => format!(
                    "Remote control: connected · account {} · {} · open {url}",
                    self.account_ref.as_deref().unwrap_or("account"),
                    self.status_detail
                ),
                None => format!(
                    "Remote control: connected · account {} · {}",
                    self.account_ref.as_deref().unwrap_or("account"),
                    self.status_detail
                ),
            },
            Status::Stopping => {
                "Remote control: stopping · confirming the runner is offline".to_string()
            }
            Status::Failed => {
                if self
                    .ownership_blocked_until
                    .is_some_and(|deadline| Instant::now() < deadline)
                {
                    format!(
                        "Remote control: lost after connecting · {} · reconnect waits for the server lease to drain",
                        self.status_detail
                    )
                } else {
                    format!(
                        "Remote control: failed before connecting · {} · /rc to retry",
                        self.status_detail
                    )
                }
            }
        }
    }

    /// Whether the web mirror can carry an approval decision right now.
    ///
    /// `Connecting` deliberately does not qualify: there is not yet an
    /// attachment/run cursor able to carry a typed approval, so the local
    /// card stays the only actionable surface until `Connected`. A
    /// transport failure also disqualifies — a dead relay cannot deliver a
    /// decision, and the local card remains the source of truth either way.
    ///
    /// Mirror semantics: this never gates *local* input. It only decides
    /// whether the approval card is *also* shared with the web.
    pub fn can_share_approval_with_web(&self) -> bool {
        let attached = match self.status {
            // A connection alone is not enough. Until a concrete typed turn
            // id is bound, `record_remote_approval` has nowhere safe to send
            // a decision, so the card stays local-only.
            Status::Connected | Status::Stopping => self.active_run.is_some(),
            Status::Off | Status::Connecting | Status::Failed => false,
        };
        attached && !self.applying_remote_command
    }

    /// Record that the *local* surface answered an approval, so a late web
    /// decision for the same tool is acknowledged as "no longer pending"
    /// instead of double-answering the engine. First decision wins; the
    /// other surface is told.
    pub fn resolve_pending_approval(&mut self, tool_id: &str, approved: bool) -> bool {
        let gate = projected_approval_id(tool_id);
        if self.pending_approvals.remove(&gate).is_none() {
            return false;
        }
        if let Some(active) = self.active_run.clone() {
            self.upload_envelope(
                &active.run_id,
                "approval.resolved",
                Some(&active.turn_id),
                json!({
                    "id": gate,
                    "approval_id": gate,
                    "decision": if approved { "approved" } else { "denied" },
                    "decided_by": "terminal",
                }),
            );
        }
        true
    }

    pub fn set_applying_remote_command(&mut self, value: bool) {
        self.applying_remote_command = value;
    }

    /// Test-only: put the controller into the exact state a live connected
    /// mirror with an attached run would be in, without a relay worker.
    #[cfg(test)]
    pub(crate) fn force_mirror_connected_for_tests(&mut self, run_id: &str, turn_id: &str) {
        self.status = Status::Connected;
        self.status_detail = "web mirror connected".to_string();
        self.attached_run_id = Some(run_id.to_string());
        self.active_run = Some(ActiveRelayRun {
            run_id: run_id.to_string(),
            turn_id: turn_id.to_string(),
        });
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
            || self.pending_local_turn_run.as_deref() == Some(run_id)
    }

    /// A remotely-owned turn must reach a terminal engine event before the
    /// user can release the relay lease. Dropping the worker earlier would
    /// discard the run binding and strand the control-plane ledger while the
    /// local engine continued producing results.
    pub fn has_active_run(&self) -> bool {
        self.active_run.is_some() || self.pending_local_turn_run.is_some()
    }

    /// Bind the server-confirmed attachment to the local turn that was
    /// already running when `/rc` was invoked.
    ///
    /// With a typed runtime turn id the binding is immediate. During the
    /// narrow dispatch-before-`TurnStarted` window, the run is parked and the
    /// first typed start event completes the binding. Replays are idempotent:
    /// an existing binding is never replaced and no runtime envelope is
    /// emitted by this method itself.
    pub fn attach_current_local_turn(&mut self, turn_id: Option<&str>) -> bool {
        if self.status != Status::Connected || self.has_active_run() {
            return false;
        }
        let Some(run_id) = self.attached_run_id.clone() else {
            return false;
        };
        match turn_id.map(str::trim).filter(|turn_id| !turn_id.is_empty()) {
            Some(turn_id) => {
                self.active_run = Some(ActiveRelayRun {
                    run_id,
                    turn_id: turn_id.to_string(),
                });
            }
            None => self.pending_local_turn_run = Some(run_id),
        }
        true
    }

    /// Release a dispatch-window binding when the local dispatcher becomes
    /// idle without ever producing a typed `TurnStarted`. The account-owned
    /// attachment remains connected and can accept a later web prompt; only
    /// the nonexistent turn lease is removed.
    pub fn release_unstarted_local_turn(&mut self) -> bool {
        self.pending_local_turn_run.take().is_some()
    }

    /// A remote prompt can fail during local route preparation before the
    /// engine owns a turn and therefore before it can emit `EngineEvent::Error`.
    /// That failure is still terminal for the account-owned run.
    pub fn fail_active_dispatch(&mut self, error: &str) {
        self.fail_active_run("dispatch_failed", error);
    }

    fn apply_attachment(&mut self, attachment: &RemoteAttachment) {
        self.attached_run_id = Some(attachment.run_id.clone());
        self.reconcile_runtime_cursor(&attachment.run_id, attachment.runtime_cursor);
        let local_cursor = self
            .pending_runtime_events
            .get(&attachment.run_id)
            .and_then(|events| events.last_key_value().map(|(seq, _)| *seq))
            .unwrap_or(0);
        let cursor = self.event_seq.entry(attachment.run_id.clone()).or_insert(0);
        *cursor = (*cursor).max(attachment.runtime_cursor).max(local_cursor);
        self.flush_pending_runtime_events(&attachment.run_id);
        // `snapshot_present` is server history, not proof that this freshly
        // loaded TUI process has uploaded its current saved history. The local
        // marker below prevents ordinary same-process reconnect duplication.
    }

    pub fn upload_snapshot(&mut self, run_id: &str, messages: &[Message]) {
        if self.uploaded_snapshots.contains(run_id) {
            return;
        }
        let seq = self.next_runtime_seq(run_id);
        let envelope = bounded_session_snapshot_envelope(seq, messages);
        if self.queue_runtime_envelope(run_id, envelope) {
            self.uploaded_snapshots.insert(run_id.to_string());
        }
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
        _input: &Value,
        _approval_key: &str,
        _intent_summary: Option<&str>,
    ) -> String {
        let gate = projected_approval_id(tool_id);
        self.pending_approvals.insert(
            gate.clone(),
            PendingRemoteApproval {
                tool_id: tool_id.to_string(),
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
        // `/rc` can attach while the host is still preparing a turn. The
        // server run is known first; `TurnStarted` supplies the authoritative
        // runtime turn id later. Promote exactly once, before the ordinary
        // projection below observes the event.
        if let EngineEvent::TurnStarted { turn_id, .. } = event
            && self.active_run.is_none()
            && let Some(run_id) = self.pending_local_turn_run.take()
        {
            self.active_run = Some(ActiveRelayRun {
                run_id,
                turn_id: turn_id.clone(),
            });
        }
        let Some(active) = self.active_run.clone() else {
            return;
        };
        match event {
            EngineEvent::MessageDelta { content, .. } => {
                self.upload_delta(&active.run_id, &active.turn_id, content);
            }
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
                if self.resync_required.remove(&active.run_id) {
                    // Deltas were shed under pressure during this turn; the UI
                    // must now upload a bounded current snapshot so account
                    // truth is restored at the terminal boundary.
                    self.resync_ready.push(active.run_id.clone());
                }
                self.active_run = None;
                self.pending_local_turn_run = None;
            }
            EngineEvent::Error {
                envelope,
                recoverable,
            } if !recoverable => {
                self.fail_active_run(&envelope.code, &envelope.message);
            }
            _ => {}
        }
    }

    fn fail_active_run(&mut self, code: &str, error: &str) {
        let Some(active) = self.active_run.clone() else {
            return;
        };
        let message = bounded_remote_error_message(error);
        let item_id = projected_error_item_id(&active.run_id, &active.turn_id, code);
        self.upload_envelope(
            &active.run_id,
            "item.failed",
            Some(&active.turn_id),
            json!({
                "item": {
                    "id": item_id,
                    "kind": "error",
                    "status": "failed",
                    "summary": message,
                    "detail": message,
                }
            }),
        );
        self.upload_envelope(
            &active.run_id,
            "turn.completed",
            Some(&active.turn_id),
            json!({ "turn": { "status": "failed", "usage": {} } }),
        );
        self.active_run = None;
        self.pending_local_turn_run = None;
    }

    fn upload_envelope(
        &mut self,
        run_id: &str,
        event: &str,
        turn_id: Option<&str>,
        payload: Value,
    ) {
        let seq = self.next_runtime_seq(run_id);
        let envelope = runtime_envelope(
            seq,
            event,
            turn_id,
            chrono::Utc::now().to_rfc3339(),
            payload,
        );
        self.queue_runtime_envelope(run_id, envelope);
    }

    fn next_runtime_seq(&self, run_id: &str) -> u64 {
        let acknowledged = self.event_seq.get(run_id).copied().unwrap_or(0);
        let pending = self
            .pending_runtime_events
            .get(run_id)
            .and_then(|events| events.last_key_value().map(|(seq, _)| *seq))
            .unwrap_or(0);
        acknowledged.max(pending).saturating_add(1)
    }

    fn queue_runtime_envelope(&mut self, run_id: &str, envelope: Value) -> bool {
        // Per-run sequence order must reach the transport in order, so any
        // deferred delta is handed off before a later envelope is queued.
        self.hand_off_deferred(run_id);
        self.queue_runtime_envelope_inner(run_id, envelope, false)
    }

    fn queue_runtime_envelope_inner(&mut self, run_id: &str, envelope: Value, defer: bool) -> bool {
        let Some(seq) = runtime_envelope_seq(&envelope) else {
            self.status_detail = "a local runtime event had no valid sequence".to_string();
            return false;
        };
        let encoded_len = serde_json::to_vec(&envelope)
            .map(|body| body.len())
            .unwrap_or(usize::MAX);
        if encoded_len > MAX_RUNTIME_ENVELOPE_BYTES {
            self.status_detail = "a local runtime event exceeded the safe relay limit".to_string();
            return false;
        }
        let integrity = runtime_envelope_event(&envelope).is_some_and(integrity_critical_event);
        let already_pending = self
            .pending_runtime_events
            .get(run_id)
            .and_then(|events| events.get(&seq))
            .is_some();
        if already_pending {
            let entry = self
                .pending_runtime_events
                .get(run_id)
                .and_then(|events| events.get(&seq))
                .expect("checked above");
            if entry.envelope != envelope {
                self.status_detail =
                    "a local runtime sequence changed before acknowledgement".to_string();
                return false;
            }
        } else {
            if !self.reserve_capacity(run_id, encoded_len, integrity) {
                return false;
            }
            self.pending_runtime_events
                .entry(run_id.to_string())
                .or_default()
                .insert(
                    seq,
                    PendingRuntimeEnvelope {
                        envelope: envelope.clone(),
                        encoded_len,
                        integrity,
                        handed_off: false,
                    },
                );
            self.pending_event_count += 1;
            self.pending_encoded_bytes = self.pending_encoded_bytes.saturating_add(encoded_len);
        }
        self.event_seq
            .entry(run_id.to_string())
            .and_modify(|cursor| *cursor = (*cursor).max(seq))
            .or_insert(seq);
        if defer {
            self.deferred_delta.insert(run_id.to_string(), seq);
        } else {
            if let Some(entry) = self
                .pending_runtime_events
                .get_mut(run_id)
                .and_then(|events| events.get_mut(&seq))
            {
                entry.handed_off = true;
            }
            self.send_runtime_envelope(run_id, envelope);
            self.persist_journal();
        }
        true
    }

    /// Bounded-journal admission control.
    ///
    /// Integrity-critical envelopes may use the full budget, ordinary deltas
    /// only the unreserved share. A shed delta marks the run for terminal-
    /// boundary resynchronization; a shed integrity envelope can never happen
    /// silently — the relay fails closed and local input stays locked through
    /// the server lease expiry.
    fn reserve_capacity(&mut self, run_id: &str, encoded_len: usize, integrity: bool) -> bool {
        let (event_budget, byte_budget) = if integrity {
            (MAX_JOURNAL_EVENTS, MAX_JOURNAL_ENCODED_BYTES)
        } else {
            (
                MAX_JOURNAL_EVENTS - JOURNAL_RESERVED_INTEGRITY_EVENTS,
                MAX_JOURNAL_ENCODED_BYTES - JOURNAL_RESERVED_INTEGRITY_BYTES,
            )
        };
        if self.pending_event_count < event_budget
            && self.pending_encoded_bytes.saturating_add(encoded_len) <= byte_budget
        {
            return true;
        }
        if integrity {
            self.status = Status::Failed;
            self.status_detail =
                "the runtime delivery buffer overflowed; waiting for the last server lease to expire safely"
                    .to_string();
            self.ownership_blocked_until = Some(Instant::now() + OWNERSHIP_LOCK_AFTER_FAILURE);
        } else {
            self.resync_required.insert(run_id.to_string());
        }
        false
    }

    /// Streams a message delta, coalescing into the run's deferred envelope
    /// while that envelope has provably never been handed to the transport.
    fn upload_delta(&mut self, run_id: &str, turn_id: &str, content: &str) {
        if let Some(seq) = self.deferred_delta.get(run_id).copied() {
            if self.try_coalesce_delta(run_id, seq, turn_id, content) {
                return;
            }
            self.hand_off_deferred(run_id);
        }
        let seq = self.next_runtime_seq(run_id);
        let envelope = runtime_envelope(
            seq,
            "item.delta",
            Some(turn_id),
            chrono::Utc::now().to_rfc3339(),
            json!({ "kind": "agent_message", "delta": content }),
        );
        self.queue_runtime_envelope_inner(run_id, envelope, true);
    }

    fn try_coalesce_delta(&mut self, run_id: &str, seq: u64, turn_id: &str, content: &str) -> bool {
        let Some(entry) = self
            .pending_runtime_events
            .get_mut(run_id)
            .and_then(|events| events.get_mut(&seq))
        else {
            return false;
        };
        if entry.handed_off
            || entry.envelope.get("turn_id").and_then(Value::as_str) != Some(turn_id)
        {
            return false;
        }
        let Some(existing) = entry
            .envelope
            .pointer("/payload/delta")
            .and_then(Value::as_str)
        else {
            return false;
        };
        let merged = format!("{existing}{content}");
        let mut candidate = entry.envelope.clone();
        candidate["payload"]["delta"] = Value::String(merged);
        let encoded_len = serde_json::to_vec(&candidate)
            .map(|body| body.len())
            .unwrap_or(usize::MAX);
        if encoded_len > DELTA_COALESCE_BYTE_CAP {
            return false;
        }
        let old_len = entry.encoded_len;
        entry.envelope = candidate;
        entry.encoded_len = encoded_len;
        self.pending_encoded_bytes = self
            .pending_encoded_bytes
            .saturating_sub(old_len)
            .saturating_add(encoded_len);
        true
    }

    /// Hands the run's deferred delta to the transport. From this point the
    /// envelope may have reached the server and becomes immutable.
    fn hand_off_deferred(&mut self, run_id: &str) {
        let Some(seq) = self.deferred_delta.remove(run_id) else {
            return;
        };
        let Some(envelope) = self
            .pending_runtime_events
            .get_mut(run_id)
            .and_then(|events| events.get_mut(&seq))
            .map(|entry| {
                entry.handed_off = true;
                entry.envelope.clone()
            })
        else {
            return;
        };
        self.send_runtime_envelope(run_id, envelope);
        self.persist_journal();
    }

    fn hand_off_all_deferred(&mut self) {
        let runs: Vec<String> = self.deferred_delta.keys().cloned().collect();
        for run_id in runs {
            self.hand_off_deferred(&run_id);
        }
    }

    /// The UI drains this after each engine event batch and answers with
    /// `upload_resync_snapshot` for the returned run.
    pub fn take_pending_resync(&mut self) -> Option<String> {
        self.resync_ready.pop()
    }

    /// Uploads a bounded current-history snapshot to repair account truth
    /// after deltas were shed under pressure.
    pub fn upload_resync_snapshot(&mut self, run_id: &str, messages: &[Message]) {
        let seq = self.next_runtime_seq(run_id);
        let envelope = bounded_session_snapshot_envelope(seq, messages);
        self.queue_runtime_envelope(run_id, envelope);
    }

    fn persist_journal(&mut self) {
        let Some(journal) = &self.journal else {
            return;
        };
        if journal.persist(&self.pending_runtime_events).is_err() {
            // Crash durability is degraded, but nothing is lost silently: the
            // live relay keeps every envelope in memory and `/rc stop` still
            // requires the server-confirmed drain.
            self.status_detail =
                "the delivery journal could not be written; stop waits for server confirmation"
                    .to_string();
        }
    }

    /// Replaces the in-memory pending set from a verified journal load.
    fn reset_pending_from(&mut self, restored: HashMap<String, BTreeMap<u64, Value>>) {
        self.pending_runtime_events.clear();
        self.deferred_delta.clear();
        self.pending_event_count = 0;
        self.pending_encoded_bytes = 0;
        for (run_id, events) in restored {
            let mut pending = BTreeMap::new();
            for (seq, envelope) in events {
                let encoded_len = serde_json::to_vec(&envelope)
                    .map(|body| body.len())
                    .unwrap_or(usize::MAX);
                let integrity =
                    runtime_envelope_event(&envelope).is_some_and(integrity_critical_event);
                self.pending_event_count += 1;
                self.pending_encoded_bytes = self.pending_encoded_bytes.saturating_add(encoded_len);
                pending.insert(
                    seq,
                    PendingRuntimeEnvelope {
                        envelope,
                        encoded_len,
                        integrity,
                        handed_off: false,
                    },
                );
            }
            if let Some((top, _)) = pending.last_key_value() {
                let top = *top;
                self.event_seq
                    .entry(run_id.clone())
                    .and_modify(|cursor| *cursor = (*cursor).max(top))
                    .or_insert(top);
            }
            if !pending.is_empty() {
                self.pending_runtime_events.insert(run_id, pending);
            }
        }
    }

    fn send_runtime_envelope(&self, run_id: &str, envelope: Value) {
        let Some(tx) = &self.worker_tx else {
            return;
        };
        let _ = tx.send(WorkerCommand::Upload {
            run_id: run_id.to_string(),
            acknowledgements: Vec::new(),
            envelopes: vec![envelope],
        });
    }

    fn flush_pending_runtime_events(&mut self, run_id: &str) {
        // A reconnect resend covers everything, deferred deltas included;
        // after this every envelope may have reached the server.
        self.deferred_delta.remove(run_id);
        let mut to_send = Vec::new();
        if let Some(events) = self.pending_runtime_events.get_mut(run_id) {
            for entry in events.values_mut() {
                entry.handed_off = true;
                to_send.push(entry.envelope.clone());
            }
        }
        if to_send.is_empty() {
            return;
        }
        for envelope in to_send {
            self.send_runtime_envelope(run_id, envelope);
        }
        self.persist_journal();
    }

    fn flush_all_pending(&mut self) {
        let runs: Vec<String> = self.pending_runtime_events.keys().cloned().collect();
        for run_id in runs {
            self.flush_pending_runtime_events(&run_id);
        }
    }

    fn reconcile_runtime_cursor(&mut self, run_id: &str, cursor: u64) {
        if cursor > JS_MAX_SAFE_INTEGER {
            self.status_detail = "the server returned an unsafe runtime cursor".to_string();
            return;
        }
        let mut empty = false;
        let mut retired_any = false;
        if let Some(events) = self.pending_runtime_events.get_mut(run_id) {
            let retired: Vec<u64> = events.range(..=cursor).map(|(seq, _)| *seq).collect();
            for seq in retired {
                if let Some(entry) = events.remove(&seq) {
                    retired_any = true;
                    self.pending_event_count = self.pending_event_count.saturating_sub(1);
                    self.pending_encoded_bytes =
                        self.pending_encoded_bytes.saturating_sub(entry.encoded_len);
                }
            }
            empty = events.is_empty();
        }
        if empty {
            self.pending_runtime_events.remove(run_id);
        }
        if self
            .deferred_delta
            .get(run_id)
            .is_some_and(|seq| *seq <= cursor)
        {
            self.deferred_delta.remove(run_id);
        }
        self.event_seq
            .entry(run_id.to_string())
            .and_modify(|known| *known = (*known).max(cursor))
            .or_insert(cursor);
        if retired_any {
            // Compact the acknowledged prefix out of the journal promptly.
            self.persist_journal();
        }
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

/// Opaque identity for the enrolled folder. It is a hash of the workspace
/// path only: every session opened in the same folder shares one target, so
/// the control plane sees one grant per folder rather than one per session
/// (the session itself travels separately as `sessionRef`).
pub fn target_ref(workspace: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codewhale-remote-target:v2\0");
    hasher.update(workspace.to_string_lossy().as_bytes());
    format!("target_{}", &bytes_to_hex(&hasher.finalize())[..32])
}

/// Status-bar banner shown while the web owns this session. When the control
/// plane advertised a session link the banner leads with it; otherwise it
/// falls back to the opaque account and runner receipts.
pub fn remote_control_banner(account_ref: &str, runner_id: &str, run_url: Option<&str>) -> String {
    match run_url {
        Some(url) => format!("WEB MIRROR · {url} · /rc stop"),
        None => format!("WEB MIRROR · account {account_ref} · runner {runner_id} · /rc stop"),
    }
}

/// Transcript note announcing where the live session can be followed.
pub fn remote_control_link_notice(run_url: &str) -> String {
    format!(
        "Remote control is live at {run_url} — run /rc open to open it in your browser, or /rc link to print it. Both surfaces stay usable; one turn runs at a time."
    )
}

fn runtime_envelope(
    seq: u64,
    event: &str,
    turn_id: Option<&str>,
    timestamp: String,
    payload: Value,
) -> Value {
    json!({
        "schema_version": 1,
        "seq": seq,
        "event": event,
        "kind": event,
        "turn_id": turn_id,
        "timestamp": timestamp,
        "payload": payload,
    })
}

fn runtime_envelope_seq(envelope: &Value) -> Option<u64> {
    envelope
        .get("seq")
        .and_then(Value::as_u64)
        .filter(|seq| (1..=JS_MAX_SAFE_INTEGER).contains(seq))
}

fn bounded_session_snapshot_envelope(seq: u64, messages: &[Message]) -> Value {
    let timestamp = chrono::Utc::now().to_rfc3339();
    let candidates = messages
        .iter()
        .rev()
        .filter_map(project_session_message)
        .take(MAX_SNAPSHOT_MESSAGES)
        .collect::<Vec<_>>();
    let mut kept = Vec::<Value>::new();
    for (role, text) in candidates {
        let full = json!({ "role": role, "text": text });
        kept.insert(0, full);
        if snapshot_envelope_len(seq, &timestamp, &kept) <= SNAPSHOT_ENVELOPE_BYTE_BUDGET {
            continue;
        }
        kept.remove(0);
        let max_chars = text.chars().count();
        let mut low = 0usize;
        let mut high = max_chars;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            let prefix = unicode_prefix(&text, mid);
            kept.insert(0, json!({ "role": role, "text": prefix }));
            let fits =
                snapshot_envelope_len(seq, &timestamp, &kept) <= SNAPSHOT_ENVELOPE_BYTE_BUDGET;
            kept.remove(0);
            if fits {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        if low >= MIN_TRUNCATED_MESSAGE_CHARS || (kept.is_empty() && low > 0) {
            kept.insert(
                0,
                json!({ "role": role, "text": unicode_prefix(&text, low) }),
            );
        }
        break;
    }
    let envelope = runtime_envelope(
        seq,
        "session.snapshot",
        None,
        timestamp,
        json!({ "messages": kept }),
    );
    debug_assert!(
        serde_json::to_vec(&envelope).is_ok_and(|body| body.len() <= SNAPSHOT_ENVELOPE_BYTE_BUDGET)
    );
    envelope
}

fn project_session_message(message: &Message) -> Option<(String, String)> {
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
    if text.trim().is_empty() {
        return None;
    }
    Some((
        role.to_string(),
        text.chars()
            .take(MAX_SNAPSHOT_MESSAGE_CHARS)
            .collect::<String>(),
    ))
}

fn snapshot_envelope_len(seq: u64, timestamp: &str, messages: &[Value]) -> usize {
    serde_json::to_vec(&runtime_envelope(
        seq,
        "session.snapshot",
        None,
        timestamp.to_string(),
        json!({ "messages": messages }),
    ))
    .map(|body| body.len())
    .unwrap_or(usize::MAX)
}

fn unicode_prefix(value: &str, chars: usize) -> String {
    value.chars().take(chars).collect()
}

fn projected_approval_id(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"local-runtime:approval\0");
    hasher.update(raw.as_bytes());
    format!("local_approval_{}", &bytes_to_hex(&hasher.finalize())[..24])
}

/// Whether this view is the approval card for exactly `gate` (the projected
/// approval id). Used by the web mirror to dismiss the matching card — never
/// an unrelated approval that happens to be on top.
pub(crate) fn view_is_approval_for_gate(
    view: &dyn crate::tui::views::ModalView,
    gate: &str,
) -> bool {
    view.kind() == crate::tui::views::ModalKind::Approval
        && view
            .approval_request_id()
            .is_some_and(|tool_id| projected_approval_id(tool_id) == gate)
}

fn projected_error_item_id(run_id: &str, turn_id: &str, code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"local-runtime:error\0");
    hasher.update(run_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(turn_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(code.as_bytes());
    format!("local_item_{}", &bytes_to_hex(&hasher.finalize())[..24])
}

fn bounded_remote_error_message(error: &str) -> String {
    let without_nul = error.replace('\0', " ");
    let redacted = codewhale_config::persistence::redact_secrets(&without_nul);
    let message = redacted.trim();
    if message.is_empty() {
        return "The local model turn failed.".to_string();
    }
    crate::utils::truncate_with_ellipsis(message, MAX_REMOTE_ERROR_MESSAGE_BYTES, "…")
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
    phase: &mut RelayPhase,
) -> Result<(), String> {
    let base = runner_control_plane_base()?;
    let client = Client::builder()
        .https_only(!cfg!(debug_assertions))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Remote control could not initialize secure networking.".to_string())?;

    let saved_enrollment = load_persisted_enrollment()?;
    // The device id is stable across enrollments: it is what lets the control
    // plane fold every folder enrolled from this terminal into one computer.
    let device_id = stable_device_id(
        saved_enrollment
            .as_ref()
            .map(|saved| saved.device_id.as_str()),
    )?;
    let mut enrollment = match saved_enrollment {
        Some(saved) if saved.matches(&start, &base) => {
            match refresh_enrollment(&client, saved).await {
                Ok(enrollment) => enrollment,
                Err(error) if error == "runner_enrollment_revoked" => {
                    delete_persisted_enrollment();
                    enroll_device(&client, &base, &start, &device_id, &event_tx).await?
                }
                Err(error) => return Err(error),
            }
        }
        Some(_) => {
            delete_persisted_enrollment();
            enroll_device(&client, &base, &start, &device_id, &event_tx).await?
        }
        None => enroll_device(&client, &base, &start, &device_id, &event_tx).await?,
    };

    let connection = connect_runner(&client, &enrollment, &start).await?;
    // connect_runner answered with a server-confirmed attachment: a lease
    // exists from here on, and every later failure is a lost-after-lease
    // disconnect that must stay fail-closed.
    *phase = RelayPhase::Leased;
    let mut runner_id = connection.runner_id.clone();
    event_tx
        .send(RemoteEvent::Connected {
            account_ref: enrollment.persisted.account_ref.clone(),
            runner_id: runner_id.clone(),
            target_ref: start.target_ref.clone(),
            attachment: connection.attachment,
            links: connection.links,
        })
        .map_err(|_| "The terminal remote-control owner stopped.".to_string())?;
    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut command_cursor: HashMap<String, u64> = HashMap::new();
    let mut delivered: HashMap<(String, u64), String> = HashMap::new();
    let mut runtime_outbox = RuntimeTransportOutbox::default();
    let mut runtime_upload_tick = tokio::time::interval(RUNTIME_UPLOAD_RETRY_INTERVAL);
    runtime_upload_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut runtime_retry_delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
    let mut runtime_retry_not_before = Instant::now();

    loop {
        tokio::select! {
            command = worker_rx.recv() => {
                match command {
                    Some(WorkerCommand::Upload { run_id, acknowledgements, envelopes }) => {
                        if !envelopes.is_empty() {
                            if !acknowledgements.is_empty() || envelopes.len() != 1 {
                                return Err("The local runtime queued an invalid event batch.".to_string());
                            }
                            runtime_outbox.enqueue(&run_id, envelopes[0].clone())?;
                            continue;
                        }
                        let body = Some(json!({ "acknowledgements": acknowledgements, "envelopes": envelopes }));
                        let result = runner_request(
                            &client,
                            &enrollment,
                            Method::POST,
                            &["api", "local-runners", &runner_id, "runs", &run_id, "events"],
                            &[],
                            body.clone(),
                        )
                        .await;
                        if let Err(err) = result {
                            if err == "runner_access_token_expired" {
                                refresh_enrollment_and_reconnect(
                                    &client,
                                    &mut enrollment,
                                    &mut runner_id,
                                    &start,
                                    &event_tx,
                                )
                                .await?;
                                runner_request(
                                    &client,
                                    &enrollment,
                                    Method::POST,
                                    &["api", "local-runners", &runner_id, "runs", &run_id, "events"],
                                    &[],
                                    body,
                                )
                                .await?;
                            } else {
                                return Err(err);
                            }
                        }
                    }
                    Some(WorkerCommand::Stop) | None => {
                        // Do not return local input until the control plane has
                        // durably released this lease. Every queued runtime
                        // envelope must first drain behind the server-confirmed
                        // cursor; only then may the offline heartbeat be
                        // posted. If either cannot be confirmed, this worker
                        // errors out and the UI keeps ownership locked through
                        // the server-side lease expiry instead.
                        drain_runtime_outbox_for_stop(
                            &client,
                            &mut enrollment,
                            &mut runner_id,
                            &start,
                            &event_tx,
                            &mut runtime_outbox,
                            Instant::now() + STOP_DRAIN_DEADLINE,
                        )
                        .await?;
                        let hb = post_heartbeat(&client, &enrollment, &runner_id, &start, "offline").await;
                        if let Err(err) = hb {
                            if err == "runner_access_token_expired" {
                                refresh_enrollment_and_reconnect(
                                    &client,
                                    &mut enrollment,
                                    &mut runner_id,
                                    &start,
                                    &event_tx,
                                )
                                .await?;
                                post_heartbeat(&client, &enrollment, &runner_id, &start, "offline").await?;
                            } else {
                                return Err(err);
                            }
                        }
                        let _ = event_tx.send(RemoteEvent::Stopped);
                        return Ok(());
                    }
                }
            }
            _ = runtime_upload_tick.tick(), if !runtime_outbox.events.is_empty() => {
                if Instant::now() < runtime_retry_not_before {
                    continue;
                }
                match runtime_outbox
                    .try_flush_one(&client, &enrollment, &runner_id)
                    .await?
                {
                    RuntimeFlushOutcome::Idle => {
                        runtime_retry_delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
                        runtime_retry_not_before = Instant::now();
                    }
                    RuntimeFlushOutcome::Retryable => {
                        runtime_retry_not_before = Instant::now() + runtime_retry_delay;
                        runtime_retry_delay = runtime_retry_delay
                            .saturating_mul(2)
                            .min(RUNTIME_UPLOAD_MAX_BACKOFF);
                    }
                    RuntimeFlushOutcome::Accepted { run_id, cursor } => {
                        runtime_retry_delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
                        runtime_retry_not_before = Instant::now();
                        event_tx
                            .send(RemoteEvent::RuntimeCursor { run_id, cursor })
                            .map_err(|_| "The terminal remote-control owner stopped.".to_string())?;
                    }
                    RuntimeFlushOutcome::AccessTokenExpired => {
                        refresh_enrollment_and_reconnect(
                            &client,
                            &mut enrollment,
                            &mut runner_id,
                            &start,
                            &event_tx,
                        )
                        .await?;
                        runtime_retry_delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
                        runtime_retry_not_before = Instant::now();
                    }
                }
            }
            () = tokio::time::sleep(SYNC_INTERVAL) => {
                if enrollment_needs_refresh(&enrollment) {
                    // Proactive refresh before expiry; reconnect to keep runner lease valid.
                    match refresh_enrollment(&client, enrollment.persisted.clone()).await {
                        Ok(new_enrollment) => {
                            enrollment = new_enrollment;
                            reconnect_runner(
                                &client,
                                &enrollment,
                                &mut runner_id,
                                &start,
                                &event_tx,
                            )
                            .await?;
                        }
                        Err(err) if err == "runner_enrollment_revoked" => {
                            delete_persisted_enrollment();
                            let device_id = enrollment.persisted.device_id.clone();
                            enrollment =
                                enroll_device(&client, &base, &start, &device_id, &event_tx).await?;
                            reconnect_runner(
                                &client,
                                &enrollment,
                                &mut runner_id,
                                &start,
                                &event_tx,
                            )
                            .await?;
                        }
                        Err(err) => return Err(err),
                    }
                }
                if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                    let hb = post_heartbeat(&client, &enrollment, &runner_id, &start, "active").await;
                    if let Err(err) = hb {
                        if err == "runner_access_token_expired" {
                            refresh_enrollment_and_reconnect(
                                &client,
                                &mut enrollment,
                                &mut runner_id,
                                &start,
                                &event_tx,
                            )
                            .await?;
                            post_heartbeat(&client, &enrollment, &runner_id, &start, "active").await?;
                        } else {
                            return Err(err);
                        }
                    }
                    last_heartbeat = Instant::now();
                }
                let runs = match list_runs(&client, &enrollment, &runner_id).await {
                    Ok(v) => v,
                    Err(err) if err == "runner_access_token_expired" => {
                        refresh_enrollment_and_reconnect(
                            &client,
                            &mut enrollment,
                            &mut runner_id,
                            &start,
                            &event_tx,
                        )
                        .await?;
                        list_runs(&client, &enrollment, &runner_id).await?
                    }
                    Err(err) => return Err(err),
                };
                for run_id in runs {
                    let since = command_cursor.get(&run_id).copied().unwrap_or(0);
                    let listed_commands = match list_commands(
                        &client,
                        &enrollment,
                        &runner_id,
                        &run_id,
                        since,
                    )
                    .await
                    {
                        Ok(v) => v,
                        Err(err) if err == "runner_access_token_expired" => {
                            refresh_enrollment_and_reconnect(
                                &client,
                                &mut enrollment,
                                &mut runner_id,
                                &start,
                                &event_tx,
                            )
                            .await?;
                            list_commands(&client, &enrollment, &runner_id, &run_id, since).await?
                        }
                        Err(err) => return Err(err),
                    };
                    for listed in listed_commands {
                        let seq = listed.seq;
                        if !listed.ack_status.is_empty() {
                            if listed.ack_status == "accepted" {
                                let rr = recover_run(
                                    &client,
                                    &enrollment,
                                    &runner_id,
                                    &run_id,
                                    "accepted command has no terminal acknowledgement after runner restart",
                                )
                                .await;
                                if let Err(err) = rr {
                                    if err == "runner_access_token_expired" {
                                        refresh_enrollment_and_reconnect(
                                            &client,
                                            &mut enrollment,
                                            &mut runner_id,
                                            &start,
                                            &event_tx,
                                        )
                                        .await?;
                                        recover_run(
                                            &client,
                                            &enrollment,
                                            &runner_id,
                                            &run_id,
                                            "accepted command has no terminal acknowledgement after runner restart",
                                        )
                                        .await?;
                                    } else {
                                        return Err(err);
                                    }
                                }
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
                            let up = upload_command_accepted(
                                &client,
                                &enrollment,
                                &runner_id,
                                &run_id,
                                seq,
                                &command,
                            )
                            .await;
                            if let Err(err) = up {
                                if err == "runner_access_token_expired" {
                                    refresh_enrollment_and_reconnect(
                                        &client,
                                        &mut enrollment,
                                        &mut runner_id,
                                        &start,
                                        &event_tx,
                                    )
                                    .await?;
                                    upload_command_accepted(
                                        &client,
                                        &enrollment,
                                        &runner_id,
                                        &run_id,
                                        seq,
                                        &command,
                                    )
                                    .await?;
                                } else {
                                    return Err(err);
                                }
                            }
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

impl RuntimeTransportOutbox {
    fn enqueue(&mut self, run_id: &str, envelope: Value) -> Result<(), String> {
        if !valid_opaque_ref(run_id) {
            return Err("The local runtime queued an invalid run id.".to_string());
        }
        let seq = runtime_envelope_seq(&envelope)
            .ok_or_else(|| "The local runtime queued an invalid event sequence.".to_string())?;
        let encoded = serde_json::to_vec(&envelope)
            .map_err(|_| "The local runtime could not encode an event.".to_string())?;
        if encoded.len() > MAX_RUNTIME_ENVELOPE_BYTES {
            return Err("The local runtime queued an oversized event.".to_string());
        }
        let key = (run_id.to_string(), seq);
        if let Some(existing) = self.events.get(&key) {
            if existing != &envelope {
                return Err(
                    "The local runtime changed an unacknowledged event sequence.".to_string(),
                );
            }
            return Ok(());
        }
        self.events.insert(key, envelope);
        Ok(())
    }

    async fn try_flush_one(
        &mut self,
        client: &Client,
        enrollment: &LiveEnrollment,
        runner_id: &str,
    ) -> Result<RuntimeFlushOutcome, String> {
        let Some(((run_id, seq), envelope)) = self
            .events
            .first_key_value()
            .map(|(key, value)| (key.clone(), value.clone()))
        else {
            return Ok(RuntimeFlushOutcome::Idle);
        };
        match post_runtime_event(client, enrollment, runner_id, &run_id, seq, &envelope).await? {
            RuntimePostOutcome::Retryable => Ok(RuntimeFlushOutcome::Retryable),
            RuntimePostOutcome::AccessTokenExpired => Ok(RuntimeFlushOutcome::AccessTokenExpired),
            RuntimePostOutcome::Accepted(cursor) => {
                self.events.retain(|(pending_run, pending_seq), _| {
                    pending_run != &run_id || *pending_seq > cursor
                });
                Ok(RuntimeFlushOutcome::Accepted { run_id, cursor })
            }
        }
    }
}

/// Flushes every queued runtime envelope through the server-confirmed cursor
/// before a stop may be acknowledged. Emits `RuntimeCursor` events so the
/// controller compacts its journal as acknowledgements land. Failing to drain
/// by `deadline` is a hard error: the stop is *not* confirmed and the caller
/// must leave ownership locked.
#[allow(clippy::too_many_arguments)]
async fn drain_runtime_outbox_for_stop(
    client: &Client,
    enrollment: &mut LiveEnrollment,
    runner_id: &mut String,
    start: &RemoteStart,
    event_tx: &mpsc::UnboundedSender<RemoteEvent>,
    outbox: &mut RuntimeTransportOutbox,
    deadline: Instant,
) -> Result<(), String> {
    let mut delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
    while !outbox.events.is_empty() {
        if Instant::now() >= deadline {
            return Err(
                "queued runtime events were not server-acknowledged in time; the stop was not confirmed"
                    .to_string(),
            );
        }
        match outbox.try_flush_one(client, enrollment, runner_id).await? {
            RuntimeFlushOutcome::Idle => break,
            RuntimeFlushOutcome::Accepted { run_id, cursor } => {
                delay = RUNTIME_UPLOAD_RETRY_INTERVAL;
                let _ = event_tx.send(RemoteEvent::RuntimeCursor { run_id, cursor });
            }
            RuntimeFlushOutcome::Retryable => {
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2).min(RUNTIME_UPLOAD_MAX_BACKOFF);
            }
            RuntimeFlushOutcome::AccessTokenExpired => {
                refresh_enrollment_and_reconnect(client, enrollment, runner_id, start, event_tx)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn post_runtime_event(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &str,
    run_id: &str,
    seq: u64,
    envelope: &Value,
) -> Result<RuntimePostOutcome, String> {
    let url = control_plane_url(
        &enrollment.persisted.control_plane_base,
        &["api", "local-runners", runner_id, "runs", run_id, "events"],
        &[],
    )?;
    let response = match client
        .post(url)
        .bearer_auth(&enrollment.access_token)
        .json(&json!({
            "acknowledgements": [],
            "envelopes": [envelope],
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(RuntimePostOutcome::Retryable),
    };
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Ok(RuntimePostOutcome::AccessTokenExpired);
    }
    if matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
    {
        return Ok(RuntimePostOutcome::Retryable);
    }
    if !status.is_success() {
        return Err(format!(
            "The remote-control server rejected runtime event {seq} ({status})."
        ));
    }
    let value = match read_bounded_json(response).await {
        Ok(value) => value,
        Err(_) => return Ok(RuntimePostOutcome::Retryable),
    };
    let Some(cursor) = value
        .get("cursor")
        .and_then(Value::as_u64)
        .filter(|cursor| *cursor >= seq && *cursor <= JS_MAX_SAFE_INTEGER)
    else {
        // A success without a durable cursor is indistinguishable from a lost
        // response. Retain and retry the exact same event body.
        return Ok(RuntimePostOutcome::Retryable);
    };
    Ok(RuntimePostOutcome::Accepted(cursor))
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
    device_id: &str,
    event_tx: &mpsc::UnboundedSender<RemoteEvent>,
) -> Result<LiveEnrollment, String> {
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
        let enrollment = enrollment_from_exchange(exchange, base, device_id, start)?;
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
) -> Result<RunnerConnection, String> {
    let value = runner_request(
        client,
        enrollment,
        Method::POST,
        &["api", "local-runners", "connect"],
        &[],
        Some(connect_runner_body(enrollment, start)),
    )
    .await?;
    parse_runner_connection(&value, enrollment, start)
}

fn connect_runner_body(enrollment: &LiveEnrollment, start: &RemoteStart) -> Value {
    let mut body = json!({
        "deviceId": enrollment.persisted.device_id,
        "targetRef": start.target_ref,
        "displayLabel": start.workspace_label,
        "runtimeVersion": start.runtime_version,
        "runtimeCommit": start.runtime_commit,
        "capabilities": CAPABILITIES,
        "status": "active",
        // This is the only session attachment input. It is an opaque runtime
        // id, never a workspace path, prompt, environment, or credential.
        "sessionRef": start.session_id,
    });
    if let Some(repo) = start
        .git_remote
        .as_deref()
        .and_then(normalize_observed_git_repo)
    {
        body["gitRemote"] = json!(repo);
    }
    body
}

/// Collapse a git remote to `owner/name`. Paths, credentials, and unknown
/// hosts are dropped so the control plane never receives a folder identity.
pub fn normalize_observed_git_repo(input: &str) -> Option<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    let stripped = raw
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .replace("https://github.com/", "")
        .replace("https://www.github.com/", "")
        .replace("http://github.com/", "")
        .replace("https://cnb.cool/", "")
        .replace("https://gitee.com/", "")
        .replace("git@github.com:", "")
        .replace("git@gitee.com:", "");
    if stripped.starts_with('/') || stripped.contains(":\\") || stripped.contains("\\\\") {
        return None;
    }
    let parts: Vec<&str> = stripped
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let owner = *parts.get(parts.len().checked_sub(2)?)?;
    let name = *parts.last()?;
    if owner.len() > 80 || name.len() > 80 {
        return None;
    }
    if !owner
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return None;
    }
    if matches!(owner, "." | "..") || matches!(name, "." | "..") {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

pub fn observed_git_repo(workspace: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_observed_git_repo(std::str::from_utf8(&output.stdout).ok()?)
}

fn parse_runner_connection(
    value: &Value,
    enrollment: &LiveEnrollment,
    start: &RemoteStart,
) -> Result<RunnerConnection, String> {
    let response = value
        .as_object()
        .filter(|record| {
            record.len() == 2 && record.contains_key("runner") && record.contains_key("attachment")
        })
        .ok_or_else(|| "Codewhale returned an invalid runner attachment response.".to_string())?;
    let runner = response
        .get("runner")
        .and_then(Value::as_object)
        .ok_or_else(|| "Codewhale returned an invalid runner lease.".to_string())?;
    let runner_id = runner
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .map(ToString::to_string)
        .ok_or_else(|| "Codewhale returned an invalid runner lease.".to_string())?;
    let runner_binding_matches = runner.get("userId").and_then(Value::as_str)
        == Some(enrollment.persisted.account_ref.as_str())
        && runner.get("deviceId").and_then(Value::as_str)
            == Some(enrollment.persisted.device_id.as_str())
        && runner.get("targetRef").and_then(Value::as_str) == Some(start.target_ref.as_str())
        && runner.get("runtimeVersion").and_then(Value::as_str)
            == Some(start.runtime_version.as_str())
        && runner.get("runtimeCommit").and_then(Value::as_str)
            == Some(start.runtime_commit.as_str())
        && runner.get("controlPath").and_then(Value::as_str) == Some("outbound_relay")
        && runner.get("status").and_then(Value::as_str) == Some("active")
        && runner.get("active").and_then(Value::as_bool) == Some(true)
        && exact_capabilities(runner.get("capabilities"));
    if !runner_binding_matches {
        return Err("Codewhale returned a runner lease for a different session.".to_string());
    }

    let attachment = response
        .get("attachment")
        .and_then(Value::as_object)
        .filter(|record| {
            record.len() == 4
                && record.contains_key("runId")
                && record.contains_key("workspaceId")
                && record.contains_key("runtimeCursor")
                && record.contains_key("snapshotPresent")
        })
        .ok_or_else(|| "Codewhale returned an invalid session attachment.".to_string())?;
    let run_id = attachment
        .get("runId")
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .map(ToString::to_string)
        .ok_or_else(|| "Codewhale returned an invalid attached run.".to_string())?;
    let workspace_id = attachment
        .get("workspaceId")
        .and_then(Value::as_str)
        .filter(|value| valid_opaque_ref(value))
        .map(ToString::to_string)
        .ok_or_else(|| "Codewhale returned an invalid attached workspace.".to_string())?;
    let runtime_cursor = attachment
        .get("runtimeCursor")
        .and_then(Value::as_u64)
        .filter(|value| *value <= JS_MAX_SAFE_INTEGER)
        .ok_or_else(|| "Codewhale returned an invalid runtime event cursor.".to_string())?;
    let snapshot_present = attachment
        .get("snapshotPresent")
        .and_then(Value::as_bool)
        .ok_or_else(|| "Codewhale returned an invalid snapshot receipt.".to_string())?;

    let links = parse_remote_links(runner, &run_id);

    Ok(RunnerConnection {
        runner_id,
        attachment: RemoteAttachment {
            run_id,
            workspace_id,
            runtime_cursor,
            snapshot_present,
        },
        links,
    })
}

/// Read the optional `runUrl` / `computerUrl` advertised on the runner lease.
/// Absent fields yield `None`; present-but-invalid values are dropped (never
/// displayed, never opened) rather than failing the attachment, because a
/// link is a convenience receipt and not part of the ownership contract.
fn parse_remote_links(runner: &serde_json::Map<String, Value>, run_id: &str) -> RemoteLinks {
    let run_url = runner
        .get("runUrl")
        .and_then(Value::as_str)
        .and_then(|value| validate_run_url(value, run_id));
    let computer_url = runner
        .get("computerUrl")
        .and_then(Value::as_str)
        .and_then(validate_computer_url);
    RemoteLinks {
        run_url,
        computer_url,
    }
}

/// Parse a web link and accept it only on the Codewhale app origin (or, in
/// debug builds only, a loopback development origin) with no credentials,
/// port, or fragment — the same shape `validate_authorization_url` enforces.
fn app_link_url(value: &str) -> Option<Url> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return None;
    }
    let url = Url::parse(trimmed).ok()?;
    let production_origin =
        url.scheme() == "https" && url.host_str() == Some(APP_ORIGIN_HOST) && url.port().is_none();
    let debug_loopback = cfg!(debug_assertions)
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if !(production_origin || debug_loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

/// `/session?run=<runId>` for exactly the attached run; anything else is
/// dropped so the terminal can never send the user to a different session.
fn validate_run_url(value: &str, run_id: &str) -> Option<String> {
    let url = app_link_url(value)?;
    let pairs = url.query_pairs().collect::<Vec<_>>();
    if url.path() != "/session" || pairs.len() != 1 || pairs[0].0 != "run" || pairs[0].1 != run_id {
        return None;
    }
    Some(url.to_string())
}

/// `/settings` (optionally `?section=…`) on the app origin.
fn validate_computer_url(value: &str) -> Option<String> {
    let url = app_link_url(value)?;
    let pairs = url.query_pairs().collect::<Vec<_>>();
    if url.path() != "/settings"
        || pairs.len() > 1
        || pairs.iter().any(|(key, _)| *key != "section")
    {
        return None;
    }
    Some(url.to_string())
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
        return Err("runner_access_token_expired".to_string());
    }
    if !response.status().is_success() {
        let status = response.status();
        let excerpt = rejection_excerpt(response).await;
        return Err(match excerpt {
            Some(reason) => {
                format!("The remote-control server rejected a request ({status}): {reason}.")
            }
            None => format!("The remote-control server rejected a request ({status})."),
        });
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
        let status = response.status();
        let excerpt = rejection_excerpt(response).await;
        return Err(match excerpt {
            Some(reason) => {
                format!("Codewhale rejected remote-control enrollment ({status}): {reason}.")
            }
            None => format!("Codewhale rejected remote-control enrollment ({status})."),
        });
    }
    read_bounded_json(response).await
}

/// Bounded error-body read: at most MAX_RESPONSE_BYTES, so a misbehaving
/// control plane cannot force an unbounded in-memory read through the
/// rejection-excerpt path.
async fn rejection_excerpt(response: reqwest::Response) -> Option<String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BYTES {
            return None;
        }
    }
    sanitized_rejection_excerpt(&body)
}

/// Sanitized, bounded reason excerpt from a rejection body. Only the
/// conventional error fields are read, control characters are stripped, and
/// the excerpt is capped — raw server bytes are never echoed further.
fn sanitized_rejection_excerpt(body: &[u8]) -> Option<String> {
    let parsed: Value = serde_json::from_slice(body).ok()?;
    for field in ["error", "message", "title"] {
        if let Some(text) = parsed.get(field).and_then(Value::as_str) {
            let cleaned: String = text
                .chars()
                .filter(|character| !character.is_control())
                .take(140)
                .collect();
            let trimmed = cleaned.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
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

fn load_persisted_device_identity() -> Result<Option<PersistedDeviceIdentity>, String> {
    let Some(raw) = codewhale_secrets::Secrets::auto_detect()
        .get(DEVICE_IDENTITY_SECRET_SLOT)
        .map_err(|error| format!("Could not read the saved remote-control device id: {error}"))?
    else {
        return Ok(None);
    };
    // An unreadable identity is replaced rather than fatal: the worst case is
    // one extra computer row, never a lost session.
    Ok(serde_json::from_str(&raw).ok())
}

fn save_persisted_device_identity(identity: &PersistedDeviceIdentity) -> Result<(), String> {
    let raw = serde_json::to_string(identity)
        .map_err(|_| "Could not encode the remote-control device id.".to_string())?;
    codewhale_secrets::Secrets::auto_detect()
        .set(DEVICE_IDENTITY_SECRET_SLOT, &raw)
        .map_err(|error| format!("Could not securely save the remote-control device id: {error}"))
}

/// Load (or mint and persist) the machine-stable device id.
fn stable_device_id(enrollment_device_id: Option<&str>) -> Result<String, String> {
    let saved = load_persisted_device_identity()?;
    let (device_id, needs_save) = resolve_device_identity(saved, enrollment_device_id);
    if needs_save {
        save_persisted_device_identity(&PersistedDeviceIdentity {
            schema_version: 1,
            device_id: device_id.clone(),
        })?;
    }
    Ok(device_id)
}

fn delete_persisted_enrollment() {
    if let Err(error) = codewhale_secrets::Secrets::auto_detect().delete(ENROLLMENT_SECRET_SLOT) {
        tracing::warn!("could not delete revoked remote-control enrollment: {error}");
    }
}

async fn refresh_enrollment_and_reconnect(
    client: &Client,
    enrollment: &mut LiveEnrollment,
    runner_id: &mut String,
    start: &RemoteStart,
    event_tx: &mpsc::UnboundedSender<RemoteEvent>,
) -> Result<(), String> {
    let base = enrollment.persisted.control_plane_base.clone();
    match refresh_enrollment(client, enrollment.persisted.clone()).await {
        Ok(new_enrollment) => {
            *enrollment = new_enrollment;
            reconnect_runner(client, enrollment, runner_id, start, event_tx).await
        }
        Err(err) if err == "runner_enrollment_revoked" => {
            delete_persisted_enrollment();
            let device_id = enrollment.persisted.device_id.clone();
            *enrollment = enroll_device(client, &base, start, &device_id, event_tx).await?;
            reconnect_runner(client, enrollment, runner_id, start, event_tx).await
        }
        Err(err) => Err(err),
    }
}

async fn reconnect_runner(
    client: &Client,
    enrollment: &LiveEnrollment,
    runner_id: &mut String,
    start: &RemoteStart,
    event_tx: &mpsc::UnboundedSender<RemoteEvent>,
) -> Result<(), String> {
    let connection = connect_runner(client, enrollment, start).await?;
    *runner_id = connection.runner_id;
    event_tx
        .send(RemoteEvent::Attachment {
            attachment: connection.attachment,
            links: connection.links,
        })
        .map_err(|_| "The terminal remote-control owner stopped.".to_string())
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

fn valid_session_ref(value: &str) -> bool {
    (1..=160).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'@')
        })
        && !value.contains("..")
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
    use crate::models::Role;
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, Request, Respond, ResponseTemplate,
        matchers::{body_json, method, path, query_param},
    };

    #[derive(Clone, Default)]
    struct AmbiguousRuntimeResponder {
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    impl Respond for AmbiguousRuntimeResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).expect("runtime request JSON");
            let mut bodies = self.bodies.lock().expect("runtime request bodies");
            bodies.push(body);
            if bodies.len() == 1 {
                // Model a committed request whose response was truncated in
                // transit. The client must keep the exact body and retry.
                ResponseTemplate::new(200).set_body_raw("{", "application/json")
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "accepted": [],
                    "count": 0,
                    "cursor": 1
                }))
            }
        }
    }

    fn text_message(role: &str, text: impl Into<String>) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    fn fixture_start() -> RemoteStart {
        RemoteStart {
            workspace_label: "private-project".to_string(),
            target_ref: "target_fixture".to_string(),
            session_id: "session:fixture@01".to_string(),
            runtime_version: "0.9.6".to_string(),
            runtime_commit: "a".repeat(40),
            journal_dir: None,
            git_remote: None,
        }
    }

    fn fixture_enrollment(base: &str) -> LiveEnrollment {
        LiveEnrollment {
            persisted: PersistedEnrollment {
                schema_version: 1,
                control_plane_base: base.to_string(),
                runner_enrollment_id: "enrollment_fixture".to_string(),
                account_ref: "account_fixture".to_string(),
                device_id: "device_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                target_grant_ref: "grant_fixture".to_string(),
                runtime_version: "0.9.6".to_string(),
                runtime_commit: "a".repeat(40),
                bootstrap_secret: "b".repeat(43),
            },
            access_token: "fixture-runner-access-token".to_string(),
        }
    }

    fn fixture_connection_response() -> Value {
        json!({
            "runner": {
                "id": "runner_fixture",
                "userId": "account_fixture",
                "deviceId": "device_fixture",
                "targetRef": "target_fixture",
                "displayLabel": "private-project",
                "runtimeVersion": "0.9.6",
                "runtimeCommit": "a".repeat(40),
                "capabilities": CAPABILITIES,
                "controlPath": "outbound_relay",
                "status": "active",
                "active": true,
                "capacity": 1,
                "lastHeartbeatAt": "2026-08-08T12:00:00.000Z",
                "expiresAt": "2026-08-08T12:01:30.000Z",
                "revokedAt": "",
                "createdAt": "2026-08-08T12:00:00.000Z",
                "updatedAt": "2026-08-08T12:00:00.000Z"
            },
            "attachment": {
                "runId": "run_fixture",
                "workspaceId": "workspace_fixture",
                "runtimeCursor": 41,
                "snapshotPresent": false
            }
        })
    }

    #[test]
    fn observed_git_repo_is_owner_name_not_a_path() {
        assert_eq!(
            normalize_observed_git_repo("git@github.com:Hmbown/CodeWhale.git").as_deref(),
            Some("Hmbown/CodeWhale")
        );
        assert_eq!(
            normalize_observed_git_repo("https://github.com/Hmbown/cwc.git").as_deref(),
            Some("Hmbown/cwc")
        );
        assert_eq!(
            normalize_observed_git_repo("/Volumes/VIXinSSD/CW/codewhale"),
            None
        );
    }

    #[test]
    fn connect_body_can_carry_an_observed_repo_without_a_path() {
        let enrollment = fixture_enrollment("https://api.codewhale.net/");
        let mut start = fixture_start();
        start.git_remote = Some("git@github.com:Hmbown/CodeWhale.git".to_string());
        let body = connect_runner_body(&enrollment, &start);
        assert_eq!(body["gitRemote"], "Hmbown/CodeWhale");
        assert!(body.get("workspacePath").is_none());
        assert!(body.get("path").is_none());
    }

    #[test]
    fn target_identity_is_stable_without_exposing_the_path() {
        let target = target_ref(Path::new("/Users/alice/private/project"));
        assert!(target.starts_with("target_"));
        assert_eq!(target.len(), 39);
        assert!(!target.contains("alice"));
        // Every session opened in the same folder shares one target, so the
        // control plane keeps one grant per folder rather than one per `/rc`.
        assert_eq!(
            target,
            target_ref(Path::new("/Users/alice/private/project"))
        );
        assert_ne!(target, target_ref(Path::new("/Users/alice/private/other")));
    }

    #[tokio::test]
    async fn connect_request_sends_only_the_opaque_session_ref_for_attachment() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        let enrollment = fixture_enrollment(&format!("{}/", server.uri()));
        let start = fixture_start();
        let expected = json!({
            "deviceId": "device_fixture",
            "targetRef": "target_fixture",
            "displayLabel": "private-project",
            "runtimeVersion": "0.9.6",
            "runtimeCommit": "a".repeat(40),
            "capabilities": CAPABILITIES,
            "status": "active",
            "sessionRef": "session:fixture@01"
        });
        assert_eq!(connect_runner_body(&enrollment, &start), expected);
        for forbidden in [
            "sessionId",
            "workspacePath",
            "path",
            "prompt",
            "environment",
            "env",
            "token",
            "credential",
        ] {
            assert!(expected.get(forbidden).is_none(), "leaked {forbidden}");
        }
        Mock::given(method("POST"))
            .and(path("/api/local-runners/connect"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_connection_response()))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("fixture client");

        let connection = connect_runner(&client, &enrollment, &start)
            .await
            .expect("strict runner attachment");

        assert_eq!(connection.runner_id, "runner_fixture");
        assert_eq!(connection.attachment.run_id, "run_fixture");
        assert_eq!(connection.attachment.runtime_cursor, 41);
        assert!(!connection.attachment.snapshot_present);
    }

    #[test]
    fn connection_without_links_yields_no_urls() {
        let enrollment = fixture_enrollment("https://api.codewhale.net/");
        let start = fixture_start();
        let connection =
            parse_runner_connection(&fixture_connection_response(), &enrollment, &start)
                .expect("legacy lease without links still attaches");
        assert_eq!(connection.links, RemoteLinks::default());
        assert!(connection.links.run_url.is_none());
        assert!(connection.links.computer_url.is_none());
    }

    #[test]
    fn connection_links_are_parsed_from_the_runner_lease() {
        let enrollment = fixture_enrollment("https://api.codewhale.net/");
        let start = fixture_start();
        let mut value = fixture_connection_response();
        value["runner"]["runUrl"] = json!("https://app.codewhale.net/session?run=run_fixture");
        value["runner"]["computerUrl"] =
            json!("https://app.codewhale.net/settings?section=workspaces");
        let connection = parse_runner_connection(&value, &enrollment, &start)
            .expect("lease with links attaches");
        assert_eq!(
            connection.links.run_url.as_deref(),
            Some("https://app.codewhale.net/session?run=run_fixture")
        );
        assert_eq!(
            connection.links.computer_url.as_deref(),
            Some("https://app.codewhale.net/settings?section=workspaces")
        );
    }

    #[test]
    fn connection_links_off_origin_or_for_another_run_are_dropped_not_fatal() {
        let enrollment = fixture_enrollment("https://api.codewhale.net/");
        let start = fixture_start();
        for spoofed in [
            "http://app.codewhale.net/session?run=run_fixture",
            "https://app.codewhale.net.evil.example/session?run=run_fixture",
            "https://evil.example/session?run=run_fixture",
            "https://user:pw@app.codewhale.net/session?run=run_fixture",
            "https://app.codewhale.net:8443/session?run=run_fixture",
            "https://app.codewhale.net/session?run=run_fixture#token=abc",
            "https://app.codewhale.net/session?run=run_other",
            "https://app.codewhale.net/session?run=run_fixture&next=https://evil.example",
            "https://app.codewhale.net/logout?run=run_fixture",
            "",
            "not a url",
        ] {
            let mut value = fixture_connection_response();
            value["runner"]["runUrl"] = json!(spoofed);
            value["runner"]["computerUrl"] = json!(spoofed);
            let connection = parse_runner_connection(&value, &enrollment, &start)
                .expect("a bad link must not break the attachment");
            assert!(
                connection.links.run_url.is_none(),
                "run link accepted: {spoofed}"
            );
            assert!(
                connection.links.computer_url.is_none(),
                "computer link accepted: {spoofed}"
            );
        }
        // A non-string value is treated as absent.
        let mut value = fixture_connection_response();
        value["runner"]["runUrl"] = json!(42);
        let connection = parse_runner_connection(&value, &enrollment, &start).unwrap();
        assert!(connection.links.run_url.is_none());
    }

    #[test]
    fn banner_leads_with_the_session_link_when_present() {
        let with_link = remote_control_banner(
            "account_fixture",
            "runner_fixture",
            Some("https://app.codewhale.net/session?run=run_fixture"),
        );
        assert_eq!(
            with_link,
            "WEB MIRROR · https://app.codewhale.net/session?run=run_fixture · /rc stop"
        );
        let without_link = remote_control_banner("account_fixture", "runner_fixture", None);
        assert_eq!(
            without_link,
            "WEB MIRROR · account account_fixture · runner runner_fixture · /rc stop"
        );
        let notice =
            remote_control_link_notice("https://app.codewhale.net/session?run=run_fixture");
        assert!(notice.starts_with(
            "Remote control is live at https://app.codewhale.net/session?run=run_fixture"
        ));
        assert!(notice.contains("/rc open"));
        assert!(notice.contains("/rc link"));
    }

    #[test]
    fn controller_exposes_links_only_while_connected() {
        let mut controller = RemoteControlController::default();
        assert!(controller.run_url().is_none());
        let (worker_tx, _worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 0,
                    snapshot_present: false,
                },
                links: RemoteLinks {
                    run_url: Some("https://app.codewhale.net/session?run=run_fixture".to_string()),
                    computer_url: Some(
                        "https://app.codewhale.net/settings?section=workspaces".to_string(),
                    ),
                },
            })
            .unwrap();
        controller.try_next_event().unwrap();
        assert_eq!(
            controller.run_url(),
            Some("https://app.codewhale.net/session?run=run_fixture")
        );
        assert_eq!(
            controller.computer_url(),
            Some("https://app.codewhale.net/settings?section=workspaces")
        );
        assert!(
            controller
                .status_line()
                .contains("open https://app.codewhale.net/session?run=run_fixture")
        );

        event_tx.send(RemoteEvent::Stopped).unwrap();
        controller.try_next_event().unwrap();
        assert!(controller.run_url().is_none());
        assert!(controller.computer_url().is_none());
    }

    #[test]
    fn connecting_never_blocks_local_prompts_and_shares_approvals_only_once_attached() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Connecting;
        controller.status_detail = "waiting for account authorization".to_string();
        // Mirror semantics: there is no local-input gate at all. The only
        // shared-decision surface is the approval card, and only once a
        // typed turn is bound.
        assert!(
            !controller.can_share_approval_with_web(),
            "without a confirmed run cursor the web cannot receive an approval"
        );

        controller.status = Status::Connected;
        controller.attached_run_id = Some("run_fixture".to_string());
        assert!(
            !controller.can_share_approval_with_web(),
            "a connected idle session has no typed turn to receive approvals"
        );
        assert!(controller.attach_current_local_turn(Some("turn_fixture")));
        assert!(controller.can_share_approval_with_web());
    }

    #[test]
    fn persisted_device_identity_survives_a_reload_and_outlives_enrollments() {
        let (minted, needs_save) = resolve_device_identity(None, None);
        assert!(needs_save);
        assert!(minted.starts_with("device_"));
        assert!(valid_opaque_ref(&minted));

        let identity = PersistedDeviceIdentity {
            schema_version: 1,
            device_id: minted.clone(),
        };
        let raw = serde_json::to_string(&identity).expect("encode device identity");
        let reloaded: PersistedDeviceIdentity =
            serde_json::from_str(&raw).expect("decode device identity");
        assert_eq!(reloaded, identity);

        // A saved identity wins over any enrollment's id and needs no re-save.
        let (resolved, needs_save) =
            resolve_device_identity(Some(reloaded.clone()), Some("device_enrolled"));
        assert_eq!(resolved, minted);
        assert!(!needs_save);

        // Without a saved identity, an existing enrollment's device id is
        // adopted (upgrading terminals keep their computer row) and persisted.
        let (adopted, needs_save) = resolve_device_identity(None, Some("device_enrolled"));
        assert_eq!(adopted, "device_enrolled");
        assert!(needs_save);

        // An unreadable identity is replaced, never fatal.
        let broken = PersistedDeviceIdentity {
            schema_version: 2,
            device_id: "x".to_string(),
        };
        let (replaced, needs_save) = resolve_device_identity(Some(broken), None);
        assert_ne!(replaced, "x");
        assert!(needs_save);
        assert!(serde_json::from_str::<PersistedDeviceIdentity>("{\"deviceId\":\"a\"}").is_err());
    }

    #[test]
    fn attachment_response_validation_fails_closed() {
        let enrollment = fixture_enrollment("https://api.codewhale.net/");
        let start = fixture_start();
        let valid = fixture_connection_response();
        assert!(parse_runner_connection(&valid, &enrollment, &start).is_ok());

        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("attachment");
        assert!(parse_runner_connection(&missing, &enrollment, &start).is_err());

        let mut oversized_cursor = valid.clone();
        oversized_cursor["attachment"]["runtimeCursor"] = json!(JS_MAX_SAFE_INTEGER + 1);
        assert!(parse_runner_connection(&oversized_cursor, &enrollment, &start).is_err());

        let mut false_receipt = valid.clone();
        false_receipt["attachment"]["snapshotPresent"] = json!("false");
        assert!(parse_runner_connection(&false_receipt, &enrollment, &start).is_err());

        let mut extra_authority = valid.clone();
        extra_authority["attachment"]["workspacePath"] = json!("/private/project");
        assert!(parse_runner_connection(&extra_authority, &enrollment, &start).is_err());

        let mut wrong_control_path = valid;
        wrong_control_path["runner"]["controlPath"] = json!("direct_native");
        assert!(parse_runner_connection(&wrong_control_path, &enrollment, &start).is_err());
    }

    #[test]
    fn attachment_cursor_seeds_the_first_runtime_event_sequence() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 41,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();

        assert!(matches!(
            controller.try_next_event(),
            Some(RemoteEvent::Connected { .. })
        ));
        controller.upload_snapshot("run_fixture", &[]);

        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("expected snapshot upload");
        };
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0]["event"], "session.snapshot");
        assert_eq!(envelopes[0]["seq"], 42);
    }

    #[test]
    fn connected_attachment_adopts_the_existing_turn_and_streams_typed_state_once() {
        let (mut controller, mut worker_rx, event_tx) = wired_controller();
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 11,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        assert!(matches!(
            controller.try_next_event(),
            Some(RemoteEvent::Connected { .. })
        ));
        assert!(
            !controller.can_share_approval_with_web(),
            "a connected attachment without a bound typed turn keeps approvals local"
        );
        assert!(controller.attach_current_local_turn(Some("turn_existing")));
        assert!(
            controller.can_share_approval_with_web(),
            "the bound active turn can carry typed approvals to the web"
        );
        assert!(controller.has_active_run());
        assert!(controller.active_run_matches("run_fixture"));
        assert!(
            !controller.attach_current_local_turn(Some("turn_duplicate")),
            "replaying the attachment must not replace or duplicate the turn"
        );

        controller.observe_engine_event(&EngineEvent::MessageDelta {
            index: 0,
            content: "existing turn output".to_string(),
        });
        controller.observe_engine_event(&EngineEvent::ToolCallStarted {
            id: "tool_existing".to_string(),
            name: "shell".to_string(),
            input: json!({ "never": "relayed" }),
        });
        let gate = controller.record_remote_approval(
            "tool_existing",
            "shell",
            "approve bounded fixture",
            &json!({ "credential": "must-not-cross" }),
            "approval-key",
            None,
        );

        let mut envelopes = Vec::new();
        for _ in 0..3 {
            let WorkerCommand::Upload {
                envelopes: batch, ..
            } = worker_rx.try_recv().expect("typed active-turn upload")
            else {
                panic!("active-turn state must use the runtime envelope channel");
            };
            envelopes.extend(batch);
        }
        assert_eq!(
            envelopes
                .iter()
                .map(|event| event["event"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["item.delta", "item.started", "approval.required"]
        );
        assert!(
            envelopes
                .iter()
                .all(|event| event["turn_id"] == "turn_existing")
        );
        assert_eq!(
            envelopes[2]["payload"]["approval_id"].as_str(),
            Some(gate.as_str())
        );
        let projected = serde_json::to_string(&envelopes).unwrap();
        assert!(!projected.contains("must-not-cross"));
        assert!(!projected.contains("approval-key"));
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_window_attachment_promotes_on_typed_start_and_reconnect_is_idempotent() {
        let (mut controller, mut worker_rx, event_tx) = wired_controller();
        let attachment = RemoteAttachment {
            run_id: "run_fixture".to_string(),
            workspace_id: "workspace_fixture".to_string(),
            runtime_cursor: 3,
            snapshot_present: false,
        };
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: attachment.clone(),
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();

        assert!(controller.attach_current_local_turn(None));
        assert!(
            controller.has_active_run(),
            "the dispatch window gates stop"
        );
        assert!(controller.active_run_matches("run_fixture"));
        assert!(
            !controller.can_share_approval_with_web(),
            "a pending dispatch has no typed turn id, so approvals stay local"
        );
        assert!(worker_rx.try_recv().is_err());

        controller.observe_engine_event(&EngineEvent::TurnStarted {
            turn_id: "turn_started_later".to_string(),
            created_at: chrono::Utc::now(),
            route: None,
        });
        let WorkerCommand::Upload { envelopes, .. } =
            worker_rx.try_recv().expect("one typed turn start")
        else {
            panic!("turn start must use the runtime envelope channel");
        };
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0]["seq"], 4);
        assert_eq!(envelopes[0]["event"], "turn.started");
        assert_eq!(envelopes[0]["turn_id"], "turn_started_later");
        let started_envelope = envelopes[0].clone();
        assert!(
            controller.can_share_approval_with_web(),
            "typed TurnStarted promotes the dispatch into a web-owned active turn"
        );

        event_tx
            .send(RemoteEvent::Attachment {
                attachment,
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();
        assert!(
            !controller.attach_current_local_turn(Some("turn_replayed")),
            "a reconnect must not rebind the live turn"
        );
        let WorkerCommand::Upload { envelopes, .. } = worker_rx
            .try_recv()
            .expect("the unacknowledged start is replayed unchanged")
        else {
            panic!("runtime replay must use the envelope channel");
        };
        assert_eq!(
            envelopes,
            vec![started_envelope],
            "retry safety preserves the original sequence and payload"
        );
        assert!(worker_rx.try_recv().is_err());

        controller.observe_engine_event(&turn_complete_event());
        let WorkerCommand::Upload { envelopes, .. } =
            worker_rx.try_recv().expect("one terminal receipt")
        else {
            panic!("turn completion must use the runtime envelope channel");
        };
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0]["seq"], 5);
        assert_eq!(envelopes[0]["event"], "turn.completed");
        assert_eq!(envelopes[0]["turn_id"], "turn_started_later");
        assert!(!controller.has_active_run());
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_window_that_ends_before_typed_start_returns_to_idle_attachment() {
        let (mut controller, mut worker_rx, event_tx) = wired_controller();
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 8,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();

        assert!(controller.attach_current_local_turn(None));
        assert!(controller.has_active_run());
        assert!(controller.release_unstarted_local_turn());
        assert!(!controller.has_active_run());
        assert!(!controller.can_share_approval_with_web());
        assert!(
            !controller.release_unstarted_local_turn(),
            "reconciling the same idle boundary is idempotent"
        );
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn fresh_controller_refreshes_old_server_snapshot_then_deduplicates_reconnects() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 7,
                    snapshot_present: true,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();

        controller.upload_snapshot("run_fixture", &[]);
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("fresh controller must refresh saved history");
        };
        assert_eq!(envelopes[0]["seq"], 8);

        event_tx
            .send(RemoteEvent::Attachment {
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 7,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();
        controller.upload_snapshot("run_fixture", &[]);
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("unacknowledged snapshot must be retried");
        };
        assert_eq!(envelopes[0]["seq"], 8);
        controller.upload_snapshot("run_fixture", &[]);
        assert!(worker_rx.try_recv().is_err());

        event_tx
            .send(RemoteEvent::Attachment {
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 8,
                    snapshot_present: true,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();
        controller.upload_snapshot("run_fixture", &[]);
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn reconnect_cursor_retires_only_the_acknowledged_prefix() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        controller.event_seq.insert("run_fixture".to_string(), 6);
        controller.upload_envelope(
            "run_fixture",
            "item.delta",
            None,
            json!({ "delta": "seven" }),
        );
        controller.upload_envelope(
            "run_fixture",
            "item.delta",
            None,
            json!({ "delta": "eight" }),
        );
        worker_rx.try_recv().unwrap();
        worker_rx.try_recv().unwrap();

        event_tx
            .send(RemoteEvent::Attachment {
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 7,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();

        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("seq 8 must remain pending");
        };
        assert_eq!(envelopes.len(), 1);
        assert_eq!(envelopes[0]["seq"], 8);
        assert_eq!(
            controller
                .pending_runtime_events
                .get("run_fixture")
                .unwrap()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![8]
        );

        event_tx
            .send(RemoteEvent::Attachment {
                attachment: RemoteAttachment {
                    run_id: "run_fixture".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 6,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("older cursor cannot discard seq 8");
        };
        assert_eq!(envelopes[0]["seq"], 8);

        event_tx
            .send(RemoteEvent::RuntimeCursor {
                run_id: "run_fixture".to_string(),
                cursor: 8,
            })
            .unwrap();
        controller.try_next_event().unwrap();
        assert!(
            !controller
                .pending_runtime_events
                .contains_key("run_fixture")
        );
    }

    #[tokio::test]
    async fn ambiguous_success_retries_the_identical_runtime_event_until_cursor_acceptance() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        let responder = AmbiguousRuntimeResponder::default();
        Mock::given(method("POST"))
            .and(path(
                "/api/local-runners/runner_fixture/runs/run_fixture/events",
            ))
            .respond_with(responder.clone())
            .expect(2)
            .mount(&server)
            .await;
        let enrollment = fixture_enrollment(&format!("{}/", server.uri()));
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("fixture client");
        let envelope = runtime_envelope(
            1,
            "item.delta",
            None,
            "2026-08-08T12:00:00Z".to_string(),
            json!({ "delta": "exact body" }),
        );
        let mut outbox = RuntimeTransportOutbox::default();
        outbox
            .enqueue("run_fixture", envelope)
            .expect("queue runtime event");

        assert_eq!(
            outbox
                .try_flush_one(&client, &enrollment, "runner_fixture")
                .await
                .unwrap(),
            RuntimeFlushOutcome::Retryable
        );
        assert_eq!(outbox.events.len(), 1);
        assert_eq!(
            outbox
                .try_flush_one(&client, &enrollment, "runner_fixture")
                .await
                .unwrap(),
            RuntimeFlushOutcome::Accepted {
                run_id: "run_fixture".to_string(),
                cursor: 1,
            }
        );
        assert!(outbox.events.is_empty());
        let bodies = responder.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
    }

    #[test]
    fn snapshot_envelope_is_unicode_safe_and_keeps_newest_history() {
        let messages = (0..80)
            .map(|index| {
                let marker = format!("message-{index:02}-");
                text_message(
                    if index % 2 == 0 { "user" } else { "assistant" },
                    marker + &"🫧\"\\\n".repeat(1_500),
                )
            })
            .collect::<Vec<_>>();

        let envelope = bounded_session_snapshot_envelope(1, &messages);
        let encoded = serde_json::to_vec(&envelope).unwrap();
        let retained = envelope["payload"]["messages"].as_array().unwrap();

        assert!(encoded.len() <= SNAPSHOT_ENVELOPE_BYTE_BUDGET);
        assert!(encoded.len() < MAX_RUNTIME_ENVELOPE_BYTES);
        assert!(!retained.is_empty());
        assert!(retained.len() <= MAX_SNAPSHOT_MESSAGES);
        assert!(
            retained.last().unwrap()["text"]
                .as_str()
                .unwrap()
                .starts_with("message-79-")
        );
        for message in retained {
            let text = message["text"].as_str().unwrap();
            assert!(!text.contains('\u{FFFD}'));
            assert!(text.is_char_boundary(text.len()));
        }
    }

    #[test]
    fn snapshot_truncation_pins_the_exact_encoded_byte_boundary() {
        let source = "🫧\"\\\n".repeat(40_000);
        let message = text_message("assistant", source);
        let envelope = bounded_session_snapshot_envelope(9, std::slice::from_ref(&message));
        let encoded = serde_json::to_vec(&envelope).unwrap();
        assert!(encoded.len() <= SNAPSHOT_ENVELOPE_BYTE_BUDGET);

        let retained = envelope["payload"]["messages"][0]["text"].as_str().unwrap();
        let projected = project_session_message(&message).unwrap().1;
        let retained_chars = retained.chars().count();
        let next = projected.chars().nth(retained_chars).unwrap();
        let mut one_more = retained.to_string();
        one_more.push(next);
        let timestamp = envelope["timestamp"].as_str().unwrap();
        let expanded = vec![json!({ "role": "assistant", "text": one_more })];
        assert!(
            snapshot_envelope_len(9, timestamp, &expanded) > SNAPSHOT_ENVELOPE_BYTE_BUDGET,
            "one more Unicode scalar must cross the chosen encoded boundary"
        );
    }

    #[test]
    fn fatal_engine_error_projects_failure_and_releases_the_remote_run() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.activate_prompt("run_fixture", "turn_fixture");
        let secret = "sk-runtime-secret-that-must-not-cross-the-relay";
        let message = format!(
            "DeepSeek API key: {secret}\n{}",
            "🫧".repeat(MAX_REMOTE_ERROR_MESSAGE_BYTES)
        );

        controller.observe_engine_event(&EngineEvent::Error {
            envelope: crate::error_taxonomy::ErrorEnvelope::new(
                crate::error_taxonomy::ErrorCategory::Authentication,
                crate::error_taxonomy::ErrorSeverity::Critical,
                false,
                "llm_auth_error",
                message,
            ),
            recoverable: false,
        });

        assert!(!controller.has_active_run());
        let WorkerCommand::Upload {
            envelopes: failed, ..
        } = worker_rx.try_recv().expect("fatal item upload")
        else {
            panic!("fatal error must upload an item.failed envelope");
        };
        let WorkerCommand::Upload {
            envelopes: completed,
            ..
        } = worker_rx.try_recv().expect("fatal turn upload")
        else {
            panic!("fatal error must upload a terminal turn envelope");
        };
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["seq"], 1);
        assert_eq!(failed[0]["event"], "item.failed");
        assert_eq!(failed[0]["turn_id"], "turn_fixture");
        assert_eq!(failed[0]["payload"]["item"]["kind"], "error");
        assert_eq!(failed[0]["payload"]["item"]["status"], "failed");
        let projected = failed[0]["payload"]["item"]["detail"]
            .as_str()
            .expect("bounded error detail");
        assert!(projected.len() <= MAX_REMOTE_ERROR_MESSAGE_BYTES);
        assert!(projected.is_char_boundary(projected.len()));
        assert!(!projected.contains(secret));
        assert!(projected.contains("[redacted]"));

        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["seq"], 2);
        assert_eq!(completed[0]["event"], "turn.completed");
        assert_eq!(completed[0]["turn_id"], "turn_fixture");
        assert_eq!(completed[0]["payload"]["turn"]["status"], "failed");
        assert_eq!(
            controller.pending_runtime_events["run_fixture"]
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn recoverable_engine_error_stays_nonterminal_for_provider_fallback() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.activate_prompt("run_fixture", "turn_fixture");

        controller.observe_engine_event(&EngineEvent::Error {
            envelope: crate::error_taxonomy::ErrorEnvelope::network(
                "temporary provider connection failure",
            ),
            recoverable: true,
        });

        assert!(controller.active_run_matches("run_fixture"));
        assert!(controller.pending_runtime_events.is_empty());
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn terminal_pre_dispatch_error_uses_the_same_failure_projection() {
        let mut controller = RemoteControlController::default();
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.activate_prompt("run_fixture", "turn_fixture");

        controller.fail_active_dispatch(
            "DeepSeek API key: sk-preflight-secret-that-must-not-cross-the-relay",
        );

        assert!(!controller.has_active_run());
        let WorkerCommand::Upload { envelopes, .. } =
            worker_rx.try_recv().expect("pre-dispatch item upload")
        else {
            panic!("pre-dispatch error must upload item.failed");
        };
        assert_eq!(envelopes[0]["event"], "item.failed");
        assert!(!envelopes[0].to_string().contains("sk-preflight-secret"));
        let WorkerCommand::Upload { envelopes, .. } =
            worker_rx.try_recv().expect("pre-dispatch turn upload")
        else {
            panic!("pre-dispatch error must upload turn.completed");
        };
        assert_eq!(envelopes[0]["event"], "turn.completed");
        assert_eq!(envelopes[0]["payload"]["turn"]["status"], "failed");
        assert!(worker_rx.try_recv().is_err());
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

    #[tokio::test]
    async fn pre_lease_failure_never_locks_and_allows_immediate_retry() {
        let (mut controller, _worker_rx, event_tx) = wired_controller();
        controller.status = Status::Connecting;
        event_tx
            .send(RemoteEvent::FailedPreLease(
                "Codewhale rejected remote-control enrollment (403): client version not accepted."
                    .to_string(),
            ))
            .unwrap();
        assert!(matches!(
            controller.try_next_event(),
            Some(RemoteEvent::FailedPreLease(_))
        ));
        assert_eq!(controller.status, Status::Failed);
        assert!(
            controller.ownership_blocked_until.is_none(),
            "a rejection before any lease must never start the reconnect blackout"
        );
        let line = controller.status_line();
        assert!(line.contains("failed before connecting"), "{line}");
        assert!(line.contains("/rc to retry"), "{line}");
        assert!(
            line.contains("403"),
            "the sanitized HTTP status must surface: {line}"
        );
        assert!(line.contains("client version not accepted"), "{line}");

        // Stopping after a pre-lease failure is an ordinary reset (no lease
        // to drain, no blackout to honor).
        controller.stop();
        assert_eq!(controller.status, Status::Off);

        // Immediate retry is allowed — no lease drain wait.
        let result = controller.start(RemoteStart {
            workspace_label: "fixture".to_string(),
            target_ref: "target_fixture".to_string(),
            session_id: "session_fixture".to_string(),
            runtime_version: "0.9.1".to_string(),
            runtime_commit: "a".repeat(40),
            journal_dir: None,
            git_remote: None,
        });
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn sanitized_rejection_excerpt_reads_only_bounded_error_fields() {
        assert_eq!(
            sanitized_rejection_excerpt(
                br#"{"error":"client version not accepted","details":"noise"}"#
            )
            .as_deref(),
            Some("client version not accepted")
        );
        assert_eq!(
            sanitized_rejection_excerpt(br#"{"message":"enrollment closed"}"#).as_deref(),
            Some("enrollment closed")
        );
        // Control characters are stripped, never echoed. A JSON-escaped NUL
        // parses into the value; the strip must remove it. A raw NUL byte
        // makes serde_json reject the body outright, which is also safe
        // (no excerpt) — assert both directions.
        let with_control = "{\"error\":\"bad\\u0000opaque\"}".to_string();
        assert_eq!(
            sanitized_rejection_excerpt(with_control.as_bytes()).as_deref(),
            Some("badopaque")
        );
        let raw_nul = "{\"error\":\"bad\u{0}opaque\"}".to_string();
        assert_eq!(sanitized_rejection_excerpt(raw_nul.as_bytes()), None);
        // Non-JSON bodies yield no excerpt.
        assert_eq!(sanitized_rejection_excerpt(b"<html>403</html>"), None);
        // Overlong reasons are capped.
        let long = format!("{{\"error\":\"{}\"}}", "x".repeat(400));
        let excerpt = sanitized_rejection_excerpt(long.as_bytes()).expect("capped excerpt");
        assert!(excerpt.chars().count() <= 140);
    }

    #[test]
    fn view_gate_matching_never_confuses_two_approval_cards() {
        use crate::tui::approval::ApprovalRequest;
        use crate::tui::approval::ApprovalView;
        use crate::tui::views::ViewStack;

        let request = ApprovalRequest::new(
            "tool_A",
            "edit",
            "Edit A",
            &serde_json::json!({ "file": "a" }),
            "approval_key_A",
        );
        let card = ApprovalView::new(request);
        let gate_a = projected_approval_id("tool_A");
        let gate_b = projected_approval_id("tool_B");

        let mut stack = ViewStack::new();
        stack.push(card);
        assert!(
            stack.top_matches_approval_gate(&gate_a),
            "the matching gate must match"
        );
        assert!(
            !stack.top_matches_approval_gate(&gate_b),
            "a different gate must NEVER match this card — the whole point of identity-aware dismissal"
        );
        assert!(!stack.top_matches_approval_gate("local_approval_missing"));
    }

    #[tokio::test]
    async fn enrollment_rejection_carries_a_sanitized_actionable_reason() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/device"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": "client version not accepted",
                "documentation": "https://example.test/docs"
            })))
            .mount(&server)
            .await;
        let client = Client::builder()
            .https_only(false)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .expect("test client");
        let error = public_request(
            &client,
            Method::POST,
            Url::parse(&format!(
                "{}/oauth/device",
                server.uri().trim_end_matches('/')
            ))
            .expect("server url"),
            json!({ "audience": "codewhale-runner" }),
        )
        .await
        .expect_err("403 must fail");
        assert!(error.contains("403"), "{error}");
        assert!(error.contains("client version not accepted"), "{error}");
        assert!(
            !error.contains("documentation"),
            "only the conventional error fields may surface: {error}"
        );
    }

    #[tokio::test]
    async fn failed_relay_keeps_reconnect_blocked_until_lease_expiry() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Failed;
        controller.ownership_blocked_until = Some(Instant::now() + Duration::from_secs(90));
        // Mirror semantics: local input is never locked, but reconnecting
        // while the server lease may still be live is refused — the web must
        // not see two runners for the same session.
        let start = RemoteStart {
            workspace_label: "fixture".to_string(),
            target_ref: "target_fixture".to_string(),
            session_id: "session_fixture".to_string(),
            runtime_version: "0.9.1".to_string(),
            runtime_commit: "a".repeat(40),
            journal_dir: None,
            git_remote: None,
        };
        assert!(controller.start(start.clone()).is_err());
        // The web cannot answer shared approvals while the relay is failed.
        assert!(!controller.can_share_approval_with_web());
        controller.ownership_blocked_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(controller.start(start).is_ok());
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
            },
        );

        controller.stop();
        assert_eq!(controller.status, Status::Failed);
        assert_eq!(controller.pending_approvals.len(), 1);

        let event = controller.try_next_event();
        assert!(matches!(
            event,
            Some(RemoteEvent::OwnershipRestored { approvals })
                if approvals.len() == 1 && approvals[0].tool_id == "tool_fixture"
        ));
        assert_eq!(controller.status, Status::Off);
        assert!(controller.pending_approvals.is_empty());
    }

    #[test]
    fn cancelling_a_connect_keeps_reconnect_blocked_until_lease_drain() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Connecting;
        controller.stop();

        assert_eq!(controller.status, Status::Failed);
        // Mirror semantics: nothing about a cancelled connect locks local
        // input; reconnect stays blocked until the possible lease drains.
        let result = controller.start(RemoteStart {
            workspace_label: "fixture".to_string(),
            target_ref: "target_fixture".to_string(),
            session_id: "session_fixture".to_string(),
            runtime_version: "0.9.1".to_string(),
            runtime_commit: "a".repeat(40),
            journal_dir: None,
            git_remote: None,
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("previous remote lease"));
    }

    #[test]
    fn failed_worker_retains_snapshot_marker_and_exact_unacked_event() {
        let mut controller = RemoteControlController::default();
        controller.status = Status::Connected;
        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.upload_snapshot("run-1", &[]);
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("snapshot queued");
        };
        let exact = envelopes[0].clone();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Failed("fixture disconnect".to_string()))
            .unwrap();

        assert!(matches!(
            controller.try_next_event(),
            Some(RemoteEvent::Failed(_))
        ));
        assert!(controller.uploaded_snapshots.contains("run-1"));
        assert_eq!(
            controller.pending_runtime_events["run-1"]
                .values()
                .next()
                .map(|entry| &entry.envelope),
            Some(&exact)
        );
        // Fail-closed: the web can no longer answer shared approvals, and
        // reconnecting waits out the possible server lease.
        assert!(!controller.can_share_approval_with_web());
        assert!(controller.ownership_blocked_until.is_some());
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

    fn wired_controller() -> (
        RemoteControlController,
        mpsc::UnboundedReceiver<WorkerCommand>,
        mpsc::UnboundedSender<RemoteEvent>,
    ) {
        let mut controller = RemoteControlController::default();
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        (controller, worker_rx, event_tx)
    }

    fn turn_complete_event() -> EngineEvent {
        EngineEvent::TurnComplete {
            usage: crate::models::Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        }
    }

    #[test]
    fn stop_refusal_holds_until_terminal_event_is_acknowledged() {
        let (mut controller, mut worker_rx, event_tx) = wired_controller();
        controller.activate_prompt("run_fixture", "turn_fixture");
        let refusal = controller.stop_refusal().expect("active turn blocks stop");
        assert!(refusal.contains("active remote turn"), "{refusal}");

        controller.observe_engine_event(&turn_complete_event());
        assert!(
            !controller.has_active_run(),
            "the terminal event releases the run binding"
        );
        let refusal = controller
            .stop_refusal()
            .expect("a queued but unacknowledged terminal event must still block stop");
        assert!(refusal.contains("acknowledged"), "{refusal}");
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("the terminal envelope must be handed to the transport");
        };
        assert_eq!(envelopes[0]["event"], "turn.completed");
        let seq = envelopes[0]["seq"].as_u64().expect("terminal seq");

        event_tx
            .send(RemoteEvent::RuntimeCursor {
                run_id: "run_fixture".to_string(),
                cursor: seq,
            })
            .unwrap();
        controller.try_next_event().unwrap();
        assert_eq!(
            controller.stop_refusal(),
            None,
            "a server-acknowledged terminal event unblocks stop"
        );
    }

    #[test]
    fn failed_stop_stays_fail_closed_with_no_dual_ownership() {
        let (mut controller, _worker_rx, event_tx) = wired_controller();
        controller.status = Status::Connected;
        controller.stop();
        assert_eq!(controller.status, Status::Stopping);
        assert!(
            !controller.can_share_approval_with_web(),
            "stopping must close the shared-decision channel before confirmation"
        );

        // The worker could not confirm the drain or the offline heartbeat.
        event_tx
            .send(RemoteEvent::Failed(
                "the offline heartbeat could not be delivered".to_string(),
            ))
            .unwrap();
        let event = controller.try_next_event().unwrap();
        assert!(matches!(event, RemoteEvent::Failed(_)));
        assert!(
            !controller.can_share_approval_with_web(),
            "an unconfirmed stop must stay fail-closed through the lease expiry"
        );
        assert!(controller.ownership_blocked_until.is_some());
        assert!(
            controller.status_line().contains("lost after connecting"),
            "{}",
            controller.status_line()
        );
        assert!(
            controller.try_next_event().is_none(),
            "ownership must not be restored while the lease could still be live"
        );
    }

    #[tokio::test]
    async fn stop_drain_flushes_runtime_outbox_with_byte_identical_retries() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        let responder = AmbiguousRuntimeResponder::default();
        Mock::given(method("POST"))
            .and(path(
                "/api/local-runners/runner_fixture/runs/run_fixture/events",
            ))
            .respond_with(responder.clone())
            .expect(2)
            .mount(&server)
            .await;
        let mut enrollment = fixture_enrollment(&format!("{}/", server.uri()));
        let mut runner_id = "runner_fixture".to_string();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("fixture client");
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let mut outbox = RuntimeTransportOutbox::default();
        outbox
            .enqueue(
                "run_fixture",
                runtime_envelope(
                    1,
                    "turn.completed",
                    Some("turn_fixture"),
                    "2026-08-08T12:00:00Z".to_string(),
                    json!({ "turn": { "status": "completed", "usage": {} } }),
                ),
            )
            .expect("queue terminal envelope");

        drain_runtime_outbox_for_stop(
            &client,
            &mut enrollment,
            &mut runner_id,
            &fixture_start(),
            &event_tx,
            &mut outbox,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("the drain must complete before stop is confirmed");

        assert!(outbox.events.is_empty(), "the outbox must drain fully");
        let RemoteEvent::RuntimeCursor { run_id, cursor } = event_rx
            .try_recv()
            .expect("cursor event for journal compaction")
        else {
            panic!("drain must surface the server cursor");
        };
        assert_eq!(run_id, "run_fixture");
        assert_eq!(cursor, 1);
        let bodies = responder.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2, "ambiguous response must be retried");
        assert_eq!(bodies[0], bodies[1], "retries must be byte-identical");
    }

    #[tokio::test]
    async fn stop_drain_deadline_failure_refuses_to_confirm_stop() {
        crate::tls::ensure_rustls_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/api/local-runners/runner_fixture/runs/run_fixture/events",
            ))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let mut enrollment = fixture_enrollment(&format!("{}/", server.uri()));
        let mut runner_id = "runner_fixture".to_string();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("fixture client");
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let mut outbox = RuntimeTransportOutbox::default();
        outbox
            .enqueue(
                "run_fixture",
                runtime_envelope(
                    1,
                    "turn.completed",
                    Some("turn_fixture"),
                    "2026-08-08T12:00:00Z".to_string(),
                    json!({ "turn": { "status": "completed", "usage": {} } }),
                ),
            )
            .expect("queue terminal envelope");

        let error = drain_runtime_outbox_for_stop(
            &client,
            &mut enrollment,
            &mut runner_id,
            &fixture_start(),
            &event_tx,
            &mut outbox,
            Instant::now() + Duration::from_millis(700),
        )
        .await
        .expect_err("an undrained outbox must fail the stop");
        assert!(error.contains("not confirmed"), "{error}");
        assert!(
            !outbox.events.is_empty(),
            "the exact unacknowledged envelope must be retained for the reconnect resend"
        );
    }

    #[test]
    fn journal_roundtrip_restores_unacknowledged_envelopes_byte_identically() {
        let dir = tempfile::tempdir().expect("journal tempdir");
        let journal =
            RuntimeEventJournal::open(dir.path(), "session:fixture@01").expect("journal setup");
        assert!(journal.load().expect("missing file is empty").is_empty());

        let delta = runtime_envelope(
            1,
            "item.delta",
            Some("turn_fixture"),
            "2026-08-08T12:00:00Z".to_string(),
            json!({ "kind": "agent_message", "delta": "exact 🫧 body" }),
        );
        let terminal = runtime_envelope(
            2,
            "turn.completed",
            Some("turn_fixture"),
            "2026-08-08T12:00:01Z".to_string(),
            json!({ "turn": { "status": "completed", "usage": {} } }),
        );
        let mut pending: HashMap<String, BTreeMap<u64, PendingRuntimeEnvelope>> = HashMap::new();
        let mut events = BTreeMap::new();
        for envelope in [delta.clone(), terminal.clone()] {
            let seq = runtime_envelope_seq(&envelope).unwrap();
            let encoded_len = serde_json::to_vec(&envelope).unwrap().len();
            let integrity = runtime_envelope_event(&envelope).is_some_and(integrity_critical_event);
            events.insert(
                seq,
                PendingRuntimeEnvelope {
                    envelope,
                    encoded_len,
                    integrity,
                    handed_off: true,
                },
            );
        }
        pending.insert("run_fixture".to_string(), events);
        journal.persist(&pending).expect("atomic persist");

        let reopened =
            RuntimeEventJournal::open(dir.path(), "session:fixture@01").expect("journal reopen");
        let restored = reopened.load().expect("verified load");
        let events = restored.get("run_fixture").expect("restored run");
        assert_eq!(events.len(), 2);
        assert_eq!(
            serde_json::to_vec(&events[&1]).unwrap(),
            serde_json::to_vec(&delta).unwrap(),
            "a restored envelope must re-serialize byte-identically for ambiguous retries"
        );
        assert_eq!(
            serde_json::to_vec(&events[&2]).unwrap(),
            serde_json::to_vec(&terminal).unwrap()
        );

        // Compaction: an empty pending set removes the file entirely.
        pending.get_mut("run_fixture").unwrap().clear();
        journal.persist(&pending).expect("compacting persist");
        assert!(!journal.path.exists(), "acknowledged journals are deleted");
    }

    #[cfg(unix)]
    #[test]
    fn journal_directory_and_file_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().expect("journal tempdir");
        let dir = base.path().join("journal");
        let journal = RuntimeEventJournal::open(&dir, "session:fixture@01").expect("journal setup");
        let mut pending: HashMap<String, BTreeMap<u64, PendingRuntimeEnvelope>> = HashMap::new();
        let envelope = runtime_envelope(
            1,
            "turn.completed",
            None,
            "2026-08-08T12:00:00Z".to_string(),
            json!({ "turn": { "status": "completed", "usage": {} } }),
        );
        let encoded_len = serde_json::to_vec(&envelope).unwrap().len();
        pending.insert(
            "run_fixture".to_string(),
            BTreeMap::from([(
                1,
                PendingRuntimeEnvelope {
                    envelope,
                    encoded_len,
                    integrity: true,
                    handed_off: true,
                },
            )]),
        );
        journal.persist(&pending).expect("atomic persist");
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "journal directory must be private");
        let file_mode = std::fs::metadata(&journal.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "journal file must be owner-only");
    }

    #[test]
    fn corrupt_journal_fails_closed_and_start_quarantines_it() {
        let dir = tempfile::tempdir().expect("journal tempdir");
        let probe =
            RuntimeEventJournal::open(dir.path(), "session:fixture@01").expect("journal setup");
        std::fs::write(&probe.path, b"{ not json").expect("plant corrupt journal");

        let mut controller = RemoteControlController::default();
        let error = controller
            .start(RemoteStart {
                journal_dir: Some(dir.path().to_path_buf()),
                ..fixture_start()
            })
            .expect_err("a corrupt journal must fail closed");
        assert_eq!(error, JOURNAL_UNTRUSTED_ERROR);
        assert_eq!(controller.status, Status::Off, "no relay may start");
        assert!(
            !probe.path.exists(),
            "the untrusted journal must not stay in place"
        );
        assert!(
            probe.path.with_extension("corrupt").exists(),
            "the untrusted journal is quarantined, not silently discarded"
        );

        // A mismatched session tag is equally untrusted.
        let other =
            RuntimeEventJournal::open(dir.path(), "session:fixture@02").expect("journal setup");
        std::fs::write(
            &other.path,
            serde_json::to_vec(&json!({
                "schemaVersion": JOURNAL_SCHEMA_VERSION,
                "session": "00000000000000000000000000000000",
                "runs": {},
            }))
            .unwrap(),
        )
        .expect("plant mismatched journal");
        assert_eq!(other.load().unwrap_err(), JOURNAL_UNTRUSTED_ERROR);
    }

    #[test]
    fn start_recovers_journaled_envelopes_and_resends_on_connect() {
        let dir = tempfile::tempdir().expect("journal tempdir");
        {
            let journal =
                RuntimeEventJournal::open(dir.path(), "session:fixture@01").expect("journal setup");
            let envelope = runtime_envelope(
                3,
                "turn.completed",
                Some("turn_fixture"),
                "2026-08-08T12:00:00Z".to_string(),
                json!({ "turn": { "status": "completed", "usage": {} } }),
            );
            let encoded_len = serde_json::to_vec(&envelope).unwrap().len();
            let pending = HashMap::from([(
                "run_fixture".to_string(),
                BTreeMap::from([(
                    3,
                    PendingRuntimeEnvelope {
                        envelope,
                        encoded_len,
                        integrity: true,
                        handed_off: true,
                    },
                )]),
            )]);
            journal
                .persist(&pending)
                .expect("previous process persisted");
        }

        let mut controller = RemoteControlController::default();
        let journal =
            RuntimeEventJournal::open(dir.path(), "session:fixture@01").expect("journal setup");
        controller.reset_pending_from(journal.load().expect("clean recovery"));
        controller.journal = Some(journal);
        assert!(
            controller.has_unacknowledged_integrity_events(),
            "recovered terminal state must gate /rc stop until acknowledged"
        );
        assert!(controller.stop_refusal().is_some());

        let (worker_tx, mut worker_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        controller.worker_tx = Some(worker_tx);
        controller.event_rx = Some(event_rx);
        event_tx
            .send(RemoteEvent::Connected {
                account_ref: "account_fixture".to_string(),
                runner_id: "runner_fixture".to_string(),
                target_ref: "target_fixture".to_string(),
                attachment: RemoteAttachment {
                    run_id: "run_other".to_string(),
                    workspace_id: "workspace_fixture".to_string(),
                    runtime_cursor: 0,
                    snapshot_present: false,
                },
                links: RemoteLinks::default(),
            })
            .unwrap();
        controller.try_next_event().unwrap();
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("recovered envelopes must resend on connect");
        };
        assert_eq!(envelopes[0]["seq"], 3);
        assert_eq!(envelopes[0]["event"], "turn.completed");
    }

    #[test]
    fn delta_pressure_sheds_to_resync_and_preserves_integrity_capacity() {
        let (mut controller, _worker_rx, _event_tx) = wired_controller();
        let delta_budget = MAX_JOURNAL_EVENTS - JOURNAL_RESERVED_INTEGRITY_EVENTS;
        for index in 0..delta_budget {
            assert!(
                controller.queue_runtime_envelope(
                    "run_fixture",
                    runtime_envelope(
                        (index + 1) as u64,
                        "item.delta",
                        Some("turn_fixture"),
                        "2026-08-08T12:00:00Z".to_string(),
                        json!({ "kind": "agent_message", "delta": index.to_string() }),
                    ),
                ),
                "delta {index} fits the unreserved budget"
            );
        }
        assert_eq!(controller.pending_event_count, delta_budget);

        let shed_seq = (delta_budget + 1) as u64;
        assert!(
            !controller.queue_runtime_envelope(
                "run_fixture",
                runtime_envelope(
                    shed_seq,
                    "item.delta",
                    Some("turn_fixture"),
                    "2026-08-08T12:00:00Z".to_string(),
                    json!({ "kind": "agent_message", "delta": "over budget" }),
                ),
            ),
            "a delta beyond the unreserved budget is shed"
        );
        assert_eq!(controller.pending_event_count, delta_budget);
        assert!(controller.resync_required.contains("run_fixture"));
        assert_ne!(
            controller.status,
            Status::Failed,
            "delta pressure is ordinary and must not fail the relay"
        );

        // Reserved capacity keeps the terminal boundary deliverable, and the
        // terminal boundary schedules the resynchronization snapshot.
        controller.activate_prompt("run_fixture", "turn_fixture");
        controller.observe_engine_event(&turn_complete_event());
        assert!(
            controller.has_unacknowledged_integrity_events(),
            "the terminal envelope must use the reserved capacity"
        );
        assert_eq!(
            controller.take_pending_resync().as_deref(),
            Some("run_fixture"),
            "the shed run resynchronizes at its terminal boundary"
        );
        controller.upload_resync_snapshot("run_fixture", &[]);
        assert!(
            controller
                .pending_runtime_events
                .get("run_fixture")
                .is_some_and(|events| events.values().any(|entry| runtime_envelope_event(
                    &entry.envelope
                ) == Some("session.snapshot"))),
            "the bounded snapshot restores account truth"
        );
    }

    #[test]
    fn integrity_overflow_fails_closed_without_restoring_input() {
        let (mut controller, _worker_rx, _event_tx) = wired_controller();
        controller.status = Status::Connected;
        for index in 0..MAX_JOURNAL_EVENTS {
            assert!(controller.queue_runtime_envelope(
                "run_fixture",
                runtime_envelope(
                    (index + 1) as u64,
                    "item.failed",
                    Some("turn_fixture"),
                    "2026-08-08T12:00:00Z".to_string(),
                    json!({ "item": { "id": index.to_string(), "kind": "error" } }),
                ),
            ));
        }
        assert!(!controller.queue_runtime_envelope(
            "run_fixture",
            runtime_envelope(
                (MAX_JOURNAL_EVENTS + 1) as u64,
                "turn.completed",
                Some("turn_fixture"),
                "2026-08-08T12:00:00Z".to_string(),
                json!({ "turn": { "status": "failed", "usage": {} } }),
            ),
        ));
        assert_eq!(
            controller.status,
            Status::Failed,
            "losing integrity state can never be silent"
        );
        assert!(
            !controller.can_share_approval_with_web(),
            "a failed-closed relay keeps the shared-decision channel closed"
        );
    }

    #[test]
    fn message_deltas_coalesce_until_a_handoff_boundary() {
        let (mut controller, mut worker_rx, _event_tx) = wired_controller();
        controller.activate_prompt("run_fixture", "turn_fixture");
        controller.observe_engine_event(&EngineEvent::MessageDelta {
            index: 0,
            content: "Hello ".to_string(),
        });
        controller.observe_engine_event(&EngineEvent::MessageDelta {
            index: 0,
            content: "world".to_string(),
        });
        assert!(
            worker_rx.try_recv().is_err(),
            "deferred deltas coalesce before any transport handoff"
        );
        let events = controller
            .pending_runtime_events
            .get("run_fixture")
            .unwrap();
        assert_eq!(events.len(), 1, "both deltas share one envelope");
        assert_eq!(
            events.values().next().unwrap().envelope["payload"]["delta"],
            "Hello world"
        );

        controller.observe_engine_event(&EngineEvent::ToolCallStarted {
            id: "tool_fixture".to_string(),
            name: "shell".to_string(),
            input: json!({}),
        });
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("the coalesced delta must hand off before a later event");
        };
        assert_eq!(envelopes[0]["event"], "item.delta");
        assert_eq!(envelopes[0]["payload"]["delta"], "Hello world");
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("the tool event follows the coalesced delta");
        };
        assert_eq!(envelopes[0]["event"], "item.started");

        // Once handed off an envelope is immutable; new deltas open a fresh
        // envelope that flushes on the next UI poll.
        controller.observe_engine_event(&EngineEvent::MessageDelta {
            index: 0,
            content: "again".to_string(),
        });
        assert!(worker_rx.try_recv().is_err());
        assert!(controller.try_next_event().is_none());
        let WorkerCommand::Upload { envelopes, .. } = worker_rx.try_recv().unwrap() else {
            panic!("the UI poll hands the deferred delta to the transport");
        };
        assert_eq!(envelopes[0]["payload"]["delta"], "again");
    }
}
