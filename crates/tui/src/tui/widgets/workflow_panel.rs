//! WorkflowPanel — unified activity surface for workflow / sub-agent progress.
//!
//! Issue #4121 (CODEWHALE_0_8_68 §2.4). Progress lives here instead of flooding
//! the chat transcript: a collapsible header above the composer plus an
//! expanded phase/row body. Events are applied through [`WorkflowPanelEvent`].
//!
//! Issue #4122 routes the same event stream into a compact history card that
//! reuses this state machine: collapsed summarizes lifecycle/children/phases/
//! failures/elapsed; expanded adds phase/child summaries, artifact links,
//! final result, and failure details. Direct sub-agent cards share helpers
//! from this module where practical.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tui::ui_text::truncate_line_to_width;
use crate::tui::widgets::Renderable;

/// Maximum worker rows rendered under the selected phase.
const MAX_VISIBLE_ROWS: usize = 8;
/// Maximum phase summary chips shown in the expanded body.
const MAX_PHASE_SUMMARY: usize = 6;

/// Lifecycle of the active (or most recently completed) workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowPanelLifecycle {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl WorkflowPanelLifecycle {
    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Pending)
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "success",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn color(self) -> ratatui::style::Color {
        match self {
            Self::Pending => palette::TEXT_MUTED,
            Self::Running => palette::STATUS_WARNING,
            Self::Succeeded => palette::STATUS_SUCCESS,
            Self::Failed => palette::STATUS_ERROR,
            Self::Cancelled => palette::TEXT_MUTED,
        }
    }
}

/// Per-task / per-worker row status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRowStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    SchemaFailed,
}

impl WorkflowRowStatus {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::SchemaFailed => "schema",
        }
    }

    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    #[must_use]
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::SchemaFailed)
    }

    #[must_use]
    pub fn is_cancel(self) -> bool {
        matches!(self, Self::Cancelled)
    }

    fn color(self) -> ratatui::style::Color {
        match self {
            Self::Pending => palette::TEXT_MUTED,
            Self::Running => palette::STATUS_WARNING,
            Self::Succeeded => palette::STATUS_SUCCESS,
            Self::Failed | Self::SchemaFailed => palette::STATUS_ERROR,
            Self::Cancelled => palette::TEXT_MUTED,
        }
    }

    fn from_ir_status(status: &str) -> Self {
        match status {
            "succeeded" | "completed" | "success" | "done" => Self::Succeeded,
            "failed" | "error" | "replay_diverged" => Self::Failed,
            "cancelled" | "canceled" => Self::Cancelled,
            "budget_exceeded" => Self::Failed,
            "running" => Self::Running,
            "pending" => Self::Pending,
            other if other.contains("schema") => Self::SchemaFailed,
            _ => Self::Failed,
        }
    }
}

/// One worker/task row under a phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPanelRow {
    pub task_id: String,
    pub label: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub strength: Option<String>,
    pub worktree: bool,
    pub workspace: Option<PathBuf>,
    pub status: WorkflowRowStatus,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub error: Option<String>,
    pub schema_error: Option<String>,
}

/// One ordered phase group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPanelPhase {
    pub title: String,
    pub rows: Vec<WorkflowPanelRow>,
}

impl WorkflowPanelPhase {
    fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
        }
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let mut done = 0usize;
        let mut running = 0usize;
        let mut failed = 0usize;
        let mut cancelled = 0usize;
        for row in &self.rows {
            match row.status {
                WorkflowRowStatus::Succeeded => done += 1,
                WorkflowRowStatus::Running | WorkflowRowStatus::Pending => running += 1,
                WorkflowRowStatus::Failed | WorkflowRowStatus::SchemaFailed => failed += 1,
                WorkflowRowStatus::Cancelled => cancelled += 1,
            }
        }
        (done, running, failed, cancelled)
    }
}

/// Events the panel understands. Mirrors the tool-side `WorkflowUiEvent`
/// shape so #4122 can forward JSON without re-encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowPanelEvent {
    RunStarted {
        run_id: String,
        workflow_id: Option<String>,
        workflow_goal: Option<String>,
        source_path: Option<PathBuf>,
        token_budget: Option<u64>,
        at_ms: u64,
    },
    RunCompleted {
        status: WorkflowPanelLifecycle,
        error: Option<String>,
        at_ms: u64,
    },
    RunCancelled {
        reason: String,
        at_ms: u64,
    },
    PhaseStarted {
        title: String,
        at_ms: u64,
    },
    TaskStarted {
        task_id: String,
        label: Option<String>,
        profile: Option<String>,
        model: Option<String>,
        strength: Option<String>,
        resolved_model: Option<String>,
        worktree: bool,
        workspace: Option<PathBuf>,
        at_ms: u64,
    },
    TaskCompleted {
        task_id: String,
        status: WorkflowRowStatus,
        at_ms: u64,
    },
    TaskSchemaValidationFailed {
        task_id: String,
        message: String,
        at_ms: u64,
    },
    BudgetUpdated {
        total: Option<u64>,
        spent: u64,
        remaining: Option<u64>,
        at_ms: u64,
    },
}

