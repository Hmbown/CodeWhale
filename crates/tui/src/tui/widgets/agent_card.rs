//! In-transcript cards for sub-agent activity (issue #128).
//!
//! Two cards consume the #130 mailbox stream and render live in the chat
//! transcript:
//!
//! - [`DelegateCard`] — single `agent_spawn` invocation. Live tree of the
//!   last 3 actions plus a header with status / glyph / role.
//! - [`FanoutCard`] — `rlm` fanout (or any future multi-child dispatch).
//!   Dot-grid of worker slots (`●` filled, `○` pending) plus an aggregate
//!   counts line.
//!
//! Both cards are state machines updated by [`apply_to_delegate`] /
//! [`apply_to_fanout`]. The sidebar (see `tui/sidebar.rs`) defers detail
//! to whichever card is active in the transcript, so these are the
//! primary status surface.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;
use crate::tools::subagent::MailboxMessage;
use crate::tui::widgets::tool_card::{ToolFamily, family_glyph, family_label};

/// Maximum number of recent actions kept on a `DelegateCard`. Older entries
/// are dropped from the head; an ellipsis row signals truncation.
pub const DELEGATE_MAX_ACTIONS: usize = 3;

/// Lifecycle of a delegated / fanned-out agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLifecycle {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentLifecycle {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Pending => palette::TEXT_MUTED,
            Self::Running => palette::STATUS_WARNING,
            Self::Completed => palette::STATUS_SUCCESS,
            Self::Failed => palette::STATUS_ERROR,
            Self::Cancelled => palette::TEXT_MUTED,
        }
    }
}

/// Card for a single delegated `agent_spawn` invocation.
///
/// Stores the last [`DELEGATE_MAX_ACTIONS`] action lines; older entries are
/// truncated and a single ellipsis row is rendered above the visible tail.
#[derive(Debug, Clone)]
pub struct DelegateCard {
    pub agent_id: String,
    pub agent_type: String,
    pub status: AgentLifecycle,
    pub summary: Option<String>,
    actions: Vec<String>,
    truncated: bool,
}

