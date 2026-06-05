//! Relay command.

use crate::tui::app::{App, AppAction};
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Relay;
impl Command for Relay {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "relay",
            aliases: &["batonpass", "\u{63E5}\u{529B}"],
            usage: "/relay [focus]",
            description_id: MessageId::CmdRelayDescription,
        }
    }
    fn execute(&self, app: &mut App, arg: Option<&str>) -> CommandResult {
        let focus = arg.map(str::trim).filter(|value| !value.is_empty());
        let message = build_relay_instruction(app, focus);
        CommandResult::with_message_and_action(
            "Preparing session relay at .deepseek/handoff.md...",
            AppAction::SendMessage(message),
        )
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "pending",
        crate::tools::plan::StepStatus::InProgress => "in_progress",
        crate::tools::plan::StepStatus::Completed => "completed",
    }
}

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Create a compact session relay for a future CodeWhale thread.");
    let _ = writeln!(out);
    let _ = writeln!(out, "Write or update `.deepseek/handoff.md`.");
    let _ = writeln!(out, "Keep the existing file path for compatibility, but title the artifact `# Session relay`.");
    let _ = writeln!(out);
    let _ = writeln!(out, "Current session snapshot:");
    let _ = writeln!(out, "- Workspace: {}", app.workspace.display());
    let _ = writeln!(out, "- Mode: {}", app.mode.label());
    let _ = writeln!(out, "- Model: {}", app.model_display_label());
    if let Some(focus) = focus {
        let _ = writeln!(out, "- Requested relay focus: {focus}");
    }
    if let Some(quarry) = app.hunt.quarry.as_deref() {
        let _ = writeln!(out, "- Hunt quarry: {quarry}");
    }
    if let Some(budget) = app.hunt.token_budget {
        let _ = writeln!(out, "- Hunt token budget: {budget}");
    }
    if let Ok(todos) = app.todos.try_lock() {
        let snapshot = todos.snapshot();
        if !snapshot.items.is_empty() {
            let _ = writeln!(out, "\nWork checklist (primary progress surface, {}% complete):", snapshot.completion_pct);
            for item in snapshot.items {
                let _ = writeln!(out, "- #{} [{}] {}", item.id, item.status.as_str(), item.content);
            }
        }
    } else {
        let _ = writeln!(out, "\nWork checklist: unavailable because the checklist is busy.");
    }
    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if snapshot.explanation.is_some() || !snapshot.items.is_empty() {
            let _ = writeln!(out, "\nOptional strategy metadata from update_plan:");
            if let Some(explanation) = snapshot.explanation.as_deref() {
                let _ = writeln!(out, "- Explanation: {explanation}");
            }
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(out, "\nStrategy metadata: unavailable because plan state is busy.");
    }
    let _ = writeln!(out, "\nKeep it under about 900 words. After writing, report the path and the single next action.");
    out
}