impl WorkflowPanelEvent {
    /// Parse one flattened tool UI event (`{"type":"…", …}`).
    pub fn from_json_value(value: &Value) -> Option<Self> {
        let event_type = value.get("type")?.as_str()?;
        let at_ms = value
            .get("at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_else(now_ms);
        match event_type {
            "run_started" => Some(Self::RunStarted {
                run_id: value
                    .get("run_id")
                    .and_then(Value::as_str)
                    .unwrap_or("workflow")
                    .to_string(),
                workflow_id: opt_str(value, "workflow_id"),
                workflow_goal: opt_str(value, "workflow_goal"),
                source_path: opt_str(value, "source_path").map(PathBuf::from),
                token_budget: value.get("token_budget").and_then(Value::as_u64),
                at_ms,
            }),
            "run_completed" => {
                let status = value
                    .get("status")
                    .and_then(Value::as_str)
                    .map(lifecycle_from_status)
                    .unwrap_or(WorkflowPanelLifecycle::Succeeded);
                Some(Self::RunCompleted {
                    status,
                    error: opt_str(value, "error"),
                    at_ms,
                })
            }
            "run_cancelled" => Some(Self::RunCancelled {
                reason: opt_str(value, "reason").unwrap_or_else(|| "cancelled".to_string()),
                at_ms,
            }),
            "phase_started" => Some(Self::PhaseStarted {
                title: opt_str(value, "title").unwrap_or_else(|| "Phase".to_string()),
                at_ms,
            }),
            "task_started" => Some(Self::TaskStarted {
                task_id: opt_str(value, "task_id")?,
                // Prefer typed workflow metadata over generic label so rows
                // never fall back to prompt parsing (#4119).
                label: opt_str(value, "workflow_task_label").or_else(|| opt_str(value, "label")),
                profile: opt_str(value, "profile"),
                model: opt_str(value, "model").or_else(|| opt_str(value, "resolved_model")),
                strength: opt_str(value, "strength"),
                resolved_model: opt_str(value, "resolved_model"),
                worktree: value
                    .get("worktree")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                workspace: opt_str(value, "workspace").map(PathBuf::from),
                at_ms,
            }),
            "task_completed" => {
                let status = value
                    .get("status")
                    .and_then(Value::as_str)
                    .map(WorkflowRowStatus::from_ir_status)
                    .unwrap_or(WorkflowRowStatus::Succeeded);
                Some(Self::TaskCompleted {
                    task_id: opt_str(value, "task_id")?,
                    status,
                    at_ms,
                })
            }
            "task_schema_validation_failed" => Some(Self::TaskSchemaValidationFailed {
                task_id: opt_str(value, "task_id")?,
                message: opt_str(value, "message").unwrap_or_else(|| "schema failed".to_string()),
                at_ms,
            }),
            "budget_updated" => Some(Self::BudgetUpdated {
                total: value.get("total").and_then(Value::as_u64),
                spent: value.get("spent").and_then(Value::as_u64).unwrap_or(0),
                remaining: value.get("remaining").and_then(Value::as_u64),
                at_ms,
            }),
            // Logs are intentionally not surfaced in the panel body — they
            // would re-flood the surface the panel exists to protect.
            "log" => None,
            _ => None,
        }
    }
}

/// Collapsible workflow activity panel.
#[derive(Debug, Clone)]
pub struct WorkflowPanel {
    pub run_id: String,
    pub label: String,
    pub lifecycle: WorkflowPanelLifecycle,
    pub expanded: bool,
    /// When true the panel accepts `t`/`c` keyboard shortcuts.
    pub keyboard_focus: bool,
    pub phases: Vec<WorkflowPanelPhase>,
    pub selected_phase: usize,
    pub budget_total: Option<u64>,
    pub budget_spent: u64,
    pub budget_remaining: Option<u64>,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    /// Optional final result / verification summary for the history card.
    pub result_summary: Option<String>,
    /// Source script path or other durable artifact pointer.
    pub source_path: Option<PathBuf>,
    /// Spillover / full-output path when the tool result was large.
    pub spillover_path: Option<PathBuf>,
    /// Set when the operator requested cancel from the panel (mouse/key).
    /// Consumed by the host to drive `/workflow cancel`.
    cancel_emit: Option<String>,
}

/// Extra fields the history card can show that are not part of the live panel
/// progress surface (artifact links, final result text).
#[derive(Debug, Clone, Default)]
pub struct WorkflowHistoryExtras {
    pub result_summary: Option<String>,
    pub source_path: Option<PathBuf>,
    pub spillover_path: Option<PathBuf>,
    pub verification_summary: Option<String>,
}

impl WorkflowPanel {
    #[must_use]
    pub fn new(run_id: impl Into<String>, label: impl Into<String>, at_ms: u64) -> Self {
        Self {
            run_id: run_id.into(),
            label: label.into(),
            lifecycle: WorkflowPanelLifecycle::Running,
            expanded: true, // auto-expand while running
            keyboard_focus: false,
            phases: Vec::new(),
            selected_phase: 0,
            budget_total: None,
            budget_spent: 0,
            budget_remaining: None,
            started_at_ms: at_ms,
            completed_at_ms: None,
            error: None,
            cancel_requested: false,
            result_summary: None,
            source_path: None,
            spillover_path: None,
            cancel_emit: None,
        }
    }

    /// Hydrate panel state from a workflow tool JSON payload (run record or a
    /// snapshot produced by [`Self::to_run_json`]). Prefers the typed `events`
    /// array when present; falls back to summary + phase fields.
    #[must_use]
    pub fn from_run_json(value: &Value) -> Option<Self> {
        if value.get("action").and_then(Value::as_str) == Some("status") {
            return None;
        }
        let run_id = value
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?
            .to_string();
        let label = value
            .get("workflow_goal")
            .and_then(Value::as_str)
            .or_else(|| value.get("workflow_id").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&run_id)
            .to_string();
        let at_ms = value
            .get("started_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut panel = Self::new(run_id.clone(), label.clone(), at_ms);

        if let Some(events) = value.get("events").and_then(Value::as_array) {
            for event in events {
                let mut event = event.clone();
                if let Some(obj) = event.as_object_mut() {
                    obj.entry("run_id".to_string())
                        .or_insert_with(|| Value::String(run_id.clone()));
                }
                panel.apply_json_event(&event);
            }
        } else if let Some(phases) = value.get("phases").and_then(Value::as_array) {
            for phase_val in phases {
                let title = phase_val
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Work");
                panel.phases.push(WorkflowPanelPhase::new(title));
                let phase_idx = panel.phases.len() - 1;
                if let Some(rows) = phase_val.get("rows").and_then(Value::as_array) {
                    for row in rows {
                        let task_id = row
                            .get("task_id")
                            .and_then(Value::as_str)
                            .unwrap_or("task")
                            .to_string();
                        let status = row
                            .get("status")
                            .and_then(Value::as_str)
                            .map(WorkflowRowStatus::from_ir_status)
                            .unwrap_or(WorkflowRowStatus::Pending);
                        panel.phases[phase_idx].rows.push(WorkflowPanelRow {
                            task_id: task_id.clone(),
                            label: row
                                .get("label")
                                .and_then(Value::as_str)
                                .unwrap_or(&task_id)
                                .to_string(),
                            profile: opt_str(row, "profile"),
                            model: opt_str(row, "model"),
                            strength: opt_str(row, "strength"),
                            worktree: row
                                .get("worktree")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            workspace: opt_str(row, "workspace").map(PathBuf::from),
                            status,
                            started_at_ms: row
                                .get("started_at_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or(at_ms),
                            completed_at_ms: row.get("completed_at_ms").and_then(Value::as_u64),
                            error: opt_str(row, "error"),
                            schema_error: opt_str(row, "schema_error"),
                        });
                    }
                }
            }
            if !panel.phases.is_empty() {
                panel.selected_phase = panel.phases.len() - 1;
            }
        } else if let Some(child_count) =
            value
                .get("child_count")
                .and_then(Value::as_u64)
                .or_else(|| {
                    value
                        .get("child_ids")
                        .and_then(Value::as_array)
                        .map(|a| a.len() as u64)
                })
        {
            // Bare summary without events: synthesize a Work phase so child
            // count still surfaces on the history card.
            if child_count > 0 {
                let mut phase = WorkflowPanelPhase::new("Work");
                for i in 0..child_count {
                    let id = value
                        .get("child_ids")
                        .and_then(Value::as_array)
                        .and_then(|ids| ids.get(i as usize))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("child-{i}"));
                    phase.rows.push(WorkflowPanelRow {
                        task_id: id.clone(),
                        label: id,
                        profile: None,
                        model: None,
                        strength: None,
                        worktree: false,
                        workspace: None,
                        status: WorkflowRowStatus::Succeeded,
                        started_at_ms: at_ms,
                        completed_at_ms: value.get("completed_at_ms").and_then(Value::as_u64),
                        error: None,
                        schema_error: None,
                    });
                }
                panel.phases.push(phase);
            }
        }

        if let Some(status) = value.get("status").and_then(Value::as_str) {
            let life = lifecycle_from_status(status);
            if life.is_terminal() {
                panel.lifecycle = life;
                panel.completed_at_ms = value
                    .get("completed_at_ms")
                    .and_then(Value::as_u64)
                    .or(panel.completed_at_ms);
            } else if panel.lifecycle.is_running() {
                panel.lifecycle = life;
            }
        }
        if let Some(error) = opt_str(value, "error") {
            panel.error = Some(error);
        }
        if let Some(spent) = value.get("budget_spent").and_then(Value::as_u64) {
            panel.budget_spent = spent;
        }
        if let Some(total) = value
            .get("token_budget")
            .or_else(|| value.get("budget_total"))
            .and_then(Value::as_u64)
        {
            panel.budget_total = Some(total);
        }
        if let Some(remaining) = value.get("budget_remaining").and_then(Value::as_u64) {
            panel.budget_remaining = Some(remaining);
        }
        // Apply extras after events so RunStarted reset does not wipe them.
        if panel.source_path.is_none() {
            panel.source_path = opt_str(value, "source_path").map(PathBuf::from);
        }
        if panel.result_summary.is_none() {
            panel.result_summary = value
                .get("result")
                .and_then(summarize_result_value)
                .or_else(|| opt_str(value, "result_summary"));
        }
        if let Some(verification) = value.get("verification")
            && let Some(summary) = verification.get("summary").and_then(Value::as_str)
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                panel.result_summary = Some(match panel.result_summary.take() {
                    Some(existing) => format!("{existing} · verify: {trimmed}"),
                    None => format!("verify: {trimmed}"),
                });
            }
        }
        // Prefer the goal label from the payload when events used a fallback.
        if !label.is_empty() && panel.label == run_id {
            panel.label = label;
        }
        Some(panel)
    }

