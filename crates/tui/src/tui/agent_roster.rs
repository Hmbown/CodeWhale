//! Receipts-only projection of every agent that ran this session (#5479).
//!
//! One row per agent, retained after completion, ordered oldest-first so the
//! session reads as a history rather than a snapshot of whatever happens to be
//! running. This is the data model the agents rail renders; `/agents` renders
//! the same rows as text so headless and `exec` surfaces agree with the TUI.
//!
//! ## The truth rule
//!
//! Every number here is an `Option`, and `None` renders as `—`, never as `0`.
//! The distinction is the whole point: "this worker reported 96,300 input
//! tokens" and "no usage receipt exists for this worker" are different facts,
//! and a rail that prints `0` for the second one is lying in the direction that
//! makes Codewhale look cheap. Nothing here estimates, derives a token count
//! from text, or back-fills a missing receipt — values come from
//! [`AgentRunUsage`], which is populated from immutable per-response route
//! audits, or they are absent.
//!
//! A finished agent keeps the numbers it finished with: rows are built from the
//! retained worker record, never recomputed from live state.

use std::collections::BTreeMap;

use crate::tools::subagent::{AgentWorkerRecord, AgentWorkerStatus};

/// What a row is doing, collapsed to one glanceable state.
///
/// Deliberately coarser than [`AgentWorkerStatus`]: the rail needs a glyph and
/// a sort rank, not the full lifecycle. The precise status stays on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterState {
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl RosterState {
    /// Single-width glyph. Filled = attention, hollow = at rest.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Waiting => "◐",
            Self::Done => "○",
            Self::Failed => "✗",
            Self::Cancelled => "⊘",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    const fn from_worker(status: AgentWorkerStatus) -> Self {
        match status {
            AgentWorkerStatus::Queued
            | AgentWorkerStatus::Starting
            | AgentWorkerStatus::Running
            | AgentWorkerStatus::ModelWait
            | AgentWorkerStatus::RunningTool => Self::Running,
            AgentWorkerStatus::WaitingForUser => Self::Waiting,
            AgentWorkerStatus::Completed => Self::Done,
            AgentWorkerStatus::Failed => Self::Failed,
            AgentWorkerStatus::Cancelled | AgentWorkerStatus::Interrupted => Self::Cancelled,
        }
    }
}

/// One agent's row. Every optional field means "no receipt", not "zero".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRosterRow {
    pub worker_id: String,
    /// Session name, else role, else the fleet type — whichever the user named.
    pub display_name: String,
    pub model: String,
    pub state: RosterState,
    pub status: AgentWorkerStatus,
    /// The agent's current step or last tool, in one line. `None` when the
    /// worker has not reported an event yet.
    pub activity: Option<String>,
    /// Wall time: elapsed for a live agent, final duration for a finished one.
    pub millis: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_microusd: Option<u64>,
    pub steps_taken: u32,
    /// Set when this agent was spawned by another; the parent aggregates it.
    pub parent_run_id: Option<String>,
    pub run_id: String,
}

impl AgentRosterRow {
    /// `n/m done` for a workflow parent, from its children's terminal states.
    #[must_use]
    fn workflow_progress(&self, rows: &[Self]) -> Option<(usize, usize)> {
        let children = rows
            .iter()
            .filter(|row| row.parent_run_id.as_deref() == Some(self.run_id.as_str()))
            .collect::<Vec<_>>();
        if children.is_empty() {
            return None;
        }
        let done = children
            .iter()
            .filter(|row| row.state.is_terminal())
            .count();
        Some((done, children.len()))
    }
}

/// Build the roster from retained worker records.
///
/// `now_ms` is passed in rather than read from the clock so the projection is a
/// pure function — the caller supplies the same instant it renders with, and
/// tests get deterministic elapsed values.
#[must_use]
pub fn build_agent_roster(records: &[AgentWorkerRecord], now_ms: u64) -> Vec<AgentRosterRow> {
    let mut rows: Vec<AgentRosterRow> = records
        .iter()
        .map(|record| row_from_record(record, now_ms))
        .collect();
    // Oldest first: the rail is a history of the session, and a list that
    // reorders itself as agents finish is unreadable while you are watching it.
    rows.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
    rows.sort_by_key(|row| creation_key(records, &row.worker_id));
    rows
}

