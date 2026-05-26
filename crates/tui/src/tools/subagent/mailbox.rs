//! Mailbox abstraction for sub-agent runtime coordination.
//!
//! Monotonic sequence numbers give every consumer a consistent ordering even
//! when multiple subscribers (e.g. UI card + parent agent) drain
//! independently; close-as-cancel lets a single signal both stop new mail and
//! propagate cancellation through nested children.

// Some surface here is producer-only inside this crate today and consumed by
// #128's UI cards in a follow-up; suppress the dead-code warnings until then
// rather than deleting capabilities the design depends on.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::models::Usage;

use super::SubAgentType;

/// Stable, structured progress envelope shared across the sub-agent surface.
///
/// Tracks the lifecycle of a single agent (identified by `agent_id`) end to
/// end: spawn, per-step progress, tool execution, completion / failure /
/// cancellation, and parent → child topology so consumers can render trees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MailboxMessage {
    /// Agent has been started (background task is running).
    Started {
        agent_id: String,
        agent_type: String,
    },
    /// Free-form human-readable progress (mirrors `Event::AgentProgress`).
    Progress { agent_id: String, status: String },
    /// A tool call inside the agent has started.
    ToolCallStarted {
        agent_id: String,
        tool_name: String,
        step: u32,
    },
    /// A tool call inside the agent has finished.
    ToolCallCompleted {
        agent_id: String,
        tool_name: String,
        step: u32,
        ok: bool,
    },
    /// A child agent was spawned by this agent.
    ChildSpawned { parent_id: String, child_id: String },
    /// Agent completed successfully (carries the summary line shown in the
    /// transcript; full result is still available via `agent_result`).
    Completed { agent_id: String, summary: String },
    /// Agent failed with the carried error message.
    Failed { agent_id: String, error: String },
    /// Cancellation propagated to this agent.
    Cancelled { agent_id: String },
    /// Incremental token usage from a sub-agent's API call.
    /// Published after each turn so the parent's cost counter updates live.
    TokenUsage {
        agent_id: String,
        /// Model that produced this usage, used for pricing.
        model: String,
        /// Provider usage payload, including cache-hit/cache-miss fields.
        usage: Usage,
    },
}

impl MailboxMessage {
    /// `agent_id` of the message subject (for `ChildSpawned` this is the
    /// child, since that's the new lifecycle being announced).
    #[must_use]
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Started { agent_id, .. }
            | Self::Progress { agent_id, .. }
            | Self::ToolCallStarted { agent_id, .. }
            | Self::ToolCallCompleted { agent_id, .. }
            | Self::Completed { agent_id, .. }
            | Self::Failed { agent_id, .. }
            | Self::Cancelled { agent_id }
            | Self::TokenUsage { agent_id, .. } => agent_id,
            Self::ChildSpawned { child_id, .. } => child_id,
        }
    }

    pub(crate) fn started(agent_id: impl Into<String>, agent_type: SubAgentType) -> Self {
        Self::Started {
            agent_id: agent_id.into(),
            agent_type: agent_type.as_str().to_string(),
        }
    }

    pub(crate) fn progress(agent_id: impl Into<String>, status: impl Into<String>) -> Self {
        Self::Progress {
            agent_id: agent_id.into(),
            status: status.into(),
        }
    }

    pub(crate) fn token_usage(
        agent_id: impl Into<String>,
        model: impl Into<String>,
        usage: Usage,
    ) -> Self {
        Self::TokenUsage {
            agent_id: agent_id.into(),
            model: model.into(),
            usage,
        }
    }
}

/// One delivery: a sequence number plus the message. The sequence is
/// monotonic across the entire mailbox (not per-agent) so a single ordering
/// is well-defined even when multiple sub-agents share one mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxEnvelope {
    pub seq: u64,
    pub message: MailboxMessage,
}

/// Sender side of the mailbox.
///
/// Cheaply cloneable (everything inside is `Arc`/atomic). Cloning a
/// `Mailbox` shares the same delivery channel, sequence counter, watch
/// notifier, and close/cancel state — so a child runtime that clones its
/// parent's `Mailbox` participates in the same stream.
#[derive(Clone)]
pub struct Mailbox {
    inner: Arc<MailboxInner>,
}

struct MailboxInner {
    tx: mpsc::UnboundedSender<MailboxEnvelope>,
    next_seq: AtomicU64,
    seq_tx: watch::Sender<u64>,
    closed: AtomicBool,
    cancel_token: CancellationToken,
}

/// Receiver side of the mailbox. Not `Clone` — only the original creator
/// can drain. Use `Mailbox::subscribe()` for fanout (UI cards + parent both
/// observing the same stream).
pub struct MailboxReceiver {
    rx: mpsc::UnboundedReceiver<MailboxEnvelope>,
    pending: VecDeque<MailboxEnvelope>,
}

