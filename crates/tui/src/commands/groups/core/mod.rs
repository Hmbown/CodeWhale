//! Core commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

mod help;
mod clear;
mod exit;
mod model;
mod models;
mod provider;
mod links;
mod feedback;
mod home;
mod workspace;
mod subagents;
mod agent;
mod profile;
mod relay;

use crate::commands::traits::{Command, CommandGroup};

pub struct CoreCommands;
impl CommandGroup for CoreCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(help::Help),
            Box::new(clear::Clear),
            Box::new(exit::Exit),
            Box::new(model::Model),
            Box::new(models::Models),
            Box::new(provider::Provider),
            Box::new(links::Links),
            Box::new(feedback::Feedback),
            Box::new(home::Home),
            Box::new(workspace::Workspace),
            Box::new(subagents::Subagents),
            Box::new(agent::Agent),
            Box::new(profile::Profile),
            Box::new(relay::Relay),
        ]
    }
}

// ── Helper functions ───────────────────────────────────────────────────────

/// Parse a depth-prefixed argument like "2 some text" -> (2, "some text").
/// Used by both `agent` and `rlm` commands.
pub(crate) fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "pending",
        crate::tools::plan::StepStatus::InProgress => "in_progress",
        crate::tools::plan::StepStatus::Completed => "completed",
    }
}

fn build_relay_instruction(app: &crate::tui::app::App, focus: Option<&str>) -> String {
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
