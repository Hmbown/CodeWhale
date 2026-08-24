//! Per-session control socket — the supervised-operation control surface.
//!
//! This module is the codewhale side of "session control/communication API
//! for supervised operation" (#5533). When the
//! `[control_socket]` config table sets `enabled = true`, the interactive
//! TUI binds one unix domain socket per *running* session at
//!
//! ```text
//! <sessions-dir>/<session-id>/control.sock     (mode 0600)
//! ```
//!
//! where `<sessions-dir>` is the same directory the session store uses
//! (`SessionManager::sessions_dir`, typically `~/.codewhale/sessions`) and
//! `<session-id>` is the session the TUI currently owns. The socket lives
//! inside the per-session artifact directory, so `delete_session` and the
//! orphan-reclaim sweep remove it together with the rest of the session's
//! artifacts, and a crashed process leaves at most a stale socket file that
//! the next bind takes over (connect-probe + unlink, a known-good
//! socket-ownership pattern).
//!
//! # Transport
//!
//! Newline-framed JSON-RPC, one request per connection: connect, write one
//! request line, read one response line, close. Requests:
//!
//! ```json
//! {"id":"1","method":"message","params":{"text":"hello"}}
//! {"id":"2","method":"interrupt","params":{}}
//! {"id":"3","method":"relaunch","params":{}}
//! {"id":"4","method":"status","params":{}}
//! ```
//!
//! Success responses echo the id and carry a `type`-tagged result:
//!
//! ```json
//! {"id":"1","result":{"type":"message_sent","delivery":"dispatched"}}
//! {"id":"2","result":{"type":"interrupted","cancelled":true}}
//! {"id":"3","result":{"type":"relaunching"}}
//! {"id":"4","result":{"type":"status","turn_state":"idle","goal":{"objective":null,"status":"active","paused":false}}}
//! ```
//!
//! Failures are `{"id":…,"error":{"code":…,"message":…}}` with codes
//! `invalid_request`, `command_error`, `timeout`, and `server_unavailable`.
//!
//! # Verbs
//!
//! - `message` — delivers `text` as a structured user message through the
//!   ordinary composer dispatch path (`dispatch_composer_message`): dispatched
//!   immediately when the app is idle, queued when a turn is in flight
//!   (queued delivery is the default under load, matching the supervisor
//!   contract). The response's `delivery` field reports which happened.
//! - `interrupt` — the exact Esc-shaped "cancel the active turn" body
//!   (`escape_cancel_request`), shared with the Esc key path so the two
//!   cannot drift. `cancelled` reports whether active work was in flight.
//! - `relaunch` — routed through the slash-command path
//!   (`crate::commands::execute("/relaunch", app)`): **no relaunch logic
//!   lives here**. The `/relaunch` command is built on the
//!   `pr/relaunch-command` branch; this verb is the seam that calls the same
//!   command the user's `/relaunch` would. Until that command lands, the
//!   verb reports the command's own "unknown command" error verbatim, and
//!   once it lands the verb inherits its save-and-quit handoff with no
//!   changes here.
//! - `status` — answered by the socket thread directly from a snapshot the
//!   event loop republishes every iteration: `turn_state`
//!   (`idle | in_progress | waiting`) and `goal`
//!   (`objective`, `status`, `paused`).
//!
//! # Wiring (insertion points)
//!
//! 1. `run_event_loop` (crates/tui/src/tui/ui/event_loop.rs) constructs a
//!    [`SessionControl`] once and, at the top of the frame loop, calls
//!    [`SessionControl::reconcile`] (bind/rebind/unbind when the owned
//!    session id changes), [`SessionControl::update_status`] (publish the
//!    snapshot for `status`), and [`SessionControl::drain`] (execute queued
//!    verbs on the UI thread; a `true` return asks the loop to quit, which
//!    is how `relaunch` reuses the ordinary `/exit` teardown).
//! 2. The socket runs on background threads; verbs that touch UI state cross
//!    to the event loop over an mpsc channel and answer over a response
//!    channel with a 5 s timeout (a dispatch-to-app pattern).
//!
//! The feature is off unless `[control_socket] enabled = true`; an unset
//! table changes nothing. Unix-only: on non-unix platforms the config key
//! parses but binding is refused at runtime.

use std::io;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::thread;

use serde::{Deserialize, Serialize};

use crate::tui::app::{App, AppAction, ComposerSubmitAction, QueuedMessage, SubmitDisposition};
use crate::tui::streaming::StreamDisplayClock;
use crate::tui::ui::{DispatchRecovery, dispatch_composer_message, escape_cancel_request};

/// Socket file name inside the per-session artifact directory.
pub(crate) const SOCKET_FILE_NAME: &str = "control.sock";

