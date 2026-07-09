//! In-transcript cards for sub-agent activity (issue #128).
//!
//! Two cards consume the #130 mailbox stream and render live in the chat
//! transcript:
//!
//! - [`DelegateCard`] — single `agent` invocation. Live tree of the
//!   last 3 actions plus a header with status / glyph / role.
//! - [`FanoutCard`] — `rlm` fanout (or any future multi-child dispatch).
//!   Dot-grid of worker slots (`●` filled, `○` pending); header owns lifecycle.
//!
//! Both cards are state machines updated by [`apply_to_delegate`] /
//! [`apply_to_fanout`]. The sidebar (see `tui/sidebar.rs`) defers detail
//! to whichever card is active in the transcript, so these are the
//! primary status surface.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;
use crate::tools::subagent::MailboxMessage;
use crate::tui::ui_text::truncate_line_to_width;
use crate::tui::widgets::tool_card::{ToolFamily, family_glyph, family_label};
use unicode_width::UnicodeWidthStr;

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
    /// Interrupted with a continuable checkpoint (e.g. API timeout); not
    /// running, but recoverable from its checkpoint.
    Interrupted,
}

impl AgentLifecycle {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Pending => palette::TEXT_MUTED,
            Self::Running => palette::STATUS_WARNING,
            Self::Completed => palette::STATUS_SUCCESS,
            Self::Failed => palette::STATUS_ERROR,
            Self::Cancelled => palette::TEXT_MUTED,
            Self::Interrupted => palette::STATUS_WARNING,
        }
    }
}