impl Mailbox {
    /// Create a new mailbox bound to the given cancellation token. Closing
    /// the mailbox (or dropping the last sender) cancels this token. Runtimes
    /// that derive from the same token observe that cancellation; detached
    /// background `agent_open` sessions use their own runtime token.
    #[must_use]
    pub fn new(cancel_token: CancellationToken) -> (Self, MailboxReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (seq_tx, _) = watch::channel(0);
        let inner = MailboxInner {
            tx,
            next_seq: AtomicU64::new(0),
            seq_tx,
            closed: AtomicBool::new(false),
            cancel_token,
        };
        (
            Self {
                inner: Arc::new(inner),
            },
            MailboxReceiver {
                rx,
                pending: VecDeque::new(),
            },
        )
    }

    /// Subscribe to seq-bump notifications. Each `recv()` returns when the
    /// sequence counter advances, signaling new mail without copying it —
    /// the consumer then calls `drain` (or `recv_one` on its own receiver).
    /// Multiple subscribers may exist; this is the fanout primitive.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.seq_tx.subscribe()
    }

    /// Send a message; returns `Some(seq)` on success, `None` if the
    /// mailbox is already closed (callers should treat this as "the
    /// receiver is gone, stop publishing").
    pub fn send(&self, message: MailboxMessage) -> Option<u64> {
        if self.inner.closed.load(Ordering::Acquire) {
            return None;
        }
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let envelope = MailboxEnvelope { seq, message };
        if self.inner.tx.send(envelope).is_err() {
            return None;
        }
        let _ = self.inner.seq_tx.send_replace(seq);
        Some(seq)
    }

    /// Whether the mailbox has been closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// Close the mailbox AND cancel the bound cancellation token.
    ///
    /// "Close-as-cancel": there's no useful state where the consumer is gone
    /// but producers bound to this mailbox token should keep publishing.
    /// Closing cancels the bound token; directly derived `child_runtime()`
    /// children observe it, while detached `agent_open` sessions rely on their
    /// own explicit cancellation.
    pub fn close(&self) {
        if !self.inner.closed.swap(true, Ordering::AcqRel) {
            self.inner.cancel_token.cancel();
        }
    }
}

impl MailboxReceiver {
    fn sync_pending(&mut self) {
        while let Ok(env) = self.rx.try_recv() {
            self.pending.push_back(env);
        }
    }

    /// Whether any envelopes are buffered (or arrived since last check).
    pub fn has_pending(&mut self) -> bool {
        self.sync_pending();
        !self.pending.is_empty()
    }

    /// Drain all currently available envelopes, in delivery order.
    pub fn drain(&mut self) -> Vec<MailboxEnvelope> {
        self.sync_pending();
        self.pending.drain(..).collect()
    }

    /// Await the next envelope, with backpressure-aware blocking. Returns
    /// `None` when every sender has been dropped and the buffer is drained.
    pub async fn recv(&mut self) -> Option<MailboxEnvelope> {
        if let Some(env) = self.pending.pop_front() {
            return Some(env);
        }
        self.rx.recv().await
    }

    /// Awaits the next envelope with a timeout. Useful in tests.
    #[allow(dead_code)]
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<MailboxEnvelope> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .ok()
            .flatten()
    }
}

