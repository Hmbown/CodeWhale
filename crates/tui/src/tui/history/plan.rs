//! Rendering for plan-update transcript cells.

use ratatui::text::Line;

use crate::tools::plan::{PlanSnapshot, StepStatus};

use super::{
    ToolStatus, render_compact_kv, render_tool_header, tool_status_label, tool_value_style,
};

/// Cell for plan updates emitted by the plan tool.
#[derive(Debug, Clone)]
pub struct PlanUpdateCell {
    pub snapshot: PlanSnapshot,
    pub status: ToolStatus,
}

impl PlanUpdateCell {
    /// Render the plan update cell into lines.
    pub fn lines_with_motion(&self, width: u16, low_motion: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(render_tool_header(
            "Strategy",
            tool_status_label(self.status),
            self.status,
            None,
            low_motion,
        ));

        render_plan_snapshot_lines(&self.snapshot, &mut lines, width);

        lines
    }
}

fn render_plan_snapshot_lines(snapshot: &PlanSnapshot, lines: &mut Vec<Line<'static>>, width: u16) {
    render_plan_optional(lines, "title", snapshot.title.as_deref(), width);
    render_plan_optional(lines, "objective", snapshot.objective.as_deref(), width);
    render_plan_optional(lines, "context", snapshot.context_summary.as_deref(), width);
    render_plan_optional(lines, "explain", snapshot.explanation.as_deref(), width);
    render_plan_list(lines, "source", &snapshot.sources_used, width);
    render_plan_list(lines, "file", &snapshot.critical_files, width);
    render_plan_list(lines, "constraint", &snapshot.constraints, width);
    render_plan_optional(
        lines,
        "approach",
        snapshot.recommended_approach.as_deref(),
        width,
    );
    render_plan_optional(
        lines,
        "verify",
        snapshot.verification_plan.as_deref(),
        width,
    );
    render_plan_optional(lines, "risk", snapshot.risks_and_unknowns.as_deref(), width);
    render_plan_optional(lines, "handoff", snapshot.handoff_packet.as_deref(), width);

    for step in &snapshot.items {
        let marker = match step.status {
            StepStatus::Completed => "done",
            StepStatus::InProgress => "live",
            StepStatus::Pending => "next",
        };
        lines.extend(render_compact_kv(
            marker,
            &step.step,
            tool_value_style(),
            width,
        ));
    }
}

fn render_plan_optional(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<&str>,
    width: u16,
) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        lines.extend(render_compact_kv(label, value, tool_value_style(), width));
    }
}

fn render_plan_list(lines: &mut Vec<Line<'static>>, label: &str, values: &[String], width: u16) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            lines.extend(render_compact_kv(label, value, tool_value_style(), width));
        }
    }
}