/// Hard cap on one request line (initial-request bound).
#[cfg(unix)]
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Accept-loop poll interval while the listener is idle.
#[cfg(unix)]
const CONNECTION_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A client that connects and never sends is dropped after this long.
#[cfg(unix)]
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Response writes give up after this long rather than blocking forever.
#[cfg(unix)]
const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a verb may wait for the event loop to handle it
/// (`APP_RESPONSE_TIMEOUT`).
#[cfg(unix)]
const DISPATCH_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimum pause between bind retries after a refused takeover, so another
/// live process holding the socket cannot turn the per-frame reconcile into
/// a connect-probe and warn-log flood. Shortened under `#[cfg(test)]` so the
/// backoff itself is testable without sleeping for seconds.
#[cfg(not(test))]
const BIND_RETRY_BACKOFF: Duration = Duration::from_secs(5);
#[cfg(test)]
const BIND_RETRY_BACKOFF: Duration = Duration::from_millis(200);

// ── Protocol ────────────────────────────────────────────────────────────────

/// One request line: `{"id": … , "method": … , "params": …}`.
/// Windows builds construct this type only in the portable protocol tests;
/// the plain Windows lib build leaves it unreachable, so the lint allowance
/// below is scoped to exactly that case (unix builds use it via the socket
/// runtime, and CI denies dead code on the MSVC test gate).
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Deserialize)]
struct Request {
    id: String,
    #[serde(flatten)]
    method: Method,
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Method {
    Message(MessageParams),
    Interrupt(EmptyParams),
    Relaunch(EmptyParams),
    Status(EmptyParams),
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Deserialize)]
struct MessageParams {
    text: String,
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Deserialize)]
struct EmptyParams {}

/// A verb the socket thread hands to the event loop, plus the way back.
#[derive(Debug)]
pub(crate) struct PendingCommand {
    pub(crate) id: String,
    pub(crate) command: ControlCommand,
    pub(crate) respond_to: mpsc::Sender<String>,
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug)]
pub(crate) enum ControlCommand {
    Message { text: String },
    Interrupt,
    Relaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnState {
    Idle,
    InProgress,
    Waiting,
}

/// Goal state visible to supervisors over the `status` verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GoalSnapshot {
    pub(crate) objective: Option<String>,
    pub(crate) status: String,
    pub(crate) paused: bool,
}

/// The `status` answer, republished by the event loop every iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusSnapshot {
    pub(crate) turn_state: TurnState,
    pub(crate) goal: GoalSnapshot,
}

/// Success envelope (`SuccessResponse` shape).
#[derive(Debug, Serialize)]
struct SuccessResponse {
    id: String,
    result: ResponseResult,
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseResult {
    MessageSent {
        delivery: &'static str,
    },
    Interrupted {
        cancelled: bool,
    },
    Relaunching,
    Status {
        turn_state: TurnState,
        goal: GoalSnapshot,
    },
}

/// Error envelope (`ErrorResponse` shape).
#[derive(Debug, Serialize)]
struct ErrorResponse {
    id: String,
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

fn response_ok(id: String, result: ResponseResult) -> String {
    serde_json::to_string(&SuccessResponse { id, result }).unwrap_or_else(|_| {
        r#"{"id":"","error":{"code":"internal_error","message":"failed to encode response"}}"#
            .to_string()
    })
}

fn response_error(id: &str, code: &'static str, message: String) -> String {
    serde_json::to_string(&ErrorResponse {
        id: id.to_string(),
        error: ErrorBody { code, message },
    })
    .unwrap_or_else(|_| {
        r#"{"id":"","error":{"code":"internal_error","message":"failed to encode response"}}"#
            .to_string()
    })
}

// ── UI-side handle ──────────────────────────────────────────────────────────

/// The event-loop side of the control surface. Cheap to poll every frame:
/// reconcile/update/drain are all no-ops (or near no-ops) when disabled.
pub(crate) struct SessionControl {
    enabled: bool,
    sessions_dir: Option<PathBuf>,
    bound_session: Option<String>,
    socket: Option<ControlSocketHandle>,
    commands_tx: Option<mpsc::Sender<PendingCommand>>,
    commands_rx: mpsc::Receiver<PendingCommand>,
    status: Arc<Mutex<StatusSnapshot>>,
    /// When the last bind attempt failed (e.g. another live process owns the
    /// socket), retries for *that session* back off so a refused takeover
    /// cannot become a per-frame connect-probe and log flood.
    last_bind_failure: Option<(String, Instant)>,
}

impl SessionControl {
    pub(crate) fn new(enabled: bool) -> Self {
        Self::new_with_sessions_dir(enabled, None)
    }