fn creation_key(records: &[AgentWorkerRecord], worker_id: &str) -> u64 {
    records
        .iter()
        .find(|record| record.spec.worker_id == worker_id)
        .map_or(u64::MAX, |record| record.created_at_ms)
}

fn row_from_record(record: &AgentWorkerRecord, now_ms: u64) -> AgentRosterRow {
    let state = RosterState::from_worker(record.status);
    AgentRosterRow {
        worker_id: record.spec.worker_id.clone(),
        display_name: display_name(record),
        model: record.spec.model.clone(),
        state,
        status: record.status,
        activity: activity_line(record),
        millis: wall_millis(record, now_ms),
        input_tokens: record.usage.input_tokens,
        output_tokens: record.usage.output_tokens,
        cost_microusd: record.usage.cost_microusd,
        steps_taken: record.steps_taken,
        parent_run_id: record.parent_run_id.clone(),
        run_id: record.spec.run_id.clone(),
    }
}

fn display_name(record: &AgentWorkerRecord) -> String {
    record
        .spec
        .session_name
        .clone()
        .or_else(|| {
            record
                .spec
                .child_route
                .as_ref()
                .and_then(|route| route.resolved_profile_id.clone())
                .filter(|profile| !profile.trim().is_empty())
        })
        .or_else(|| record.spec.role.clone())
        .unwrap_or_else(|| record.spec.agent_type.as_str().to_string())
}

/// The agent's current step or last tool, in one line.
///
/// Preference order is most-specific-first: the newest event naming a tool, then
/// the newest event carrying a message, then the worker's latest message. A
/// finished worker shows what it finished doing, not a stale "running" line.
fn activity_line(record: &AgentWorkerRecord) -> Option<String> {
    let from_events = record.events.iter().rev().find_map(|event| {
        event
            .tool_name
            .as_ref()
            .map(|tool| match event.step {
                Some(step) => format!("step {step} · {tool}"),
                None => tool.clone(),
            })
            .or_else(|| event.message.clone())
    });
    from_events
        .or_else(|| record.latest_message.clone())
        .or_else(|| record.result_summary.clone())
        .map(|line| one_line(&line))
}

/// Collapse to a single line and bound it. Rail rows are one row.
fn one_line(text: &str) -> String {
    const MAX_CHARS: usize = 72;
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() <= MAX_CHARS {
        return flattened;
    }
    let kept = flattened.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{kept}…")
}

/// Elapsed for a live agent, final duration for a finished one.
///
/// `None` when the worker has no start timestamp — a queued worker has not
/// started, and reporting `0s` would imply it had.
fn wall_millis(record: &AgentWorkerRecord, now_ms: u64) -> Option<u64> {
    let started = record.started_at_ms?;
    let end = record.completed_at_ms.unwrap_or(now_ms);
    Some(end.saturating_sub(started))
}

/// `3m 29s`, `12s`, `450ms`. Compact because it shares a row.
#[must_use]
pub fn format_duration(millis: u64) -> String {
    if millis < 1_000 {
        return format!("{millis}ms");
    }
    let seconds = millis / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {rest}s");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

/// `96.3k`, `1.2M`, `812`. Never rounds a real count to zero.
#[must_use]
pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    if tokens < 1_000_000 {
        return format!("{:.1}k", tokens as f64 / 1_000.0);
    }
    format!("{:.1}M", tokens as f64 / 1_000_000.0)
}

/// Absent receipts render as `—`. See the truth rule in the module docs.
fn or_dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_string())
}

