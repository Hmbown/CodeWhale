//! Lifecycle event outbox: a local JSONL log of session/turn/subagent
//! lifecycle events plus an optional webhook fan-out.
//!
//! This is the machine-readable sibling of the TUI shell-hook system. Hooks
//! fire shell commands per event and are TUI-only; the outbox appends one
//! JSON line per event to a config-gated file and needs no per-event
//! configuration. It is additive and opt-in: with no path configured,
//! [`LifecycleOutbox::emit`] is a no-op.
//!
//! # Line schema
//!
//! Every line is a `codewhale_protocol::runtime::RuntimeEventEnvelope`:
//!
//! ```json
//! {"schema_version": 1, "seq": 3, "event": "turn_start", "kind": "turn.started",
//!  "thread_id": "…", "turn_id": "…", "item_id": null, "timestamp": "…",
//!  "created_at": "…", "payload": {…}}
//! ```
//!
//! - `seq` is unique and monotonic per outbox file. Every append takes an
//!   exclusive advisory lock on a `<path>.lock` sidecar file (`flock` on
//!   Unix, `LockFileEx` on Windows) and holds it across the recovery of the
//!   last complete line's `seq` (bounded tail scan, so an outbox that grows
//!   unbounded is never re-read in full) and the append itself — concurrent
//!   writers sharing one file (many codewhale sessions on one machine)
//!   cannot duplicate seqs or append them out of order.
//! - `event` is the snake-case lifecycle name (`turn_start`, `turn_end`, …);
//!   `kind` is the dotted kind (`turn.started`, `turn.failed`, …).
//! - Payloads are constructed by the emit sites from bounded, pre-redacted
//!   fields only — never raw tool arguments, environment, or full transcript
//!   text. [`bounded_text`] enforces the same ceilings as the desktop
//!   notification payloads: headline ≤ 80, detail ≤ 120, preview ≤ 200
//!   characters.
//!
//! # Delivery model
//!
//! [`LifecycleOutbox::emit`] never blocks the caller: it enqueues the event
//! on an internal channel and a single writer task appends lines in order;
//! concurrent writers sharing one file are serialized by the outbox's
//! per-append exclusive lock. If no tokio runtime is available the event is
//! dropped with a warning.
//! Webhook POSTs (`{"at": …, "event": …}`) fan out after the local append,
//! off the append path: delivery uses bounded retries inside the sink (two
//! retries with exponential back-off) and failures are logged and dropped,
//! never fed back into the agent loop. The fan-out is itself bounded — at
//! most [`WEBHOOK_MAX_IN_FLIGHT`] deliveries run concurrently, and a full
//! backlog drops the newest delivery instead of queueing it.
//!
//! # Session ownership of turn boundaries
//!
//! A session owns its turn events: the process that starts a turn is the
//! one that ends it — except when it cannot. A session killed mid-turn
//! (SIGKILL, a closed pane, a crashed host) dies between the `turn_start`
//! and `turn_end` appends, leaving an unpaired `turn_start` in the file.
//!
//! - **Graceful shutdown** closes the loop for catchable signals: the TUI's
//!   terminating-signal task calls [`LifecycleOutbox::reconcile_interrupted_turns`]
//!   before exiting, which appends a synthetic `turn_end`
//!   (`status: "interrupted"`, `payload.reconciled: true`, a `reason` naming
//!   the signal) for every turn this session left open.
//! - **SIGKILL runs no code**, so nothing can be appended at death; that is
//!   what **boot reconciliation** covers: on the next session start the
//!   session calls [`LifecycleOutbox::reconcile_interrupted_turns`] again
//!   (reason `boot_reconciliation`) and the same scan pairs anything the
//!   killed process left behind. The session owns its events across process
//!   lifetimes.
//!
//! Both paths derive the open turn from the *file* (a `turn_start` with no
//! matching `turn_end` for the same `thread_id`), never from in-memory
//! state, so a signal handler that races the writer task cannot fabricate a
//! duplicate `turn_end`. The scan and the appends run under the outbox's
//! exclusive lock, so two reconcilers (or a reconciler racing another
//! session's writer) serialize and never double-append.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use codewhale_protocol::runtime::{RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION, RuntimeEventEnvelope};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::WebhookHookSink;

/// Text-length ceilings for outbox payload fields. Mirrors the desktop
/// notification payload limits so the outbox never carries more than the
/// lock-screen-capable surface already does.
pub const OUTBOX_HEADLINE_MAX_CHARS: usize = 80;
pub const OUTBOX_DETAIL_MAX_CHARS: usize = 120;
pub const OUTBOX_PREVIEW_MAX_CHARS: usize = 200;

/// Suffix appended when [`bounded_text`] truncates a field.
pub const OUTBOX_TRUNCATION_MARKER: &str = "…";

/// How far back from EOF the seq-recovery scan reads. Outbox lines are
/// bounded (payload ceilings above plus envelope overhead), so a line can
/// never approach this window and the last complete line is always inside it.
const SEQ_RECOVERY_TAIL_BYTES: u64 = 64 * 1024;

/// Upper bound on concurrently in-flight webhook fan-out tasks. Each
/// delivery runs bounded retries inside the sink (≈ 30 s worst case against
/// a dead endpoint), so the fan-out is capped: when all slots are busy the
/// newest delivery is logged and dropped instead of queued unbounded. The
/// local append path is never blocked either way.
const WEBHOOK_MAX_IN_FLIGHT: usize = 4;

/// How long [`LifecycleOutbox::reconcile_interrupted_turns`] waits for this
/// process's own queued events to drain before it scans the file. Only the
/// signal path can observe a non-empty queue (a `turn_start` still on the
/// channel); boot reconciliation runs before the first emit and returns
/// immediately. The wait is bounded so a wedged writer can never trap the
/// terminating-signal handler.
const RECONCILE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Poll interval for the reconcile drain wait above.
const RECONCILE_DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// One lifecycle event destined for the outbox.
///
/// Construct one per emit site. `payload` must only contain bounded,
/// pre-redacted fields; apply [`bounded_text`] to anything free-form (error
/// messages, previews) before inserting it.
#[derive(Debug, Clone)]
pub struct LifecycleEvent {
    /// Snake-case event name, e.g. `"turn_start"`.
    pub event: String,
    /// Dotted event kind, e.g. `"turn.started"` or `"turn.failed"`.
    pub kind: String,
    /// Owning session/thread id. Empty when the producer has none.
    pub thread_id: String,
    /// Current turn id, when known.
    pub turn_id: Option<String>,
    /// Current item id, when known.
    pub item_id: Option<String>,
    /// Bounded, redacted event payload.
    pub payload: Value,
}

/// The lifecycle outbox handle.
///
/// Cheap to clone (an `Arc`). When constructed without a path the outbox is
/// disabled and every `emit` is a no-op.
#[derive(Clone)]
pub struct LifecycleOutbox {
    inner: Option<Arc<OutboxInner>>,
}