    /// Test seam: `sessions_dir` bypasses `SessionManager::default_location()`
    /// so tests never touch the real `~/.codewhale/sessions`.
    fn new_with_sessions_dir(enabled: bool, sessions_dir: Option<PathBuf>) -> Self {
        let (commands_tx, commands_rx) = mpsc::channel();
        Self {
            enabled,
            sessions_dir,
            bound_session: None,
            socket: None,
            commands_tx: enabled.then_some(commands_tx),
            commands_rx,
            status: Arc::new(Mutex::new(StatusSnapshot {
                turn_state: TurnState::Idle,
                goal: GoalSnapshot {
                    objective: None,
                    status: "active".to_string(),
                    paused: false,
                },
            })),
            last_bind_failure: None,
        }
    }

    /// Bind/rebind the socket when the owned session id appears or changes,
    /// and unbind when it disappears (session switch or teardown). Runs on
    /// the event-loop thread but only spawns a thread on an actual change.
    pub(crate) fn reconcile(&mut self, current_session_id: Option<&str>) {
        if !self.enabled {
            return;
        }
        let Some(id) = current_session_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            // No session yet (fresh session before the first snapshot) or
            // the id went away: release whatever we hold.
            self.socket = None;
            self.bound_session = None;
            return;
        };
        if self.bound_session.as_deref() == Some(id) {
            return;
        }
        // A refused takeover must not retry every frame: back off so the
        // connect probe and its warning log run at most every few seconds.
        // Keyed on the session id so switching sessions is never delayed by
        // another session's refusal.
        if let Some((failed_id, failed_at)) = &self.last_bind_failure
            && failed_id == id
            && failed_at.elapsed() < BIND_RETRY_BACKOFF
        {
            return;
        }
        // Session id changed: drop the old listener first so the socket file
        // is unlinked before the new one binds.
        self.socket = None;
        self.bound_session = None;

        let sessions_dir = match self.sessions_dir.clone() {
            Some(dir) => dir,
            None => {
                let manager = match crate::session_manager::SessionManager::default_location() {
                    Ok(manager) => manager,
                    Err(error) => {
                        tracing::warn!(%error, "control socket: cannot resolve the sessions directory");
                        return;
                    }
                };
                let dir = manager.sessions_dir().to_path_buf();
                self.sessions_dir = Some(dir.clone());
                dir
            }
        };
        let Some(commands_tx) = self.commands_tx.clone() else {
            return;
        };
        match bind_control_socket(&sessions_dir, id, commands_tx, Arc::clone(&self.status)) {
            Ok(handle) => {
                tracing::info!(
                    session = id,
                    path = %sessions_dir.join(id).join(SOCKET_FILE_NAME).display(),
                    "control socket listening"
                );
                self.bound_session = Some(id.to_string());
                self.socket = Some(handle);
                self.last_bind_failure = None;
            }
            Err(error) => {
                tracing::warn!(session = id, %error, "control socket: bind failed; session control unavailable");
                self.last_bind_failure = Some((id.to_string(), Instant::now()));
            }
        }
    }

    /// Republish the `status` snapshot from the current app state. Runs every
    /// frame; the mutex write happens only when something actually changed.
    pub(crate) fn update_status(&self, app: &App) {
        if !self.enabled {
            return;
        }
        let snapshot = snapshot_from_app(app);
        let Ok(mut guard) = self.status.try_lock() else {
            return; // the socket thread is answering a `status` request; skip a frame
        };
        if *guard != snapshot {
            *guard = snapshot;
        }
    }

    /// Execute verbs queued by the socket thread on the UI thread and answer
    /// their clients. Returns `true` when a verb requested app quit (the
    /// `relaunch` seam) — the caller returns from the event loop and reuses
    /// the ordinary `/exit` teardown path.
    pub(crate) async fn drain(
        &mut self,
        app: &mut App,
        config: &crate::config::Config,
        engine_handle: &crate::core::engine::EngineHandle,
        current_streaming_text: &mut String,
        stream_display_clock: &mut StreamDisplayClock,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        let mut quit = false;
        while let Ok(pending) = self.commands_rx.try_recv() {
            let (do_quit, response) = execute_command(
                app,
                config,
                engine_handle,
                current_streaming_text,
                stream_display_clock,
                pending.id.clone(),
                pending.command,
            )
            .await;
            // The client may have disconnected while we worked; that must
            // never fail the loop.
            let _ = pending.respond_to.send(response);
            quit |= do_quit;
        }
        quit
    }
}

fn snapshot_from_app(app: &App) -> StatusSnapshot {
    let turn_state =
        if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
            TurnState::InProgress
        } else if app.goal_continuation_waiting {
            TurnState::Waiting
        } else {
            TurnState::Idle
        };
    // A paused goal parks its objective in `paused_goal_objective`, so the
    // snapshot surfaces the objective that is actually in flight.
    let objective = app
        .goal
        .objective
        .clone()
        .or_else(|| app.paused_goal_objective.clone());
    StatusSnapshot {
        turn_state,
        goal: GoalSnapshot {
            objective,
            status: app.goal.status.as_str().to_string(),
            paused: app.paused || app.paused_goal_objective.is_some(),
        },
    }
}