impl DelegateCard {
    #[must_use]
    pub fn new(agent_id: impl Into<String>, agent_type: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type: agent_type.into(),
            status: AgentLifecycle::Pending,
            summary: None,
            actions: Vec::new(),
            truncated: false,
        }
    }

    pub fn push_action(&mut self, action: impl Into<String>) {
        self.actions.push(action.into());
        if self.actions.len() > DELEGATE_MAX_ACTIONS {
            // Drop one head entry per overflow so steady-state is exactly
            // DELEGATE_MAX_ACTIONS lines; the ellipsis row signals the rest.
            self.actions.remove(0);
            self.truncated = true;
        }
    }

    #[must_use]
    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.actions.len() + 3);
        lines.push(card_header(
            ToolFamily::Delegate,
            self.status,
            &self.agent_type,
            &self.agent_id,
        ));
        if self.truncated {
            lines.push(Line::from(Span::styled(
                "  \u{2026}".to_string(), // …
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        for action in &self.actions {
            lines.push(Line::from(vec![
                Span::styled("  \u{2502} ", Style::default().fg(palette::TEXT_DIM)),
                Span::styled(
                    truncate_action(action, 200),
                    Style::default().fg(palette::TEXT_TOOL_OUTPUT),
                ),
            ]));
        }
        if self.status.is_terminal()
            && let Some(summary) = self.summary.as_ref()
        {
            lines.push(Line::from(vec![
                Span::styled("  \u{2570} ", Style::default().fg(palette::TEXT_DIM)),
                Span::styled(
                    truncate_action(summary, 200),
                    Style::default().fg(self.status.color()),
                ),
            ]));
        }
        lines
    }

    /// Number of actions held — exposed for tests; bounded at
    /// `DELEGATE_MAX_ACTIONS`.
    #[must_use]
    #[cfg(test)]
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// Whether the head was truncated (older actions dropped).
    #[must_use]
    #[cfg(test)]
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// One worker slot in a fanout group.
#[derive(Debug, Clone)]
pub struct WorkerSlot {
    /// Stable logical worker key. Stays tied to the worker slot even after a
    /// concrete sub-agent id exists.
    pub worker_id: String,
    /// Concrete agent id once spawned; placeholders use the worker id.
    pub agent_id: String,
    pub status: AgentLifecycle,
}

impl WorkerSlot {
    #[must_use]
    pub fn new(worker_id: impl Into<String>, status: AgentLifecycle) -> Self {
        let worker_id = worker_id.into();
        Self {
            agent_id: worker_id.clone(),
            worker_id,
            status,
        }
    }
}

/// Card for `rlm` (or any multi-child dispatch) fanout: dot-grid +
/// aggregate counts.
///
/// Slots are added as `ChildSpawned` envelopes arrive (or pre-allocated by
/// the engine when the worker count is known up front); each slot
/// transitions independently as its `Completed` / `Failed` / `Cancelled`
/// envelope is observed.
#[derive(Debug, Clone)]
pub struct FanoutCard {
    pub kind: String,
    pub workers: Vec<WorkerSlot>,
}

impl FanoutCard {
    #[must_use]
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            workers: Vec::new(),
        }
    }

    /// Pre-seed worker slots when the fanout size is known up front.
    #[allow(dead_code)]
    pub fn with_workers<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for id in ids {
            self.workers
                .push(WorkerSlot::new(id.into(), AgentLifecycle::Pending));
        }
        self
    }

    /// Update or insert a worker by id.
    pub fn upsert_worker(&mut self, agent_id: &str, status: AgentLifecycle) {
        if let Some(slot) = self
            .workers
            .iter_mut()
            .find(|s| s.agent_id == agent_id || s.worker_id == agent_id)
        {
            slot.agent_id = agent_id.to_string();
            slot.status = status;
        } else {
            self.workers.push(WorkerSlot::new(agent_id, status));
        }
    }

    /// Attach a real agent id to the first pending placeholder slot. Fanout
    /// cards are seeded from task ids before child agents exist; when a child
    /// starts, this keeps the dot count stable instead of appending a second
    /// circle for the same unit of work.
    pub fn claim_pending_worker(&mut self, agent_id: &str, status: AgentLifecycle) {
        if let Some(slot) = self.workers.iter_mut().find(|s| s.agent_id == agent_id) {
            slot.status = status;
            return;
        }
        if let Some(slot) = self
            .workers
            .iter_mut()
            .find(|s| matches!(s.status, AgentLifecycle::Pending))
        {
            slot.agent_id = agent_id.to_string();
            slot.status = status;
            return;
        }
        self.upsert_worker(agent_id, status);
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let mut done = 0usize;
        let mut running = 0usize;
        let mut failed = 0usize;
        let mut pending = 0usize;
        for slot in &self.workers {
            match slot.status {
                AgentLifecycle::Completed => done += 1,
                AgentLifecycle::Running => running += 1,
                AgentLifecycle::Failed | AgentLifecycle::Cancelled => failed += 1,
                AgentLifecycle::Pending => pending += 1,
            }
        }
        (done, running, failed, pending)
    }

    #[must_use]
    pub fn dot_grid(&self) -> String {
        let mut s = String::with_capacity(self.workers.len());
        for slot in &self.workers {
            let glyph = match slot.status {
                AgentLifecycle::Completed => '\u{25CF}', // ●
                AgentLifecycle::Running => '\u{25D0}',   // ◐
                AgentLifecycle::Failed => '\u{00D7}',    // ×
                AgentLifecycle::Cancelled => '\u{2298}', // ⊘
                AgentLifecycle::Pending => '\u{25CB}',   // ○
            };
            s.push(glyph);
        }
        s
    }

    #[must_use]
    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(3);
        let header_status = self.aggregate_status();
        let title = format!("{} ({} workers)", self.kind, self.workers.len());
        let family = if matches!(self.kind.as_str(), "rlm_open" | "rlm_eval" | "rlm") {
            ToolFamily::Rlm
        } else {
            ToolFamily::Fanout
        };
        lines.push(card_header(family, header_status, &self.kind, &title));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                self.dot_grid(),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        let (done, running, failed, pending) = self.counts();
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(
                    "{done} done \u{00B7} {running} running \u{00B7} {failed} failed \u{00B7} {pending} pending"
                ),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]));
        lines
    }

    fn aggregate_status(&self) -> AgentLifecycle {
        let (done, running, failed, pending) = self.counts();
        if running > 0 || pending > 0 {
            AgentLifecycle::Running
        } else if failed > 0 && done == 0 {
            AgentLifecycle::Failed
        } else if done > 0 {
            AgentLifecycle::Completed
        } else {
            AgentLifecycle::Pending
        }
    }

    /// Worker count (slots seeded or observed via mailbox).
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