impl Default for LifecycleOutbox {
    fn default() -> Self {
        Self::disabled()
    }
}

impl LifecycleOutbox {
    /// Create an outbox writing to `path` when set and non-empty.
    ///
    /// `webhook_url` optionally adds a webhook fan-out (POST `{"at", "event"}`,
    /// best-effort); `webhook_token` is its optional bearer token. Webhook
    /// delivery is configured independently of the file: it only ever runs
    /// when `webhook_url` is set, and it never replaces the local append.
    /// Deliveries run on detached, bounded tasks after the append, so a slow
    /// or dead endpoint cannot stall the append of queued events.
    pub fn new(
        path: Option<PathBuf>,
        webhook_url: Option<String>,
        webhook_token: Option<String>,
    ) -> Self {
        let path = match path {
            Some(path) if !path.as_os_str().is_empty() => path,
            _ => return Self::disabled(),
        };
        let webhook = webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| WebhookHookSink::new_with_token(url.to_string(), webhook_token));
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            inner: Some(Arc::new(OutboxInner {
                path,
                webhook,
                sender,
                receiver: Mutex::new(Some(receiver)),
                writer_spawned: AtomicBool::new(false),
                spawn_lock: Mutex::new(()),
                webhook_slots: Arc::new(Semaphore::new(WEBHOOK_MAX_IN_FLIGHT)),
                pending: Arc::new(AtomicUsize::new(0)),
            })),
        }
    }

    /// A disabled outbox that drops every event.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// True when a path was configured and events will be written.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Emit one lifecycle event.
    ///
    /// Never blocks: the event is queued for the outbox's writer task (spawned
    /// lazily on the current tokio runtime on first use). Events queued with
    /// no runtime available — or after the writer task is gone — are dropped
    /// with a warning. Delivery failures inside the writer are logged and
    /// dropped as well; the outbox is observability, not control flow.
    pub fn emit(&self, event: LifecycleEvent) {
        let Some(inner) = self.inner.clone() else {
            return;
        };
        if let Err(error) = inner.enqueue(event) {
            tracing::warn!(target: "lifecycle_outbox", %error, "lifecycle event dropped");
        }
    }

    /// Emit one lifecycle event synchronously, on the calling thread.
    ///
    /// Bypasses the async queue: the seq is recovered and the line appended
    /// under the outbox's cross-process exclusive lock before this returns.
    /// No tokio runtime is required. This is the append primitive for paths
    /// that must land a line before the process dies (terminating-signal
    /// handlers) and for deterministic offline writers (fixture generators,
    /// tests); ordinary emit sites keep using the non-blocking [`emit`].
    ///
    /// Returns the written envelope. A disabled outbox appends nothing and
    /// returns an error.
    pub fn emit_blocking(&self, event: LifecycleEvent) -> Result<RuntimeEventEnvelope> {
        let Some(inner) = self.inner.clone() else {
            anyhow::bail!("lifecycle outbox is disabled");
        };
        let (envelope, append_result) = append_event_under_lock(&inner.path, event)?;
        append_result?;
        Ok(envelope)
    }

    /// Reconcile interrupted turns owned by `thread_id`: scan the outbox for
    /// this thread's `turn_start` lines that lack a matching `turn_end` and
    /// append one synthetic `turn_end` for each (`status: "interrupted"`,
    /// `payload.reconciled: true`, `payload.reason: reason`). Returns the
    /// number of synthetic ends appended (0 when the file is healthy or the
    /// outbox is disabled).
    ///
    /// Runs synchronously: first it waits (bounded) for this process's own
    /// queued events to drain so a still-queued `turn_start` is visible to
    /// the scan, then it scans the whole file and appends under the outbox's
    /// exclusive lock. Holding the lock across scan and appends makes the
    /// reconciliation idempotent across concurrent sessions: a second
    /// reconciler sees the first one's synthetic ends and appends nothing.
    ///
    /// Call it once at session start (reason `"boot_reconciliation"` — the
    /// SIGKILL/closed-pane recovery path, since a killed process cannot
    /// append its own ends) and again from a terminating-signal handler
    /// (reason `"signal:SIGTERM"` etc. — the graceful-shutdown path).
    /// A disabled outbox is a no-op returning 0.
    pub fn reconcile_interrupted_turns(&self, thread_id: &str, reason: &str) -> Result<usize> {
        let Some(inner) = self.inner.clone() else {
            return Ok(0);
        };
        inner.reconcile_interrupted_turns(thread_id, reason)
    }
}

struct OutboxInner {
    path: PathBuf,
    webhook: Option<WebhookHookSink>,
    sender: UnboundedSender<LifecycleEvent>,
    /// The writer task's receive half. Taken exactly once by the writer task.
    receiver: Mutex<Option<UnboundedReceiver<LifecycleEvent>>>,
    writer_spawned: AtomicBool,
    /// Serializes the lazy writer-task spawn so two racing first emits cannot
    /// start two writers.
    spawn_lock: Mutex<()>,
    /// Concurrency cap for webhook fan-out tasks (see [`WEBHOOK_MAX_IN_FLIGHT`]).
    webhook_slots: Arc<Semaphore>,
    /// Events enqueued on the channel and not yet appended (bumped on
    /// enqueue, decremented after each append attempt). Lets the synchronous
    /// paths (blocking emit, reconciliation) wait a bounded time for this
    /// process's own queue to drain before they touch the file.
    pending: Arc<AtomicUsize>,
}

impl OutboxInner {
    /// Queue an event and make sure the writer task exists to drain it.
    ///
    /// Ordering: `send` happens before the spawn so events queued before the
    /// writer starts are drained first, preserving enqueue order.
    fn enqueue(self: &Arc<Self>, event: LifecycleEvent) -> Result<()> {
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.sender.send(event).is_err() {
            // The writer task is gone; nothing will ever drain this event.
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(anyhow::anyhow!("lifecycle outbox writer task is gone"));
        }
        self.ensure_writer_spawned();
        Ok(())
    }

