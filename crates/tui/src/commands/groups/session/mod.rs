//! Session command area: saving, forking, resuming, exporting, and the
//! `/relay` session-handoff artifact.

mod rename;
#[allow(clippy::module_inception)]
mod session;

use std::fmt::Write as _;

use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "rename" | "gaiming" | "chongmingming" => rename::rename(app, arg),
        "save" => session::save(app, arg),
        "fork" | "branch" => session::fork(app),
        "new" => session::new_session(app, arg),
        "sessions" | "resume" => session::sessions(app, arg),
        "relay" | "batonpass" | "接力" => relay(app, arg),
        "load" | "jiazai" => session::load(app, arg),
        "compact" | "yasuo" => session::compact(app),
        "purge" | "qingchu" => session::purge(app),
        "export" | "daochu" => session::export(app, arg),
        _ => return None,
    };
    Some(result)
}

/// Ask the active model to write a compact relay artifact for the next thread.
///
/// The visible command is `/relay` (with `/接力` for Chinese users), but the
/// durable file path remains `.deepseek/handoff.md` for compatibility with
/// existing sessions and startup prompt loading.
pub fn relay(app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg.map(str::trim).filter(|value| !value.is_empty());
    let message = build_relay_instruction(app, focus);
    CommandResult::with_message_and_action(
        "Preparing session relay at .deepseek/handoff.md...",
        AppAction::SendMessage(message),
    )
}

fn build_relay_instruction(app: &App, focus: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Create a compact session relay (接力) for a future CodeWhale thread."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Write or update `.deepseek/handoff.md`.");
    let _ = writeln!(
        out,
        "Keep the existing file path for compatibility, but title the artifact `# Session relay`."
    );
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
            let _ = writeln!(
                out,
                "\nWork checklist (primary progress surface, {}% complete):",
                snapshot.completion_pct
            );
            for item in snapshot.items {
                let _ = writeln!(
                    out,
                    "- #{} [{}] {}",
                    item.id,
                    item.status.as_str(),
                    item.content
                );
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nWork checklist: unavailable because the checklist is busy."
        );
    }

    if let Ok(plan) = app.plan_state.try_lock() {
        let snapshot = plan.snapshot();
        if !snapshot.is_empty() {
            let _ = writeln!(out, "\nOptional strategy metadata from update_plan:");
            write_plan_field(&mut out, "Title", snapshot.title.as_deref());
            write_plan_field(&mut out, "Objective", snapshot.objective.as_deref());
            write_plan_field(&mut out, "Context", snapshot.context_summary.as_deref());
            write_plan_field(&mut out, "Explanation", snapshot.explanation.as_deref());
            write_plan_list(&mut out, "Source", &snapshot.sources_used);
            write_plan_list(&mut out, "Critical file", &snapshot.critical_files);
            write_plan_list(&mut out, "Constraint", &snapshot.constraints);
            write_plan_field(
                &mut out,
                "Recommended approach",
                snapshot.recommended_approach.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Verification plan",
                snapshot.verification_plan.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Risks and unknowns",
                snapshot.risks_and_unknowns.as_deref(),
            );
            write_plan_field(
                &mut out,
                "Handoff packet",
                snapshot.handoff_packet.as_deref(),
            );
            for item in snapshot.items {
                let _ = writeln!(out, "- [{}] {}", plan_status_label(&item.status), item.step);
            }
        }
    } else {
        let _ = writeln!(
            out,
            "\nStrategy metadata: unavailable because plan state is busy."
        );
    }

    let _ = writeln!(
        out,
        "\nBefore writing, inspect the current transcript context and any live tool evidence you need. Do not invent test results, file changes, blockers, or decisions."
    );
    let _ = writeln!(
        out,
        "\nUse this compact structure:\n\
         # Session relay\n\
         \n\
         ## Goal\n\
         [the user's objective and any explicit constraints]\n\
         \n\
         ## Current work\n\
         [the active Work checklist item, progress, and what is mid-flight]\n\
         \n\
         ## Files and state\n\
         [changed files, important paths, sub-agents/RLM sessions, commands run]\n\
         \n\
         ## Decisions\n\
         [why key choices were made]\n\
         \n\
         ## Verification\n\
         [what passed, what failed, what was not run]\n\
         \n\
         ## Next action\n\
         [one concrete action for the next thread]"
    );
    let _ = writeln!(
        out,
        "\nKeep it under about 900 words unless the session genuinely needs more. After writing, report the path and the single next action."
    );
    out
}

fn write_plan_field(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let _ = writeln!(out, "- {label}: {value}");
    }
}

fn write_plan_list(out: &mut String, label: &str, values: &[String]) {
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            let _ = writeln!(out, "- {label}: {value}");
        }
    }
}

fn plan_status_label(status: &crate::tools::plan::StepStatus) -> &'static str {
    match status {
        crate::tools::plan::StepStatus::Pending => "pending",
        crate::tools::plan::StepStatus::InProgress => "in_progress",
        crate::tools::plan::StepStatus::Completed => "completed",
    }
}