/// Execute one verb on the UI thread. Returns `(quit, response_json)`.
async fn execute_command(
    app: &mut App,
    config: &crate::config::Config,
    engine_handle: &crate::core::engine::EngineHandle,
    current_streaming_text: &mut String,
    stream_display_clock: &mut StreamDisplayClock,
    id: String,
    command: ControlCommand,
) -> (bool, String) {
    match command {
        ControlCommand::Message { text } => {
            if text.trim().is_empty() {
                return (
                    false,
                    response_error(
                        &id,
                        "invalid_request",
                        "message text must not be empty".to_string(),
                    ),
                );
            }
            // Queued delivery is the default under load: while a turn is in
            // flight the message waits like any queued follow-up; an idle
            // app dispatches immediately.
            let busy =
                app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"));
            let disposition = if busy {
                SubmitDisposition::Queue
            } else {
                SubmitDisposition::Immediate
            };
            let message = QueuedMessage::new(text, None);
            // Delivery failures surface through the app's own status/toast
            // and recovery paths; the verb still answers with what it asked
            // for (dispatched vs queued).
            let _ = dispatch_composer_message(
                app,
                config,
                engine_handle,
                message,
                DispatchRecovery::Immediate,
                ComposerSubmitAction::Submit(disposition),
            )
            .await;
            app.needs_redraw = true;
            let delivery = if busy { "queued" } else { "dispatched" };
            (
                false,
                response_ok(id, ResponseResult::MessageSent { delivery }),
            )
        }
        ControlCommand::Interrupt => {
            let had_active_work = app.is_loading
                || app.is_compacting
                || app.manual_compaction_queued
                || app.goal_continuation_waiting
                || app.paused
                || app.paused_goal_objective.is_some()
                || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"));
            if !had_active_work {
                // Nothing Esc-cancel would cancel: quiet no-op, like an Esc
                // on an idle app that has nothing else to act on.
                return (
                    false,
                    response_ok(id, ResponseResult::Interrupted { cancelled: false }),
                );
            }
            let _ = escape_cancel_request(
                app,
                engine_handle,
                current_streaming_text,
                stream_display_clock,
            );
            app.needs_redraw = true;
            (
                false,
                response_ok(id, ResponseResult::Interrupted { cancelled: true }),
            )
        }
        ControlCommand::Relaunch => {
            // Seam: the exact same command path `/relaunch` uses. When the
            // /relaunch command lands (pr/relaunch-command), this returns its
            // save-and-quit action and the quit flag below reuses the /exit
            // teardown; until then the command's own error is reported.
            let result = crate::commands::execute("/relaunch", app);
            if result.is_error {
                return (
                    false,
                    response_error(
                        &id,
                        "command_error",
                        result
                            .message
                            .unwrap_or_else(|| "relaunch failed".to_string()),
                    ),
                );
            }
            let quit = matches!(result.action, Some(AppAction::Quit));
            app.needs_redraw = true;
            (quit, response_ok(id, ResponseResult::Relaunching))
        }
    }
}

// ── Socket server (unix only) ───────────────────────────────────────────────

/// Bound listener + its accept thread. Dropping unbinds: the accept thread
/// stops within one poll interval, the socket file is unlinked if this
/// process still owns it, and in-flight connections finish on their own.
#[cfg(unix)]
pub(crate) struct ControlSocketHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for ControlSocketHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // The accept loop polls at CONNECTION_POLL_INTERVAL and never
            // blocks on a connection (each connection has its own thread),
            // so this join is bounded and cannot deadlock.
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
impl std::fmt::Debug for ControlSocketHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlSocketHandle")
            .field("stopped", &self.stop.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
#[allow(dead_code)] // kept so the SessionControl field type is portable
pub(crate) struct ControlSocketHandle;

/// Bind `<sessions-dir>/<session-id>/control.sock` (0600) and serve it.
/// Refused when another live process already serves that path; a stale file
/// (crash leftover, nothing answering) is taken over.
#[cfg(unix)]
pub(crate) fn bind_control_socket(
    sessions_dir: &Path,
    session_id: &str,
    commands_tx: mpsc::Sender<PendingCommand>,
    status: Arc<Mutex<StatusSnapshot>>,
) -> io::Result<ControlSocketHandle> {
    let session_dir = sessions_dir.join(session_id);
    fs::create_dir_all(&session_dir)?;
    let path = session_dir.join(SOCKET_FILE_NAME);
    prepare_socket_path(&path)?;

    let listener = UnixListener::bind(&path)?;
    let identity = socket_file_identity(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name(format!("codewhale-control-{session_id}"))
        .spawn(move || serve(listener, path, identity, thread_stop, commands_tx, status))?;

    Ok(ControlSocketHandle {
        stop,
        thread: Some(thread),
    })
}

#[cfg(not(unix))]
pub(crate) fn bind_control_socket(
    _sessions_dir: &Path,
    _session_id: &str,
    _commands_tx: mpsc::Sender<PendingCommand>,
    _status: Arc<Mutex<StatusSnapshot>>,
) -> io::Result<ControlSocketHandle> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the per-session control socket is unix-only",
    ))
}