fn card_header(
    family: ToolFamily,
    status: AgentLifecycle,
    role: &str,
    detail: &str,
) -> Line<'static> {
    let glyph = family_glyph(family);
    let verb = family_label(family);
    let header_color = status.color();
    Line::from(vec![
        Span::styled(
            format!("{glyph} "),
            Style::default()
                .fg(header_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            verb.to_string(),
            Style::default()
                .fg(header_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(role.to_string(), Style::default().fg(palette::TEXT_PRIMARY)),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", status.label()),
            Style::default().fg(header_color),
        ),
        Span::raw(" "),
        Span::styled(detail.to_string(), Style::default().fg(palette::TEXT_MUTED)),
    ])
}

fn truncate_action(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

/// Apply a mailbox envelope to a `DelegateCard`. Returns `true` if the
/// state changed (UI may want to redraw); `false` if the envelope was for
/// a different `agent_id`.
pub fn apply_to_delegate(card: &mut DelegateCard, msg: &MailboxMessage) -> bool {
    if msg.agent_id() != card.agent_id {
        return false;
    }
    match msg {
        MailboxMessage::Started { .. } => {
            card.status = AgentLifecycle::Running;
        }
        MailboxMessage::Progress { status, .. } => {
            card.status = AgentLifecycle::Running;
            if !is_low_signal_progress(status) {
                card.push_action(status);
            }
        }
        MailboxMessage::ToolCallStarted { tool_name, .. } => {
            card.push_action(format!("{tool_name} running"));
        }
        MailboxMessage::ToolCallCompleted { tool_name, ok, .. } => {
            card.push_action(format!("{tool_name} {}", if *ok { "ok" } else { "failed" }));
        }
        MailboxMessage::Completed { summary, .. } => {
            card.status = AgentLifecycle::Completed;
            card.summary = Some(summary.clone());
        }
        MailboxMessage::Failed { error, .. } => {
            card.status = AgentLifecycle::Failed;
            card.summary = Some(error.clone());
        }
        MailboxMessage::Cancelled { .. } => {
            card.status = AgentLifecycle::Cancelled;
        }
        MailboxMessage::ChildSpawned { .. } => {
            // Delegate cards represent a single agent; child spawns belong
            // to a sibling fanout card, not this one.
            return false;
        }
        MailboxMessage::TokenUsage { .. } => {
            // Cost accumulation happens in handle_subagent_mailbox (ui.rs)
            // before this apply function is called; TokenUsage never reaches
            // this arm in practice.
            return false;
        }
    }
    true
}

fn is_low_signal_progress(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    status.contains("requesting model response")
        || status.starts_with("started (")
        || (status.starts_with("step ") && status.contains(": complete"))
}

/// Apply a mailbox envelope to a `FanoutCard`. Updates per-worker state
/// based on which child the envelope is about. Returns `true` on change.
pub fn apply_to_fanout(card: &mut FanoutCard, msg: &MailboxMessage) -> bool {
    let id = msg.agent_id();
    match msg {
        MailboxMessage::Started { .. } => {
            card.claim_pending_worker(id, AgentLifecycle::Running);
            true
        }
        MailboxMessage::Progress { .. } | MailboxMessage::ToolCallStarted { .. } => {
            card.claim_pending_worker(id, AgentLifecycle::Running);
            true
        }
        MailboxMessage::ToolCallCompleted { .. } => true,
        MailboxMessage::Completed { .. } => {
            card.upsert_worker(id, AgentLifecycle::Completed);
            true
        }
        MailboxMessage::Failed { .. } => {
            card.upsert_worker(id, AgentLifecycle::Failed);
            true
        }
        MailboxMessage::Cancelled { .. } => {
            card.upsert_worker(id, AgentLifecycle::Cancelled);
            true
        }
        MailboxMessage::ChildSpawned { child_id, .. } => {
            card.upsert_worker(child_id, AgentLifecycle::Pending);
            true
        }
        MailboxMessage::TokenUsage { .. } => {
            // Cost accumulation happens in handle_subagent_mailbox (ui.rs)
            // before this apply function is called; TokenUsage never reaches
            // this arm in practice.
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn delegate_card_truncates_to_last_three_actions_with_ellipsis() {
        let mut card = DelegateCard::new("agent_001", "general");
        card.push_action("read README.md");
        card.push_action("grep TODO");
        card.push_action("edit src/lib.rs");
        // Up to the limit — no truncation yet.
        assert!(!card.truncated());
        assert_eq!(card.action_count(), DELEGATE_MAX_ACTIONS);

        card.push_action("write tests");
        card.push_action("run cargo test");
        assert!(card.truncated(), "truncation flag flips on overflow");
        assert_eq!(
            card.action_count(),
            DELEGATE_MAX_ACTIONS,
            "stable steady-state size"
        );

        let rendered = render_to_strings(&card.render_lines(80));
        assert!(
            rendered.iter().any(|line| line.contains('\u{2026}')),
            "ellipsis indicator must render: got {rendered:?}"
        );
        // The oldest two actions ("read README.md", "grep TODO") were dropped.
        assert!(
            !rendered.iter().any(|line| line.contains("read README.md")),
            "oldest action evicted: got {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("run cargo test")),
            "newest action retained: got {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("write tests")),
            "second-newest retained: got {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("edit src/lib.rs")),
            "third-newest retained: got {rendered:?}"
        );
    }

    #[test]
    fn delegate_card_terminal_status_renders_summary_row() {
        let mut card = DelegateCard::new("agent_002", "explore");
        card.push_action("listing files");
        let msg = MailboxMessage::Completed {
            agent_id: "agent_002".into(),
            summary: "scanned 42 files, no TODOs found".into(),
        };
        assert!(apply_to_delegate(&mut card, &msg));
        assert_eq!(card.status, AgentLifecycle::Completed);
        let rendered = render_to_strings(&card.render_lines(80));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("scanned 42 files")),
            "summary row renders on terminal status: got {rendered:?}"
        );
    }

    #[test]
    fn delegate_card_ignores_low_signal_scheduler_progress() {
        let mut card = DelegateCard::new("agent_003", "general");
        let msg = MailboxMessage::progress("agent_003", "step 1/100: requesting model response");

        assert!(apply_to_delegate(&mut card, &msg));
        assert_eq!(card.status, AgentLifecycle::Running);
        assert_eq!(
            card.action_count(),
            0,
            "scheduler progress should not become a stale transcript row"
        );

        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(!rendered.contains("step 1/100"), "{rendered}");
        assert!(
            !rendered.contains("requesting model response"),
            "{rendered}"
        );
    }

    #[test]
    fn delegate_tool_rows_omit_internal_step_numbers() {
        let mut card = DelegateCard::new("agent_004", "general");

        assert!(apply_to_delegate(
            &mut card,
            &MailboxMessage::ToolCallStarted {
                agent_id: "agent_004".into(),
                tool_name: "read_file".into(),
                step: 7,
            }
        ));
        assert!(apply_to_delegate(
            &mut card,
            &MailboxMessage::ToolCallCompleted {
                agent_id: "agent_004".into(),
                tool_name: "read_file".into(),
                step: 7,
                ok: true,
            }
        ));

        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(rendered.contains("read_file"), "{rendered}");
        assert!(
            !rendered.contains("[7]"),
            "internal loop step numbers are not useful in the live card: {rendered}"
        );
    }

    #[test]
    fn delegate_card_ignores_envelopes_for_other_agents() {
        let mut card = DelegateCard::new("agent_a", "general");
        let other = MailboxMessage::progress("agent_b", "noise");
        assert!(!apply_to_delegate(&mut card, &other));
        assert_eq!(card.action_count(), 0);
    }

    #[test]
    fn fanout_card_dot_grid_renders_stateful_worker_slots() {
        let mut card = FanoutCard::new("fanout")
            .with_workers(["w_1", "w_2", "w_3", "w_4", "w_5", "w_6", "w_7"]);
        card.upsert_worker("w_1", AgentLifecycle::Completed);
        card.upsert_worker("w_2", AgentLifecycle::Completed);
        card.upsert_worker("w_3", AgentLifecycle::Running);
        card.upsert_worker("w_4", AgentLifecycle::Failed);
        // 5/6/7 stay Pending.

        // Completed fills; running and failed are distinct; pending stays open.
        assert_eq!(
            card.dot_grid(),
            "\u{25CF}\u{25CF}\u{25D0}\u{00D7}\u{25CB}\u{25CB}\u{25CB}"
        );
    }

    #[test]
    fn fanout_card_aggregate_counts_match_dot_grid() {
        let mut card = FanoutCard::new("rlm").with_workers(["w_1", "w_2", "w_3", "w_4"]);
        card.upsert_worker("w_1", AgentLifecycle::Completed);
        card.upsert_worker("w_2", AgentLifecycle::Completed);
        card.upsert_worker("w_3", AgentLifecycle::Completed);
        card.upsert_worker("w_4", AgentLifecycle::Failed);
        let rendered = render_to_strings(&card.render_lines(80));
        // The stats row is the one carrying "running" too; the header may
        // mention "done" alone via the lifecycle status badge.
        let stats = rendered
            .iter()
            .find(|line| line.contains("running") && line.contains("pending"))
            .expect("counts line present");
        assert!(stats.contains("3 done"), "completed count: {stats}");
        assert!(
            stats.contains("1 failed"),
            "failed/cancelled fold into the same bucket: {stats}"
        );
        assert!(stats.contains("0 running"), "no running: {stats}");
        assert!(stats.contains("0 pending"), "no pending: {stats}");
    }

    #[test]
    fn fanout_apply_inserts_unknown_worker_via_child_spawned() {
        let mut card = FanoutCard::new("fanout");
        let msg = MailboxMessage::ChildSpawned {
            parent_id: "root".into(),
            child_id: "agent_late".into(),
        };
        assert!(apply_to_fanout(&mut card, &msg));
        assert_eq!(card.worker_count(), 1);
        assert_eq!(card.workers[0].agent_id, "agent_late");
        assert_eq!(card.workers[0].status, AgentLifecycle::Pending);
    }

    #[test]
    fn fanout_started_claims_seeded_pending_slot_without_growing_grid() {
        let mut card = FanoutCard::new("fanout").with_workers(["task:a", "task:b"]);
        let started =
            MailboxMessage::started("agent_live", crate::tools::subagent::SubAgentType::General);

        assert!(apply_to_fanout(&mut card, &started));

        assert_eq!(card.worker_count(), 2);
        assert_eq!(card.workers[0].agent_id, "agent_live");
        assert_eq!(card.workers[0].status, AgentLifecycle::Running);
        assert_eq!(card.workers[1].agent_id, "task:b");
        assert_eq!(card.workers[1].status, AgentLifecycle::Pending);
    }

    #[test]
    fn fanout_apply_transitions_worker_through_lifecycle() {
        let mut card = FanoutCard::new("fanout").with_workers(["w_1"]);
        let started = MailboxMessage::started("w_1", crate::tools::subagent::SubAgentType::General);
        apply_to_fanout(&mut card, &started);
        assert_eq!(card.workers[0].status, AgentLifecycle::Running);

        let done = MailboxMessage::Completed {
            agent_id: "w_1".into(),
            summary: "ok".into(),
        };
        apply_to_fanout(&mut card, &done);
        assert_eq!(card.workers[0].status, AgentLifecycle::Completed);
    }

    #[test]
    fn fanout_dot_grid_arithmetic_for_various_n() {
        // Spot-check several fanout sizes with a mix of states; this is the
        // arithmetic snapshot the issue acceptance calls out.
        let cases: &[(usize, usize, &str)] = &[
            (1, 0, "\u{25CB}"),
            (1, 1, "\u{25CF}"),
            (3, 2, "\u{25CF}\u{25CF}\u{25CB}"),
            (
                7,
                3,
                "\u{25CF}\u{25CF}\u{25CF}\u{25CB}\u{25CB}\u{25CB}\u{25CB}",
            ),
        ];
        for (total, done, expected) in cases {
            let ids: Vec<String> = (0..*total).map(|i| format!("w_{i}")).collect();
            let mut card = FanoutCard::new("fanout").with_workers(ids.iter().cloned());
            for id in ids.iter().take(*done) {
                card.upsert_worker(id, AgentLifecycle::Completed);
            }
            assert_eq!(
                card.dot_grid(),
                *expected,
                "fanout dot-grid for total={total} done={done}",
            );
        }
    }
}