/// Convenience handle: a mailbox + the matching cancellation token, ready to
/// hand to a runtime. The receiver lives on the spawning side.
pub type SharedMailbox = Arc<Mutex<Option<MailboxReceiver>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    fn open() -> (Mailbox, MailboxReceiver, CancellationToken) {
        let token = CancellationToken::new();
        let (mb, rx) = Mailbox::new(token.clone());
        (mb, rx, token)
    }

    #[tokio::test]
    async fn mailbox_assigns_monotonic_sequence_numbers() {
        let (mb, _rx, _tok) = open();
        let s1 = mb
            .send(MailboxMessage::progress("a", "one"))
            .expect("seq 1");
        let s2 = mb
            .send(MailboxMessage::progress("a", "two"))
            .expect("seq 2");
        let s3 = mb
            .send(MailboxMessage::progress("b", "three"))
            .expect("seq 3");
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        assert_eq!(s3, 3);
        assert!(s2 > s1 && s3 > s2);
    }

    #[tokio::test]
    async fn mailbox_drains_in_delivery_order() {
        let (mb, mut rx, _tok) = open();
        mb.send(MailboxMessage::progress("a", "first"));
        mb.send(MailboxMessage::progress("a", "second"));
        mb.send(MailboxMessage::Completed {
            agent_id: "a".into(),
            summary: "done".into(),
        });
        let drained = rx.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].seq, 1);
        assert_eq!(drained[1].seq, 2);
        assert_eq!(drained[2].seq, 3);
        assert!(matches!(
            drained[0].message,
            MailboxMessage::Progress { .. }
        ));
        assert!(matches!(
            drained[2].message,
            MailboxMessage::Completed { .. }
        ));
        assert!(!rx.has_pending());
    }

    #[tokio::test]
    async fn subscribers_receive_seq_bumps_for_backpressure() {
        let (mb, _rx, _tok) = open();
        let mut sub_a = mb.subscribe();
        let mut sub_b = mb.subscribe();
        // Initial state: both at 0.
        assert_eq!(*sub_a.borrow(), 0);
        assert_eq!(*sub_b.borrow(), 0);

        mb.send(MailboxMessage::progress("x", "tick"));
        sub_a.changed().await.expect("subscriber a sees bump");
        sub_b.changed().await.expect("subscriber b sees bump");
        assert_eq!(*sub_a.borrow(), 1);
        assert_eq!(*sub_b.borrow(), 1);

        // A second send updates both subscribers' watch values too — even
        // though they share a single watch channel, fanout is N-to-many.
        mb.send(MailboxMessage::progress("x", "tick2"));
        sub_a.changed().await.expect("a sees second bump");
        assert_eq!(*sub_a.borrow(), 2);
    }

    #[tokio::test]
    async fn close_cancels_bound_token_and_blocks_further_sends() {
        let (mb, _rx, token) = open();
        assert!(!token.is_cancelled());
        mb.send(MailboxMessage::progress("a", "before close"));
        mb.close();
        assert!(token.is_cancelled(), "close-as-cancel: token must fire");
        assert!(mb.is_closed());
        // Further sends are no-ops, returning None instead of poisoning seq.
        assert!(
            mb.send(MailboxMessage::progress("a", "after close"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn close_propagates_to_child_tokens_across_max_spawn_depth() {
        // Mirror the runtime: root → child → grandchild (default depth 3).
        let root = CancellationToken::new();
        let child = root.child_token();
        let grandchild = child.child_token();
        let (mb, _rx) = Mailbox::new(root.clone());

        assert!(!child.is_cancelled());
        assert!(!grandchild.is_cancelled());
        mb.close();
        assert!(child.is_cancelled(), "child inherits root close");
        assert!(
            grandchild.is_cancelled(),
            "grandchild inherits too — covers default max_spawn_depth = 3"
        );
    }

    #[tokio::test]
    async fn recv_returns_envelope_then_none_after_close_and_drop() {
        let (mb, mut rx, _tok) = open();
        mb.send(MailboxMessage::progress("a", "queued"));
        let env = rx.recv().await.expect("buffered envelope");
        assert_eq!(env.seq, 1);

        // After closing AND dropping the sender, recv must yield None.
        mb.close();
        drop(mb);
        let next = rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(next.is_none(), "drained + dropped → recv yields None");
    }

    #[tokio::test]
    async fn cloned_mailbox_shares_sequence_and_close_state() {
        let (mb, mut rx, token) = open();
        let mb_clone = mb.clone();
        let s1 = mb
            .send(MailboxMessage::progress("a", "from original"))
            .unwrap();
        let s2 = mb_clone
            .send(MailboxMessage::progress("a", "from clone"))
            .unwrap();
        assert_eq!(s1, 1);
        assert_eq!(s2, 2, "clones share the seq counter");

        let drained = rx.drain();
        assert_eq!(drained.len(), 2);

        // Closing through one clone closes them all (the AtomicBool is shared).
        mb_clone.close();
        assert!(mb.is_closed());
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn agent_id_is_extractable_from_every_variant() {
        let cases: Vec<(MailboxMessage, &str)> = vec![
            (MailboxMessage::started("a1", SubAgentType::General), "a1"),
            (MailboxMessage::progress("a2", "x"), "a2"),
            (
                MailboxMessage::ToolCallStarted {
                    agent_id: "a3".into(),
                    tool_name: "read_file".into(),
                    step: 1,
                },
                "a3",
            ),
            (
                MailboxMessage::ToolCallCompleted {
                    agent_id: "a4".into(),
                    tool_name: "read_file".into(),
                    step: 1,
                    ok: true,
                },
                "a4",
            ),
            (
                MailboxMessage::ChildSpawned {
                    parent_id: "parent".into(),
                    child_id: "a5".into(),
                },
                "a5",
            ),
            (
                MailboxMessage::Completed {
                    agent_id: "a6".into(),
                    summary: "done".into(),
                },
                "a6",
            ),
            (
                MailboxMessage::Failed {
                    agent_id: "a7".into(),
                    error: "boom".into(),
                },
                "a7",
            ),
            (
                MailboxMessage::Cancelled {
                    agent_id: "a8".into(),
                },
                "a8",
            ),
            (
                MailboxMessage::TokenUsage {
                    agent_id: "a9".into(),
                    model: "deepseek-v4-flash".into(),
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 50,
                        ..Default::default()
                    },
                },
                "a9",
            ),
        ];
        for (msg, expected) in cases {
            assert_eq!(msg.agent_id(), expected, "extract failed for {msg:?}");
        }
    }
}