/// Take over the socket path, or refuse when a live server already holds it.
#[cfg(unix)]
fn prepare_socket_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                // A plain file (or directory) in the way: not ours to keep.
                fs::remove_file(path)?;
                return Ok(());
            }
            match UnixStream::connect(path) {
                // Someone answers: a live process owns this session's socket.
                // Do not steal it (a "socket busy" refusal).
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("control socket already live at {}", path.display()),
                )),
                // Stale: the file exists but nothing listens. Take over.
                Err(_) => {
                    fs::remove_file(path)?;
                    Ok(())
                }
            }
        }
    }
}

/// (device, inode) so an unlink never removes a file this process did not bind.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SocketFileIdentity {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
fn socket_file_identity(path: &Path) -> Option<SocketFileIdentity> {
    let metadata = fs::metadata(path).ok()?;
    use std::os::unix::fs::MetadataExt;
    Some(SocketFileIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
    })
}

#[cfg(unix)]
fn serve(
    listener: UnixListener,
    path: PathBuf,
    identity: Option<SocketFileIdentity>,
    stop: Arc<AtomicBool>,
    commands_tx: mpsc::Sender<PendingCommand>,
    status: Arc<Mutex<StatusSnapshot>>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                // The listener is nonblocking, and on BSD-family platforms
                // (macOS, FreeBSD) an accepted socket *inherits* O_NONBLOCK
                // from the listener — Linux does not. The per-connection
                // handler expects blocking reads/writes (bounded by request
                // caps and timeouts), so make that explicit: without it, a
                // read on macOS returns EAGAIN mid-frame on a large request
                // and the connection dies with a broken pipe on the client.
                let _ = stream.set_nonblocking(false);
                // One thread per connection: a silent client
                // must not stall other clients or the stop check.
                let tx = commands_tx.clone();
                let status = Arc::clone(&status);
                let _ = thread::Builder::new()
                    .name("codewhale-control-conn".to_string())
                    .spawn(move || handle_connection(stream, &tx, &status));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(CONNECTION_POLL_INTERVAL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                // Listener gone (e.g. the session dir was removed out from
                // under us) — stop serving; connections fail to connect from
                // here on, which is the honest state.
                tracing::debug!(%error, "control socket listener closed");
                break;
            }
        }
    }
    if let Some(identity) = identity
        && socket_file_identity(&path) == Some(identity)
    {
        let _ = fs::remove_file(&path);
    }
}

/// Serve exactly one request: read one bounded line, answer, close.
#[cfg(unix)]
fn handle_connection(
    stream: UnixStream,
    commands_tx: &mpsc::Sender<PendingCommand>,
    status: &Arc<Mutex<StatusSnapshot>>,
) {
    let _ = stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(RESPONSE_WRITE_TIMEOUT));
    let mut stream = BufReader::new(stream);

    let line = match read_request_line(&mut stream) {
        Ok(Some(line)) => line,
        Ok(None) => return, // EOF, empty frame, or timeout: close silently
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            // Oversized frame: the reader drained it, so the client can
            // finish writing and read this rejection.
            let response = response_error("", "invalid_request", error.to_string());
            write_response_line(stream.get_mut(), &response);
            return;
        }
        Err(_) => return,
    };
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    let request: Request = match serde_json::from_str(trimmed) {
        Ok(request) => request,
        Err(error) => {
            let response =
                response_error("", "invalid_request", format!("invalid request: {error}"));
            write_response_line(stream.get_mut(), &response);
            return;
        }
    };

    let response = match request.method {
        Method::Status(_) => {
            let snapshot = status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            response_ok(
                request.id,
                ResponseResult::Status {
                    turn_state: snapshot.turn_state,
                    goal: snapshot.goal,
                },
            )
        }
        Method::Message(params) => dispatch_to_app(
            request.id,
            ControlCommand::Message { text: params.text },
            commands_tx,
        ),
        Method::Interrupt(_) => dispatch_to_app(request.id, ControlCommand::Interrupt, commands_tx),
        Method::Relaunch(_) => dispatch_to_app(request.id, ControlCommand::Relaunch, commands_tx),
    };
    write_response_line(stream.get_mut(), &response);
}

