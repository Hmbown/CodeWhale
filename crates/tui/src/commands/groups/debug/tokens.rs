//! Token/cost introspection and context commands.

use crate::compaction::estimate_input_tokens_conservative;
use crate::localization::{Locale, MessageId, tr};
use crate::models::SystemPrompt;
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
    let window = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    let used = estimated.min(window as usize);
    let percent = (used as f64 / f64::from(window) * 100.0).clamp(0.0, 100.0);
    tr(locale, MessageId::CmdTokensContextWithWindow)
        .replace("{used}", &used.to_string())
        .replace("{window}", &window.to_string())
        .replace("{percent}", &format!("{percent:.1}"))
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

    let mut report = tr(locale, MessageId::CmdTokensReport)
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
        .replace("{cost}", &cost_report_amount(app, locale))
        .replace("{api_messages}", &message_count.to_string())
        .replace("{chat_messages}", &chat_count.to_string())
        .replace("{model}", &app.model);
    // `/tokens` quotes the same cost figure as `/cost`, so it carries the same
    // estimate disclaimer and the same coverage state. Two surfaces showing one
    // number must not disagree about how complete that number is (#4318).
    report.push_str(&cache_write_summary(app, locale));
    report.push_str(&cost_coverage_report(app, locale));
    CommandResult::message(report)
}

/// Session cache-write total, reported as its own class with a pointer to
/// `/cache` for the per-turn breakdown.
///
/// Cache-write is billed at a premium on the providers that publish one, so it
/// is neither folded into input nor hidden: `/tokens` shows the total and says
/// where the detail lives.
fn cache_write_summary(app: &App, locale: Locale) -> String {
    let write = app.session.total_cache_write_tokens;
    let mut out = String::from("\n");
    out.push_str(&tr(locale, MessageId::CmdTokensCacheWriteTotal).replace(
        "{write}",
        &if write > 0 {
            write.to_string()
        } else {
            tr(locale, MessageId::CmdTokensNotReported).to_string()
        },
    ));
    out
}

/// Show session cost breakdown.
///
/// The figure is an **estimate** computed from provider-reported usage and
/// published rates; it is never an invoice. Turns whose route produced no
/// authoritative price are missing from it entirely, so the coverage of the
/// number is reported alongside it rather than left implicit (#4318).
pub fn cost(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    let (priced, unpriced) = cost_coverage_counts(app);
    let has_saved_legacy_subtotal = app.session.cost_coverage_unknown_legacy
        && app.displayed_session_cost_for_currency(app.cost_currency) > 0.0;
    let headline = if priced == 0 && !has_saved_legacy_subtotal {
        MessageId::CmdCostReportUnknown
    } else if app.session.cost_coverage_unknown_legacy || unpriced > 0 {
        MessageId::CmdCostReportSubtotal
    } else {
        MessageId::CmdCostReport
    };
    let mut report = tr(locale, headline).replace("{cost}", &cost_report_amount(app, locale));
    if priced > 0 || has_saved_legacy_subtotal {
        report.push_str(&cost_breakdown_report(app));
    }
    report.push_str(&cost_coverage_report(app, locale));
    CommandResult::message(report)
}

fn cost_report_amount(app: &App, locale: Locale) -> String {
    let (priced, _) = cost_coverage_counts(app);
    let total = app.displayed_session_cost_for_currency(app.cost_currency);
    if priced > 0 || (app.session.cost_coverage_unknown_legacy && total > 0.0) {
        app.format_cost_amount_precise(total)
    } else {
        tr(locale, MessageId::CmdCostUnknownValue).to_string()
    }
}

/// The `/cost` headline decomposed into the exact terms it is computed from.
///
/// The headline is `max(parent turns + sub-agents, display high-water)` in the
/// display currency (the #244 monotonic guarantee). Those are its only inputs,
/// so the three components below always sum back to it — asserted by test, so
/// the breakdown can never drift from the number above it (#4939).
struct CostComponents {
    /// Accumulated parent-turn spend.
    parent_turns: f64,
    /// Accumulated sub-agent/background spend.
    subagents: f64,
    /// Amount by which the monotonic display floor exceeds the live
    /// accumulators after a downward reconciliation (#244). Zero whenever the
    /// live sum is the headline.
    display_floor: f64,
}

