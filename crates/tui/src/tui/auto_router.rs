//! Auto-routing helpers: deciding when to consult the auto-route flash
//! model, and building the small context window it sees.
//!
//! The TUI calls `resolve_auto_model_selection` once per user turn when
//! `app.auto_model` is set. The async function builds a recent-context
//! summary from `api_messages` (capped to six rows of up to 900 chars
//! each), passes it through `commands::resolve_auto_route_with_flash`,
//! and returns the selection (model + reasoning effort). The remaining
//! helpers are pure transforms used to build that summary.

use crate::commands;
use crate::config::Config;
use crate::models::{ContentBlock, Message};
use crate::tui::app::{App, QueuedMessage, ReasoningEffort};

/// Whether the next turn should consult the auto-route flash model.
pub(super) fn should_resolve_auto_model_selection(app: &App) -> bool {
    app.auto_model
}

/// Call the auto-route flash model with the user's draft + a short
/// recent-context window. Returns the selected model and effort.
pub(super) async fn resolve_auto_model_selection(
    app: &App,
    config: &Config,
    message: &QueuedMessage,
    latest_content: &str,
) -> commands::AutoRouteSelection {
    let latest_request = if latest_content.trim().is_empty() {
        message.display.as_str()
    } else {
        latest_content
    };
    commands::resolve_auto_route_with_flash(
        config,
        latest_request,
        &recent_auto_router_context(&app.api_messages),
        if app.auto_model { "auto" } else { "fixed" },
        app.reasoning_effort.as_setting(),
    )
    .await
}

/// Normalize the heuristic effort to the canonical auto-route effort.
pub(super) fn normalize_auto_routed_effort(effort: ReasoningEffort) -> ReasoningEffort {
    commands::normalize_auto_route_effort(effort)
}

/// Build a compact recent-context summary for the auto-route prompt.
///
/// Walks `api_messages` from the most recent turn back, skipping the
/// final draft (which is what the router is being asked to classify),
/// collects up to six non-empty rows, and reverses them so the prompt
/// reads oldest-first. Each row is `<role>: <truncated content>` and
/// is capped at 900 characters.
pub(super) fn recent_auto_router_context(messages: &[Message]) -> String {
    let mut rows = Vec::new();
    for message in messages.iter().rev().skip(1) {
        if rows.len() >= 6 {
            break;
        }
        let text = content_blocks_text(&message.content);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        rows.push(format!(
            "{}: {}",
            message.role,
            truncate_for_auto_router(text, 900)
        ));
    }
    rows.reverse();
    if rows.is_empty() {
        "No prior context.".to_string()
    } else {
        rows.join("\n")
    }
}

fn content_blocks_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                append_router_text(&mut out, text);
            }
            ContentBlock::Thinking { thinking } => {
                append_router_text(&mut out, thinking);
            }
            ContentBlock::ToolUse { name, .. } => {
                append_router_text(&mut out, &format!("[tool call: {name}]"));
            }
            ContentBlock::ToolResult { content, .. } => {
                append_router_text(&mut out, &format!("[tool result] {content}"));
            }
            _ => {}
        }
    }
    out
}

fn append_router_text(out: &mut String, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

fn truncate_for_auto_router(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContentBlock;

    fn make_msg(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn truncate_for_auto_router_honors_char_budget() {
        let s = "abcdefghij";
        assert_eq!(truncate_for_auto_router(s, 4), "abcd...");
        assert_eq!(truncate_for_auto_router(s, 10), "abcdefghij");
        assert_eq!(truncate_for_auto_router(s, 100), "abcdefghij");
    }

    #[test]
    fn recent_auto_router_context_skips_final_message_and_caps_rows() {
        // Eight messages; final one (the draft being routed) is skipped,
        // so we expect at most six of the remaining seven.
        let msgs: Vec<Message> = (0..8)
            .map(|i| {
                make_msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("turn {i}"),
                )
            })
            .collect();
        let context = recent_auto_router_context(&msgs);
        assert!(!context.contains("turn 7"), "final draft must be skipped");
        let row_count = context.lines().count();
        assert_eq!(row_count, 6);
        // Output is oldest-first.
        let first = context.lines().next().unwrap();
        assert!(first.contains("turn 1"), "got: {context}");
    }

    #[test]
    fn recent_auto_router_context_handles_empty_history() {
        assert_eq!(recent_auto_router_context(&[]), "No prior context.");
    }
}