/// Card for a single delegated `agent` invocation.
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

    /// Project this direct sub-agent card onto the shared workflow history
    /// renderer (#4122) so collapsed/expanded concepts stay aligned.
    #[must_use]
    #[allow(dead_code)] // public #4122 convergence API; covered by unit tests
    pub fn as_workflow_history_panel(
        &self,
        started_at_ms: u64,
        completed_at_ms: Option<u64>,
    ) -> crate::tui::widgets::workflow_panel::WorkflowPanel {
        use crate::tui::widgets::workflow_panel::{WorkflowPanel, WorkflowPanelLifecycle};
        let lifecycle = match self.status {
            AgentLifecycle::Pending => WorkflowPanelLifecycle::Pending,
            AgentLifecycle::Running => WorkflowPanelLifecycle::Running,
            AgentLifecycle::Completed => WorkflowPanelLifecycle::Succeeded,
            AgentLifecycle::Failed => WorkflowPanelLifecycle::Failed,
            AgentLifecycle::Cancelled => WorkflowPanelLifecycle::Cancelled,
            AgentLifecycle::Interrupted => WorkflowPanelLifecycle::Failed,
        };
        WorkflowPanel::from_direct_subagent(
            self.agent_id.clone(),
            readable_agent_role(&self.agent_type),
            lifecycle,
            started_at_ms,
            completed_at_ms,
            self.summary.clone(),
            if matches!(self.status, AgentLifecycle::Failed) {
                self.summary.clone()
            } else {
                None
            },
        )
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
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.actions.len() + 3);
        let content_width = usize::from(width);
        let role = readable_agent_role(&self.agent_type);
        let short_id = crate::session_manager::truncate_id(&self.agent_id).to_string();
        let detail = if let Some(ref summary) = self.summary {
            truncate_action(summary, 72)
        } else {
            short_id
        };
        lines.push(card_header(
            ToolFamily::Delegate,
            self.status,
            &role,
            &detail,
            content_width,
        ));
        if self.truncated {
            lines.push(Line::from(Span::styled(
                "  \u{2026}".to_string(), // …
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        for action in &self.actions {
            let prefix = "  \u{2502} ";
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(palette::TEXT_DIM)),
                Span::styled(
                    truncate_action(action, line_detail_width(content_width, prefix).min(200)),
                    Style::default().fg(palette::TEXT_TOOL_OUTPUT),
                ),
            ]));
        }
        if self.status.is_terminal()
            && let Some(summary) = self.summary.as_ref()
        {
            let prefix = "  \u{2570} ";
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(palette::TEXT_DIM)),
                Span::styled(
                    truncate_action(summary, line_detail_width(content_width, prefix).min(200)),
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

    /// Update or insert a worker by id. Returns whether the visible state
    /// changed and the card should be redrawn.
    pub fn upsert_worker(&mut self, agent_id: &str, status: AgentLifecycle) -> bool {
        if let Some(slot) = self
            .workers
            .iter_mut()
            .find(|s| s.agent_id == agent_id || s.worker_id == agent_id)
        {
            if slot.agent_id == agent_id && slot.status == status {
                return false;
            }
            slot.agent_id = agent_id.to_string();
            slot.status = status;
            true
        } else {
            self.workers.push(WorkerSlot::new(agent_id, status));
            true
        }
    }

    /// Attach a real agent id to the first pending placeholder slot. Fanout
    /// cards are seeded from task ids before child agents exist; when a child
    /// starts, this keeps the dot count stable instead of appending a second
    /// circle for the same unit of work.
    pub fn claim_pending_worker(&mut self, agent_id: &str, status: AgentLifecycle) -> bool {
        if let Some(slot) = self.workers.iter_mut().find(|s| s.agent_id == agent_id) {
            if slot.status == status {
                return false;
            }
            slot.status = status;
            return true;
        }
        if let Some(slot) = self
            .workers
            .iter_mut()
            .find(|s| matches!(s.status, AgentLifecycle::Pending))
        {
            slot.agent_id = agent_id.to_string();
            slot.status = status;
            return true;
        }
        self.upsert_worker(agent_id, status)
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
                AgentLifecycle::Failed
                | AgentLifecycle::Cancelled
                | AgentLifecycle::Interrupted => failed += 1,
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
                AgentLifecycle::Completed => '\u{25CF}',   // ●
                AgentLifecycle::Running => '\u{25D0}',     // ◐
                AgentLifecycle::Failed => '\u{00D7}',      // ×
                AgentLifecycle::Cancelled => '\u{2298}',   // ⊘
                AgentLifecycle::Pending => '\u{25CB}',     // ○
                AgentLifecycle::Interrupted => '\u{25CC}', // ◌
            };
            s.push(glyph);
        }
        s
    }

    #[must_use]
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(3);
        let content_width = usize::from(width);
        let header_status = self.aggregate_status();
        let title = format!("{} ({} workers)", self.kind, self.workers.len());
        let family = if matches!(self.kind.as_str(), "rlm_open" | "rlm_eval" | "rlm") {
            ToolFamily::Rlm
        } else {
            ToolFamily::Fanout
        };
        lines.push(card_header(
            family,
            header_status,
            &self.kind,
            &title,
            content_width,
        ));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                self.dot_grid(),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines
    }

    fn aggregate_status(&self) -> AgentLifecycle {
        let (done, running, failed, pending) = self.counts();
        if running > 0 || pending > 0 {
            AgentLifecycle::Running
        } else if self
            .workers
            .iter()
            .any(|slot| matches!(slot.status, AgentLifecycle::Interrupted))
        {
            AgentLifecycle::Interrupted
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
    width: usize,
) -> Line<'static> {
    let glyph = family_glyph(family);
    let verb = family_label(family);
    let header_color = status.color();
    let glyph_text = format!("{glyph} ");
    let status_text = format!("[{}]", status.label());
    // #4148: an `agent_type` that already reads as the verb (e.g. "delegate")
    // would otherwise render a duplicate "delegate delegate". When the role
    // collapses to the verb, the verb already carries the signal — drop the
    // echoed role rather than repeat it.
    let show_role = !role.eq_ignore_ascii_case(verb);
    let mut fixed_parts: Vec<&str> = vec![glyph_text.as_str(), verb, " "];
    if show_role {
        fixed_parts.push(role);
        fixed_parts.push(" ");
    }
    fixed_parts.push(status_text.as_str());
    fixed_parts.push(" ");
    let fixed_width = fixed_parts
        .iter()
        .map(|text| UnicodeWidthStr::width(*text))
        .sum::<usize>();
    let detail = truncate_action(detail, width.saturating_sub(fixed_width));
    let mut spans = vec![
        Span::styled(
            glyph_text,
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
    ];
    if show_role {
        spans.push(Span::styled(
            role.to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(status_text, Style::default().fg(header_color)));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        detail,
        Style::default().fg(palette::TEXT_MUTED),
    ));
    Line::from(spans)
}

/// Map agent types to human-readable role labels (#1981).
fn readable_agent_role(agent_type: &str) -> String {
    match agent_type.to_ascii_lowercase().as_str() {
        "general" => "worker".to_string(),
        "explore" => "scout".to_string(),
        "plan" => "planner".to_string(),
        "review" => "reviewer".to_string(),
        "implementer" => "builder".to_string(),
        "verifier" => "verifier".to_string(),
        "custom" => "specialist".to_string(),
        other => other.to_string(),
    }
}

fn truncate_action(text: &str, max: usize) -> String {
    truncate_line_to_width(text.trim(), max)
}

fn line_detail_width(line_width: usize, prefix: &str) -> usize {
    line_width.saturating_sub(UnicodeWidthStr::width(prefix))
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
            if card.status == AgentLifecycle::Running {
                return false;
            }
            card.status = AgentLifecycle::Running;
        }
        MailboxMessage::Progress { status, .. } => {
            let low_signal = is_low_signal_progress(status);
            if low_signal && card.status == AgentLifecycle::Running {
                return false;
            }
            card.status = AgentLifecycle::Running;
            if !low_signal {
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
        MailboxMessage::Interrupted { reason, .. } => {
            card.status = AgentLifecycle::Interrupted;
            card.summary = Some(reason.clone());
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
        MailboxMessage::Started { .. } => card.claim_pending_worker(id, AgentLifecycle::Running),
        MailboxMessage::Progress { .. } => card.claim_pending_worker(id, AgentLifecycle::Running),
        MailboxMessage::ToolCallStarted { .. } => {
            card.claim_pending_worker(id, AgentLifecycle::Running)
        }
        MailboxMessage::ToolCallCompleted { .. } => true,
        MailboxMessage::Completed { .. } => card.upsert_worker(id, AgentLifecycle::Completed),
        MailboxMessage::Failed { .. } => card.upsert_worker(id, AgentLifecycle::Failed),
        MailboxMessage::Interrupted { .. } => card.upsert_worker(id, AgentLifecycle::Interrupted),
        MailboxMessage::Cancelled { .. } => card.upsert_worker(id, AgentLifecycle::Cancelled),
        MailboxMessage::ChildSpawned { child_id, .. } => {
            card.upsert_worker(child_id, AgentLifecycle::Pending)
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
    use unicode_width::UnicodeWidthStr;

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
    fn delegate_card_header_does_not_duplicate_verb_as_role() {
        // #4148: a role that already reads as the "delegate" verb must not
        // render "delegate delegate" in the default transcript. The verb
        // stays; the echoed role is dropped.
        let card = DelegateCard::new("agent_1", "delegate");
        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(
            !rendered.contains("delegate delegate"),
            "verb must not be echoed as the role: {rendered:?}"
        );
        assert!(
            rendered.contains("delegate"),
            "the delegate verb itself must remain: {rendered:?}"
        );
        // A real role still renders next to the verb (regression guard).
        let worker = DelegateCard::new("agent_2", "general");
        let worker_rendered = render_to_strings(&worker.render_lines(80)).join("\n");
        assert!(
            worker_rendered.contains("delegate worker"),
            "distinct roles are still shown: {worker_rendered:?}"
        );
    }

    #[test]
    fn delegate_card_cjk_text_respects_render_width() {
        let mut card = DelegateCard::new("agent_e0b2dcf1", "implementer");
        card.status = AgentLifecycle::Running;
        card.summary = Some(
            "抹香鲸 agent_e0b2dcf1 running 10+ 124838ms role: implementer git: branch codex/issue-3439-zhipu-glm-fixture @ issue-3439".into(),
        );
        card.push_action("objective: QUESTION: Add Zhipu GLM as a first-class provider-scoped route for 中文输出".to_string());

        let rendered = render_to_strings(&card.render_lines(40));

        assert!(
            rendered[0].contains("builder") && rendered[0].contains("[running]"),
            "header keeps fixed status columns visible: {rendered:?}"
        );
        for line in rendered {
            let width = UnicodeWidthStr::width(line.as_str());
            assert!(width <= 40, "line width {width} exceeds 40: {line:?}");
        }
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
        assert!(
            !apply_to_delegate(&mut card, &msg),
            "repeated low-signal progress should not redraw the card"
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
    fn fanout_card_header_and_dot_grid_surface_aggregate_state() {
        let mut card = FanoutCard::new("rlm").with_workers(["w_1", "w_2", "w_3", "w_4"]);
        card.upsert_worker("w_1", AgentLifecycle::Completed);
        card.upsert_worker("w_2", AgentLifecycle::Completed);
        card.upsert_worker("w_3", AgentLifecycle::Completed);
        card.upsert_worker("w_4", AgentLifecycle::Failed);
        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(
            rendered.contains("[done]") || rendered.contains("[failed]"),
            "header should surface terminal lifecycle: {rendered}"
        );
        assert!(
            rendered.contains("\u{25CF}\u{25CF}\u{25CF}\u{00D7}"),
            "dot grid should mirror worker states: {rendered}"
        );
        assert!(
            !rendered.contains(" pending"),
            "redundant counts line should stay omitted: {rendered}"
        );
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
        let progress =
            MailboxMessage::progress("agent_live", "step 1/100: requesting model response");
        assert!(
            !apply_to_fanout(&mut card, &progress),
            "repeated progress for a running worker should not redraw"
        );
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

    #[test]
    fn delegate_interrupted_leaves_running_and_renders_reason() {
        let mut card = DelegateCard::new("agent_int", "general");
        apply_to_delegate(
            &mut card,
            &MailboxMessage::started("agent_int", crate::tools::subagent::SubAgentType::General),
        );
        assert_eq!(card.status, AgentLifecycle::Running);

        let msg = MailboxMessage::Interrupted {
            agent_id: "agent_int".into(),
            reason: "API call timed out after 120000ms; checkpoint preserved for continuation"
                .into(),
        };
        assert!(apply_to_delegate(&mut card, &msg));
        assert_eq!(card.status, AgentLifecycle::Interrupted);

        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(rendered.contains("[interrupted]"), "{rendered}");
        assert!(rendered.contains("API call timed out"), "{rendered}");
    }

    #[test]
    fn fanout_interrupted_worker_leaves_running_counts() {
        let mut card = FanoutCard::new("fanout").with_workers(["w_1", "w_2"]);
        apply_to_fanout(
            &mut card,
            &MailboxMessage::started("w_1", crate::tools::subagent::SubAgentType::General),
        );
        apply_to_fanout(
            &mut card,
            &MailboxMessage::started("w_2", crate::tools::subagent::SubAgentType::General),
        );

        let msg = MailboxMessage::Interrupted {
            agent_id: "w_1".into(),
            reason: "API call timed out".into(),
        };
        assert!(apply_to_fanout(&mut card, &msg));
        assert_eq!(card.workers[0].status, AgentLifecycle::Interrupted);
        assert_eq!(card.workers[1].status, AgentLifecycle::Running);

        // Copy dedupe (Wave 5c #4): the counts line is gone — the header and
        // dot grid carry the aggregate state instead.
        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(
            !rendered.contains("[interrupted]"),
            "one interrupted worker must not mark the fanout interrupted while another runs: {rendered}"
        );
        assert!(
            rendered.contains('\u{25D0}'),
            "dot grid should keep the running worker glyph: {rendered}"
        );
        assert!(
            rendered.contains('\u{25CC}'),
            "dot grid should mark the interrupted worker: {rendered}"
        );

        let msg = MailboxMessage::Interrupted {
            agent_id: "w_2".into(),
            reason: "API call timed out".into(),
        };
        assert!(apply_to_fanout(&mut card, &msg));
        let rendered = render_to_strings(&card.render_lines(80)).join("\n");
        assert!(
            rendered.contains("[interrupted]"),
            "aggregate header should surface interrupted once nothing runs: {rendered}"
        );
    }

    #[test]
    fn fanout_card_omits_redundant_counts_line_when_header_and_grid_present() {
        let ids: Vec<String> = (0..16).map(|i| format!("w_{i}")).collect();
        let mut card = FanoutCard::new("fanout").with_workers(ids.iter().cloned());
        for id in ids.iter().take(12) {
            card.upsert_worker(id, AgentLifecycle::Completed);
        }
        card.upsert_worker("w_12", AgentLifecycle::Running);

        let rendered = render_to_strings(&card.render_lines(80));
        assert!(
            rendered.iter().any(|line| line.contains('\u{25CF}')),
            "dot grid should remain: {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains('·')),
            "counts line should be dropped: {rendered:?}"
        );
    }

    #[test]
    fn direct_subagent_projects_onto_shared_workflow_history_card() {
        use crate::tui::widgets::workflow_panel::WorkflowHistoryExtras;

        let mut card = DelegateCard::new("agent_xyz", "explore");
        card.status = AgentLifecycle::Completed;
        card.summary = Some("mapped 4 call sites".to_string());
        let panel = card.as_workflow_history_panel(1_000, Some(5_000));
        let compact = panel.render_history_card(100, false, &WorkflowHistoryExtras::default());
        let joined = render_to_strings(&compact).join("\n");
        assert!(
            joined.contains("success") || joined.contains("explore"),
            "shared compact lifecycle: {joined}"
        );
        assert!(
            joined.contains("1 child") || joined.contains("children"),
            "shared child count: {joined}"
        );
        let expanded = panel.render_history_card(
            100,
            true,
            &WorkflowHistoryExtras {
                result_summary: Some("mapped 4 call sites".to_string()),
                ..WorkflowHistoryExtras::default()
            },
        );
        let joined = render_to_strings(&expanded).join("\n");
        assert!(joined.contains("result:"), "{joined}");
        assert!(joined.contains("mapped 4 call sites"), "{joined}");
    }
}