    /// Snapshot panel state into a JSON blob suitable for the history cell
    /// (and re-hydration via [`Self::from_run_json`]).
    #[must_use]
    pub fn to_run_json(&self) -> Value {
        let status = match self.lifecycle {
            WorkflowPanelLifecycle::Pending => "pending",
            WorkflowPanelLifecycle::Running => "running",
            WorkflowPanelLifecycle::Succeeded => "completed",
            WorkflowPanelLifecycle::Failed => "failed",
            WorkflowPanelLifecycle::Cancelled => "cancelled",
        };
        let (done, total) = self.done_total();
        let (failed, cancelled) = self.failure_cancel_counts();
        json!({
            "run_id": self.run_id,
            "status": status,
            "workflow_goal": self.label,
            "started_at_ms": self.started_at_ms,
            "completed_at_ms": self.completed_at_ms,
            "child_count": total,
            "done_count": done,
            "phase_count": self.phase_count(),
            "failure_count": failed,
            "cancel_count": cancelled,
            "error": self.error,
            "result_summary": self.result_summary,
            "source_path": self.source_path.as_ref().map(|p| p.display().to_string()),
            "spillover_path": self.spillover_path.as_ref().map(|p| p.display().to_string()),
            "token_budget": self.budget_total,
            "budget_spent": self.budget_spent,
            "budget_remaining": self.budget_remaining,
            "phases": self.phases.iter().map(|phase| {
                json!({
                    "title": phase.title,
                    "rows": phase.rows.iter().map(|row| {
                        json!({
                            "task_id": row.task_id,
                            "label": row.label,
                            "profile": row.profile,
                            "model": row.model,
                            "strength": row.strength,
                            "worktree": row.worktree,
                            "workspace": row.workspace.as_ref().map(|p| p.display().to_string()),
                            "status": row.status.label(),
                            "started_at_ms": row.started_at_ms,
                            "completed_at_ms": row.completed_at_ms,
                            "error": row.error,
                            "schema_error": row.schema_error,
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }

    /// Compact one-line history-card summary: lifecycle, children, phases,
    /// failures, elapsed (#4122 AC). The free-text goal lives on the expanded
    /// body so the fixed header summary budget (≈56 cols) never drops counts.
    #[must_use]
    pub fn compact_summary_text(&self, width: usize) -> String {
        let (_done, total) = self.done_total();
        let (failed, _cancelled) = self.failure_cancel_counts();
        let phases = self.phase_count();
        let elapsed = self.elapsed_label();
        let child_word = if total == 1 { "child" } else { "children" };
        let phase_word = if phases == 1 { "phase" } else { "phases" };
        let raw = format!(
            "workflow {life} · {total} {child_word} · {phases} {phase_word} · {failed} fail · {elapsed}",
            life = self.lifecycle.label(),
        );
        truncate_line_to_width(&raw, width.max(1))
    }

    /// Elapsed label shared with direct sub-agent cards.
    #[must_use]
    pub fn elapsed_label(&self) -> String {
        // Guard against epoch-zero starts (bare status payloads without
        // timestamps) which would otherwise render multi-year elapsed times.
        if self.started_at_ms == 0 {
            if let Some(completed) = self.completed_at_ms {
                return format_elapsed(completed);
            }
            return "0s".to_string();
        }
        let end = self.completed_at_ms.unwrap_or_else(now_ms);
        format_elapsed(end.saturating_sub(self.started_at_ms))
    }

    /// Compact summary line content (without card chrome). Callers in
    /// `history.rs` wrap this with the shared tool-header + rail.
    #[must_use]
    pub fn history_header_summary(&self, width: usize) -> String {
        self.compact_summary_text(width)
    }

    /// Expanded history-card body lines (phase/child summaries, links,
    /// result, failures). Empty when the card should stay compact.
    #[must_use]
    pub fn history_expanded_lines(
        &self,
        width: u16,
        extras: &WorkflowHistoryExtras,
    ) -> Vec<Line<'static>> {
        let content_width = usize::from(width).max(1);
        let mut lines = Vec::new();

        if !self.label.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    &format!("goal: {}", short_label(self.label.trim(), 160)),
                    content_width,
                ),
                Style::default().fg(palette::TEXT_TOOL_OUTPUT),
            )));
        }

        // Phase summary strip (same chips as the panel body).
        if !self.phases.is_empty() {
            let mut chips = Vec::new();
            for (idx, phase) in self.phases.iter().take(MAX_PHASE_SUMMARY).enumerate() {
                let (done, running, failed, cancelled) = phase.counts();
                let marker = if idx == self.selected_phase { ">" } else { " " };
                chips.push(format!(
                    "{marker}{title}[{done}✓ {running}… {failed}! {cancelled}⊘]",
                    title = short_label(&phase.title, 14),
                ));
            }
            if self.phases.len() > MAX_PHASE_SUMMARY {
                chips.push(format!("+{}", self.phases.len() - MAX_PHASE_SUMMARY));
            }
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&format!("phases: {}", chips.join("  ")), content_width),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }

        // Child summary across all phases.
        let children: Vec<String> = self
            .phases
            .iter()
            .flat_map(|p| p.rows.iter())
            .take(8)
            .map(|row| {
                format!(
                    "{label} ({status})",
                    label = short_label(&row.label, 16),
                    status = row.status.label()
                )
            })
            .collect();
        if !children.is_empty() {
            let more = self
                .phases
                .iter()
                .map(|p| p.rows.len())
                .sum::<usize>()
                .saturating_sub(children.len());
            let mut body = children.join(" · ");
            if more > 0 {
                body = format!("{body} · +{more} more");
            }
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&format!("children: {body}"), content_width),
                Style::default().fg(palette::TEXT_TOOL_OUTPUT),
            )));
        }