impl CostComponents {
    fn compute(app: &App) -> Self {
        // Each term is sanitized exactly the way the accumulator fold
        // sanitizes it, so `current` here is bitwise the `current` inside
        // `displayed_session_cost_for_currency` and the floor is exact.
        fn sanitize(amount: f64) -> f64 {
            if amount.is_finite() && amount >= 0.0 {
                amount
            } else {
                0.0
            }
        }
        let currency = app.cost_display_currency(app.cost_currency);
        let parent_turns = sanitize(app.session_cost_for_currency(currency));
        let subagents = sanitize(app.subagent_cost_for_currency(currency));
        let current = {
            let sum = parent_turns + subagents;
            if sum.is_finite() { sum } else { f64::MAX }
        };
        let headline = app.displayed_session_cost_for_currency(app.cost_currency);
        Self {
            parent_turns,
            subagents,
            display_floor: (headline - current).max(0.0),
        }
    }

    /// The recomposed headline. Test-only: production renders the components
    /// and the headline from the same state, and the tests assert this sum
    /// equals the displayed headline exactly.
    #[cfg(test)]
    fn sum(&self) -> f64 {
        self.parent_turns + self.subagents + self.display_floor
    }
}

/// Append the headline decomposition: the accumulator components the headline
/// is computed from, then parent-turn spend attributed per route from the
/// audited turn-telemetry ring.
///
/// Diagnostic composition detail like `/context report`, so plain English
/// rather than a localized template.
fn cost_breakdown_report(app: &App) -> String {
    let components = CostComponents::compute(app);
    let mut out = String::from("\n\nBreakdown (components sum to the total above):");
    out.push_str(&format!(
        "\n  Parent turns: {}",
        app.format_cost_amount_precise(components.parent_turns)
    ));
    if components.subagents > 0.0 {
        out.push_str(&format!(
            "\n  Sub-agents: {}",
            app.format_cost_amount_precise(components.subagents)
        ));
    }
    if components.display_floor > 0.0 {
        out.push_str(&format!(
            "\n  Reconciliation floor: {} (monotonic display guarantee, kept after a downward cost reconciliation)",
            app.format_cost_amount_precise(components.display_floor)
        ));
    }

    // Per-route attribution from the per-turn audits that fed the total. The
    // telemetry ring is bounded, so coverage is stated instead of implied:
    // itemized turns out of all priced turns, never a claim of completeness.
    let currency = app.cost_display_currency(app.cost_currency);
    let mut by_route: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let mut itemized: u32 = 0;
    for record in &app.session.turn_cache_history {
        let Some(audit) = record.cost_audit.as_ref() else {
            continue;
        };
        if !audit.is_priced_in(currency) {
            continue;
        }
        let Some(estimate) = audit.estimate else {
            continue;
        };
        let provider = record.provider_identity.clone().unwrap_or_else(|| {
            record.provider.map_or_else(
                || "unknown-provider".to_string(),
                |p| p.as_str().to_string(),
            )
        });
        let model = record.model.as_deref().unwrap_or("unknown-model");
        *by_route.entry(format!("{provider}/{model}")).or_insert(0.0) += estimate.amount(currency);
        itemized = itemized.saturating_add(1);
    }
    if !by_route.is_empty() {
        let (priced, _) = cost_coverage_counts(app);
        out.push_str(&format!(
            "\n  Parent-turn spend by route ({itemized} of {priced} priced turns itemized):"
        ));
        for (route, amount) in &by_route {
            out.push_str(&format!(
                "\n    {route}: {}",
                app.format_cost_amount_precise(*amount)
            ));
        }
        if itemized < priced {
            out.push_str(&format!(
                "\n    (earlier turns not itemized: turn telemetry keeps the last {})",
                App::TURN_CACHE_HISTORY_CAP
            ));
        }
    }
    out
}