    fn ensure_writer_spawned(self: &Arc<Self>) {
        if self.writer_spawned.load(Ordering::Acquire) {
            return;
        }
        let _guard = self
            .spawn_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.writer_spawned.load(Ordering::Acquire) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                target: "lifecycle_outbox",
                "no tokio runtime available; lifecycle events are queued but will not be written"
            );
            return;
        };
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(receiver) = receiver else {
            return;
        };
        let mut state = WriterState {
            path: self.path.clone(),
            webhook: self.webhook.clone(),
            webhook_slots: self.webhook_slots.clone(),
            pending: self.pending.clone(),
            receiver,
        };
        self.writer_spawned.store(true, Ordering::Release);
        handle.spawn(async move {
            state.run().await;
        });
    }

    /// See [`LifecycleOutbox::reconcile_interrupted_turns`].
    fn reconcile_interrupted_turns(&self, thread_id: &str, reason: &str) -> Result<usize> {
        self.wait_for_pending_drain();
        if thread_id.is_empty() {
            return Ok(0);
        }
        reconcile_interrupted_turns_under_lock(&self.path, thread_id, reason)
    }

    /// Wait (bounded) for this process's own queued events to be appended so
    /// a synchronous file scan sees them. Only the signal path can observe a
    /// non-empty queue; on a single-worker runtime the writer task cannot
    /// progress while this busy-waits, so the bound keeps the exit reachable.
    fn wait_for_pending_drain(&self) {
        let deadline = std::time::Instant::now() + RECONCILE_DRAIN_TIMEOUT;
        while self.pending.load(Ordering::Acquire) > 0 {
            if std::time::Instant::now() >= deadline {
                tracing::warn!(
                    target: "lifecycle_outbox",
                    pending = self.pending.load(Ordering::Acquire),
                    "lifecycle outbox queue did not drain in time; reconciling against the file as-is"
                );
                return;
            }
            std::thread::sleep(RECONCILE_DRAIN_POLL);
        }
    }
}

/// The outbox writer: owns the file path, the webhook sink, and the event
/// queue drain loop. Seq assignment is stateless: every append re-recovers
/// the last written seq under the outbox's cross-process lock.
struct WriterState {
    path: PathBuf,
    webhook: Option<WebhookHookSink>,
    /// Concurrency cap for webhook fan-out tasks (see [`WEBHOOK_MAX_IN_FLIGHT`]).
    webhook_slots: Arc<Semaphore>,
    /// Shared queue-depth counter; decremented after each append attempt so
    /// the synchronous paths can observe a drained queue.
    pending: Arc<AtomicUsize>,
    receiver: UnboundedReceiver<LifecycleEvent>,
}

impl WriterState {
    /// Drain the queue until every sender is dropped, then exit.
    async fn run(&mut self) {
        while let Some(event) = self.receiver.recv().await {
            if let Err(error) = self.deliver(event).await {
                tracing::warn!(
                    target: "lifecycle_outbox",
                    %error,
                    path = %self.path.display(),
                    "lifecycle outbox write failed"
                );
            }
            // The append attempt is complete either way; the synchronous
            // paths' drain wait observes the queue shrinking here.
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Assign a seq, build the envelope, and append it to the outbox file —
    /// all under the outbox's cross-process exclusive lock, on the blocking
    /// pool (acquiring the lock blocks on contention) — then hand the
    /// webhook POST to a detached fan-out task, independently of the append
    /// result. The drain loop never awaits webhook delivery, so a slow or
    /// dead endpoint cannot delay the local append of queued events.
    async fn deliver(&mut self, event: LifecycleEvent) -> Result<()> {
        let path = self.path.clone();
        let (envelope, append_result) =
            tokio::task::spawn_blocking(move || append_event_under_lock(&path, event))
                .await
                .map_err(|error| {
                    anyhow::anyhow!("lifecycle outbox writer task panicked: {error}")
                })??;

        if let Some(webhook) = &self.webhook {
            let payload = json!({
                "at": envelope.timestamp,
                "event": envelope,
            });
            self.fan_out_webhook(webhook.clone(), payload);
        }

        append_result
    }

    /// Hand the webhook POST to a detached task so the drain loop stays
    /// append-only. Bounded: at most [`WEBHOOK_MAX_IN_FLIGHT`] deliveries run
    /// concurrently (a full backlog drops the newest delivery with a warning
    /// rather than queueing unbounded). The task holds its slot until the
    /// POST and its bounded retries finish, logging its own failure.
    fn fan_out_webhook(&self, webhook: WebhookHookSink, payload: Value) {
        let permit = match self.webhook_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(
                    target: "lifecycle_outbox",
                    "webhook fan-out backlog full; delivery dropped"
                );
                return;
            }
        };
        tokio::spawn(async move {
            // Held until the POST (including its bounded retries) finishes.
            let _permit = permit;
            if let Err(error) = webhook.post_payload(payload).await {
                tracing::warn!(
                    target: "lifecycle_outbox",
                    %error,
                    "lifecycle webhook delivery failed (dropped)"
                );
            }
        });
    }
}

/// Serialize one event under the outbox's cross-process lock: recover the
/// next `seq` from the file tail, build the envelope, and append the line —
/// all while holding an exclusive lock, so a concurrent writer in another
/// process (or another outbox instance) cannot assign the same `seq` or
/// interleave the recovery read with the append. The envelope is returned
/// even when the append fails: the webhook fan-out needs it.
fn append_event_under_lock(
    path: &Path,
    event: LifecycleEvent,
) -> Result<(RuntimeEventEnvelope, Result<()>)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create outbox directory {}", parent.display()))?;
    }
    // The guard holds the lock from here through the append below; dropping
    // it releases the lock.
    let _lock = OutboxFileLock::acquire(path)?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("failed to open outbox {}", path.display()))?;
    let envelope = build_envelope(&mut file, path, event)?;
    let append_result = write_envelope_line(&mut file, path, &envelope);
    Ok((envelope, append_result))
}

/// Recover the next `seq` and build the envelope for `event`. The caller
/// must hold the outbox's exclusive lock.
fn build_envelope(
    file: &mut std::fs::File,
    path: &Path,
    event: LifecycleEvent,
) -> Result<RuntimeEventEnvelope> {
    let seq = recover_last_seq(file, path)?;
    Ok(RuntimeEventEnvelope {
        schema_version: RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION,
        seq,
        event: event.event,
        kind: event.kind,
        thread_id: event.thread_id,
        turn_id: event.turn_id,
        item_id: event.item_id,
        timestamp: Utc::now().to_rfc3339(),
        created_at: Some(Utc::now().to_rfc3339()),
        payload: event.payload,
        extra: Default::default(),
    })
}

/// Serialize `envelope` and append it as one line. Line + newline in a
/// single `write_all`: with O_APPEND each `write` lands contiguously at EOF,
/// and the caller holds the outbox's exclusive lock, so no other writer can
/// interleave a line between the seq recovery and this append.
fn write_envelope_line(
    file: &mut std::fs::File,
    path: &Path,
    envelope: &RuntimeEventEnvelope,
) -> Result<()> {
    let line = serde_json::to_string(envelope).context("failed to encode outbox event")?;
    let mut record = Vec::with_capacity(line.len() + 1);
    record.extend_from_slice(line.as_bytes());
    record.push(b'\n');
    file.write_all(&record)
        .and_then(|()| file.flush())
        .with_context(|| format!("failed to write outbox {}", path.display()))
}