        let result = extras
            .result_summary
            .as_deref()
            .or(self.result_summary.as_deref())
            .or(extras.verification_summary.as_deref());
        if let Some(result) = result.filter(|s| !s.trim().is_empty()) {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    &format!("result: {}", short_label(result.trim(), 160)),
                    content_width,
                ),
                Style::default().fg(palette::TEXT_TOOL_OUTPUT),
            )));
        }

        let source = extras
            .source_path
            .as_ref()
            .or(self.source_path.as_ref())
            .map(|p| p.display().to_string());
        if let Some(path) = source.filter(|s| !s.is_empty()) {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&format!("source: {path}"), content_width),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        let spill = extras
            .spillover_path
            .as_ref()
            .or(self.spillover_path.as_ref())
            .map(|p| p.display().to_string());
        if let Some(path) = spill.filter(|s| !s.is_empty()) {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&format!("artifact: {path}"), content_width),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        } else if self.lifecycle.is_terminal() {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    "transcript: full run JSON available via tool details (v)",
                    content_width,
                ),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }

        if let Some(error) = self.error.as_deref().filter(|s| !s.trim().is_empty()) {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    &format!("error: {}", short_label(error, 160)),
                    content_width,
                ),
                Style::default().fg(palette::STATUS_ERROR),
            )));
        }
        for row in self.phases.iter().flat_map(|p| p.rows.iter()) {
            if let Some(schema) = row.schema_error.as_deref() {
                lines.push(Line::from(Span::styled(
                    truncate_line_to_width(
                        &format!(
                            "schema {}: {}",
                            short_label(&row.task_id, 12),
                            short_label(schema, 120)
                        ),
                        content_width,
                    ),
                    Style::default().fg(palette::STATUS_ERROR),
                )));
            } else if row.status.is_failure()
                && let Some(err) = row.error.as_deref()
            {
                lines.push(Line::from(Span::styled(
                    truncate_line_to_width(
                        &format!(
                            "fail {}: {}",
                            short_label(&row.label, 14),
                            short_label(err, 120)
                        ),
                        content_width,
                    ),
                    Style::default().fg(palette::STATUS_ERROR),
                )));
            }
        }

        lines
    }

    /// Full history-card lines including a simple self-contained header so
    /// unit tests (and direct sub-agent cards) can render without history.rs.
    ///
    /// Public convergence API for #4122 — also exercised by unit tests and
    /// `DelegateCard::as_workflow_history_panel`.
    #[must_use]
    #[allow(dead_code)] // public API used by direct sub-agent projection + tests
    pub fn render_history_card(
        &self,
        width: u16,
        expanded: bool,
        extras: &WorkflowHistoryExtras,
    ) -> Vec<Line<'static>> {
        let content_width = usize::from(width).max(1);
        let mut lines = Vec::new();
        let glyph = if expanded { '▼' } else { '▶' };
        let summary = self.compact_summary_text(content_width.saturating_sub(2));
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&format!("{glyph} {summary}"), content_width),
            Style::default()
                .fg(self.lifecycle.color())
                .add_modifier(Modifier::BOLD),
        )));
        if expanded {
            lines.extend(self.history_expanded_lines(width, extras));
        }
        lines
    }

    /// Single-agent "mini workflow" view for direct sub-agent cards so they
    /// share the same lifecycle/elapsed/result concepts as workflow runs.
    #[must_use]
    #[allow(dead_code)] // public API used by DelegateCard + tests
    pub fn from_direct_subagent(
        agent_id: impl Into<String>,
        role: impl Into<String>,
        lifecycle: WorkflowPanelLifecycle,
        started_at_ms: u64,
        completed_at_ms: Option<u64>,
        summary: Option<String>,
        error: Option<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let role = role.into();
        let mut panel = Self::new(agent_id.clone(), role.clone(), started_at_ms);
        panel.lifecycle = lifecycle;
        panel.completed_at_ms = completed_at_ms;
        panel.expanded = false;
        panel.result_summary = summary.clone();
        panel.error = error.clone();
        let status = match lifecycle {
            WorkflowPanelLifecycle::Pending => WorkflowRowStatus::Pending,
            WorkflowPanelLifecycle::Running => WorkflowRowStatus::Running,
            WorkflowPanelLifecycle::Succeeded => WorkflowRowStatus::Succeeded,
            WorkflowPanelLifecycle::Failed => WorkflowRowStatus::Failed,
            WorkflowPanelLifecycle::Cancelled => WorkflowRowStatus::Cancelled,
        };
        let mut phase = WorkflowPanelPhase::new("Agent");
        phase.rows.push(WorkflowPanelRow {
            task_id: agent_id,
            label: role,
            profile: None,
            model: None,
            strength: None,
            worktree: false,
            workspace: None,
            status,
            started_at_ms,
            completed_at_ms,
            error,
            schema_error: None,
        });
        panel.phases.push(phase);
        panel
    }

    /// Apply a stream of events. `RunStarted` replaces any prior completed run.
    pub fn apply_event(&mut self, event: WorkflowPanelEvent) {
        match event {
            WorkflowPanelEvent::RunStarted {
                run_id,
                workflow_id,
                workflow_goal,
                source_path,
                token_budget,
                at_ms,
            } => {
                // New run replaces preserved completed state.
                *self = Self::new(
                    run_id,
                    workflow_goal
                        .or(workflow_id)
                        .unwrap_or_else(|| "workflow".to_string()),
                    at_ms,
                );
                self.budget_total = token_budget;
                self.budget_remaining = token_budget;
                self.source_path = source_path;
            }
            WorkflowPanelEvent::RunCompleted {
                status,
                error,
                at_ms,
            } => {
                self.lifecycle = if matches!(status, WorkflowPanelLifecycle::Running) {
                    WorkflowPanelLifecycle::Succeeded
                } else {
                    status
                };
                self.error = error;
                self.completed_at_ms = Some(at_ms);
                // Preserve expanded/collapsed choice; do not auto-hide.
            }
            WorkflowPanelEvent::RunCancelled { reason, at_ms } => {
                self.finalize_running_rows(WorkflowRowStatus::Cancelled, at_ms);
                self.lifecycle = WorkflowPanelLifecycle::Cancelled;
                self.error = Some(reason);
                self.completed_at_ms = Some(at_ms);
            }
            WorkflowPanelEvent::PhaseStarted { title, at_ms: _ } => {
                if self.phases.last().is_some_and(|phase| phase.title == title) {
                    return;
                }
                self.phases.push(WorkflowPanelPhase::new(title));
                self.selected_phase = self.phases.len().saturating_sub(1);
                if self.lifecycle.is_running() {
                    self.expanded = true;
                }
            }
            WorkflowPanelEvent::TaskStarted {
                task_id,
                label,
                profile,
                model,
                strength,
                resolved_model,
                worktree,
                workspace,
                at_ms,
            } => {
                if self.phases.is_empty() {
                    self.phases.push(WorkflowPanelPhase::new("Work"));
                    self.selected_phase = 0;
                }
                let phase_idx = self.selected_phase.min(self.phases.len().saturating_sub(1));
                let display_model = resolved_model.or(model);
                let row = WorkflowPanelRow {
                    task_id: task_id.clone(),
                    label: label
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| task_id.clone()),
                    profile,
                    model: display_model,
                    strength,
                    worktree,
                    workspace,
                    status: WorkflowRowStatus::Running,
                    started_at_ms: at_ms,
                    completed_at_ms: None,
                    error: None,
                    schema_error: None,
                };
                if let Some(existing) = self.find_row_mut(&task_id) {
                    *existing = row;
                } else if let Some(phase) = self.phases.get_mut(phase_idx) {
                    phase.rows.push(row);
                }
                self.lifecycle = WorkflowPanelLifecycle::Running;
                self.expanded = true;
            }
            WorkflowPanelEvent::TaskCompleted {
                task_id,
                status,
                at_ms,
            } => {
                if let Some(row) = self.find_row_mut(&task_id) {
                    row.status = status;
                    row.completed_at_ms = Some(at_ms);
                }
            }
            WorkflowPanelEvent::TaskSchemaValidationFailed {
                task_id,
                message,
                at_ms,
            } => {
                if let Some(row) = self.find_row_mut(&task_id) {
                    row.status = WorkflowRowStatus::SchemaFailed;
                    row.schema_error = Some(message);
                    row.completed_at_ms = Some(at_ms);
                } else {
                    // Schema can fire before/without a started task.
                    if self.phases.is_empty() {
                        self.phases.push(WorkflowPanelPhase::new("Work"));
                    }
                    let phase_idx = self.selected_phase.min(self.phases.len().saturating_sub(1));
                    if let Some(phase) = self.phases.get_mut(phase_idx) {
                        phase.rows.push(WorkflowPanelRow {
                            task_id,
                            label: "schema".to_string(),
                            profile: None,
                            model: None,
                            strength: None,
                            worktree: false,
                            workspace: None,
                            status: WorkflowRowStatus::SchemaFailed,
                            started_at_ms: at_ms,
                            completed_at_ms: Some(at_ms),
                            error: None,
                            schema_error: Some(message),
                        });
                    }
                }
            }
            WorkflowPanelEvent::BudgetUpdated {
                total,
                spent,
                remaining,
                at_ms: _,
            } => {
                if total.is_some() {
                    self.budget_total = total;
                }
                self.budget_spent = spent;
                self.budget_remaining = remaining;
            }
        }
    }

    pub fn apply_json_event(&mut self, value: &Value) {
        if let Some(event) = WorkflowPanelEvent::from_json_value(value) {
            self.apply_event(event);
        }
    }

    pub fn apply_json_events(&mut self, values: &[Value]) {
        for value in values {
            self.apply_json_event(value);
        }
    }

    #[must_use]
    pub fn toggle_expanded(&mut self) -> bool {
        self.expanded = !self.expanded;
        true
    }

    pub fn select_next_phase(&mut self) {
        if self.phases.is_empty() {
            return;
        }
        self.selected_phase = (self.selected_phase + 1) % self.phases.len();
    }

    pub fn select_prev_phase(&mut self) {
        if self.phases.is_empty() {
            return;
        }
        self.selected_phase = self
            .selected_phase
            .checked_sub(1)
            .unwrap_or(self.phases.len() - 1);
    }

    /// Request cancel from the panel. Returns the run id when a cancel should
    /// be dispatched to the workflow tool. Running children stay running until
    /// the host interrupt/cancel path finalizes them (or a terminal event
    /// arrives).
    pub fn request_cancel(&mut self) -> Option<String> {
        if !self.lifecycle.is_running() || self.cancel_requested {
            return None;
        }
        self.cancel_requested = true;
        self.cancel_emit = Some(self.run_id.clone());
        self.cancel_emit.clone()
    }

    /// Take a pending cancel emit (run_id) once.
    pub fn take_cancel_emit(&mut self) -> Option<String> {
        self.cancel_emit.take()
    }

    /// Interrupt finalizes every still-running child as cancelled and marks
    /// the run cancelled. Preserves the panel until the next workflow starts.
    pub fn finalize_interrupt(&mut self) {
        if self.lifecycle.is_terminal() {
            return;
        }
        let at = now_ms();
        self.finalize_running_rows(WorkflowRowStatus::Cancelled, at);
        self.lifecycle = WorkflowPanelLifecycle::Cancelled;
        self.completed_at_ms = Some(at);
        if self.error.is_none() {
            self.error = Some("interrupted".to_string());
        }
    }

    /// Handle a key while the panel has keyboard focus.
    /// Returns true when the key was consumed.
    pub fn handle_key(&mut self, ch: char) -> bool {
        if !self.keyboard_focus {
            return false;
        }
        match ch {
            't' | 'T' | ' ' => self.toggle_expanded(),
            'c' | 'C' | 'x' | 'X' => self.request_cancel().is_some(),
            'n' | 'N' | 'j' | 'J' => {
                self.select_next_phase();
                true
            }
            'p' | 'P' | 'k' | 'K' => {
                self.select_prev_phase();
                true
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn done_total(&self) -> (usize, usize) {
        let mut done = 0usize;
        let mut total = 0usize;
        for phase in &self.phases {
            for row in &phase.rows {
                total += 1;
                if !row.status.is_running() {
                    done += 1;
                }
            }
        }
        (done, total)
    }

    #[must_use]
    pub fn phase_count(&self) -> usize {
        self.phases.len()
    }

    #[must_use]
    pub fn failure_cancel_counts(&self) -> (usize, usize) {
        let mut failed = 0usize;
        let mut cancelled = 0usize;
        for phase in &self.phases {
            for row in &phase.rows {
                if row.status.is_failure() {
                    failed += 1;
                } else if row.status.is_cancel() {
                    cancelled += 1;
                }
            }
        }
        (failed, cancelled)
    }

    /// Header line: expand glyph, lifecycle, label, done/total, phases,
    /// fail/cancel counts, budget spent/remaining.
    #[must_use]
    pub fn header_text(&self, width: usize) -> String {
        let glyph = if self.expanded { '▼' } else { '▶' };
        let (done, total) = self.done_total();
        let (failed, cancelled) = self.failure_cancel_counts();
        let phases = self.phase_count();
        let budget = match (self.budget_spent, self.budget_remaining, self.budget_total) {
            (spent, Some(remaining), _) => format!(" budget {spent}/{remaining} left"),
            (spent, None, Some(total)) => format!(" budget {spent}/{total}"),
            (spent, None, None) if spent > 0 => format!(" budget {spent}"),
            _ => String::new(),
        };
        let cancel_hint = if self.lifecycle.is_running() {
            if self.cancel_requested {
                " · cancelling…"
            } else {
                " · [c] cancel"
            }
        } else {
            ""
        };
        let elapsed = {
            let end = self.completed_at_ms.unwrap_or_else(now_ms);
            format_elapsed(end.saturating_sub(self.started_at_ms))
        };
        let focus = if self.keyboard_focus { "*" } else { "" };
        let raw = format!(
            "{glyph}{focus} workflow {life} · {label} · {done}/{total} · {phases} phases · {failed} fail · {cancelled} cancel · {elapsed}{budget}{cancel_hint}",
            life = self.lifecycle.label(),
            label = self.label,
        );
        truncate_line_to_width(&raw, width.max(1))
    }

    #[must_use]
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let content_width = usize::from(width).max(1);
        let mut lines = Vec::with_capacity(12);
        lines.push(Line::from(Span::styled(
            self.header_text(content_width),
            Style::default()
                .fg(self.lifecycle.color())
                .add_modifier(Modifier::BOLD),
        )));

        if !self.expanded {
            return lines;
        }

        // Phase summary strip.
        if !self.phases.is_empty() {
            let mut chips = Vec::new();
            for (idx, phase) in self.phases.iter().take(MAX_PHASE_SUMMARY).enumerate() {
                let (done, running, failed, cancelled) = phase.counts();
                let marker = if idx == self.selected_phase { ">" } else { " " };
                chips.push(format!(
                    "{marker}{title}[{done}✓ {running}… {failed}! {cancelled}⊘]",
                    title = short_label(&phase.title, 14),
                ));
            }
            if self.phases.len() > MAX_PHASE_SUMMARY {
                chips.push(format!("+{}", self.phases.len() - MAX_PHASE_SUMMARY));
            }
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&chips.join("  "), content_width),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }

        // Selected phase rows.
        if let Some(phase) = self.phases.get(self.selected_phase) {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    &format!("phase: {} ({} rows)", phase.title, phase.rows.len()),
                    content_width,
                ),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )));

            let now = now_ms();
            let shown = phase.rows.len().min(MAX_VISIBLE_ROWS);
            for row in phase.rows.iter().take(shown) {
                lines.push(self.render_row_line(row, content_width, now));
            }
            if phase.rows.len() > shown {
                lines.push(Line::from(Span::styled(
                    format!("  … {} more", phase.rows.len() - shown),
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
        } else if self.lifecycle.is_running() {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width("waiting for phases…", content_width),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }

        if let Some(error) = self.error.as_deref() {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&format!("error: {error}"), content_width),
                Style::default().fg(palette::STATUS_ERROR),
            )));
        }

        if self.keyboard_focus {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(
                    "[t] toggle  [c] cancel  [j/k] phase  click header to toggle",
                    content_width,
                ),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::ITALIC),
            )));
        }

        lines
    }

    fn render_row_line(&self, row: &WorkflowPanelRow, width: usize, now_ms: u64) -> Line<'static> {
        let elapsed_ms = row
            .completed_at_ms
            .unwrap_or(now_ms)
            .saturating_sub(row.started_at_ms);
        let elapsed = format_elapsed(elapsed_ms);
        let role = row.profile.as_deref().unwrap_or("-");
        let model = match (row.model.as_deref(), row.strength.as_deref()) {
            (Some(m), Some(s)) => format!("{m}/{s}"),
            (Some(m), None) => m.to_string(),
            (None, Some(s)) => s.to_string(),
            (None, None) => "-".to_string(),
        };
        let worktree = if row.worktree { "wt" } else { "main" };
        let schema = row
            .schema_error
            .as_deref()
            .or(row.error.as_deref())
            .map(|e| format!(" !{}", short_label(e, 24)))
            .unwrap_or_default();
        let text = format!(
            "  {status:<9} {label} · {role} · {model} · {worktree} · {elapsed}{schema}",
            status = row.status.label(),
            label = short_label(&row.label, 18),
        );
        Line::from(Span::styled(
            truncate_line_to_width(&text, width),
            Style::default().fg(row.status.color()),
        ))
    }

    fn find_row_mut(&mut self, task_id: &str) -> Option<&mut WorkflowPanelRow> {
        for phase in &mut self.phases {
            if let Some(row) = phase.rows.iter_mut().find(|r| r.task_id == task_id) {
                return Some(row);
            }
        }
        None
    }

    fn finalize_running_rows(&mut self, status: WorkflowRowStatus, at_ms: u64) {
        for phase in &mut self.phases {
            for row in &mut phase.rows {
                if row.status.is_running() {
                    row.status = status;
                    row.completed_at_ms = Some(at_ms);
                }
            }
        }
    }
}