/// Hand a verb to the event loop and wait (bounded) for its answer.
#[cfg(unix)]
fn dispatch_to_app(
    id: String,
    command: ControlCommand,
    commands_tx: &mpsc::Sender<PendingCommand>,
) -> String {
    let (respond_to, rx) = mpsc::channel();
    if let Err(error) = commands_tx.send(PendingCommand {
        id: id.clone(),
        command,
        respond_to,
    }) {
        return response_error(
            &id,
            "server_unavailable",
            format!("failed to dispatch request: {error}"),
        );
    }
    match rx.recv_timeout(DISPATCH_RESPONSE_TIMEOUT) {
        Ok(response) => response,
        Err(mpsc::RecvTimeoutError::Timeout) => response_error(
            &id,
            "timeout",
            format!(
                "timed out waiting for the app to handle the request after {} ms",
                DISPATCH_RESPONSE_TIMEOUT.as_millis()
            ),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => response_error(
            &id,
            "server_unavailable",
            "request handling failed: app response channel closed".to_string(),
        ),
    }
}

/// One newline-terminated line, bounded. `Ok(None)` = EOF before any content.
/// The cap is enforced *while* reading (a hostile peer cannot make us buffer
/// an unbounded line), and on oversize the remainder of the frame is
/// discarded through a fixed-size buffer — memory stays bounded, and a client
/// that wrote the whole request can still receive the rejection.
#[cfg(unix)]
fn read_request_line(stream: &mut BufReader<UnixStream>) -> io::Result<Option<String>> {
    let mut capped = stream.by_ref().take(MAX_REQUEST_BYTES as u64 + 1);
    let mut line = Vec::new();
    let read = capped.read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None); // EOF before any content
    }
    if line.last() != Some(&b'\n') {
        // The frame exceeded the cap (or was torn mid-line). Discard the
        // remainder so a well-behaved client finishes its write and reads
        // the rejection; a torn frame's write lands nowhere and is ignored.
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf[..n].contains(&b'\n') {
                        break;
                    }
                }
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
        ));
    }
    Ok(Some(String::from_utf8_lossy(&line).into_owned()))
}

