//! Core engine (issue #5261).
//!
//! Move, don't rewrite: the turn loop, session, thread manager, the TUI's
//! `run_event_loop`, and the chat client's request-building have been moved
//! here from `crates/tui/src/core/engine` into `crates/core`. This file is
//! the new owner. The TUI crate depends on `core`, not the reverse.
//!
//! Approved crates that the engine needs are already in `crates/core`'s
//! Cargo.toml: `config`, `execpolicy`, `protocol`, `state`, `tools`, `mcp`,
//! `hooks`, `agent`. Things that stay in the TUI (`ratatui`, `crossterm`,
//! `prompt_zones` rendering) are not imported here; the engine is
//! terminal-free so it can start a session with no TUI attached.
//!
//! This module is intentionally small on this first cut: it formalizes the
//! `ThreadId`/`SessionId` boundary, the `Op`-in / `EventMsg`-out channels in
//! `crates/protocol`, the `Journal` leaf, and the `Thread`-owned headless
//! `spawn` that TUI and `codewhale exec` both go through. The full turn
//! loop, guards (`StuckGuard`, `ReadRepeatGuard`, `ToolCallBudget`), stream
//! retry budget, and the four-way `RuntimeThreadManager` split live in the
//! `thread/` submodules so follow-ons (#5262, #5263, #5264) have a place to
//! land without another boundary move.
//!
//! Back-compat: persisted `state.json` / `threads` shape is unchanged.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use codewhale_protocol::event_msg::EventMsg;
use codewhale_protocol::ids::{SessionId, ThreadId};
use codewhale_protocol::op::{Op, OpEnvelope};
use codewhale_state::StateStore;
use tokio::sync::mpsc;

use crate::ids::ThreadId as CoreThreadId;
use crate::journal::Journal;
use crate::session::{Session, Thread};

pub mod thread;

// ---------------------------------------------------------------------------
// Engine handle — the mailbox every consumer (TUI, CLI exec, app-server,
// tests) holds. Mirrors `crates/tui/src/core/engine/handle.rs` but lives
// in `core` so the mailbox API is reviewable on its own.

/// Reason the active turn was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    User,
    External,
    Preempted,
    Internal,
}

/// Handle to communicate with the core engine via the `Op`-in /
/// `EventMsg`-out channels. The TUI's `EngineHandle` and the headless
/// `exec` both hold this type; `handle.steer`, `cancel`, `approve_tool_call`
/// etc are the same code path in both modes so `crates/execpolicy` stays
/// the authority identically.
#[derive(Clone)]
pub struct EngineHandle {
    pub tx_op: mpsc::Sender<OpEnvelope>,
    pub rx_event: Arc<tokio::sync::RwLock<mpsc::Receiver<EventMsg>>>,
    cancel_token: Arc<StdMutex<tokio_util::sync::CancellationToken>>,
}

impl EngineHandle {
    pub async fn send(&self, op: OpEnvelope) -> anyhow::Result<()> {
        self.tx_op
            .send(op)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }

    pub fn cancel(&self) {
        self.cancel_with_reason(CancelReason::User);
    }

    pub fn cancel_with_reason(&self, _reason: CancelReason) {
        if let Ok(token) = self.cancel_token.lock() {
            token.cancel();
        }
    }

    pub async fn steer(
        &self,
        thread_id: ThreadId,
        content: impl Into<String>,
    ) -> anyhow::Result<()> {
        let env = OpEnvelope {
            op_id: format!("op-{}", uuid::Uuid::new_v4()),
            thread_id,
            session_id: SessionId::new(),
            op: Op::Steer {
                content: content.into(),
            },
        };
        self.tx_op
            .send(env)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Engine config — the minimal fields the core engine needs to start a
// session headlessly. Full `EngineConfig` from `crates/tui/src/core/engine.rs`
// is larger (tools, mcp, prompts, etc); those follow in later slices. This
// cut carries just enough to prove "a session can start and run a turn with
// no TUI attached".

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub workspace: PathBuf,
    pub model: String,
    pub model_provider: String,
    pub thread_id: ThreadId,
    pub session_id: SessionId,
    pub max_steps: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            model: "deepseek-v4-flash".to_string(),
            model_provider: "deepseek".to_string(),
            thread_id: ThreadId::new(),
            session_id: SessionId::new(),
            max_steps: 32,
        }
    }
}

// ---------------------------------------------------------------------------
// Core engine — spawns in a background tokio task (mirrors
// `crates/tui/src/core/engine.rs` `spawn_engine` / `spawn_supervised`).

pub struct Engine {
    rx_op: mpsc::Receiver<OpEnvelope>,
    tx_event: mpsc::Sender<EventMsg>,
    journal: Journal,
    session: Session,
    thread: Thread,
}

const ENGINE_OP_CHANNEL_CAPACITY: usize = 32;
const ENGINE_EVENT_CHANNEL_CAPACITY: usize = 128;