fn joined(values: &std::collections::BTreeSet<String>) -> String {
    values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The honesty block appended to `/cost` and `/tokens`: what the estimate covers
/// and what it cannot.
///
/// Both surfaces render the same block from the same session counters, so they
/// cannot disagree about completeness (#4318).
pub(crate) fn cost_coverage_report(app: &App, locale: Locale) -> String {
    let (priced, unpriced) = cost_coverage_counts(app);
    let mut out = String::from("\n\n");
    out.push_str(&tr(locale, MessageId::CmdCostEstimateOnly));
    out.push('\n');
    if app.session.cost_coverage_unknown_legacy {
        // A restored pre-coverage session has real money and no evidence of what
        // it covers. Saying "0 of 0 priced" here would assert the total is
        // complete, so the unknown state is stated instead.
        out.push_str(&tr(locale, MessageId::CmdCostCoverageUnknownLegacy));
    } else {
        out.push_str(
            &tr(locale, MessageId::CmdCostCoverage)
                .replace("{priced}", &priced.to_string())
                .replace("{turns}", &(priced.saturating_add(unpriced)).to_string()),
        );
    }
    if unpriced > 0 {
        let reasons = match app.cost_display_currency(app.cost_currency) {
            crate::pricing::CostCurrency::Usd => &app.session.cost_unpriced_reasons,
            crate::pricing::CostCurrency::Cny => &app.session.cost_cny_unpriced_reasons,
        };
        out.push('\n');
        out.push_str(
            &tr(locale, MessageId::CmdCostUnpricedTurns)
                .replace("{unpriced}", &unpriced.to_string())
                .replace("{reasons}", &joined(reasons)),
        );
    }
    if !app.session.cost_unpriced_classes.is_empty() {
        out.push('\n');
        out.push_str(
            &tr(locale, MessageId::CmdCostUnpricedClasses)
                .replace("{classes}", &joined(&app.session.cost_unpriced_classes)),
        );
    }
    if !app.session.cost_pricing_provenances.is_empty() {
        out.push('\n');
        out.push_str(
            &tr(locale, MessageId::CmdCostPricingProvenance)
                .replace("{sources}", &joined(&app.session.cost_pricing_provenances)),
        );
    }
    if !app.session.cost_live_pricing_defects.is_empty() {
        out.push('\n');
        out.push_str(
            &tr(locale, MessageId::CmdCostLivePricingDowngraded)
                .replace("{defects}", &joined(&app.session.cost_live_pricing_defects)),
        );
    }
    if !app.session.cost_live_pricing_unusable_defects.is_empty() {
        out.push('\n');
        out.push_str(
            &tr(locale, MessageId::CmdCostLivePricingUnavailable).replace(
                "{defects}",
                &joined(&app.session.cost_live_pricing_unusable_defects),
            ),
        );
    }
    if !app.session.cost_route_receipts.is_empty() {
        out.push('\n');
        out.push_str(&tr(locale, MessageId::CmdCostRoutesHeader));
        for receipt in &app.session.cost_route_receipts {
            out.push_str("\n  ");
            out.push_str(receipt);
        }
    }
    out
}

fn cost_coverage_counts(app: &App) -> (u32, u32) {
    match app.cost_display_currency(app.cost_currency) {
        crate::pricing::CostCurrency::Usd => (
            app.session.cost_priced_turns,
            app.session.cost_unpriced_turns,
        ),
        crate::pricing::CostCurrency::Cny => (
            app.session.cost_cny_priced_turns,
            app.session.cost_cny_unpriced_turns,
        ),
    }
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
/// `/context json`, `/context prompt-json`, and `/context summary` expose the diagnostic source map
/// from #3143 without replacing the inspector surface.
pub fn context(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(subcommand) = arg.map(str::trim).filter(|arg| !arg.is_empty()) else {
        return CommandResult::action(AppAction::OpenContextInspector);
    };

    match subcommand {
        "prompt-json" | "prompt_json" | "prompt" => {
            let context = crate::context_report::build_prompt_context(app);
            CommandResult::message(crate::context_report::prompt_context_json(&context))
        }
        "report" | "json" | "summary" => {
            let report = crate::context_report::build_context_report(app);
            match subcommand {
                "report" => {
                    CommandResult::message(crate::context_report::format_context_report(&report))
                }
                "json" => {
                    CommandResult::message(crate::context_report::context_report_json(&report))
                }
                "summary" => {
                    CommandResult::message(crate::context_report::format_context_summary(&report))
                }
                _ => unreachable!(),
            }
        }
        other => CommandResult::error(format!(
            "Unknown /context subcommand: {other}. Use report, json, prompt-json, or summary."
        )),
    }
}

#[cfg(test)]
mod cost_breakdown_tests {
    use super::*;
    use crate::config::Config;
    use crate::pricing::{CostCurrency, CostEstimate, TurnCostAudit};
    use crate::tui::app::{TuiOptions, TurnCacheRecord};
    use std::path::PathBuf;
    use std::time::Instant;

    fn test_app() -> App {
        let options = TuiOptions {
            skills_dir: PathBuf::from("/tmp/test-skills"),
            ..crate::test_support::test_tui_options(PathBuf::from("/tmp/test-workspace"))
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app.cost_currency = CostCurrency::Usd;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app
    }

    fn priced_audit(estimate: CostEstimate) -> TurnCostAudit {
        TurnCostAudit {
            estimate: Some(estimate),
            provenance: None,
            unpriced_classes: Vec::new(),
            unpriced_reason: None,
            live_pricing_defect: None,
            usd_priced: true,
            cny_priced: estimate.cny > 0.0,
        }
    }

    fn turn_record(model: &str, audit: TurnCostAudit) -> TurnCacheRecord {
        TurnCacheRecord {
            provider: Some(crate::config::ApiProvider::Deepseek),
            provider_identity: None,
            model: Some(model.to_string()),
            auto_model: false,
            input_tokens: 100,
            output_tokens: 10,
            cache_hit_tokens: None,
            cache_miss_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_audit: Some(audit),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        }
    }

    /// The decomposition's terms are exactly the headline's inputs, so their
    /// sum reproduces the headline — including when the #244 monotonic floor,
    /// not the live accumulators, is the number on display (#4939).
    #[test]
    fn cost_breakdown_components_sum_to_headline() {
        let mut app = test_app();
        app.session.cost_priced_turns = 2;
        app.accrue_session_cost_estimate(CostEstimate {
            usd: 0.05,
            cny: 0.0,
        });
        app.accrue_subagent_cost_estimate(CostEstimate {
            usd: 0.02,
            cny: 0.0,
        });

        // Live sum is the headline: no floor component.
        let components = CostComponents::compute(&app);
        assert_eq!(components.parent_turns, 0.05);
        assert_eq!(components.subagents, 0.02);
        assert_eq!(components.display_floor, 0.0);
        assert_eq!(
            components.sum(),
            app.displayed_session_cost_for_currency(CostCurrency::Usd),
            "components must sum to the /cost headline"
        );

        // After a downward reconciliation the high-water is the headline; the
        // difference surfaces as an explicit floor component, and the sum still
        // reproduces the headline exactly.
        app.session.displayed_cost_high_water = 0.10;
        let components = CostComponents::compute(&app);
        assert!(components.display_floor > 0.0);
        assert_eq!(
            components.sum(),
            app.displayed_session_cost_for_currency(CostCurrency::Usd),
            "floor component must absorb exactly the high-water excess"
        );

        let msg = cost(&mut app).message.expect("cost report");
        assert!(msg.contains("Breakdown"), "{msg}");
        assert!(msg.contains("Parent turns: $0.0500"), "{msg}");
        assert!(msg.contains("Sub-agents: $0.0200"), "{msg}");
        assert!(msg.contains("Reconciliation floor:"), "{msg}");
    }

    /// Per-route attribution comes from the same `TurnCostAudit`s that fed the
    /// total, in the display currency; with every priced turn itemized, the
    /// route amounts account for the whole parent component. CNY amounts are
    /// the audits' provider-published CNY figures — never an FX projection of
    /// the USD column (#4939).
    #[test]
    fn cost_breakdown_itemizes_routes_from_turn_audits() {
        let mut app = test_app();
        app.cost_currency = CostCurrency::Cny;
        let turns = [
            CostEstimate {
                usd: 0.01,
                cny: 0.07,
            },
            CostEstimate {
                usd: 0.02,
                cny: 0.14,
            },
        ];
        for estimate in turns {
            let audit = priced_audit(estimate);
            app.record_turn_cost_audit(&audit);
            app.accrue_session_cost_estimate(estimate);
            app.push_turn_cache_record(turn_record("deepseek-chat", audit));
        }

        let components = CostComponents::compute(&app);
        assert_eq!(
            components.sum(),
            app.displayed_session_cost_for_currency(CostCurrency::Cny),
            "CNY components must sum to the CNY headline"
        );

        let msg = cost(&mut app).message.expect("cost report");
        assert!(
            msg.contains("Parent-turn spend by route (2 of 2 priced turns itemized):"),
            "{msg}"
        );
        // 0.07 + 0.14 accumulated in ring order equals the parent component's
        // accumulation, so the route line shows the whole parent spend.
        let route_amount = app.format_cost_amount_precise(components.parent_turns);
        assert!(
            msg.contains(&format!("deepseek/deepseek-chat: {route_amount}")),
            "{msg}"
        );
        assert!(msg.contains("¥"), "CNY display must use CNY symbol: {msg}");
    }

    /// An unpriced headline renders no breakdown: decomposing a number that is
    /// not being shown would fabricate amounts the report just declined to
    /// claim.
    #[test]
    fn cost_breakdown_absent_when_headline_unknown() {
        let mut app = test_app();
        let msg = cost(&mut app).message.expect("cost report");
        assert!(!msg.contains("Breakdown"), "{msg}");
        assert!(!msg.contains("Parent turns:"), "{msg}");
    }
}