/// Boot/shutdown reconciliation core: under the outbox's exclusive lock,
/// scan the file for `thread_id`'s `turn_start` lines lacking a matching
/// `turn_end` and append one synthetic `turn_end` for each.
///
/// The scan and the appends share one lock acquisition: a second reconciler
/// (or a reconciler racing another session's writer) is serialized behind
/// this one, then sees the synthetic ends this one wrote and appends nothing.
/// That is what makes the reconciliation idempotent — the session owns its
/// events, and ownership is settled by file truth, not by in-memory state.
///
/// Only the canonical SIGKILL shape is repaired: exactly one `turn_start`
/// and no `turn_end` for a turn. A turn with duplicate starts can never
/// satisfy a 1:1 pairing consumer no matter what is appended, so it is left
/// alone with a warning.
fn reconcile_interrupted_turns_under_lock(
    path: &Path,
    thread_id: &str,
    reason: &str,
) -> Result<usize> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create outbox directory {}", parent.display()))?;
    }
    let _lock = OutboxFileLock::acquire(path)?;

    // The lock excludes every cooperating writer, so this snapshot is stable
    // for the whole scan + append sequence below.
    let mut starts: std::collections::HashMap<String, usize> = Default::default();
    let mut ends: std::collections::HashMap<String, usize> = Default::default();
    let mut workspaces: std::collections::HashMap<String, Value> = Default::default();
    match std::fs::read_to_string(path) {
        Ok(text) => {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(envelope) = serde_json::from_str::<RuntimeEventEnvelope>(line) else {
                    // A torn trailing line from a crash mid-append. It cannot
                    // pair with anything; skip it.
                    tracing::debug!(
                        target: "lifecycle_outbox",
                        "skipping unparseable outbox line during reconciliation"
                    );
                    continue;
                };
                if envelope.thread_id != thread_id {
                    continue;
                }
                let Some(turn_id) = envelope.turn_id.as_ref() else {
                    continue;
                };
                match envelope.event.as_str() {
                    "turn_start" => {
                        *starts.entry(turn_id.clone()).or_default() += 1;
                        if let Some(workspace) = envelope.payload.get("workspace") {
                            workspaces
                                .entry(turn_id.clone())
                                .or_insert_with(|| workspace.clone());
                        }
                    }
                    "turn_end" => {
                        *ends.entry(turn_id.clone()).or_default() += 1;
                    }
                    _ => {}
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // No outbox yet: nothing to reconcile.
            return Ok(0);
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "failed to read outbox {} during reconciliation: {error}",
                path.display()
            ));
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("failed to open outbox {}", path.display()))?;

    let mut reconciled = 0usize;
    for (turn_id, starts) in starts {
        let ends = ends.get(&turn_id).copied().unwrap_or(0);
        if starts == 1 && ends == 0 {
            let envelope = build_envelope(
                &mut file,
                path,
                LifecycleEvent {
                    event: "turn_end".to_string(),
                    kind: "turn.interrupted".to_string(),
                    thread_id: thread_id.to_string(),
                    turn_id: Some(turn_id.clone()),
                    item_id: None,
                    payload: json!({
                        "status": "interrupted",
                        "reconciled": true,
                        "reason": reason,
                        "workspace": workspaces.get(&turn_id).cloned().unwrap_or(Value::Null),
                    }),
                },
            )?;
            write_envelope_line(&mut file, path, &envelope)?;
            reconciled += 1;
            tracing::info!(
                target: "lifecycle_outbox",
                %thread_id,
                %turn_id,
                seq = envelope.seq,
                %reason,
                "reconciled interrupted turn with a synthetic turn_end"
            );
        } else if starts > 1 {
            tracing::warn!(
                target: "lifecycle_outbox",
                %thread_id,
                %turn_id,
                starts,
                ends,
                "turn has duplicate turn_start lines; reconciliation cannot restore 1:1 pairing"
            );
        }
    }
    Ok(reconciled)
}

/// The outbox's cross-process exclusive lock: an advisory lock on a
/// `<outbox>.lock` sidecar file. Advisory means only cooperating outbox
/// writers respect it; downstream readers of the outbox file are unaffected.
struct OutboxFileLock {
    /// Kept open for the lock's lifetime; closing the descriptor releases it.
    _file: std::fs::File,
}

impl OutboxFileLock {
    fn acquire(outbox_path: &Path) -> Result<Self> {
        let lock_path = outbox_lock_path(outbox_path);
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&lock_path)
            .with_context(|| format!("failed to open outbox lock {}", lock_path.display()))?;
        lock_file_exclusive(&file)
            .with_context(|| format!("failed to lock outbox {}", lock_path.display()))?;
        Ok(Self { _file: file })
    }
}

/// `<outbox>.lock` — a sibling sidecar that holds no data. It is never
/// truncated or rotated with the outbox, so the lock survives outbox file
/// churn.
fn outbox_lock_path(outbox_path: &Path) -> PathBuf {
    let mut name = outbox_path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Block until this process holds an exclusive advisory lock on `file`.
/// Runs on the blocking pool; the critical section it protects is a bounded
/// tail read plus one line append, so contention is momentary.
#[cfg(unix)]
fn lock_file_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use rustix::fs::{FlockOperation, flock};
    use rustix::io::Errno;
    use std::os::unix::io::AsFd;

    loop {
        // flock(2) is per open-file-description: released when this fd is
        // closed, even if the process crashes, so no stale-lock recovery is
        // ever needed. It excludes other flock holders of the same file,
        // including other outbox instances in this same process. (rustix is
        // compiled without its `std` feature in this tree, so its errors are
        // `Errno` here.)
        match flock(file.as_fd(), FlockOperation::LockExclusive) {
            Ok(()) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(error) => {
                return Err(std::io::Error::from_raw_os_error(error.raw_os_error()));
            }
        }
    }
}

