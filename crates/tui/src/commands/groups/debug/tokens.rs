//! Token/cost introspection and context commands.

use crate::compaction::estimate_input_tokens_conservative;
use crate::localization::{Locale, MessageId, tr};
use crate::models::{SystemPrompt, context_window_for_model};
use crate::tui::app::{App, AppAction};

use super::CommandResult;

fn token_count(value: Option<u32>, locale: Locale) -> String {
    value.map_or_else(
        || tr(locale, MessageId::CmdTokensNotReported).to_string(),
        |tokens| tokens.to_string(),
    )
}

fn active_context_summary(app: &App, locale: Locale) -> String {
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    match context_window_for_model(&app.model) {
        Some(window) => {
            let used = estimated.min(window as usize);
            let percent = (used as f64 / f64::from(window) * 100.0).clamp(0.0, 100.0);
            tr(locale, MessageId::CmdTokensContextWithWindow)
                .replace("{used}", &used.to_string())
                .replace("{window}", &window.to_string())
                .replace("{percent}", &format!("{percent:.1}"))
        }
        None => tr(locale, MessageId::CmdTokensContextUnknownWindow)
            .replace("{estimated}", &estimated.to_string()),
    }
}

fn cache_summary(app: &App, locale: Locale) -> String {
    match (
        app.session.last_prompt_cache_hit_tokens,
        app.session.last_prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => tr(locale, MessageId::CmdTokensCacheBoth)
            .replace("{hit}", &hit.to_string())
            .replace("{miss}", &miss.to_string()),
        (Some(hit), None) => {
            tr(locale, MessageId::CmdTokensCacheHitOnly).replace("{hit}", &hit.to_string())
        }
        (None, Some(miss)) => {
            tr(locale, MessageId::CmdTokensCacheMissOnly).replace("{miss}", &miss.to_string())
        }
        (None, None) => tr(locale, MessageId::CmdTokensNotReported).to_string(),
    }
}

/// Show token usage for session
pub fn tokens(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    let message_count = app.api_messages.len();
    let chat_count = app.history.len();

    let report = tr(locale, MessageId::CmdTokensReport)
        .replace("{active}", &active_context_summary(app, locale))
        .replace(
            "{input}",
            &token_count(app.session.last_prompt_tokens, locale),
        )
        .replace(
            "{output}",
            &token_count(app.session.last_completion_tokens, locale),
        )
        .replace("{cache}", &cache_summary(app, locale))
        .replace("{total}", &app.session.total_tokens.to_string())
        .replace(
            "{cost}",
            &app.format_cost_amount_precise(
                app.displayed_session_cost_for_currency(app.cost_currency),
            ),
        )
        .replace("{api_messages}", &message_count.to_string())
        .replace("{chat_messages}", &chat_count.to_string())
        .replace("{model}", &app.model);
    CommandResult::message(report)
}

/// Show session cost breakdown
pub fn cost(app: &mut App) -> CommandResult {
    let total = app.displayed_session_cost_for_currency(app.cost_currency);
    let report = tr(app.ui_locale, MessageId::CmdCostReport)
        .replace("{cost}", &app.format_cost_amount_precise(total));
    CommandResult::message(report)
}

/// Show current system prompt
pub fn system_prompt(app: &mut App) -> CommandResult {
    let prompt_text = match &app.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|b| b.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
        None => "(no system prompt)".to_string(),
    };

    // Truncate if too long
    let display = if prompt_text.len() > 500 {
        // Find a valid UTF-8 char boundary at or before byte 500
        let truncate_at = prompt_text
            .char_indices()
            .take_while(|(i, _)| *i <= 500)
            .last()
            .map_or(0, |(i, _)| i);
        format!(
            "{}...\n\n(truncated, {} chars total)",
            &prompt_text[..truncate_at],
            prompt_text.len()
        )
    } else {
        prompt_text
    };

    CommandResult::message(format!(
        "System Prompt ({} mode):\n─────────────────────────────\n{}",
        app.mode.label(),
        display
    ))
}

/// Show context window usage.
///
/// `/context` keeps opening the interactive inspector. `/context report`,
/// `/context json`, and `/context summary` expose the diagnostic source map
/// from #3143 without replacing the inspector surface.
pub fn context(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(subcommand) = arg.map(str::trim).filter(|arg| !arg.is_empty()) else {
        return CommandResult::action(AppAction::OpenContextInspector);
    };

    let report = crate::context_report::build_context_report(app);
    match subcommand {
        "report" => CommandResult::message(crate::context_report::format_context_report(&report)),
        "json" => CommandResult::message(crate::context_report::context_report_json(&report)),
        "summary" => CommandResult::message(crate::context_report::format_context_summary(&report)),
        other => CommandResult::error(format!(
            "Unknown /context subcommand: {other}. Use report, json, or summary."
        )),
    }
}