#[cfg(unix)]
fn write_response_line(stream: &mut UnixStream, value: &str) {
    let _ = writeln!(stream, "{value}");
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Verb parsing ──────────────────────────────────────────────────────

    #[test]
    fn parses_each_verb_request() {
        let message: Request =
            serde_json::from_str(r#"{"id":"1","method":"message","params":{"text":"hi"}}"#)
                .expect("message request");
        assert_eq!(message.id, "1");
        assert!(matches!(message.method, Method::Message(p) if p.text == "hi"));

        for (raw, want) in [
            (
                r#"{"id":"2","method":"interrupt","params":{}}"#,
                "interrupt",
            ),
            (r#"{"id":"3","method":"relaunch","params":{}}"#, "relaunch"),
            (r#"{"id":"4","method":"status","params":{}}"#, "status"),
        ] {
            let request: Request = serde_json::from_str(raw).expect("verb request");
            let got = match request.method {
                Method::Message(_) => "message",
                Method::Interrupt(_) => "interrupt",
                Method::Relaunch(_) => "relaunch",
                Method::Status(_) => "status",
            };
            assert_eq!(got, want);
        }
    }

    #[test]
    fn rejects_unknown_verb() {
        let error = serde_json::from_str::<Request>(r#"{"id":"1","method":"dance","params":{}}"#)
            .expect_err("unknown verb must not parse");
        let message = error.to_string();
        assert!(message.contains("unknown variant"), "{message}");
    }

    #[test]
    fn rejects_missing_or_wrong_params() {
        // message without `text`
        let error = serde_json::from_str::<Request>(r#"{"id":"1","method":"message","params":{}}"#)
            .expect_err("message without text must not parse");
        assert!(error.to_string().contains("missing field"), "{error}");

        // missing params entirely
        let error = serde_json::from_str::<Request>(r#"{"id":"1","method":"status"}"#)
            .expect_err("missing params must not parse");
        assert!(!error.to_string().is_empty());

        // non-string text
        let error =
            serde_json::from_str::<Request>(r#"{"id":"1","method":"message","params":{"text":7}}"#)
                .expect_err("numeric text must not parse");
        assert!(!error.to_string().is_empty());

        // non-string id
        let error = serde_json::from_str::<Request>(r#"{"id":7,"method":"status","params":{}}"#)
            .expect_err("numeric id must not parse");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn serializes_responses_in_the_response_envelope_shape() {
        let sent = response_ok(
            "1".into(),
            ResponseResult::MessageSent { delivery: "queued" },
        );
        assert_eq!(
            sent,
            r#"{"id":"1","result":{"type":"message_sent","delivery":"queued"}}"#
        );

        let interrupted = response_ok("2".into(), ResponseResult::Interrupted { cancelled: true });
        assert_eq!(
            interrupted,
            r#"{"id":"2","result":{"type":"interrupted","cancelled":true}}"#
        );

        let relaunching = response_ok("3".into(), ResponseResult::Relaunching);
        assert_eq!(relaunching, r#"{"id":"3","result":{"type":"relaunching"}}"#);

        let status = response_ok(
            "4".into(),
            ResponseResult::Status {
                turn_state: TurnState::Idle,
                goal: GoalSnapshot {
                    objective: Some("ship it".to_string()),
                    status: "active".to_string(),
                    paused: false,
                },
            },
        );
        assert_eq!(
            status,
            r#"{"id":"4","result":{"type":"status","turn_state":"idle","goal":{"objective":"ship it","status":"active","paused":false}}}"#
        );

        let error = response_error("9", "invalid_request", "nope".to_string());
        assert_eq!(
            error,
            r#"{"id":"9","error":{"code":"invalid_request","message":"nope"}}"#
        );
    }

    // ── Socket framing (unix) ─────────────────────────────────────────────

    #[cfg(unix)]
    fn test_endpoint() -> (
        tempfile::TempDir,
        PathBuf,
        mpsc::Receiver<PendingCommand>,
        ControlSocketHandle,
    ) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let sessions_dir = temp.path().join("sessions");
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(StatusSnapshot {
            turn_state: TurnState::Idle,
            goal: GoalSnapshot {
                objective: Some("goal".to_string()),
                status: "active".to_string(),
                paused: false,
            },
        }));
        let handle = bind_control_socket(&sessions_dir, "test-session", tx, status).expect("bind");
        (
            temp,
            sessions_dir.join("test-session").join(SOCKET_FILE_NAME),
            rx,
            handle,
        )
    }

    #[cfg(unix)]
    fn request_response(path: &Path, request: &str) -> String {
        let mut stream = UnixStream::connect(path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        writeln!(stream, "{request}").expect("write request");
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .expect("read response");
        response
    }

    #[cfg(unix)]
    #[test]
    fn status_verb_answers_over_the_socket() {
        let (_temp, path, _rx, _handle) = test_endpoint();
        let response = request_response(&path, r#"{"id":"4","method":"status","params":{}}"#);
        let value: serde_json::Value = serde_json::from_str(&response).expect("response is json");
        assert_eq!(value["id"], "4");
        assert_eq!(value["result"]["type"], "status");
        assert_eq!(value["result"]["turn_state"], "idle");
        assert_eq!(value["result"]["goal"]["objective"], "goal");
        assert_eq!(value["result"]["goal"]["paused"], false);
    }

    #[cfg(unix)]
    #[test]
    fn message_verb_reaches_the_app_channel_and_answers() {
        let (_temp, path, rx, _handle) = test_endpoint();

        // The test stands in for the event loop on the other end of the
        // channel: it receives the verb and answers like `drain` would.
        let server = std::thread::spawn(move || {
            let pending = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("verb queued");
            assert_eq!(pending.id, "1");
            match pending.command {
                ControlCommand::Message { text } => assert_eq!(text, "hello"),
                other => panic!("expected Message, got {other:?}"),
            }
            pending
                .respond_to
                .send(response_ok(
                    "1".into(),
                    ResponseResult::MessageSent {
                        delivery: "dispatched",
                    },
                ))
                .expect("answer");
        });

        let response = request_response(
            &path,
            r#"{"id":"1","method":"message","params":{"text":"hello"}}"#,
        );
        server.join().expect("server thread");
        let value: serde_json::Value = serde_json::from_str(&response).expect("response is json");
        assert_eq!(value["id"], "1");
        assert_eq!(value["result"]["type"], "message_sent");
        assert_eq!(value["result"]["delivery"], "dispatched");
    }

    #[cfg(unix)]
    #[test]
    fn malformed_json_gets_invalid_request_error() {
        let (_temp, path, _rx, _handle) = test_endpoint();
        let response = request_response(&path, "{not json");
        let value: serde_json::Value = serde_json::from_str(&response).expect("response is json");
        assert_eq!(value["id"], "");
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[cfg(unix)]
    #[test]
    fn unknown_verb_gets_invalid_request_error() {
        let (_temp, path, _rx, _handle) = test_endpoint();
        let response = request_response(&path, r#"{"id":"7","method":"dance","params":{}}"#);
        let value: serde_json::Value = serde_json::from_str(&response).expect("response is json");
        assert_eq!(value["id"], "");
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[cfg(unix)]
    #[test]
    fn empty_line_closes_without_a_response() {
        let (_temp, path, _rx, _handle) = test_endpoint();
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        writeln!(stream).expect("write empty line");
        let mut response = String::new();
        let read = BufReader::new(stream)
            .read_line(&mut response)
            .expect("read");
        assert_eq!(read, 0, "empty line must close the connection silently");
        assert!(response.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn oversized_request_is_rejected_with_an_error() {
        let (_temp, path, _rx, _handle) = test_endpoint();
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let blob = "x".repeat(MAX_REQUEST_BYTES + 16);
        let request = format!(r#"{{"id":"1","method":"message","params":{{"text":"{blob}"}}}}"#);
        writeln!(stream, "{request}").expect("write oversized request");
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .expect("read rejection");
        let value: serde_json::Value = serde_json::from_str(&response).expect("response is json");
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[cfg(unix)]
    #[test]
    fn bind_refuses_a_live_socket_and_takes_over_a_stale_file() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let sessions_dir = temp.path().join("sessions");
        let socket_path = sessions_dir.join("test-session").join(SOCKET_FILE_NAME);
        let status = Arc::new(Mutex::new(StatusSnapshot {
            turn_state: TurnState::Idle,
            goal: GoalSnapshot {
                objective: None,
                status: "active".to_string(),
                paused: false,
            },
        }));
        let (tx, _rx) = mpsc::channel();

        // A stale plain file is taken over.
        fs::create_dir_all(socket_path.parent().expect("parent")).expect("mkdir");
        fs::write(&socket_path, b"stale").expect("write stale file");
        let handle = bind_control_socket(
            &sessions_dir,
            "test-session",
            tx.clone(),
            Arc::clone(&status),
        )
        .expect("bind over a stale file");
        drop(handle);

        // A live listener is refused.
        let _ = fs::remove_file(&socket_path);
        let live = UnixListener::bind(&socket_path).expect("bind live listener");
        let error = bind_control_socket(&sessions_dir, "test-session", tx, status)
            .expect_err("must refuse a live socket");
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(live);
        let _ = fs::remove_file(&socket_path);
    }

    #[cfg(unix)]
    #[test]
    fn drop_unbinds_and_unlinks_the_socket() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let sessions_dir = temp.path().join("sessions");
        let socket_path = sessions_dir.join("test-session").join(SOCKET_FILE_NAME);
        let (tx, _rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(StatusSnapshot {
            turn_state: TurnState::Idle,
            goal: GoalSnapshot {
                objective: None,
                status: "active".to_string(),
                paused: false,
            },
        }));
        let handle = bind_control_socket(&sessions_dir, "test-session", tx, status).expect("bind");
        assert!(socket_path.exists(), "socket file exists while bound");
        drop(handle);
        // The accept thread unlinks within one poll interval.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while socket_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !socket_path.exists(),
            "socket file must be unlinked after drop"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_backs_off_after_a_refused_takeover() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let sessions_dir = temp.path().join("sessions");
        let socket_path = sessions_dir.join("sess").join(SOCKET_FILE_NAME);
        fs::create_dir_all(socket_path.parent().expect("parent")).expect("mkdir");

        let mut control = SessionControl::new_with_sessions_dir(true, Some(sessions_dir.clone()));

        // A live listener occupies the path: the takeover is refused.
        let live = UnixListener::bind(&socket_path).expect("bind live listener");
        control.reconcile(Some("sess"));
        assert!(
            control.bound_session.is_none(),
            "refused bind must not claim"
        );

        // The other process goes away, but the backoff still holds.
        drop(live);
        let _ = fs::remove_file(&socket_path);
        control.reconcile(Some("sess"));
        assert!(
            control.bound_session.is_none(),
            "backoff must suppress an immediate rebind"
        );

        // After the backoff window, the same session binds successfully.
        std::thread::sleep(BIND_RETRY_BACKOFF + Duration::from_millis(50));
        control.reconcile(Some("sess"));
        assert_eq!(control.bound_session.as_deref(), Some("sess"));

        // Reconcile with the same id is a no-op; a different id rebinds.
        control.reconcile(Some("sess"));
        assert_eq!(control.bound_session.as_deref(), Some("sess"));
        control.reconcile(Some("other"));
        assert_eq!(control.bound_session.as_deref(), Some("other"));
        drop(control);
    }
}
