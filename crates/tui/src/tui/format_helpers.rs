//! Small string builders that compose status-bar / footer chips and
//! one-off informational messages.
//!
//! Each helper is a pure function over a small slice of `App` or
//! response data. Grouped here so the composer/footer renderer doesn't
//! need to scroll past their bodies, and so the labels can be unit
//! tested in isolation.

use crate::models::Usage;
use crate::tui::app::App;

/// Build the multi-line "Cache warmup complete: …" status message
/// shown after a prefix-cache warmup turn finishes. Handles all four
/// combinations of `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`
/// being present or absent so we never report "0% cache hit" for an
/// API call that didn't surface telemetry at all.
pub(super) fn cache_warmup_result(usage: &Usage) -> String {
    let cache = match (
        usage.prompt_cache_hit_tokens,
        usage.prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => format!("Cache warmup complete: hit {hit} | miss {miss}"),
        (Some(hit), None) => format!("Cache warmup complete: hit {hit} | miss unavailable"),
        (None, Some(miss)) => format!("Cache warmup complete: hit unavailable | miss {miss}"),
        (None, None) => "Cache warmup complete: cache telemetry unavailable".to_string(),
    };
    format!(
        "{cache}\nNote: the first warmup is usually a miss. Later requests that reuse the same stable prefix may hit the provider cache; a hit is not guaranteed."
    )
}

/// Format prefix stability info for the opt-in TUI footer chip.
pub(super) fn prefix_stability_chip(app: &App) -> Option<(String, ratatui::style::Color)> {
    let pct = app.prefix_stability_pct?;
    let changes = app.prefix_change_count;

    let color = if changes == 0 {
        // Perfect stability: green
        ratatui::style::Color::Green
    } else if pct >= 95 {
        // Excellent: green
        ratatui::style::Color::Green
    } else if pct >= 80 {
        // Good: yellow
        ratatui::style::Color::Yellow
    } else {
        // Poor: red — cache is churning
        ratatui::style::Color::Red
    };

    let label = if changes == 0 {
        format!("cache prefix {pct}%")
    } else {
        format!(
            "cache prefix {pct}% ({changes} change{})",
            if changes == 1 { "" } else { "s" }
        )
    };

    Some((label, color))
}

/// Render the response body for `/models` / `models list` — the current
/// model is starred and other available models follow underneath.
pub(super) fn available_models_message(current_model: &str, models: &[String]) -> String {
    let mut lines = vec![format!("Available models ({})", models.len())];
    for model in models {
        if model == current_model {
            lines.push(format!("* {model} (current)"));
        } else {
            lines.push(format!("  {model}"));
        }
    }
    lines.join("\n")
}

/// Build the multi-line balance result message for the `/balance` command.
pub(super) fn balance_result(balance: &crate::client::ProviderBalance, app: &App) -> String {
    use crate::config::ApiProvider;

    let mut lines: Vec<String> = Vec::new();

    match balance.provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
            lines.push(format!(
                "{} Account Balance",
                balance.provider.display_name()
            ));
            lines.push("─".repeat(40));
            match balance.is_available {
                Some(true) => {}
                Some(false) => {
                    lines.push("Balance endpoint reports: unavailable".to_string());
                }
                None => {}
            }
            if let Some(ref currency) = balance.currency {
                lines.push(format!("Currency: {currency}"));
            }
            if let Some(ref total) = balance.total_balance {
                lines.push(format!("Total balance: {total}"));
            }
            if let Some(ref topped_up) = balance.topped_up_balance {
                lines.push(format!("Topped-up balance: {topped_up}"));
            }
            if let Some(ref granted) = balance.granted_balance {
                lines.push(format!("Granted balance: {granted}"));
            }
        }
        ApiProvider::Openrouter => {
            lines.push("OpenRouter Credits".to_string());
            lines.push("─".repeat(40));
            if let Some(credits) = balance.total_credits {
                lines.push(format!("Total credits purchased: {credits:.2}"));
            }
            if let Some(usage) = balance.total_usage {
                lines.push(format!("Total usage: {usage:.2}"));
            }
            if let (Some(credits), Some(usage)) = (balance.total_credits, balance.total_usage) {
                let remaining = credits - usage;
                lines.push(format!("Remaining credits: {remaining:.2}"));
            }
        }
        _ => {
            lines.push(format!(
                "Balance info for {}",
                balance.provider.display_name()
            ));
        }
    }

    // Append session cost summary for context
    let session_total = app.displayed_session_cost_for_currency(app.cost_currency);
    let cost_label = app.format_cost_amount(session_total);
    lines.push(String::new());
    lines.push(format!(
        "This session (estimated): {cost_label}  |  {} tokens",
        app.session.total_conversation_tokens
    ));

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_models_message_marks_current_model() {
        let models = vec![
            "deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
        ];
        let msg = available_models_message("deepseek-v4-pro", &models);
        assert!(msg.contains("* deepseek-v4-pro (current)"), "got: {msg}");
        assert!(msg.contains("  deepseek-v4-flash"), "got: {msg}");
        assert!(msg.starts_with("Available models (2)"), "got: {msg}");
    }

    #[test]
    fn cache_warmup_result_handles_missing_telemetry() {
        let usage = Usage {
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            ..Default::default()
        };
        let msg = cache_warmup_result(&usage);
        assert!(msg.contains("cache telemetry unavailable"), "got: {msg}");
    }
}