impl Renderable for WorkflowPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let lines = self.render_lines(area.width);
        let paragraph = Paragraph::new(lines);
        paragraph.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if width == 0 {
            return 0;
        }
        self.render_lines(width).len() as u16
    }
}

fn lifecycle_from_status(status: &str) -> WorkflowPanelLifecycle {
    match status {
        "running" => WorkflowPanelLifecycle::Running,
        "completed" | "succeeded" | "success" => WorkflowPanelLifecycle::Succeeded,
        "failed" | "error" => WorkflowPanelLifecycle::Failed,
        "cancelled" | "canceled" => WorkflowPanelLifecycle::Cancelled,
        "pending" => WorkflowPanelLifecycle::Pending,
        _ => WorkflowPanelLifecycle::Failed,
    }
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn short_label(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.width() <= max {
        return trimmed.to_string();
    }
    truncate_line_to_width(trimmed, max)
}

/// Format an elapsed duration for panel headers and history cards. Shared with
/// direct sub-agent cards so both surfaces use the same vocabulary.
#[must_use]
pub fn format_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn summarize_result_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(short_label(t, 200))
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(items) => Some(format!("{} item(s)", items.len())),
        Value::Object(map) => {
            if let Some(s) = map
                .get("summary")
                .or_else(|| map.get("message"))
                .or_else(|| map.get("text"))
                .and_then(Value::as_str)
            {
                let t = s.trim();
                if !t.is_empty() {
                    return Some(short_label(t, 200));
                }
            }
            Some(format!("{} field(s)", map.len()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn started_panel() -> WorkflowPanel {
        let mut panel = WorkflowPanel::new("workflow_abc", "ship v0.8.68", 1_000);
        panel.apply_event(WorkflowPanelEvent::PhaseStarted {
            title: "Analyze".to_string(),
            at_ms: 1_100,
        });
        panel.apply_event(WorkflowPanelEvent::TaskStarted {
            task_id: "t1".to_string(),
            label: Some("scout crates".to_string()),
            profile: Some("explore".to_string()),
            model: Some("flash".to_string()),
            strength: Some("low".to_string()),
            resolved_model: Some("deepseek-v4-flash".to_string()),
            worktree: true,
            workspace: Some(PathBuf::from("/tmp/wt-1")),
            at_ms: 1_200,
        });
        panel
    }

    #[test]
    fn header_shows_lifecycle_counts_budget_and_expand_glyph() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::BudgetUpdated {
            total: Some(10_000),
            spent: 1_200,
            remaining: Some(8_800),
            at_ms: 1_300,
        });
        let header = panel.header_text(120);
        assert!(header.contains('▼'), "running auto-expands: {header}");
        assert!(header.contains("running"), "{header}");
        assert!(header.contains("ship v0.8.68"), "{header}");
        assert!(header.contains("0/1"), "{header}");
        assert!(header.contains("1 phases"), "{header}");
        assert!(header.contains("0 fail"), "{header}");
        assert!(header.contains("0 cancel"), "{header}");
        assert!(
            header.contains("budget 1200/8800 left") || header.contains("budget 1"),
            "{header}"
        );
    }

    #[test]
    fn body_shows_phases_and_selected_phase_rows() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::PhaseStarted {
            title: "Verify".to_string(),
            at_ms: 2_000,
        });
        panel.apply_event(WorkflowPanelEvent::TaskStarted {
            task_id: "t2".to_string(),
            label: Some("run tests".to_string()),
            profile: Some("implementer".to_string()),
            model: Some("pro".to_string()),
            strength: None,
            resolved_model: None,
            worktree: false,
            workspace: None,
            at_ms: 2_100,
        });
        // selected phase is Verify (latest)
        let lines = panel.render_lines(100);
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let joined = text.join("\n");
        assert!(joined.contains("Analyze"), "{joined}");
        assert!(joined.contains("Verify"), "{joined}");
        assert!(joined.contains("run tests"), "{joined}");
        assert!(joined.contains("implementer"), "{joined}");
        assert!(joined.contains("pro"), "{joined}");
        assert!(joined.contains("main"), "{joined}"); // no worktree
        // Analyze scout is not in selected phase body
        assert!(!joined.contains("scout crates"), "{joined}");
    }

    #[test]
    fn rows_show_status_label_role_model_worktree_elapsed_schema() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::TaskSchemaValidationFailed {
            task_id: "t1".to_string(),
            message: "missing field foo".to_string(),
            at_ms: 1_500,
        });
        let lines = panel.render_lines(120);
        let joined: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("schema"), "{joined}");
        assert!(joined.contains("scout crates"), "{joined}");
        assert!(joined.contains("explore"), "{joined}");
        assert!(joined.contains("deepseek-v4-flash"), "{joined}");
        assert!(joined.contains("wt"), "{joined}");
        assert!(joined.contains("missing field"), "{joined}");
    }

    #[test]
    fn auto_expands_while_running_and_preserves_completed_until_next() {
        let mut panel = started_panel();
        assert!(panel.expanded);
        panel.expanded = false;
        // Task start while running forces re-expand
        panel.apply_event(WorkflowPanelEvent::TaskStarted {
            task_id: "t3".to_string(),
            label: Some("more".to_string()),
            profile: None,
            model: None,
            strength: None,
            resolved_model: None,
            worktree: false,
            workspace: None,
            at_ms: 1_400,
        });
        assert!(panel.expanded);

        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t1".to_string(),
            status: WorkflowRowStatus::Succeeded,
            at_ms: 2_000,
        });
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t3".to_string(),
            status: WorkflowRowStatus::Succeeded,
            at_ms: 2_100,
        });
        panel.apply_event(WorkflowPanelEvent::RunCompleted {
            status: WorkflowPanelLifecycle::Succeeded,
            error: None,
            at_ms: 2_200,
        });
        assert_eq!(panel.lifecycle, WorkflowPanelLifecycle::Succeeded);
        // Still visible (preserved)
        assert_eq!(panel.run_id, "workflow_abc");
        let header = panel.header_text(80);
        assert!(header.contains("success"), "{header}");

        // Next workflow replaces
        panel.apply_event(WorkflowPanelEvent::RunStarted {
            run_id: "workflow_next".to_string(),
            workflow_id: None,
            workflow_goal: Some("next run".to_string()),
            source_path: None,
            token_budget: None,
            at_ms: 3_000,
        });
        assert_eq!(panel.run_id, "workflow_next");
        assert_eq!(panel.label, "next run");
        assert!(panel.phases.is_empty());
        assert!(panel.expanded);
        assert_eq!(panel.lifecycle, WorkflowPanelLifecycle::Running);
    }

    #[test]
    fn interrupt_finalizes_running_children_as_cancelled() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::TaskStarted {
            task_id: "t2".to_string(),
            label: Some("second".to_string()),
            profile: None,
            model: None,
            strength: None,
            resolved_model: None,
            worktree: false,
            workspace: None,
            at_ms: 1_300,
        });
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t1".to_string(),
            status: WorkflowRowStatus::Succeeded,
            at_ms: 1_400,
        });
        panel.finalize_interrupt();
        assert_eq!(panel.lifecycle, WorkflowPanelLifecycle::Cancelled);
        let t1 = panel
            .phases
            .iter()
            .flat_map(|p| p.rows.iter())
            .find(|r| r.task_id == "t1")
            .expect("t1");
        let t2 = panel
            .phases
            .iter()
            .flat_map(|p| p.rows.iter())
            .find(|r| r.task_id == "t2")
            .expect("t2");
        assert_eq!(t1.status, WorkflowRowStatus::Succeeded);
        assert_eq!(t2.status, WorkflowRowStatus::Cancelled);
        let (failed, cancelled) = panel.failure_cancel_counts();
        assert_eq!(failed, 0);
        assert_eq!(cancelled, 1);
    }

    #[test]
    fn keyboard_and_mouse_toggle_and_cancel() {
        let mut panel = started_panel();
        assert!(panel.expanded);
        assert!(panel.toggle_expanded());
        assert!(!panel.expanded);
        assert!(panel.toggle_expanded());
        assert!(panel.expanded);

        // Without focus, keys ignored
        assert!(!panel.handle_key('t'));

        panel.keyboard_focus = true;
        assert!(panel.handle_key('t'));
        assert!(!panel.expanded);

        let run_id = panel.request_cancel().expect("cancel");
        assert_eq!(run_id, "workflow_abc");
        assert!(panel.cancel_requested);
        // Second cancel is a no-op
        assert!(panel.request_cancel().is_none());
        assert_eq!(panel.take_cancel_emit().as_deref(), Some("workflow_abc"));
        assert!(panel.take_cancel_emit().is_none());
    }

    #[test]
    fn json_events_round_trip_without_log_flood() {
        let mut panel = WorkflowPanel::new("w1", "goal", 0);
        let events = vec![
            json!({
                "type": "run_started",
                "at_ms": 10,
                "run_id": "w1",
                "workflow_goal": "demo",
                "token_budget": 5000
            }),
            json!({"type": "log", "at_ms": 11, "message": "should not appear"}),
            json!({"type": "phase_started", "at_ms": 12, "title": "Analyze"}),
            json!({
                "type": "task_started",
                "at_ms": 13,
                "task_id": "a",
                "label": "scout",
                "profile": "explore",
                "resolved_model": "flash",
                "worktree": true
            }),
            json!({
                "type": "budget_updated",
                "at_ms": 14,
                "total": 5000,
                "spent": 100,
                "remaining": 4900
            }),
            json!({
                "type": "task_completed",
                "at_ms": 15,
                "task_id": "a",
                "status": "succeeded"
            }),
            json!({
                "type": "run_completed",
                "at_ms": 16,
                "status": "completed"
            }),
        ];
        panel.apply_json_events(&events);
        assert_eq!(panel.label, "demo");
        assert_eq!(panel.lifecycle, WorkflowPanelLifecycle::Succeeded);
        assert_eq!(panel.budget_spent, 100);
        assert_eq!(panel.budget_remaining, Some(4900));
        let joined: String = panel
            .render_lines(100)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains("should not appear"), "{joined}");
        assert!(joined.contains("scout"), "{joined}");
        assert!(joined.contains("done"), "{joined}");
    }

    #[test]
    fn desired_height_is_zero_width_safe_and_collapsed_is_one() {
        let mut panel = started_panel();
        assert_eq!(panel.desired_height(0), 0);
        panel.expanded = false;
        assert_eq!(panel.desired_height(80), 1);
        panel.expanded = true;
        assert!(panel.desired_height(80) >= 3);
    }

    #[test]
    fn failure_and_cancel_counts_roll_up_in_header() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::TaskStarted {
            task_id: "t2".to_string(),
            label: Some("b".to_string()),
            profile: None,
            model: None,
            strength: None,
            resolved_model: None,
            worktree: false,
            workspace: None,
            at_ms: 1_300,
        });
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t1".to_string(),
            status: WorkflowRowStatus::Failed,
            at_ms: 1_400,
        });
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t2".to_string(),
            status: WorkflowRowStatus::Cancelled,
            at_ms: 1_500,
        });
        let (failed, cancelled) = panel.failure_cancel_counts();
        assert_eq!(failed, 1);
        assert_eq!(cancelled, 1);
        let header = panel.header_text(100);
        assert!(header.contains("1 fail"), "{header}");
        assert!(header.contains("1 cancel"), "{header}");
        assert!(header.contains("2/2"), "{header}");
    }

    #[test]
    fn task_started_json_prefers_workflow_task_label_over_generic_label() {
        // #4119: panel rows use typed workflow metadata, not prompt text.
        let event = WorkflowPanelEvent::from_json_value(&json!({
            "type": "task_started",
            "task_id": "child-1",
            "label": "fallback-label",
            "workflow_task_label": "typed-label",
            "workflow_run_id": "run-xyz",
            "workflow_phase_id": "dispatch",
            "workflow_child_index": 2,
            "at_ms": 42,
        }))
        .expect("task_started parses");
        match event {
            WorkflowPanelEvent::TaskStarted { label, .. } => {
                assert_eq!(label.as_deref(), Some("typed-label"));
            }
            other => panic!("expected TaskStarted, got {other:?}"),
        }

        let mut panel = WorkflowPanel::new("run-xyz", "goal", 1);
        panel.apply_json_event(&json!({
            "type": "task_started",
            "task_id": "child-1",
            "label": "fallback-label",
            "workflow_task_label": "typed-label",
            "at_ms": 42,
        }));
        let row = panel
            .phases
            .iter()
            .flat_map(|phase| phase.rows.iter())
            .find(|row| row.task_id == "child-1")
            .expect("row recorded");
        assert_eq!(row.label, "typed-label");
    }

    #[test]
    fn compact_history_card_summarizes_lifecycle_children_phases_failures_elapsed() {
        let mut panel = started_panel();
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t1".to_string(),
            status: WorkflowRowStatus::Failed,
            at_ms: 2_000,
        });
        panel.apply_event(WorkflowPanelEvent::RunCompleted {
            status: WorkflowPanelLifecycle::Failed,
            error: Some("scout failed".to_string()),
            at_ms: 2_100,
        });
        let lines = panel.render_history_card(120, false, &WorkflowHistoryExtras::default());
        assert_eq!(lines.len(), 1, "compact is a single summary line");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains('▶'), "collapsed glyph: {joined}");
        assert!(
            joined.contains("failed") || joined.contains("fail"),
            "{joined}"
        );
        assert!(joined.contains("1 child"), "{joined}");
        assert!(joined.contains("1 phase"), "{joined}");
        assert!(joined.contains("1 fail"), "{joined}");
        // elapsed is present (0s or more depending on timestamps)
        assert!(
            joined.contains('s') || joined.contains('m'),
            "elapsed time expected: {joined}"
        );
        // Goal is reserved for the expanded body so compact stays under the
        // tool-header summary budget.
        assert!(
            !joined.contains("ship v0.8.68"),
            "compact must not spend budget on free-text goal: {joined}"
        );
    }

    #[test]
    fn expanded_history_card_shows_phase_child_result_links_and_failures() {
        let mut panel = started_panel();
        panel.source_path = Some(PathBuf::from("workflows/demo.workflow.js"));
        panel.apply_event(WorkflowPanelEvent::TaskCompleted {
            task_id: "t1".to_string(),
            status: WorkflowRowStatus::Failed,
            at_ms: 2_000,
        });
        if let Some(row) = panel.find_row_mut("t1") {
            row.error = Some("timeout waiting for model".to_string());
        }
        panel.apply_event(WorkflowPanelEvent::RunCompleted {
            status: WorkflowPanelLifecycle::Failed,
            error: Some("phase Analyze failed".to_string()),
            at_ms: 2_100,
        });
        let extras = WorkflowHistoryExtras {
            result_summary: Some("no ship blockers found".to_string()),
            source_path: None,
            spillover_path: Some(PathBuf::from("/tmp/workflow-out.json")),
            verification_summary: None,
        };
        let lines = panel.render_history_card(120, true, &extras);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains('▼'), "expanded glyph: {joined}");
        assert!(joined.contains("goal:"), "{joined}");
        assert!(joined.contains("ship v0.8.68"), "{joined}");
        assert!(joined.contains("phases:"), "{joined}");
        assert!(joined.contains("Analyze"), "{joined}");
        assert!(joined.contains("children:"), "{joined}");
        assert!(joined.contains("scout crates"), "{joined}");
        assert!(joined.contains("result:"), "{joined}");
        assert!(joined.contains("no ship blockers"), "{joined}");
        assert!(
            joined.contains("source:") || joined.contains("demo.workflow"),
            "{joined}"
        );
        assert!(joined.contains("artifact:"), "{joined}");
        assert!(joined.contains("error:"), "{joined}");
        assert!(joined.contains("phase Analyze failed"), "{joined}");
        assert!(
            joined.contains("fail") || joined.contains("timeout"),
            "{joined}"
        );
    }

    #[test]
    fn direct_subagent_card_reuses_history_renderer() {
        let panel = WorkflowPanel::from_direct_subagent(
            "agent_abc",
            "explore",
            WorkflowPanelLifecycle::Succeeded,
            1_000,
            Some(4_500),
            Some("found 3 call sites".to_string()),
            None,
        );
        let compact = panel.render_history_card(100, false, &WorkflowHistoryExtras::default());
        let joined: String = compact
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains("success") || joined.contains("explore"),
            "{joined}"
        );
        assert!(
            joined.contains("1 child") || joined.contains("1 children"),
            "{joined}"
        );
        assert!(joined.contains("3s") || joined.contains("s"), "{joined}");

        let expanded = panel.render_history_card(
            100,
            true,
            &WorkflowHistoryExtras {
                result_summary: Some("found 3 call sites".to_string()),
                ..WorkflowHistoryExtras::default()
            },
        );
        let joined: String = expanded
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("children:"), "{joined}");
        assert!(joined.contains("result:"), "{joined}");
        assert!(joined.contains("found 3 call sites"), "{joined}");
    }

    #[test]
    fn from_run_json_round_trips_events_into_history_card() {
        let value = json!({
            "run_id": "workflow_demo",
            "status": "completed",
            "workflow_goal": "ship it",
            "started_at_ms": 1000,
            "completed_at_ms": 5000,
            "events": [
                {
                    "type": "run_started",
                    "at_ms": 1000,
                    "run_id": "workflow_demo",
                    "workflow_goal": "ship it"
                },
                {"type": "phase_started", "at_ms": 1100, "title": "Build"},
                {
                    "type": "task_started",
                    "at_ms": 1200,
                    "task_id": "t1",
                    "label": "compile",
                    "profile": "implementer"
                },
                {
                    "type": "task_completed",
                    "at_ms": 4000,
                    "task_id": "t1",
                    "status": "succeeded"
                },
                {"type": "run_completed", "at_ms": 5000, "status": "completed"}
            ]
        });
        let panel = WorkflowPanel::from_run_json(&value).expect("hydrate");
        assert_eq!(panel.lifecycle, WorkflowPanelLifecycle::Succeeded);
        let compact = panel.compact_summary_text(120);
        assert!(compact.contains("1 child"), "{compact}");
        assert!(compact.contains("success"), "{compact}");
        let expanded = panel.history_expanded_lines(120, &WorkflowHistoryExtras::default());
        let joined: String = expanded
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("goal:"), "{joined}");
        assert!(joined.contains("ship it"), "{joined}");
        assert!(joined.contains("Build"), "{joined}");
        assert!(joined.contains("compile"), "{joined}");
    }
}