/// Block until this process holds an exclusive byte-range lock on `file`.
/// Runs on the blocking pool; the critical section it protects is a bounded
/// tail read plus one line append, so contention is momentary.
#[cfg(windows)]
fn lock_file_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx};

    loop {
        // A std file handle is synchronous (no FILE_FLAG_OVERLAPPED), so
        // LockFileEx waits for the range on contention; the explicit
        // ERROR_LOCK_VIOLATION retry below also covers a platform where it
        // returns instead. `u32::MAX` byte pairs lock the whole file
        // including future growth; the lock dies with the handle on close,
        // even if the process crashes.
        let mut overlapped =
            std::mem::MaybeUninit::<windows_sys::Win32::System::IO::OVERLAPPED>::zeroed();
        let result = unsafe {
            LockFileEx(
                file.as_raw_handle() as _,
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                u32::MAX,
                u32::MAX,
                overlapped.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_LOCK_VIOLATION as i32) {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Recover the seq to continue from: the `seq` of the outbox file's last
/// complete line, plus 1 — or 1 for an empty file.
///
/// Only the tail of the file is read (bounded by [`SEQ_RECOVERY_TAIL_BYTES`]);
/// outbox lines are bounded far below that window, so the last complete line
/// is always within it. A partial trailing line from a crash mid-write is
/// ignored (the previous newline-terminated line wins). The caller must hold
/// the outbox's exclusive lock, so the read cannot race another writer's
/// append.
fn recover_last_seq(file: &mut std::fs::File, path: &Path) -> Result<u64> {
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat outbox {}", path.display()))?
        .len();
    if len == 0 {
        return Ok(1);
    }
    let start = len.saturating_sub(SEQ_RECOVERY_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("failed to seek outbox {}", path.display()))?;
    let mut tail = vec![0u8; (len - start) as usize];
    file.read_exact(&mut tail)
        .with_context(|| format!("failed to read outbox {}", path.display()))?;

    let line = match tail.iter().rposition(|byte| *byte == b'\n') {
        // The bytes after the final newline are a torn trailing line from a
        // crash mid-write; drop them. What remains ends at a newline, so the
        // last complete line is the bytes after the previous newline.
        Some(last_nl) => {
            let body = &tail[..last_nl];
            match body.iter().rposition(|byte| *byte == b'\n') {
                Some(idx) => &body[idx + 1..],
                None => body,
            }
        }
        // No newline at all: no complete line inside this tail (a line can
        // only exceed the tail window by violating the bounded-line
        // invariant). Treat the file as not-yet-writable.
        None => return Ok(1),
    };
    let line = std::str::from_utf8(line).context("outbox tail is not UTF-8")?;
    if line.trim().is_empty() {
        return Ok(1);
    }
    let envelope: RuntimeEventEnvelope =
        serde_json::from_str(line).context("failed to parse last outbox line")?;
    Ok(envelope.seq.saturating_add(1))
}

/// Bound free-form text to at most `max_chars` characters, stripping control
/// characters and ANSI escape sequences and collapsing whitespace runs first.
///
/// The limit counts Unicode scalar values, not bytes, so multi-byte text gets
/// the same ceiling as ASCII. The result is safe to embed in an outbox
/// payload. Callers remain responsible for only ever passing non-secret
/// fields (error messages, previews, model/provider labels — never raw tool
/// arguments, environment, or full transcript text), the same discipline the
/// desktop notification payloads enforce.
pub fn bounded_text(text: &str, max_chars: usize) -> String {
    let cleaned: String = strip_ansi(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut truncated = false;
    let mut out = String::new();
    let mut char_count = 0usize;
    for ch in cleaned.chars() {
        if char_count + 1 > max_chars {
            truncated = true;
            break;
        }
        out.push(ch);
        char_count += 1;
    }
    if truncated {
        // Make room for the marker while staying under the character ceiling.
        let marker_chars = OUTBOX_TRUNCATION_MARKER.chars().count();
        while char_count + marker_chars > max_chars {
            out.pop();
            char_count -= 1;
        }
        out.push_str(OUTBOX_TRUNCATION_MARKER);
    }
    out
}

/// Remove ANSI escape sequences and remaining control characters from
/// `text`, leaving only the visible content.
///
/// Covers what terminal tooling actually emits into status lines: CSI
/// (`ESC [` through the final byte in `0x40..=0x7E`), OSC (`ESC ]` through
/// BEL or ST `ESC \`), DCS/SOS/PM/APC (`ESC P/X/^/_` through ST), and plain
/// two-character escapes (`ESC c`). Unterminated sequences are dropped
/// whole. [`bounded_text`] then applies whitespace collapse and truncation
/// on top.
fn strip_ansi(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::with_capacity(text.len());
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            if !ch.is_control() {
                out.push(ch);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                // CSI: parameters/intermediates until the final byte.
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: terminated by BEL or by ST (`ESC \`).
                while let Some(c) = chars.next() {
                    match c {
                        '\x07' => break,
                        '\x1b' if chars.next_if_eq(&'\\').is_some() => break,
                        _ => {}
                    }
                }
            }
            Some('P' | 'X' | '^' | '_') => {
                // DCS/SOS/PM/APC: terminated by ST (`ESC \`); drop whole if
                // unterminated.
                while let Some(c) = chars.next() {
                    if c == '\x1b' && chars.next_if_eq(&'\\').is_some() {
                        break;
                    }
                }
            }
            Some(_) => {
                // Two-character escape (e.g. `ESC c`): the body byte is
                // consumed above and dropped.
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_outbox_path(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        (dir, path)
    }

    fn event(name: &str, kind: &str) -> LifecycleEvent {
        LifecycleEvent {
            event: name.to_string(),
            kind: kind.to_string(),
            thread_id: "session-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            item_id: None,
            payload: json!({"status": "completed"}),
        }
    }

    fn writer_state(path: PathBuf, webhook: Option<WebhookHookSink>) -> WriterState {
        WriterState {
            path,
            webhook,
            webhook_slots: Arc::new(Semaphore::new(WEBHOOK_MAX_IN_FLIGHT)),
            pending: Arc::new(AtomicUsize::new(0)),
            receiver: tokio::sync::mpsc::unbounded_channel().1,
        }
    }

    async fn deliver_all(state: &mut WriterState, events: Vec<LifecycleEvent>) {
        for event in events {
            state.deliver(event).await.expect("deliver");
        }
    }

    async fn read_lines(path: &Path) -> Vec<Value> {
        let text = tokio::fs::read_to_string(path).await.expect("read outbox");
        text.lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
            .collect()
    }

    /// Synchronous [`read_lines`] for the runtime-free tests.
    fn read_lines_blocking(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).expect("read outbox");
        text.lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
            .collect()
    }

    /// Like [`read_lines`], but drops a torn trailing line instead of
    /// failing: the concurrent test polls mid-append, when the file can end
    /// in a half-written record.
    async fn read_lines_lenient(path: &Path) -> Vec<Value> {
        let text = tokio::fs::read_to_string(path).await.expect("read outbox");
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    /// Open `path` for recovery the way the writer does (creating it when
    /// missing) and return the seq recovery would continue from.
    fn recover_last_seq_from(path: &Path) -> u64 {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .expect("open outbox");
        recover_last_seq(&mut file, path).expect("recover")
    }

    #[tokio::test]
    async fn appends_one_jsonl_line_per_event_with_envelope_schema() {
        let (_dir, path) = temp_outbox_path("schema.jsonl");
        let mut state = writer_state(path.clone(), None);
        deliver_all(&mut state, vec![event("turn_start", "turn.started")]).await;

        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line["schema_version"], 1);
        assert_eq!(line["seq"], 1);
        assert_eq!(line["event"], "turn_start");
        assert_eq!(line["kind"], "turn.started");
        assert_eq!(line["thread_id"], "session-1");
        assert_eq!(line["turn_id"], "turn-1");
        assert_eq!(line["item_id"], Value::Null);
        assert!(line["timestamp"].as_str().is_some());
        assert!(line["payload"]["status"].as_str() == Some("completed"));
    }

    /// Every emit site now carries `payload.workspace` (and subagent events
    /// additionally `payload.subagent`) for consumer-side routing. The writer
    /// must preserve those fields verbatim through the envelope round trip
    /// for every event type.
    #[tokio::test]
    async fn payload_workspace_and_subagent_fields_survive_the_round_trip() {
        let (_dir, path) = temp_outbox_path("routing-fields.jsonl");
        let mut state = writer_state(path.clone(), None);
        let workspace = "/home/cw/wt-lane";
        let subagent = "explore-1";
        let subagent_payload = json!({ "workspace": workspace, "subagent": subagent });
        deliver_all(
            &mut state,
            vec![
                LifecycleEvent {
                    event: "session_start".to_string(),
                    kind: "session.started".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: None,
                    item_id: None,
                    payload: json!({ "workspace": workspace }),
                },
                LifecycleEvent {
                    event: "turn_start".to_string(),
                    kind: "turn.started".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    item_id: None,
                    payload: json!({ "workspace": workspace }),
                },
                LifecycleEvent {
                    event: "turn_end".to_string(),
                    kind: "turn.completed".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    item_id: None,
                    payload: json!({ "workspace": workspace }),
                },
                LifecycleEvent {
                    event: "turn_stalled".to_string(),
                    kind: "turn.stalled".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    item_id: None,
                    payload: json!({ "workspace": workspace }),
                },
                LifecycleEvent {
                    event: "subagent_spawn".to_string(),
                    kind: "subagent.spawned".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    item_id: None,
                    payload: subagent_payload.clone(),
                },
                LifecycleEvent {
                    event: "subagent_complete".to_string(),
                    kind: "subagent.completed".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    item_id: None,
                    payload: subagent_payload.clone(),
                },
                LifecycleEvent {
                    event: "session_end".to_string(),
                    kind: "session.ended".to_string(),
                    thread_id: "session-1".to_string(),
                    turn_id: None,
                    item_id: None,
                    payload: json!({ "workspace": workspace }),
                },
            ],
        )
        .await;

        let lines = read_lines(&path).await;
        let events: Vec<&str> = lines
            .iter()
            .map(|line| line["event"].as_str().expect("event"))
            .collect();
        assert_eq!(
            events,
            vec![
                "session_start",
                "turn_start",
                "turn_end",
                "turn_stalled",
                "subagent_spawn",
                "subagent_complete",
                "session_end",
            ],
            "the routing-field contract must cover every lifecycle event type"
        );
        for line in &lines {
            assert_eq!(
                line["payload"]["workspace"],
                json!(workspace),
                "workspace must survive the round trip for event {}",
                line["event"]
            );
        }
        for event in ["subagent_spawn", "subagent_complete"] {
            let line = lines
                .iter()
                .find(|line| line["event"] == event)
                .expect(event);
            assert_eq!(
                line["payload"]["subagent"],
                json!(subagent),
                "subagent must survive the round trip for event {event}"
            );
        }
    }

    #[tokio::test]
    async fn seq_is_monotonic_and_recovers_across_reopen() {
        let (_dir, path) = temp_outbox_path("seq.jsonl");
        let mut state = writer_state(path.clone(), None);
        deliver_all(
            &mut state,
            vec![
                event("session_start", "session.started"),
                event("turn_start", "turn.started"),
                event("turn_end", "turn.completed"),
            ],
        )
        .await;

        // A fresh writer (new process, same file) continues the sequence.
        let mut reopened = writer_state(path.clone(), None);
        deliver_all(&mut reopened, vec![event("turn_start", "turn.started")]).await;

        let lines = read_lines(&path).await;
        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| line["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
    }

    #[test]
    fn missing_and_empty_files_start_at_seq_1() {
        let (_dir, path) = temp_outbox_path("empty.jsonl");
        assert_eq!(recover_last_seq_from(&path), 1);

        std::fs::write(&path, "").expect("empty file");
        assert_eq!(recover_last_seq_from(&path), 1);
    }

    #[test]
    fn partial_trailing_line_is_ignored_during_recovery() {
        let (_dir, path) = temp_outbox_path("partial.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{{\"schema_version\":1,\"seq\":3,\"event\":\"turn_",
                r#"{"schema_version":1,"seq":1,"event":"session_start","kind":"session.started","thread_id":"s","turn_id":null,"item_id":null,"timestamp":"t","payload":{}}"#,
                r#"{"schema_version":1,"seq":2,"event":"turn_start","kind":"turn.started","thread_id":"s","turn_id":null,"item_id":null,"timestamp":"t","payload":{}}"#,
            ),
        )
        .expect("write partial outbox");
        // The torn trailing line is not a complete record; recovery continues
        // from the last complete line's seq (2) → next seq 3.
        let mut file = std::fs::File::open(&path).expect("open outbox");
        assert_eq!(recover_last_seq(&mut file, &path).expect("recover"), 3);
    }

    #[tokio::test]
    async fn emit_queues_and_writes_in_order_without_blocking() {
        let (_dir, path) = temp_outbox_path("emit.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
        assert!(outbox.is_enabled());

        outbox.emit(event("session_start", "session.started"));
        outbox.emit(event("turn_start", "turn.started"));
        outbox.emit(event("turn_end", "turn.completed"));

        // The writer task drains asynchronously; wait for the lines to land.
        for _ in 0..100 {
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|meta| meta.len() > 0)
                && read_lines(&path).await.len() >= 3
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 3, "expected all queued events to be written");
        let events: Vec<&str> = lines
            .iter()
            .map(|line| line["event"].as_str().expect("event"))
            .collect();
        assert_eq!(events, vec!["session_start", "turn_start", "turn_end"]);
        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| line["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3], "seq must be assigned in emit order");
    }

    /// Multiple outbox instances (as concurrent codewhale sessions on one
    /// machine do) sharing one file must produce unique, increasing seqs with
    /// no lost lines: the exclusive sidecar lock makes tail-recovery +
    /// append atomic, where the live bug produced duplicate seqs (10×2, …)
    /// and out-of-order appends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writers_share_one_file_with_unique_increasing_seqs() {
        let (_dir, path) = temp_outbox_path("concurrent.jsonl");
        const WRITERS: usize = 4;
        const EVENTS_PER_WRITER: usize = 40;
        let total = WRITERS * EVENTS_PER_WRITER;

        let mut emitters = Vec::with_capacity(WRITERS);
        for writer in 0..WRITERS {
            let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
            emitters.push(tokio::spawn(async move {
                for i in 0..EVENTS_PER_WRITER {
                    outbox.emit(LifecycleEvent {
                        event: "turn_start".to_string(),
                        kind: "turn.started".to_string(),
                        thread_id: format!("session-{writer}"),
                        turn_id: Some(format!("turn-{writer}-{i}")),
                        item_id: None,
                        payload: json!({"writer": writer, "event_index": i}),
                    });
                }
                // Dropping the handle drops this writer's sender; its writer
                // task drains the remaining queue and exits.
                drop(outbox);
            }));
        }
        for emitter in emitters {
            emitter.await.expect("emitter task");
        }

        // Writer tasks drain asynchronously; wait for every line to land.
        let mut lines = Vec::new();
        for _ in 0..500 {
            let current = read_lines_lenient(&path).await;
            if current.len() >= total {
                lines = current;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(lines.len(), total, "no line may be lost or duplicated");
        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| line["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(
            seqs,
            (1..=total as u64).collect::<Vec<_>>(),
            "seqs must be unique and increasing in file order"
        );
    }

    /// A synchronous append lands with the correct seq and no runtime: the
    /// blocking primitive the terminating-signal path and offline fixture
    /// generators rely on.
    #[test]
    fn emit_blocking_appends_without_a_tokio_runtime() {
        let (_dir, path) = temp_outbox_path("blocking.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
        assert!(outbox.is_enabled());

        let envelope = outbox
            .emit_blocking(event("turn_start", "turn.started"))
            .expect("blocking emit");
        assert_eq!(envelope.seq, 1);
        let envelope = outbox
            .emit_blocking(event("turn_end", "turn.completed"))
            .expect("blocking emit");
        assert_eq!(envelope.seq, 2);

        let lines = read_lines_blocking(&path);
        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| line["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn disabled_outbox_reconciles_nothing() {
        let outbox = LifecycleOutbox::disabled();
        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            0
        );
    }

    #[test]
    fn reconcile_pairs_this_sessions_unpaired_start_and_touches_nothing_else() {
        let (_dir, path) = temp_outbox_path("reconcile.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);

        // Healthy pair for session-1 (must be left alone).
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: "session-1".to_string(),
                turn_id: Some("turn-done".to_string()),
                item_id: None,
                payload: json!({ "workspace": "/home/cw/session-1" }),
            })
            .expect("emit");
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_end".to_string(),
                kind: "turn.completed".to_string(),
                thread_id: "session-1".to_string(),
                turn_id: Some("turn-done".to_string()),
                item_id: None,
                payload: json!({ "status": "completed" }),
            })
            .expect("emit");

        // Unpaired start for session-1: the killed-mid-turn fixture.
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: "session-1".to_string(),
                turn_id: Some("turn-killed".to_string()),
                item_id: None,
                payload: json!({ "workspace": "/home/cw/session-1" }),
            })
            .expect("emit");

        // Unpaired start for a DIFFERENT session: not ours to own.
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: "session-2".to_string(),
                turn_id: Some("turn-foreign".to_string()),
                item_id: None,
                payload: json!({ "workspace": "/home/cw/session-2" }),
            })
            .expect("emit");

        let appended = outbox
            .reconcile_interrupted_turns("session-1", "boot_reconciliation")
            .expect("reconcile");
        assert_eq!(appended, 1, "exactly the session-1 unpaired start");

        let lines = read_lines_blocking(&path);
        // Original 4 lines + 1 synthetic end, nothing else.
        assert_eq!(lines.len(), 5);

        let synthetic = lines
            .iter()
            .find(|line| line["event"] == "turn_end" && line["turn_id"] == "turn-killed")
            .expect("synthetic turn_end");
        assert_eq!(synthetic["kind"], "turn.interrupted");
        assert_eq!(synthetic["thread_id"], "session-1");
        assert_eq!(synthetic["payload"]["status"], "interrupted");
        assert_eq!(synthetic["payload"]["reconciled"], true);
        assert_eq!(synthetic["payload"]["reason"], "boot_reconciliation");
        assert_eq!(
            synthetic["payload"]["workspace"],
            json!("/home/cw/session-1"),
            "the synthetic end inherits the start's routing workspace"
        );

        // The foreign unpaired start is untouched (its own session owns it).
        assert!(
            lines
                .iter()
                .all(|line| { !(line["event"] == "turn_end" && line["thread_id"] == "session-2") }),
            "session-2's start must stay unpaired here"
        );

        // Seq stays strictly monotonic in file order across the append.
        let seqs: Vec<u64> = lines
            .iter()
            .map(|line| line["seq"].as_u64().expect("seq"))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);

        // The synthetic end is the LAST line (appended after the scan).
        assert_eq!(lines.last().expect("last")["turn_id"], "turn-killed");
    }

    #[test]
    fn reconcile_is_idempotent_and_leaves_healthy_files_alone() {
        let (_dir, path) = temp_outbox_path("reconcile-idem.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);

        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: "session-1".to_string(),
                turn_id: Some("turn-killed".to_string()),
                item_id: None,
                payload: json!({ "workspace": "/home/cw/session-1" }),
            })
            .expect("emit");

        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            1
        );
        // Second boot (or a racing reconciler behind the lock) appends nothing.
        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            0
        );
        assert_eq!(read_lines_blocking(&path).len(), 2);

        // A fully healthy thread needs nothing either.
        let (_dir, healthy) = temp_outbox_path("reconcile-healthy.jsonl");
        let outbox = LifecycleOutbox::new(Some(healthy.clone()), None, None);
        outbox
            .emit_blocking(event("turn_start", "turn.started"))
            .expect("emit");
        outbox
            .emit_blocking(event("turn_end", "turn.completed"))
            .expect("emit");
        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            0
        );
        assert_eq!(read_lines_blocking(&healthy).len(), 2);
    }

    #[test]
    fn reconcile_appends_nothing_when_the_outbox_does_not_exist() {
        let (_dir, path) = temp_outbox_path("reconcile-missing.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            0
        );
        assert!(
            !path.exists(),
            "reconciliation must not create an empty outbox"
        );
    }

    /// Reconcile a file that ends in a torn trailing line (a crash
    /// mid-append): the torn line is skipped, the complete lines before it
    /// still reconcile.
    #[test]
    fn reconcile_skips_a_torn_trailing_line() {
        let (_dir, path) = temp_outbox_path("reconcile-torn.jsonl");
        let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
        outbox
            .emit_blocking(LifecycleEvent {
                event: "turn_start".to_string(),
                kind: "turn.started".to_string(),
                thread_id: "session-1".to_string(),
                turn_id: Some("turn-killed".to_string()),
                item_id: None,
                payload: json!({ "workspace": "/home/cw/session-1" }),
            })
            .expect("emit");
        // Simulate the crash mid-append.
        let mut bytes = std::fs::read(&path).expect("read");
        bytes.extend_from_slice(b"{\"schema_version\":1,\"seq\":2,\"event\":\"turn_");
        std::fs::write(&path, bytes).expect("write torn tail");

        assert_eq!(
            outbox
                .reconcile_interrupted_turns("session-1", "boot_reconciliation")
                .expect("reconcile"),
            1
        );
    }

    #[test]
    fn disabled_outbox_drops_events_and_reports_disabled() {
        let outbox = LifecycleOutbox::new(None, None, None);
        assert!(!outbox.is_enabled());
        outbox.emit(event("turn_start", "turn.started")); // must not panic

        let empty_path = LifecycleOutbox::new(Some(PathBuf::new()), None, None);
        assert!(!empty_path.is_enabled());

        let default = LifecycleOutbox::default();
        assert!(!default.is_enabled());
    }

    #[test]
    fn webhook_only_configures_without_a_file_path() {
        // `webhook_url` without `path` is stored losslessly in config; the
        // outbox handle itself only activates on a path.
        let outbox = LifecycleOutbox::new(
            None,
            Some("https://example.com/hook".to_string()),
            Some("token".to_string()),
        );
        assert!(!outbox.is_enabled());
    }

    #[test]
    fn bounded_text_truncates_to_limit_with_marker() {
        assert_eq!(bounded_text("short", 80), "short");
        let long = "x".repeat(200);
        let bounded = bounded_text(&long, OUTBOX_DETAIL_MAX_CHARS);
        assert_eq!(bounded.chars().count(), OUTBOX_DETAIL_MAX_CHARS);
        assert!(bounded.ends_with(OUTBOX_TRUNCATION_MARKER));
        assert!(bounded.starts_with('x'));
    }

    #[test]
    fn bounded_text_strips_controls_and_collapses_whitespace() {
        assert_eq!(
            bounded_text("line\x1b[31m one\n\n  two\t", 80),
            "line one two"
        );
        assert_eq!(bounded_text("", 80), "");
        assert_eq!(bounded_text("   \n\t  ", 80), "");
    }

    #[test]
    fn bounded_text_strips_full_ansi_escape_sequences() {
        // CSI with parameters and an SGR reset: only visible text survives.
        assert_eq!(bounded_text("\x1b[1;31mbold red\x1b[0m", 80), "bold red");
        // OSC 8 hyperlink: the target URL is dropped, the label survives.
        assert_eq!(
            bounded_text("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\", 80),
            "link"
        );
        // BEL-terminated OSC (window title).
        assert_eq!(bounded_text("\x1b]0;title\x07text", 80), "text");
        // Two-character escape (`ESC c`): the `c` is consumed, the `b` stays.
        assert_eq!(bounded_text("a\x1bcb", 80), "ab");
        // Unterminated CSI is dropped whole.
        assert_eq!(bounded_text("x\x1b[31", 80), "x");
        // A trailing lone ESC byte is dropped.
        assert_eq!(bounded_text("a\x1b", 80), "a");
        // A two-character escape drops its body byte (`ESC b` → nothing).
        assert_eq!(bounded_text("a\x1bb", 80), "a");
    }

    #[test]
    fn bounded_text_respects_utf8_boundaries() {
        // 30 multi-byte emoji (4 bytes each) = 120 bytes but only 30 chars.
        let emoji = "🦈".repeat(30);
        let bounded = bounded_text(&emoji, OUTBOX_DETAIL_MAX_CHARS);
        assert!(bounded.chars().count() <= OUTBOX_DETAIL_MAX_CHARS);
        assert!(bounded.starts_with('🦈'));
    }

    /// The webhook transport must POST `{"at", "event"}` JSON and, when a
    /// token is configured, send it as `Authorization: Bearer <token>`.
    #[tokio::test]
    async fn webhook_posts_at_event_payload_with_bearer_token() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/hook"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer secret-token",
            ))
            .and(wiremock::matchers::body_partial_json(json!({
                "event": {"kind": "turn.started"}
            })))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let webhook = WebhookHookSink::new_with_token(
            format!("{}/hook", server.uri()),
            Some("secret-token".to_string()),
        );
        webhook
            .post_payload(json!(
                {"at": "2026-08-19T00:00:00Z", "event": {"kind": "turn.started"}}
            ))
            .await
            .expect("webhook delivery");

        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 1, "exactly one webhook POST");
    }

    /// A webhook that always fails must surface its error to the caller
    /// (which logs and drops it) — never panic, never retry forever.
    #[tokio::test]
    async fn webhook_failure_is_an_error_not_a_panic() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let webhook = WebhookHookSink::new_with_token(format!("{}/hook", server.uri()), None);
        let result = webhook.post_payload(json!({})).await;
        assert!(result.is_err(), "expected the failure to be reported");
    }

    /// Regression: webhook delivery runs on a detached fan-out
    /// task, so a slow endpoint must never delay the local append of events
    /// queued behind it. Serialized delivery of three events against a 2 s
    /// endpoint would take ≈ 6 s; the local lines must land almost
    /// immediately, and the fan-out must still deliver all three POSTs.
    #[tokio::test]
    async fn slow_webhook_does_not_delay_local_appends() {
        let (_dir, path) = temp_outbox_path("webhook-nonblocking.jsonl");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)),
            )
            .mount(&server)
            .await;

        let outbox = LifecycleOutbox::new(
            Some(path.clone()),
            Some(format!("{}/hook", server.uri())),
            None,
        );
        let started = std::time::Instant::now();
        for _ in 0..3 {
            outbox.emit(event("turn_start", "turn.started"));
        }

        // The writer task drains asynchronously; wait for the lines to land.
        for _ in 0..200 {
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|meta| meta.len() > 0)
                && read_lines_lenient(&path).await.len() >= 3
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 3, "all queued events must be written locally");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1500),
            "local appends must not wait on webhook deliveries (took {:?})",
            started.elapsed()
        );

        // The fan-out tasks finish on their own; every POST must still land.
        for _ in 0..100 {
            if server.received_requests().await.expect("requests").len() >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let requests = server.received_requests().await.expect("requests");
        assert_eq!(requests.len(), 3, "every event must fan out to the webhook");
    }

    /// Regression: the fan-out is bounded — when all
    /// [`WEBHOOK_MAX_IN_FLIGHT`] slots are busy on a stalled endpoint, newer
    /// deliveries are dropped (logged) rather than queued unbounded, and the
    /// local append of every event still proceeds.
    #[tokio::test]
    async fn full_webhook_backlog_drops_deliveries_but_not_local_appends() {
        let (_dir, path) = temp_outbox_path("webhook-backlog.jsonl");
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)),
            )
            .mount(&server)
            .await;

        let outbox = LifecycleOutbox::new(
            Some(path.clone()),
            Some(format!("{}/hook", server.uri())),
            None,
        );
        let total = WEBHOOK_MAX_IN_FLIGHT + 2;
        for _ in 0..total {
            outbox.emit(event("turn_start", "turn.started"));
        }

        for _ in 0..200 {
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|meta| meta.len() > 0)
                && read_lines_lenient(&path).await.len() >= total
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let lines = read_lines(&path).await;
        assert_eq!(
            lines.len(),
            total,
            "every event must be appended locally even when the webhook backlog is full"
        );

        // The in-flight POSTs complete; the two backlogged deliveries are gone.
        for _ in 0..100 {
            if server.received_requests().await.expect("requests").len() >= WEBHOOK_MAX_IN_FLIGHT {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let requests = server.received_requests().await.expect("requests");
        assert_eq!(
            requests.len(),
            WEBHOOK_MAX_IN_FLIGHT,
            "only the in-flight slots may be delivered; the rest are dropped"
        );
    }
}