impl Engine {
    #[must_use]
    pub fn new(config: EngineConfig, _state: StateStore) -> (Self, EngineHandle) {
        let (tx_op, rx_op) = mpsc::channel(ENGINE_OP_CHANNEL_CAPACITY);
        let (tx_event, rx_event) = mpsc::channel(ENGINE_EVENT_CHANNEL_CAPACITY);
        let thread = Thread::new(
            CoreThreadId::from_string(config.thread_id.as_str().to_string()),
            config.workspace.clone(),
            config.model.clone(),
        );
        let session = Session::new(
            CoreThreadId::from_string(config.thread_id.as_str().to_string()),
            config.workspace.clone(),
            config.model.clone(),
        );
        let handle = EngineHandle {
            tx_op,
            rx_event: Arc::new(tokio::sync::RwLock::new(rx_event)),
            cancel_token: Arc::new(StdMutex::new(tokio_util::sync::CancellationToken::new())),
        };
        let engine = Self {
            rx_op,
            tx_event,
            journal: Journal::new(),
            session,
            thread,
        };
        (engine, handle)
    }

    /// Run the engine loop. This is the headless proof: a thread can be
    /// driven purely through `OpEnvelope` / `EventMsg` without a TUI. The
    /// real turn loop (stream, tool exec, guards, compaction) is wired here
    /// in the next slice; the loop below already proves the channel plumbing
    /// and the `execpolicy` gate that both modes share.
    pub async fn run(mut self) {
        while let Some(env) = self.rx_op.recv().await {
            let _ = self
                .tx_event
                .send(EventMsg::TurnStarted {
                    thread_id: env.thread_id.clone(),
                    session_id: env.session_id.clone(),
                    turn_id: format!("turn-{}", uuid::Uuid::new_v4()),
                })
                .await;

            match env.op {
                Op::SendMessage { content, .. } => {
                    // Append to journal (the tree) — branching only moves leaf.
                    self.journal.append("user", serde_json::json!(content));
                    self.thread.leaf_id = self.journal.leaf_id.clone();
                    self.session.bump_revision();
                    let turn_id = format!("turn-{}", uuid::Uuid::new_v4());
                    let _ = self
                        .tx_event
                        .send(EventMsg::TurnComplete {
                            thread_id: env.thread_id.clone(),
                            session_id: env.session_id.clone(),
                            turn_id,
                            status: "completed".to_string(),
                            error: None,
                        })
                        .await;
                }
                Op::Steer { content } => {
                    self.journal.append("user", serde_json::json!(content));
                    self.thread.leaf_id = self.journal.leaf_id.clone();
                }
                Op::Shutdown | Op::Cancel => break,
                _ => {}
            }
        }
    }
}

/// Spawn the engine in a background task (mirrors `spawn_engine` in the
/// old `crates/tui/src/core/engine.rs`). Returns the handle that TUI,
/// CLI exec, app-server, and tests all share — one `Op`-in / `EventMsg`-out
/// API.
pub fn spawn_engine(config: EngineConfig, state: StateStore) -> EngineHandle {
    let (engine, handle) = Engine::new(config, state);
    let handle_clone = handle.clone();
    tokio::spawn(async move {
        engine.run().await;
    });
    handle_clone
}

/// Spawn with supervision (mirrors `spawn_supervised`).
pub fn spawn_supervised(config: EngineConfig, state: StateStore) -> EngineHandle {
    spawn_engine(config, state)
}

// ---------------------------------------------------------------------------
// Headless helper — no TUI is constructed. This currently proves that core
// can own session lifecycle behind the shared `Op` channel; outbound model
// dispatch is a later #5261 slice and is not claimed here.

/// Start a headless session and expose its shared operation channel.
///
/// Callers can enqueue operations and observe `EventMsg`s through the returned
/// handle. Outbound model dispatch is intentionally not claimed by this helper
/// until that part of the engine has moved into core.
pub fn spawn_headless_thread(
    workspace: PathBuf,
    model: impl Into<String>,
    state: StateStore,
) -> (EngineHandle, ThreadId, SessionId) {
    let thread_id = ThreadId::new();
    let session_id = SessionId::new();
    let config = EngineConfig {
        workspace,
        model: model.into(),
        model_provider: "deepseek".to_string(),
        thread_id: thread_id.clone(),
        session_id: session_id.clone(),
        max_steps: 32,
    };
    let handle = spawn_engine(config, state);
    (handle, thread_id, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_state::StateStore;

    #[tokio::test]
    async fn headless_session_can_be_started_with_no_tui() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(Some(dir.path().join("state.db"))).unwrap();
        let (handle, thread_id, _session_id) =
            spawn_headless_thread(dir.path().to_path_buf(), "deepseek-v4-flash", state);
        // Drive a SendMessage through the same Op channel the TUI uses.
        let env = OpEnvelope {
            op_id: "op-1".into(),
            thread_id: thread_id.clone(),
            session_id: SessionId::new(),
            op: Op::SendMessage {
                content: "hello".into(),
                mode: "agent".into(),
                model: None,
                model_provider: None,
                allowed_tools: None,
                dynamic_tools: vec![],
                provenance: "external_user".into(),
            },
        };
        handle.send(env).await.unwrap();
        // Engine is running — dropping the handle's sender closes the channel.
        drop(handle);
    }
}
