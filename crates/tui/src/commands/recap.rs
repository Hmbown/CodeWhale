//! /recap command — print a structured session state summary so the
//! model can re-orient without replaying the full transcript.
//!
//! Like Claude Code /recap: dumps goal, todos with status, last assistant
//! response summary, active sub-agents, and auto-continue state.

use crate::tui::app::App;

use super::CommandResult;

/// Print the current session recap to the transcript.
pub fn recap(app: &mut App, _arg: Option<&str>) -> CommandResult {
    let text = app.recap_text();
    if text.is_empty() {
        CommandResult::message("No session state to recap. Set a goal with /goal <objective> to get started.")
    } else {
        CommandResult::message(text)
    }
}