/// Render the roster as transcript text.
///
/// The parent session is row zero (`● main`); workflow parents collapse their
/// children into `n/m done` and the children are indented beneath them.
#[must_use]
pub fn render_agent_roster(rows: &[AgentRosterRow], parent_label: &str) -> String {
    if rows.is_empty() {
        return format!(
            "● {parent_label}\n\nNo agents have run in this session yet. \
             Spawn one with the `agent` tool, or `/fleet` to set up roles."
        );
    }

    let mut children_by_parent: BTreeMap<&str, Vec<&AgentRosterRow>> = BTreeMap::new();
    for row in rows {
        if let Some(parent) = row.parent_run_id.as_deref() {
            children_by_parent.entry(parent).or_default().push(row);
        }
    }

    let mut out = format!("● {parent_label}\n");
    for row in rows {
        // A child is printed under its parent, not again at the top level.
        if row
            .parent_run_id
            .as_deref()
            .is_some_and(|parent| rows.iter().any(|candidate| candidate.run_id == parent))
        {
            continue;
        }
        out.push_str(&render_row(row, rows, 1));
        append_descendants(&mut out, row.run_id.as_str(), &children_by_parent, rows, 2);
    }
    out.push_str(&render_totals(rows));
    out
}

/// Footer totals, labelled honestly.
///
/// When only some rows carry a usage receipt the line says so, because a bare
/// total silently implies it covers every agent listed above it.
fn render_totals(rows: &[AgentRosterRow]) -> String {
    let (input, output) = roster_totals(rows);
    if input.is_none() && output.is_none() {
        return format!(
            "\n{} agent{} · no usage receipts recorded\n",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        );
    }
    let coverage = if all_rows_have_usage(rows) {
        String::new()
    } else {
        let reported = rows
            .iter()
            .filter(|row| row.input_tokens.is_some() || row.output_tokens.is_some())
            .count();
        format!(" (receipts from {reported} of {} agents)", rows.len())
    };
    format!(
        "\n{} agent{} · {} · {}{coverage}\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        or_dash(input.map(|t| format!("↓ {}", format_tokens(t)))),
        or_dash(output.map(|t| format!("↑ {}", format_tokens(t)))),
    )
}

fn append_descendants(
    out: &mut String,
    parent_run_id: &str,
    children_by_parent: &BTreeMap<&str, Vec<&AgentRosterRow>>,
    all: &[AgentRosterRow],
    depth: usize,
) {
    for child in children_by_parent.get(parent_run_id).into_iter().flatten() {
        out.push_str(&render_row(child, all, depth));
        append_descendants(
            out,
            child.run_id.as_str(),
            children_by_parent,
            all,
            depth + 1,
        );
    }
}

fn render_row(row: &AgentRosterRow, all: &[AgentRosterRow], depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let elapsed = or_dash(row.millis.map(format_duration));
    let input = or_dash(row.input_tokens.map(|t| format!("↓ {}", format_tokens(t))));
    let output = or_dash(row.output_tokens.map(|t| format!("↑ {}", format_tokens(t))));
    let activity = match row.workflow_progress(all) {
        Some((done, total)) => format!("{done}/{total} agents done"),
        None => or_dash(row.activity.clone()),
    };
    format!(
        "{indent}{glyph} {name}   {activity}   {elapsed} · {input} · {output}\n",
        glyph = row.state.glyph(),
        name = row.display_name,
    )
}

/// Usage totals across the roster, for a footer line.
///
/// Returns `None` for a field when *no* row reported it — summing absent
/// receipts into `0` would restate the same lie the per-row rule forbids.
#[must_use]
pub fn roster_totals(rows: &[AgentRosterRow]) -> (Option<u64>, Option<u64>) {
    fn total(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
        let reported: Vec<u64> = values.flatten().collect();
        (!reported.is_empty()).then(|| reported.into_iter().fold(0u64, u64::saturating_add))
    }
    (
        total(rows.iter().map(|row| row.input_tokens)),
        total(rows.iter().map(|row| row.output_tokens)),
    )
}

/// True when at least one row reported a usage receipt. Callers use this to
/// label the totals line honestly ("partial receipts") instead of implying the
/// number covers every agent.
#[must_use]
pub fn all_rows_have_usage(rows: &[AgentRosterRow]) -> bool {
    !rows.is_empty()
        && rows
            .iter()
            .all(|row| row.input_tokens.is_some() || row.output_tokens.is_some())
}

#[cfg(test)]
mod tests;
