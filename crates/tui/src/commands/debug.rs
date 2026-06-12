#![allow(clippy::items_after_test_module)]

//! Debug commands: tokens, cost, system, context, undo, retry

use std::time::Instant;

use super::CommandResult;
use crate::client::{CacheWarmupKey, PromptInspection, inspect_prompt_for_request};
use crate::compaction::estimate_input_tokens_conservative;
use crate::dependencies::{ExternalTool, Git};
use crate::localization::{Locale, MessageId, tr};
use crate::models::{ContentBlock, MessageRequest, SystemPrompt, context_window_for_model};
use crate::tui::app::{App, AppAction, TurnCacheRecord};
use crate::tui::history::HistoryCell;

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

/// Show context window usage and prompt source map (#3143).
///
/// Subcommands:
///   /context          Alias for /context report
///   /context report   Full prompt source map with token breakdown
///   /context json     Machine-readable JSON output
///   /context summary  Brief budget pressure summary
pub fn context(app: &mut App, arg: Option<&str>) -> CommandResult {
    let sub = arg.unwrap_or("report").trim();
    match sub {
        "json" => {
            let json = context_json(app);
            CommandResult::message(json)
        }
        "summary" => {
            let summary = context_summary(app);
            CommandResult::message(summary)
        }
        "report" | "" => {
            let workspace = app.workspace.clone();
            let report = crate::context_report::build_context_report(app, &workspace);
            let formatted = crate::context_report::format_context_report(&report);
            CommandResult::message(formatted)
        }
        _ => {
            let workspace = app.workspace.clone();
            let report = crate::context_report::build_context_report(app, &workspace);
            let formatted = crate::context_report::format_context_report(&report);
            CommandResult::message(formatted)
        }
    }
}

/// Show the prompt source map as JSON (machine-readable).
pub fn context_json(app: &mut App) -> String {
    let workspace = app.workspace.clone();
    let report = crate::context_report::build_context_report(app, &workspace);
    serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!("{{ \"error\": \"{e}\" }}"))
}

/// Show a brief budget pressure summary.
pub fn context_summary(app: &mut App) -> String {
    let workspace = app.workspace.clone();
    let report = crate::context_report::build_context_report(app, &workspace);
    let mut out = String::new();
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "Context budget: {} tokens",
        report.total_estimated_tokens
    );
    if let Some(window) = report.context_window_tokens {
        let pct = report.budget_used_percent.unwrap_or(0.0);
        let bar = if pct > 90.0 {
            "CRITICAL"
        } else if pct > 70.0 {
            "HIGH"
        } else if pct > 40.0 {
            "MODERATE"
        } else {
            "LOW"
        };
        let _ = writeln!(out, "Window: {window} tokens ({pct:.1}% used — {bar})");
    }
    let _ = writeln!(out, "Sources: {} entries", report.entries.len());
    out
}

/// Show per-turn DeepSeek prefix-cache telemetry for the last N turns (#263).
///
/// `arg` is parsed as a count override (default 10, capped at the ring size).
/// Renders a fixed-width table the user can paste into a bug report.
pub fn cache(app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).filter(|s| !s.is_empty());
    if let Some(flags) = arg.and_then(|a| a.strip_prefix("inspect")) {
        let flags = flags.trim();
        let verbose = flags.split_whitespace().any(|flag| flag == "--verbose");
        let json_mode = flags.split_whitespace().any(|flag| flag == "--json");
        return CommandResult::message(format_cache_inspect(app, verbose, json_mode));
    }
    if matches!(arg, Some("warmup")) {
        return CommandResult::action(AppAction::CacheWarmup);
    }
    if matches!(arg, Some("stats")) {
        return CommandResult::message(format_cache_stats(app));
    }
    if matches!(arg, Some("zones")) {
        return CommandResult::message(format_cache_zones(app));
    }

    let want = arg.and_then(|s| s.parse::<usize>().ok()).unwrap_or(10);
    let cap = app.session.turn_cache_history.len();
    let count = want
        .min(cap)
        .min(crate::tui::app::App::TURN_CACHE_HISTORY_CAP);

    if cap == 0 {
        return CommandResult::message(tr(app.ui_locale, MessageId::CmdCacheNoData));
    }

    CommandResult::message(format_cache_history(app, count, app.ui_locale))
}

fn format_cache_inspect(app: &mut App, verbose: bool, json_mode: bool) -> String {
    if verbose && json_mode {
        return "cache inspect: --json and --verbose cannot be combined".to_string();
    }

    let reasoning_effort = if app.reasoning_effort == crate::tui::app::ReasoningEffort::Auto {
        app.last_effective_reasoning_effort
            .and_then(crate::tui::app::ReasoningEffort::api_value)
            .map(str::to_string)
    } else {
        app.reasoning_effort.api_value().map(str::to_string)
    };
    let request = MessageRequest {
        model: app.model.clone(),
        messages: app.api_messages.clone(),
        max_tokens: 0,
        system: app.system_prompt.clone(),
        tools: app.session.last_tool_catalog.clone(),
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(true),
        temperature: None,
        top_p: None,
    };
    let inspection = inspect_prompt_for_request(&request);
    let previous = app.session.last_cache_inspection.as_ref();
    let current_warmup_key = CacheWarmupKey::from_inspection(
        &format!("{:?}", app.api_provider),
        &app.model,
        app.session.last_base_url.as_deref().unwrap_or_default(),
        &inspection,
    );
    let warmup_status =
        format_warmup_status(app.session.last_warmup_key.as_ref(), &current_warmup_key);
    if json_mode {
        let output = serde_json::to_value(&inspection)
            .and_then(|mut value| {
                if let serde_json::Value::Object(ref mut object) = value {
                    object.insert(
                        "current_warmup_key".to_string(),
                        serde_json::to_value(&current_warmup_key)?,
                    );
                    object.insert(
                        "warmup_status".to_string(),
                        serde_json::Value::String(warmup_status.trim_end().to_string()),
                    );
                }
                serde_json::to_string_pretty(&value)
            })
            .unwrap_or_else(|_| {
                "{\"error\":\"cache inspection serialization failed\"}".to_string()
            });
        app.session.last_cache_inspection = Some(inspection);
        return output;
    }

    let mut out = String::new();
    out.push_str("Cache Inspect\n");
    out.push_str("Full prompt text is not printed. Hashes are SHA-256 of each rendered layer.\n");
    out.push_str(&format!(
        "Base static prefix hash: {}\n",
        inspection.base_static_prefix_hash
    ));
    out.push_str(&format!(
        "Full request prefix hash: {}\n",
        inspection.full_request_prefix_hash
    ));
    out.push_str(&format!(
        "Tool catalog hash: {}\n",
        if inspection.tool_catalog_hash.is_empty() {
            "(no tools registered)".to_string()
        } else {
            inspection.tool_catalog_hash.clone()
        }
    ));
    out.push_str(&format_static_prefix_status(previous, &inspection));
    out.push_str(&format_first_divergence(previous, &inspection));
    out.push_str(&warmup_status);
    let total_tokens: usize = inspection
        .layers
        .iter()
        .map(|layer| layer.token_estimate)
        .sum();
    out.push_str(&format!("Estimated reusable tokens: ~{total_tokens}\n"));
    out.push('\n');

    for layer in &inspection.layers {
        let mut line = format!(
            "{}: {}, chars={}, bytes={}, ~{}tok, hash={}\n",
            layer.name,
            layer.stability.label(),
            layer.char_len,
            layer.byte_len,
            layer.token_estimate,
            layer.sha256
        );
        if let Some(tool_result) = &layer.tool_result {
            let trimmed = line.trim_end_matches('\n').to_string();
            line = format!(
                "{trimmed}, original_chars={}, sent_chars={}, truncated={}, deduplicated={}\n",
                tool_result.original_chars,
                tool_result.sent_chars,
                tool_result.truncated,
                tool_result.deduplicated
            );
        }
        if let Some(turn_meta) = &layer.turn_meta {
            let trimmed = line.trim_end_matches('\n').to_string();
            line = format!(
                "{trimmed}, turn_meta_original_chars={}, turn_meta_sent_chars={}, turn_meta_deduplicated={}, turn_meta_sha256={}\n",
                turn_meta.original_chars,
                turn_meta.sent_chars,
                turn_meta.deduplicated,
                turn_meta.sha256
            );
        }
        out.push_str(&line);
    }
    if verbose {
        out.push_str("\nVerbose diff\n");
        if let Some(previous) = previous {
            out.push_str(&format_verbose_diff(previous, &inspection));
        } else {
            out.push_str("No previous inspection to compare against.\n");
        }
    }
    app.session.last_cache_inspection = Some(inspection);
    out
}

fn format_warmup_status(last_warmup: Option<&CacheWarmupKey>, current: &CacheWarmupKey) -> String {
    match last_warmup {
        None => format!(
            "Warmup status: no previous warmup (current key: {})\n",
            current.hash_short()
        ),
        Some(previous) if previous == current => {
            format!(
                "Warmup status: valid (key {} matches)\n",
                current.hash_short()
            )
        }
        Some(previous) => {
            let mut reasons = Vec::new();
            if previous.provider != current.provider {
                reasons.push("provider changed");
            }
            if previous.model != current.model {
                reasons.push("model changed");
            }
            if previous.base_url != current.base_url {
                reasons.push("base URL changed");
            }
            if previous.static_prefix_hash != current.static_prefix_hash {
                reasons.push("static prefix changed");
            }
            if previous.tool_catalog_hash != current.tool_catalog_hash {
                reasons.push("tool catalog changed");
            }
            if previous.project_pack_hash != current.project_pack_hash {
                reasons.push("project pack changed");
            }
            if previous.skills_hash != current.skills_hash {
                reasons.push("skills changed");
            }
            let reason_text = if reasons.is_empty() {
                "unknown prefix input changed".to_string()
            } else {
                reasons.join(", ")
            };
            format!(
                "Warmup status: invalid ({} -> {}; {})\n",
                previous.hash_short(),
                current.hash_short(),
                reason_text
            )
        }
    }
}

fn format_verbose_diff(previous: &PromptInspection, current: &PromptInspection) -> String {
    let mut out = String::new();
    let max_len = previous.layers.len().max(current.layers.len());
    for index in 0..max_len {
        match (previous.layers.get(index), current.layers.get(index)) {
            (Some(prev), Some(curr)) if prev == curr => {
                out.push_str(&format!("  [{index}] {} unchanged\n", curr.name));
            }
            (Some(prev), Some(curr)) => {
                out.push_str(&format!("  [{index}] {} changed\n", curr.name));
                if prev.name != curr.name {
                    out.push_str(&format!("    name: {} -> {}\n", prev.name, curr.name));
                }
                if prev.stability != curr.stability {
                    out.push_str(&format!(
                        "    stability: {} -> {}\n",
                        prev.stability.label(),
                        curr.stability.label()
                    ));
                }
                if prev.char_len != curr.char_len {
                    out.push_str(&format!(
                        "    chars: {} -> {} ({:+})\n",
                        prev.char_len,
                        curr.char_len,
                        curr.char_len as i64 - prev.char_len as i64
                    ));
                }
                if prev.sha256 != curr.sha256 {
                    out.push_str(&format!(
                        "    hash: {} -> {}\n",
                        short_hash(&prev.sha256),
                        short_hash(&curr.sha256)
                    ));
                }
            }
            (None, Some(curr)) => {
                out.push_str(&format!("  [{index}] {} added\n", curr.name));
            }
            (Some(prev), None) => {
                out.push_str(&format!("  [{index}] {} removed\n", prev.name));
            }
            (None, None) => unreachable!("index is within max_len"),
        }
    }
    out
}

fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// Render a prefix-cache stability and health summary for `/cache stats`.
///
/// Surfaces the current prefix fingerprint, stability ratio, change history,
/// and an aggregated cache-hit summary from per-turn telemetry.  When the
/// prefix has changed, a prominent warning is included so users can
/// correlate cache misses with prefix drift.
fn format_cache_stats(app: &App) -> String {
    let mut out = String::new();
    out.push_str("Cache Stats\n");

    // ── Prefix stability ──────────────────────────────────────────────
    out.push_str("\n── Prefix Stability\n");
    match app.prefix_stability_pct {
        Some(pct) => {
            let checks = app.prefix_checks_total;
            let changes = app.prefix_change_count;
            let stable_checks = checks.saturating_sub(changes);

            if changes == 0 {
                out.push_str(&format!(
                    "  Stability: {pct}% ({stable_checks}/{checks} checks)\n"
                ));
                out.push_str("  Status:    stable (no prefix changes this session)\n");
            } else {
                out.push_str(&format!(
                    "  Stability: {pct}% ({stable_checks}/{checks} checks, {changes} change{})\n",
                    if changes == 1 { "" } else { "s" }
                ));
                out.push_str("  Status:    WARNING — prefix has changed\n");
                if let Some(ref desc) = app.last_prefix_change_desc {
                    out.push_str(&format!("  Last change: {desc}\n"));
                }
            }
        }
        None => {
            out.push_str("  Stability: unknown (no checks recorded yet)\n");
            out.push_str("  Run a turn first to collect prefix stability data.\n");
        }
    }

    // ── Prefix fingerprint ────────────────────────────────────────────
    out.push_str("\n── Prefix Fingerprint\n");
    match &app.last_pinned_prefix_hash {
        Some(hash) => {
            out.push_str(&format!("  Pinned hash: {hash}\n"));
            let short = if hash.len() >= 12 { &hash[..12] } else { hash };
            out.push_str(&format!("  Short id:    {short}\n"));
            if app.prefix_change_count > 0 {
                out.push_str("  Drift:       WARNING — hash has changed during this session\n");
                out.push_str(&format!(
                    "               ({change} change{plural} detected)\n",
                    change = app.prefix_change_count,
                    plural = if app.prefix_change_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            } else {
                out.push_str("  Drift:       none (hash stable)\n");
            }
        }
        None => {
            out.push_str("  Pinned hash: unavailable\n");
            out.push_str("  Run a turn first, or use /cache inspect.\n");
        }
    }

    // ── Cache hit-rate summary ────────────────────────────────────────
    out.push_str("\n── Cache Hit Rate\n");
    let history = &app.session.turn_cache_history;
    if history.is_empty() {
        out.push_str("  No turn telemetry recorded yet.\n");
    } else {
        // Aggregate only cache-aware turns; skip turns where the provider
        // did not report cache telemetry (cache_hit_tokens is None).
        // When cache_miss_tokens is None, infer it as
        //   input_tokens − cache_hit_tokens  (matches /cache table logic).
        let mut turns = 0u64;
        let (hit, miss, input) = app.session.turn_cache_history.iter().fold(
            (0u64, 0u64, 0u64),
            |(hit, miss, input), rec| {
                let Some(hit_tokens) = rec.cache_hit_tokens else {
                    return (hit, miss, input);
                };
                let h = u64::from(hit_tokens);
                let m = u64::from(
                    rec.cache_miss_tokens
                        .unwrap_or(rec.input_tokens.saturating_sub(hit_tokens)),
                );
                turns += 1;
                (hit + h, miss + m, input + u64::from(rec.input_tokens))
            },
        );
        let total_cache = hit + miss;
        let avg_pct = if total_cache > 0 {
            (hit as f64 / total_cache as f64 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        out.push_str(&format!("  Turns recorded: {turns}\n"));
        out.push_str(&format!(
            "  Cache hit tokens:  {hit} ({avg_pct:.1}% of {total_cache} cache-aware tokens)\n",
            hit = format_tokens(hit),
            total_cache = format_tokens(total_cache),
        ));
        out.push_str(&format!(
            "  Cache miss tokens: {miss}\n",
            miss = format_tokens(miss),
        ));
        out.push_str(&format!(
            "  Total input tokens: {input}\n",
            input = format_tokens(input),
        ));
        if avg_pct < 80.0 {
            out.push_str("  NOTE: cache hit rate is low (< 80%). Check prefix stability above or consider /compact.\n");
        }
    }

    out
}

/// Render three-zone prefix contract status for `/cache zones` (#2264).
///
/// Displays the PinnedPrefix fingerprint, AppendLog size, and TurnScratch
/// state. The zones are type scaffolding only (Phase 1) — not yet
/// enforcing the full contract at request time.
fn format_cache_zones(app: &App) -> String {
    let mut out = String::new();
    out.push_str("Cache Zones (#2264 three-zone contract, Phase 1 foundation)\n");

    // ── PinnedPrefix ─────────────────────────────────────────────────
    out.push_str("\n── PinnedPrefix (system + tools, frozen baseline)\n");
    match &app.last_pinned_prefix_hash {
        Some(hash) => {
            let short = if hash.len() >= 12 { &hash[..12] } else { hash };
            out.push_str(&format!("  Short id: {short}\n"));
            if app.prefix_change_count > 0 {
                out.push_str(&format!(
                    "  Status:    WARNING — {change} drift{plural} detected\n",
                    change = app.prefix_change_count,
                    plural = if app.prefix_change_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            } else {
                out.push_str("  Status:    stable (no drift this session)\n");
            }
            if let Some(pct) = app.prefix_stability_pct {
                out.push_str(&format!("  Stability: {pct}%\n"));
            }
        }
        None => {
            out.push_str("  Status:    unavailable (not yet frozen)\n");
            out.push_str("  Run a turn first to freeze the baseline.\n");
        }
    }

    // ── AppendLog ────────────────────────────────────────────────────
    out.push_str("\n── AppendLog (conversation history, append-only)\n");
    out.push_str("  Status:      Phase 1 scaffolding — not yet wired into engine\n");
    let msg_count = app.api_messages.len();
    out.push_str(&format!("  Messages:    {msg_count}\n"));
    let history_count = app
        .api_messages
        .iter()
        .filter(|m| m.role != "system")
        .count();
    out.push_str(&format!("  History msgs: {history_count}\n"));

    // ── TurnScratch ──────────────────────────────────────────────────
    out.push_str("\n── TurnScratch (per-turn ephemeral data)\n");
    out.push_str("  Status:      Phase 1 scaffolding — not yet wired into engine\n");

    // ── Zone contract summary ────────────────────────────────────────
    out.push_str("\n── Contract Status\n");
    let has_drift = app.prefix_change_count > 0;
    out.push_str(&format!(
        "  PinnedPrefix: {}\n",
        if app.last_pinned_prefix_hash.is_some() {
            if has_drift {
                "WARNING — drifted"
            } else {
                "OK"
            }
        } else {
            "not frozen"
        }
    ));
    out.push_str("  AppendLog:    Phase 1 foundation\n");
    out.push_str("  TurnScratch:  Phase 1 foundation\n");

    out
}

/// Formats a u64 token count with a compact suffix: K for thousands,
/// M for millions. Never returns scientific notation.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_static_prefix_status(
    previous: Option<&PromptInspection>,
    current: &PromptInspection,
) -> String {
    let Some(previous) = previous else {
        return "Static base prefix stability: no previous request\n".to_string();
    };
    if previous.base_static_prefix_hash == current.base_static_prefix_hash {
        return "Static base prefix stability: OK\n".to_string();
    }

    let changed = changed_static_layers(previous, current);
    if changed.is_empty() {
        "Static base prefix stability: WARNING (base hash changed)\n".to_string()
    } else {
        format!(
            "Static base prefix stability: WARNING changed layers: {}\n",
            changed.join(", ")
        )
    }
}

fn format_first_divergence(
    previous: Option<&PromptInspection>,
    current: &PromptInspection,
) -> String {
    let Some(previous) = previous else {
        return "First divergence from previous request: unavailable\n".to_string();
    };
    let max_len = previous.layers.len().max(current.layers.len());
    for index in 0..max_len {
        match (previous.layers.get(index), current.layers.get(index)) {
            (Some(prev), Some(curr)) if prev.name == curr.name && prev.sha256 == curr.sha256 => {}
            (Some(prev), Some(curr)) if prev.name == curr.name => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (Some(_), Some(curr)) => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (None, Some(curr)) => {
                return format!("First divergence from previous request: {}\n", curr.name);
            }
            (Some(prev), None) => {
                return format!(
                    "First divergence from previous request: {} removed\n",
                    prev.name
                );
            }
            (None, None) => break,
        }
    }
    "First divergence from previous request: none\n".to_string()
}

fn changed_static_layers(previous: &PromptInspection, current: &PromptInspection) -> Vec<String> {
    current
        .layers
        .iter()
        .filter(|layer| layer.stability.label() == "static")
        .filter(|layer| {
            previous
                .layers
                .iter()
                .find(|previous_layer| previous_layer.name == layer.name)
                .is_none_or(|previous_layer| previous_layer.sha256 != layer.sha256)
        })
        .map(|layer| layer.name.clone())
        .collect()
}

fn format_cache_history(app: &App, count: usize, locale: Locale) -> String {
    let total = app.session.turn_cache_history.len();
    let start = total.saturating_sub(count);
    let rows: Vec<&TurnCacheRecord> = app.session.turn_cache_history.iter().skip(start).collect();

    let mut totals_input: u64 = 0;
    let mut totals_hit: u64 = 0;
    let mut totals_miss: u64 = 0;
    let mut header = tr(locale, MessageId::CmdCacheHeader)
        .replace("{count}", &rows.len().to_string())
        .replace("{total}", &total.to_string())
        .replace("{model}", &app.model);
    header.push_str(&"─".repeat(76));
    header.push('\n');
    header.push_str("turn   in    out   hit   miss   replay   ratio   age\n");
    header.push_str(&"─".repeat(76));
    header.push('\n');

    let now = Instant::now();
    let mut body = String::new();
    let absolute_start = total.saturating_sub(rows.len());
    for (i, rec) in rows.iter().enumerate() {
        let turn_index = absolute_start + i + 1;
        totals_input += u64::from(rec.input_tokens);

        let replay_cell = rec
            .reasoning_replay_tokens
            .map_or_else(|| "—".to_string(), |t| t.to_string());
        let age = humanize_age(now.saturating_duration_since(rec.recorded_at));

        // No cache telemetry → render `—` everywhere and don't pollute totals
        // with inferred zeros. Some providers (and some routes inside DeepSeek)
        // skip the cache fields; including a synthesized 0/N for those turns
        // would make every aggregate ratio look broken.
        let Some(hit) = rec.cache_hit_tokens else {
            body.push_str(&format!(
                "{turn:>4}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {replay:>6}   {ratio:>6}   {age}\n",
                turn = turn_index,
                input = rec.input_tokens,
                output = rec.output_tokens,
                hit = "—",
                miss = "—",
                replay = replay_cell,
                ratio = "—",
                age = age,
            ));
            continue;
        };

        let miss_reported = rec.cache_miss_tokens;
        let miss = miss_reported.unwrap_or_else(|| rec.input_tokens.saturating_sub(hit));
        let accounted = u64::from(hit) + u64::from(miss);
        let ratio = if accounted == 0 {
            "    —".to_string()
        } else {
            format!("{:>5.1}%", 100.0 * f64::from(hit) / accounted as f64)
        };
        totals_hit += u64::from(hit);
        totals_miss += u64::from(miss);

        let miss_cell = match miss_reported {
            Some(_) => format!("{miss}"),
            None => format!("{miss}*"),
        };

        body.push_str(&format!(
            "{turn:>4}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {replay:>6}   {ratio}   {age}\n",
            turn = turn_index,
            input = rec.input_tokens,
            output = rec.output_tokens,
            hit = hit,
            miss = miss_cell,
            replay = replay_cell,
            ratio = ratio,
            age = age,
        ));
    }

    let totals_accounted = totals_hit + totals_miss;
    let avg_ratio = if totals_accounted == 0 {
        "—".to_string()
    } else {
        format!(
            "{:.1}%",
            100.0 * totals_hit as f64 / totals_accounted as f64
        )
    };

    let mut footer = String::new();
    footer.push_str(&"─".repeat(76));
    footer.push('\n');
    footer.push_str(
        &tr(locale, MessageId::CmdCacheTotals)
            .replace("{sum_in}", &totals_input.to_string())
            .replace("{sum_hit}", &totals_hit.to_string())
            .replace("{sum_miss}", &totals_miss.to_string())
            .replace("{avg}", &avg_ratio),
    );
    footer.push_str(tr(locale, MessageId::CmdCacheFootnote));
    footer.push_str(tr(locale, MessageId::CmdCacheAdvice));

    format!("{header}{body}{footer}")
}

fn humanize_age(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{ContentBlock, Message, SystemBlock, Tool};
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::history::{GenericToolCell, ToolCell, ToolStatus};
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("/tmp/test-workspace"),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("/tmp/test-skills"),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app.cost_currency = crate::pricing::CostCurrency::Usd;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app
    }

    fn test_tool(name: &str) -> Tool {
        Tool {
            tool_type: Some("function".to_string()),
            name: name.to_string(),
            description: format!("{name} test tool"),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                }
            }),
            allowed_callers: None,
            defer_loading: Some(false),
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        }
    }

    #[test]
    fn test_tokens_shows_usage_info() {
        let mut app = create_test_app();
        app.session.total_tokens = 1234;
        app.session.session_cost = 0.05;
        app.session.last_prompt_tokens = Some(100);
        app.session.last_completion_tokens = Some(25);
        app.session.last_prompt_cache_hit_tokens = Some(70);
        app.session.last_prompt_cache_miss_tokens = Some(30);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "test".to_string(),
                cache_control: None,
            }],
        });
        app.history.push(HistoryCell::User {
            content: "test".to_string(),
        });

        let result = tokens(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Token Usage"));
        assert!(msg.contains("Active context:"));
        assert!(msg.contains("Last API input:"));
        assert!(msg.contains("Last API output:"));
        assert!(msg.contains("Cache hit/miss:"));
        assert!(msg.contains("70 hit / 30 miss"));
        assert!(msg.contains("Cumulative tokens:"));
        assert!(msg.contains("Approx session cost:"));
        assert!(msg.contains("API messages:"));
        assert!(msg.contains("Chat messages:"));
        assert!(msg.contains("Model:"));
    }

    #[test]
    fn test_cost_shows_spending_info() {
        let mut app = create_test_app();
        app.session.session_cost = 0.1234;
        let result = cost(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Session Cost"));
        assert!(msg.contains("Approx total spent:"));
        assert!(msg.contains("approximate"));
        assert!(msg.contains("$0.1234"));
    }

    #[test]
    fn test_system_prompt_displays_text() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text("Test system prompt".to_string()));
        let result = system_prompt(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("System Prompt"));
        assert!(msg.contains("Test system prompt"));
    }

    #[test]
    fn test_system_prompt_displays_blocks() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Blocks(vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: "Block 1".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: "Block 2".to_string(),
                cache_control: None,
            },
        ]));
        let result = system_prompt(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("System Prompt"));
        assert!(msg.contains("Block 1"));
        assert!(msg.contains("Block 2"));
    }

    #[test]
    fn test_system_prompt_none() {
        let mut app = create_test_app();
        app.system_prompt = None;
        let result = system_prompt(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("(no system prompt)"));
    }

    #[test]
    fn test_system_prompt_truncates_long_text() {
        let mut app = create_test_app();
        let long_text = "x".repeat(600);
        app.system_prompt = Some(SystemPrompt::Text(long_text));
        let result = system_prompt(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("..."));
        assert!(msg.contains("chars total"));
    }

    #[test]
    fn cache_command_reports_no_data_before_first_turn() {
        let mut app = create_test_app();
        let result = cache(&mut app, None);
        let msg = result.message.expect("cache produces a message");
        assert!(msg.contains("no turns recorded yet"), "got: {msg}");
    }

    #[test]
    fn cache_inspect_reports_hashes_without_prompt_text() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text(
            "Base policy\n\n<project_instructions source=\"AGENTS.md\">\nSECRET_PROJECT_RULE\n</project_instructions>"
                .to_string(),
        ));
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "SECRET_USER_TASK".to_string(),
                cache_control: None,
            }],
        });

        let result = cache(&mut app, Some("inspect"));
        let msg = result.message.expect("inspect output");

        assert!(msg.contains("Cache Inspect"));
        assert!(msg.contains("Base static prefix hash:"));
        assert!(msg.contains("Full request prefix hash:"));
        assert!(msg.contains("Static base prefix stability: no previous request"));
        assert!(msg.contains("First divergence from previous request: unavailable"));
        assert!(msg.contains("Global system prefix: static"));
        assert!(msg.contains("Project context: static"));
        assert!(msg.contains("User task: dynamic"));
        assert!(!msg.contains("SECRET_PROJECT_RULE"));
        assert!(!msg.contains("SECRET_USER_TASK"));
    }

    #[test]
    fn cache_inspect_uses_last_request_tool_catalog() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text("Base policy".to_string()));
        app.session.last_tool_catalog = Some(vec![test_tool("read_file")]);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Current task".to_string(),
                cache_control: None,
            }],
        });

        let msg = cache(&mut app, Some("inspect"))
            .message
            .expect("inspect output");

        assert!(msg.contains("Tool catalog hash: "), "got: {msg}");
        assert!(!msg.contains("(no tools registered)"), "got: {msg}");
        assert!(msg.contains("Tool catalog: static"), "got: {msg}");
        assert!(msg.contains("bytes="), "got: {msg}");
        assert!(msg.contains("~"), "got: {msg}");
    }

    #[test]
    fn cache_inspect_json_reports_tool_catalog_hash_and_layer_sizes() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text("Base policy".to_string()));
        app.session.last_tool_catalog = Some(vec![test_tool("read_file")]);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Current task".to_string(),
                cache_control: None,
            }],
        });

        let msg = cache(&mut app, Some("inspect --json"))
            .message
            .expect("inspect json output");
        let parsed: serde_json::Value = serde_json::from_str(&msg).expect("valid json");

        assert_eq!(parsed["tool_catalog_hash"].as_str().unwrap().len(), 64);
        assert!(
            parsed["warmup_status"]
                .as_str()
                .is_some_and(|status| status.starts_with("Warmup status: no previous warmup"))
        );
        assert!(parsed["current_warmup_key"].is_object());
        let tool_layer = parsed["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|layer| layer["name"] == "Tool catalog")
            .expect("tool catalog layer");
        assert!(tool_layer["byte_len"].as_u64().unwrap() > 0);
        assert!(tool_layer["token_estimate"].as_u64().unwrap() > 0);
    }

    fn warmup_key(model: &str, static_hash: &str) -> CacheWarmupKey {
        CacheWarmupKey {
            provider: "Deepseek".to_string(),
            model: model.to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            static_prefix_hash: static_hash.to_string(),
            tool_catalog_hash: "tool".to_string(),
            project_pack_hash: "project".to_string(),
            skills_hash: "skills".to_string(),
        }
    }

    #[test]
    fn warmup_status_reports_valid_matching_key() {
        let key = warmup_key("deepseek-v4-pro", "static-a");
        let result = format_warmup_status(Some(&key), &key);
        assert!(result.contains("Warmup status: valid"), "got: {result}");
    }

    #[test]
    fn warmup_status_reports_invalidation_reason() {
        let previous = warmup_key("deepseek-v4-pro", "static-a");
        let current = warmup_key("deepseek-v4-flash", "static-b");
        let result = format_warmup_status(Some(&previous), &current);
        assert!(result.contains("Warmup status: invalid"), "got: {result}");
        assert!(result.contains("model changed"), "got: {result}");
        assert!(result.contains("static prefix changed"), "got: {result}");
    }

    #[test]
    fn warmup_status_reports_project_and_skills_reasons() {
        let previous = warmup_key("deepseek-v4-pro", "static-a");
        let mut current = previous.clone();
        current.project_pack_hash = "project-b".to_string();
        current.skills_hash = "skills-b".to_string();

        let result = format_warmup_status(Some(&previous), &current);

        assert!(result.contains("project pack changed"), "got: {result}");
        assert!(result.contains("skills changed"), "got: {result}");
        assert!(!result.contains("; )"), "got: {result}");
    }

    #[test]
    fn cache_inspect_rejects_json_verbose_combo() {
        let mut app = create_test_app();
        let msg = cache(&mut app, Some("inspect --json --verbose"))
            .message
            .expect("inspect output");

        assert_eq!(
            msg,
            "cache inspect: --json and --verbose cannot be combined"
        );
    }

    #[test]
    fn cache_inspect_json_uses_cjk_aware_token_estimate() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text("缓存命中测试".to_string()));

        let msg = cache(&mut app, Some("inspect --json"))
            .message
            .expect("inspect json output");
        let parsed: serde_json::Value = serde_json::from_str(&msg).expect("valid json");
        let system_layer = parsed["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|layer| layer["name"] == "Global system prefix")
            .expect("system layer");

        assert_eq!(
            system_layer["token_estimate"].as_u64(),
            system_layer["char_len"].as_u64()
        );
    }

    #[test]
    fn cache_inspect_reports_divergence_from_previous_request() {
        let mut app = create_test_app();
        app.system_prompt = Some(SystemPrompt::Text(
            "Base policy\n\n## Environment\n\n- shell: powershell".to_string(),
        ));
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "Prior answer".to_string(),
                cache_control: None,
            }],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "First task".to_string(),
                cache_control: None,
            }],
        });

        let first = cache(&mut app, Some("inspect"))
            .message
            .expect("first inspect output");
        assert!(first.contains("Static base prefix stability: no previous request"));

        if let Some(last) = app.api_messages.last_mut()
            && let Some(crate::models::ContentBlock::Text { text, .. }) = last.content.first_mut()
        {
            *text = "Second task".to_string();
        }

        let second = cache(&mut app, Some("inspect"))
            .message
            .expect("second inspect output");
        assert!(second.contains("Static base prefix stability: OK"));
        assert!(second.contains("First divergence from previous request: User task"));
        assert!(second.contains("Message #1 assistant: history"));
    }

    #[test]
    fn cache_inspect_displays_tool_result_budget_metadata() {
        // Wire dedup persists to the process-global SHA spillover root.
        // Serialize through the same guard other tests use to override
        // that root, so a parallel test pointing it at a temp dir can't
        // make this test's second-sighting dedup silently fail.
        let _spill_guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        // Set a temporary spillover root so wire-dedup can persist
        // SHA-addressed tool-result files without depending on a
        // writable $HOME (nix sandboxes have a read-only home tree).
        let tmp = tempfile::tempdir().expect("tempdir");
        let _restore = {
            let prior = crate::tools::truncate::set_test_spillover_root(Some(
                tmp.path().join(".deepseek").join("tool_outputs"),
            ));
            struct Restore(Option<std::path::PathBuf>);
            impl Drop for Restore {
                fn drop(&mut self) {
                    crate::tools::truncate::set_test_spillover_root(self.0.take());
                }
            }
            Restore(prior)
        };
        let mut app = create_test_app();
        let long_output = format!("{}{}", "A".repeat(7_000), "Z".repeat(7_000));
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "shell_command".to_string(),
                input: serde_json::json!({"command": "cargo test"}),
                caller: None,
            }],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: long_output.clone(),
                is_error: None,
                content_blocks: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "tool-2".to_string(),
                name: "shell_command".to_string(),
                input: serde_json::json!({"command": "cargo test"}),
                caller: None,
            }],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-2".to_string(),
                content: long_output,
                is_error: None,
                content_blocks: None,
            }],
        });

        let result = cache(&mut app, Some("inspect"));
        let msg = result.message.expect("inspect output");

        let tool_budget_lines: Vec<_> = msg
            .lines()
            .filter(|line| line.contains("original_chars=14000"))
            .collect();
        assert_eq!(tool_budget_lines.len(), 2, "got: {msg}");

        let first_sighting = tool_budget_lines
            .iter()
            .find(|line| line.contains("deduplicated=false"))
            .expect("first tool-result sighting should report non-dedup metadata");
        assert!(first_sighting.contains("sent_chars="), "got: {msg}");
        assert!(first_sighting.contains("truncated=true"), "got: {msg}");

        let repeat_sighting = tool_budget_lines
            .iter()
            .find(|line| line.contains("deduplicated=true"))
            .expect("repeat tool-result sighting should report dedup metadata");
        assert!(repeat_sighting.contains("sent_chars="), "got: {msg}");
        assert!(repeat_sighting.contains("truncated=false"), "got: {msg}");
    }

    #[test]
    fn cache_inspect_displays_turn_meta_dedup_metadata() {
        let mut app = create_test_app();
        let turn_meta = format!(
            "<turn_meta>\nCurrent local date: 2026-05-09\n{}\n</turn_meta>",
            "Working set: src/lib.rs\n".repeat(20)
        );
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: turn_meta.clone(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "first task".to_string(),
                    cache_control: None,
                },
            ],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: turn_meta,
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "second task".to_string(),
                    cache_control: None,
                },
            ],
        });

        let result = cache(&mut app, Some("inspect"));
        let msg = result.message.expect("inspect output");

        assert!(msg.contains("turn_meta_original_chars="), "got: {msg}");
        assert!(msg.contains("turn_meta_sent_chars="), "got: {msg}");
        assert!(msg.contains("turn_meta_deduplicated=false"), "got: {msg}");
        assert!(msg.contains("turn_meta_deduplicated=true"), "got: {msg}");
        assert!(msg.contains("turn_meta_sha256="), "got: {msg}");
        assert!(!msg.contains("Working set: src/lib.rs"), "got: {msg}");
    }

    #[test]
    fn cache_command_renders_recorded_turns_with_ratio() {
        let mut app = create_test_app();
        let now = Instant::now();
        // Three turns: 75% hit, 50% hit, miss-only (provider didn't report hit).
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 4_000,
            output_tokens: 200,
            cache_hit_tokens: Some(3_000),
            cache_miss_tokens: Some(1_000),
            reasoning_replay_tokens: None,
            recorded_at: now,
        });
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 6_000,
            output_tokens: 250,
            cache_hit_tokens: Some(3_000),
            cache_miss_tokens: Some(3_000),
            reasoning_replay_tokens: Some(150),
            recorded_at: now,
        });
        // Turn 3: hit reported but provider didn't report miss separately —
        // infer miss = input − hit and mark with `*`.
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 5_000,
            output_tokens: 100,
            cache_hit_tokens: Some(2_500),
            cache_miss_tokens: None,
            reasoning_replay_tokens: None,
            recorded_at: now,
        });
        // Turn 4: no telemetry at all — must not pollute aggregate ratios.
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 1_000,
            output_tokens: 50,
            cache_hit_tokens: None,
            cache_miss_tokens: None,
            reasoning_replay_tokens: None,
            recorded_at: now,
        });

        let result = cache(&mut app, None);
        let msg = result.message.expect("cache produces a message");

        // Header reflects total rows and model.
        assert!(msg.contains("last 4 of 4 turn(s)"), "got: {msg}");
        // Per-turn ratios are rendered.
        assert!(msg.contains("75.0%"), "got: {msg}");
        assert!(msg.contains("50.0%"), "got: {msg}");
        // Turn 3: hit=2500, inferred miss=2500 → 50.0% with `*`-marked miss.
        assert!(msg.contains("2500*"), "got: {msg}");
        // Turn 4 (no telemetry) shows em-dashes and is excluded from totals.
        // Aggregate over turns 1-3: hit=8500, miss=6500 → 56.7%.
        assert!(msg.contains("avg hit ratio: 56.7%"), "got: {msg}");
        // Footer guidance is present.
        assert!(msg.contains("70%"), "got: {msg}");
    }

    #[test]
    fn cache_command_count_argument_clamps_to_history() {
        let mut app = create_test_app();
        for _ in 0..3 {
            app.push_turn_cache_record(TurnCacheRecord {
                input_tokens: 1_000,
                output_tokens: 100,
                cache_hit_tokens: Some(500),
                cache_miss_tokens: Some(500),
                reasoning_replay_tokens: None,
                recorded_at: Instant::now(),
            });
        }
        let result = cache(&mut app, Some("100"));
        let msg = result.message.expect("cache produces a message");
        // Asked for 100 turns, only 3 exist — should report "last 3 of 3".
        assert!(msg.contains("last 3 of 3 turn(s)"), "got: {msg}");
    }

    #[test]
    fn turn_cache_history_is_capped_at_50() {
        let mut app = create_test_app();
        for i in 0..(crate::tui::app::App::TURN_CACHE_HISTORY_CAP + 12) {
            app.push_turn_cache_record(TurnCacheRecord {
                input_tokens: i as u32,
                output_tokens: 1,
                cache_hit_tokens: Some(i as u32),
                cache_miss_tokens: Some(0),
                reasoning_replay_tokens: None,
                recorded_at: Instant::now(),
            });
        }
        assert_eq!(
            app.session.turn_cache_history.len(),
            crate::tui::app::App::TURN_CACHE_HISTORY_CAP
        );
        // Oldest record was evicted; newest record is still at the back.
        assert_eq!(
            app.session.turn_cache_history.back().unwrap().input_tokens,
            (crate::tui::app::App::TURN_CACHE_HISTORY_CAP + 11) as u32
        );
    }

    #[test]
    fn test_context_shows_usage_stats() {
        let mut app = create_test_app();
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        });
        app.history.push(HistoryCell::User {
            content: "Hello".to_string(),
        });

        let result = context(&mut app, None);
        assert!(result.message.is_some(), "/context should return a message");
        let msg = result.message.unwrap();
        assert!(
            msg.contains("Context Report"),
            "should contain report title"
        );
    }

    #[test]
    fn test_undo_conversation_removes_last_exchange() {
        let mut app = create_test_app();
        app.history.push(HistoryCell::User {
            content: "Hello".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Hi".to_string(),
            streaming: false,
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![],
        });

        let initial_history_len = app.history.len();
        let initial_api_len = app.api_messages.len();
        let result = undo_conversation(&mut app);

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Removed"));
        assert!(app.history.len() < initial_history_len);
        assert!(app.api_messages.len() < initial_api_len);
    }

    #[test]
    fn test_undo_conversation_nothing_to_undo() {
        let mut app = create_test_app();
        // Clear any default history
        app.history.clear();
        app.api_messages.clear();
        let result = undo_conversation(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Nothing to undo") || msg.contains("Removed"));
    }

    #[test]
    fn test_retry_with_previous_message() {
        let mut app = create_test_app();
        app.history.push(HistoryCell::User {
            content: "Test message".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Response".to_string(),
            streaming: false,
        });

        let result = retry(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Retrying"));
        assert!(msg.contains("Test message"));
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
    }

    #[test]
    fn test_retry_no_previous_message() {
        let mut app = create_test_app();
        let result = retry(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("No previous request to retry"));
        assert!(result.action.is_none());
    }

    #[test]
    fn test_retry_truncates_long_input() {
        let mut app = create_test_app();
        let long_input = "x".repeat(100);
        app.history.push(HistoryCell::User {
            content: long_input.clone(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Response".to_string(),
            streaming: false,
        });

        let result = retry(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Retrying"));
        assert!(msg.contains("..."));
    }

    #[test]
    fn test_patch_undo_requests_session_resync_after_restore() {
        use crate::snapshot::SnapshotRepo;
        use crate::test_support::lock_test_env;
        use std::sync::MutexGuard;
        use tempfile::tempdir;

        struct HomeGuard {
            prev: Option<std::ffi::OsString>,
            _lock: MutexGuard<'static, ()>,
        }

        impl Drop for HomeGuard {
            fn drop(&mut self) {
                // SAFETY: process-wide lock still held.
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }

        fn scoped_home(home: &std::path::Path) -> HomeGuard {
            let lock = lock_test_env();
            let prev = std::env::var_os("HOME");
            // SAFETY: serialized by the global env lock.
            unsafe {
                std::env::set_var("HOME", home);
            }
            HomeGuard { prev, _lock: lock }
        }

        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), b"original").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(workspace.join("a.txt"), b"modified").unwrap();
        repo.snapshot("post-turn:1").unwrap();

        let mut app = create_test_app();
        app.workspace = workspace.clone();
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "please edit a.txt".to_string(),
                cache_control: None,
            }],
        });

        let result = patch_undo(&mut app);

        assert!(!result.is_error);
        assert!(matches!(
            result.action,
            Some(AppAction::SyncSession {
                ref messages,
                ref workspace,
                ..
            }) if messages == &app.api_messages && workspace == &app.workspace
        ));
    }

    #[test]
    fn test_patch_undo_walks_back_to_older_snapshot_on_repeat() {
        use crate::snapshot::SnapshotRepo;
        use crate::test_support::lock_test_env;
        use std::sync::MutexGuard;
        use tempfile::tempdir;

        struct HomeGuard {
            prev: Option<std::ffi::OsString>,
            _lock: MutexGuard<'static, ()>,
        }

        impl Drop for HomeGuard {
            fn drop(&mut self) {
                // SAFETY: process-wide lock still held.
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }

        fn scoped_home(home: &std::path::Path) -> HomeGuard {
            let lock = lock_test_env();
            let prev = std::env::var_os("HOME");
            // SAFETY: serialized by the global env lock.
            unsafe {
                std::env::set_var("HOME", home);
            }
            HomeGuard { prev, _lock: lock }
        }

        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        let file = workspace.join("a.txt");
        std::fs::write(&file, b"zero").unwrap();
        repo.snapshot("tool:first").unwrap();
        std::fs::write(&file, b"one").unwrap();
        repo.snapshot("tool:second").unwrap();
        std::fs::write(&file, b"two").unwrap();

        let mut app = create_test_app();
        app.workspace = workspace.clone();

        let first = patch_undo(&mut app);
        assert!(!first.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");

        let second = patch_undo(&mut app);
        assert!(!second.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "zero");
    }

    #[test]
    fn test_patch_undo_prunes_tool_turn_context() {
        use crate::snapshot::SnapshotRepo;
        use crate::test_support::lock_test_env;
        use std::sync::MutexGuard;
        use tempfile::tempdir;

        struct HomeGuard {
            prev: Option<std::ffi::OsString>,
            _lock: MutexGuard<'static, ()>,
        }

        impl Drop for HomeGuard {
            fn drop(&mut self) {
                // SAFETY: process-wide lock still held.
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }

        fn scoped_home(home: &std::path::Path) -> HomeGuard {
            let lock = lock_test_env();
            let prev = std::env::var_os("HOME");
            // SAFETY: serialized by the global env lock.
            unsafe {
                std::env::set_var("HOME", home);
            }
            HomeGuard { prev, _lock: lock }
        }

        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        let file = workspace.join("a.txt");
        std::fs::write(&file, b"alpha").unwrap();
        repo.snapshot("tool:call-1").unwrap();
        std::fs::write(&file, b"alpha-fixed").unwrap();

        let mut app = create_test_app();
        app.workspace = workspace.clone();
        app.history.push(HistoryCell::User {
            content: "please edit a.txt".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "I will update the file.".to_string(),
            streaming: false,
        });
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "write_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("a.txt".to_string()),
                output: Some("updated".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));
        app.history.push(HistoryCell::Assistant {
            content: "Done, file is fixed now.".to_string(),
            streaming: false,
        });
        app.tool_cells.insert("call-1".to_string(), 2);

        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "please edit a.txt".to_string(),
                cache_control: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "I will update the file.".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({"path": "a.txt"}),
                    caller: None,
                },
            ],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string(),
                content: "updated".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Done, file is fixed now.".to_string(),
                cache_control: None,
            }],
        });

        let result = patch_undo(&mut app);

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha");
        assert_eq!(app.history.len(), 3);
        assert!(matches!(
            app.history.last(),
            Some(HistoryCell::System { content }) if content.contains("/undo reverted workspace")
        ));
        assert_eq!(app.api_messages.len(), 2);
        assert!(matches!(
            &app.api_messages[0].content[0],
            ContentBlock::Text { text, .. } if text == "please edit a.txt"
        ));
        assert_eq!(app.api_messages[1].content.len(), 1);
        assert!(matches!(
            &app.api_messages[1].content[0],
            ContentBlock::Text { text, .. } if text == "I will update the file."
        ));
    }

    #[test]
    fn test_patch_undo_prunes_pre_turn_context() {
        use crate::snapshot::SnapshotRepo;
        use crate::test_support::lock_test_env;
        use std::sync::MutexGuard;
        use tempfile::tempdir;

        struct HomeGuard {
            prev: Option<std::ffi::OsString>,
            _lock: MutexGuard<'static, ()>,
        }

        impl Drop for HomeGuard {
            fn drop(&mut self) {
                // SAFETY: process-wide lock still held.
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }

        fn scoped_home(home: &std::path::Path) -> HomeGuard {
            let lock = lock_test_env();
            let prev = std::env::var_os("HOME");
            // SAFETY: serialized by the global env lock.
            unsafe {
                std::env::set_var("HOME", home);
            }
            HomeGuard { prev, _lock: lock }
        }

        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        let file = workspace.join("a.txt");
        std::fs::write(&file, b"alpha").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(&file, b"alpha-fixed").unwrap();

        let mut app = create_test_app();
        app.workspace = workspace.clone();
        app.history.push(HistoryCell::User {
            content: "please edit a.txt".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Done, file is fixed now.".to_string(),
            streaming: false,
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "please edit a.txt".to_string(),
                cache_control: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Done, file is fixed now.".to_string(),
                cache_control: None,
            }],
        });

        let result = patch_undo(&mut app);

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha");
        assert_eq!(app.history.len(), 1);
        assert!(matches!(
            app.history.last(),
            Some(HistoryCell::System { content }) if content.contains("/undo reverted workspace")
        ));
        assert!(app.api_messages.is_empty());
    }

    #[test]
    fn test_prune_undone_tool_context_preserves_prior_tool_pairs() {
        let mut app = create_test_app();
        app.history.push(HistoryCell::User {
            content: "edit two files".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "I will update both files.".to_string(),
            streaming: false,
        });
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "write_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("a.txt".to_string()),
                output: Some("updated a".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "write_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("b.txt".to_string()),
                output: Some("updated b".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));
        app.history.push(HistoryCell::Assistant {
            content: "Done.".to_string(),
            streaming: false,
        });
        app.tool_cells.insert("call-a".to_string(), 2);
        app.tool_cells.insert("call-b".to_string(), 3);

        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "edit two files".to_string(),
                cache_control: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "I will update both files.".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "call-a".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({"path": "a.txt"}),
                    caller: None,
                },
                ContentBlock::ToolUse {
                    id: "call-b".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({"path": "b.txt"}),
                    caller: None,
                },
            ],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-a".to_string(),
                content: "updated a".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        });
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-b".to_string(),
                content: "updated b".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        });
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Done.".to_string(),
                cache_control: None,
            }],
        });

        prune_undone_tool_context(&mut app, "call-b");

        assert_eq!(app.history.len(), 3);
        assert_eq!(app.api_messages.len(), 3);
        assert!(matches!(
            &app.api_messages[1].content[..],
            [
                ContentBlock::Text { .. },
                ContentBlock::ToolUse { id, .. }
            ] if id == "call-a"
        ));
        assert!(matches!(
            &app.api_messages[2].content[0],
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call-a"
        ));
    }

    // ── /cache stats tests ──────────────────────────────────────────────

    #[test]
    fn cache_stats_no_data_before_first_turn() {
        let mut app = create_test_app();
        let result = cache(&mut app, Some("stats"));
        let msg = result.message.expect("cache stats produces a message");
        assert!(msg.contains("Cache Stats"), "got: {msg}");
        assert!(
            msg.contains("unknown (no checks recorded yet)"),
            "got: {msg}"
        );
        assert!(msg.contains("Pinned hash: unavailable"), "got: {msg}");
        assert!(msg.contains("No turn telemetry recorded yet"), "got: {msg}");
    }

    #[test]
    fn cache_stats_shows_stable_prefix_with_hash() {
        let mut app = create_test_app();
        app.prefix_stability_pct = Some(100);
        app.prefix_checks_total = 5;
        app.prefix_change_count = 0;
        app.last_pinned_prefix_hash =
            Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string());

        let result = cache(&mut app, Some("stats"));
        let msg = result.message.expect("cache stats produces a message");

        assert!(msg.contains("Stability: 100%"), "got: {msg}");
        assert!(msg.contains("stable (no prefix changes"), "got: {msg}");
        assert!(msg.contains("Pinned hash: a1b2c3d4e5f6"), "got: {msg}");
        assert!(
            msg.contains("Drift:       none (hash stable)"),
            "got: {msg}"
        );
    }

    #[test]
    fn cache_stats_warns_on_prefix_change() {
        let mut app = create_test_app();
        app.prefix_stability_pct = Some(67);
        app.prefix_checks_total = 3;
        app.prefix_change_count = 1;
        app.last_prefix_change_desc =
            Some("prefix cache invalidated: system prompt changed".to_string());
        app.last_pinned_prefix_hash = Some(
            "deadbeef0000deadbeef0000deadbeef0000deadbeef0000deadbeef0000deadbeef".to_string(),
        );

        let result = cache(&mut app, Some("stats"));
        let msg = result.message.expect("cache stats produces a message");

        assert!(msg.contains("Stability: 67%"), "got: {msg}");
        assert!(msg.contains("WARNING — prefix has changed"), "got: {msg}");
        assert!(msg.contains("system prompt changed"), "got: {msg}");
        assert!(msg.contains("Drift:       WARNING"), "got: {msg}");
        assert!(msg.contains("1 change detected"), "got: {msg}");
    }

    #[test]
    fn cache_stats_shows_cache_hit_summary() {
        let mut app = create_test_app();
        app.prefix_stability_pct = Some(100);
        app.prefix_checks_total = 1;
        app.last_pinned_prefix_hash =
            Some("abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string());

        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_hit_tokens: Some(8_000),
            cache_miss_tokens: Some(2_000),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        });
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 5_000,
            output_tokens: 500,
            cache_hit_tokens: Some(4_500),
            cache_miss_tokens: Some(500),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        });

        let result = cache(&mut app, Some("stats"));
        let msg = result.message.expect("cache stats produces a message");

        assert!(msg.contains("Turns recorded: 2"), "got: {msg}");
        // Total: 12,500 hit out of 15,000 cache-aware = 83.3%
        assert!(msg.contains("83.3%"), "got: {msg}");
    }

    #[test]
    fn cache_stats_low_hit_rate_shows_note() {
        let mut app = create_test_app();
        app.prefix_stability_pct = Some(100);
        app.prefix_checks_total = 1;
        app.last_pinned_prefix_hash =
            Some("abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string());

        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_hit_tokens: Some(1_000),
            cache_miss_tokens: Some(9_000),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        });

        let result = cache(&mut app, Some("stats"));
        let msg = result.message.expect("cache stats produces a message");

        // 10% hit rate → below 80% threshold
        assert!(msg.contains("10.0%"), "got: {msg}");
        assert!(
            msg.contains("cache hit rate is low"),
            "should show low-hit-rate advisory, got: {msg}"
        );
    }

    #[test]
    fn format_tokens_handles_all_scales() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(15_500), "15.5K");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}

/// Remove last message pair (user + assistant).
///
/// This is the old `/undo` behaviour — it removes the most recent
/// user+assistant conversation pair from history and API messages.
/// The new `/undo` first tries to revert workspace files via
/// [`patch_undo`]; if no snapshots are available it falls back to
/// this function.
pub fn undo_conversation(app: &mut App) -> CommandResult {
    // Remove from display history (up to the last user message)
    let mut removed_count = 0;
    while !app.history.is_empty() {
        let last_is_user = matches!(app.history.last(), Some(HistoryCell::User { .. }));
        app.pop_history();
        removed_count += 1;
        if last_is_user {
            break;
        }
    }

    // Remove from API messages
    while let Some(last) = app.api_messages.last() {
        if last.role == "user" {
            app.api_messages.pop();
            break;
        }
        app.api_messages.pop();
    }

    if removed_count > 0 {
        // Keep tool/index mappings consistent after truncation.
        app.tool_cells.clear();
        app.tool_details_by_cell.clear();
        app.exploring_entries.clear();
        app.ignored_tool_calls.clear();
        app.mark_history_updated();
        CommandResult::message(format!("Removed {removed_count} message(s)"))
    } else {
        CommandResult::message("Nothing to undo")
    }
}

fn prune_undone_tool_context(app: &mut App, tool_id: &str) {
    if let Some(history_idx) = app.tool_cells.get(tool_id).copied() {
        app.truncate_history_to(history_idx);
    }

    let Some((msg_idx, block_idx)) =
        app.api_messages
            .iter()
            .enumerate()
            .find_map(|(msg_idx, msg)| {
                msg.content
                    .iter()
                    .position(
                        |block| matches!(block, ContentBlock::ToolUse { id, .. } if id == tool_id),
                    )
                    .map(|block_idx| (msg_idx, block_idx))
            })
    else {
        return;
    };

    let kept_blocks = app.api_messages[msg_idx].content[..block_idx].to_vec();
    let kept_tool_ids: std::collections::HashSet<String> = kept_blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    if kept_blocks.is_empty() {
        app.api_messages.truncate(msg_idx);
        return;
    }
    let preserved_tool_results: Vec<_> =
        app.api_messages
            .iter()
            .skip(msg_idx + 1)
            .take_while(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg
                        .content
                        .iter()
                        .all(|block| tool_result_id(block).is_some())
            })
            .filter(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg.content.iter().all(|block| {
                        tool_result_id(block).is_some_and(|id| kept_tool_ids.contains(id))
                    })
            })
            .cloned()
            .collect();
    app.api_messages.truncate(msg_idx + 1);
    app.api_messages[msg_idx].content = kept_blocks;
    app.api_messages.extend(preserved_tool_results);
}

fn prune_undone_turn_context(app: &mut App) {
    if let Some(history_idx) = app
        .history
        .iter()
        .rposition(|cell| matches!(cell, HistoryCell::User { .. }))
    {
        app.truncate_history_to(history_idx);
    }

    if let Some(api_idx) = app.api_messages.iter().rposition(|msg| msg.role == "user") {
        app.api_messages.truncate(api_idx);
    }
}

fn tool_result_id(block: &ContentBlock) -> Option<&String> {
    match block {
        ContentBlock::ToolResult { tool_use_id, .. }
        | ContentBlock::ToolSearchToolResult { tool_use_id, .. }
        | ContentBlock::CodeExecutionToolResult { tool_use_id, .. } => Some(tool_use_id),
        _ => None,
    }
}

/// Revert the most recent write tool (apply_patch/edit_file/write_file) or turn.
///
/// Opens the side-git snapshot repo and finds the most recent snapshot,
/// preferring per-tool snapshots (`tool:*`) over pre-turn snapshots
/// (`pre-turn:*`). Restores files from that snapshot and shows a diff
/// summary. Falls back to conversation undo when no snapshots exist.
///
/// Posts a `HistoryCell::System` entry so the user can see what was
/// reverted in the transcript.
pub fn patch_undo(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let repo = match crate::snapshot::SnapshotRepo::open_or_init(&workspace) {
        Ok(r) => r,
        Err(e) => {
            return CommandResult::error(format!(
                "Snapshot repo unavailable for {}: {e}",
                workspace.display(),
            ));
        }
    };

    let snapshots = match repo.list(20) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to list snapshots: {e}"));
        }
    };

    if snapshots.is_empty() {
        return CommandResult::message("No snapshots found to undo — nothing to revert.");
    }

    // Prefer the newest revertable `tool:` / `pre-turn:` snapshot whose
    // tracked content differs from the current workspace. This lets
    // repeated `/undo` walk back through older snapshots instead of
    // restoring the same no-op target forever.
    let target = snapshots
        .iter()
        .filter(|s| s.label.starts_with("tool:") || s.label.starts_with("pre-turn:"))
        .find(|s| match repo.work_tree_matches_snapshot(&s.id) {
            Ok(matches) => !matches,
            Err(_) => true,
        });

    let Some(target) = target else {
        return CommandResult::message(
            "No older tool or pre-turn snapshots differ from the current workspace — nothing to revert.",
        );
    };

    if let Err(e) = repo.restore(&target.id) {
        return CommandResult::error(format!("Restore failed: {e}"));
    }

    if let Some(tool_id) = target.label.strip_prefix("tool:") {
        prune_undone_tool_context(app, tool_id);
    } else if target.label.starts_with("pre-turn:") {
        prune_undone_turn_context(app);
    }

    // Show diff stat so the user knows what changed.
    let diff_stat = Git::command()
        .map(|mut git| {
            git.args(["diff", "--stat"])
                .current_dir(&workspace)
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        })
        .unwrap_or(None);

    let short = &target.id.as_str()[..target.id.as_str().len().min(8)];
    let summary = match diff_stat {
        Some(ref stat) => {
            format!(
                "Restored snapshot '{}' ({}). Files affected:\n{stat}",
                target.label, short
            )
        }
        None => {
            format!(
                "Restored snapshot '{}' ({}). No diff changes detected.",
                target.label, short
            )
        }
    };

    // Post a system cell so the reverted state is visible in the transcript.
    app.push_history_cell(HistoryCell::System {
        content: format!(
            "/undo reverted workspace to snapshot '{}' ({})",
            target.label, short
        ),
    });

    CommandResult::with_message_and_action(
        summary,
        AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
        },
    )
}

/// Load the last user message back into the composer for editing.
///
/// Searches `app.history` for the most recent `HistoryCell::User`, copies its
/// content into `app.input`, and positions the cursor at the end so the user
/// can edit and press Enter to resubmit. The original exchange stays visible
/// in the transcript.
pub fn edit(app: &mut App) -> CommandResult {
    let last_user = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user {
        Some(content) => {
            app.input = content;
            app.cursor_position = app.input.chars().count();
            app.edit_in_progress = true;
            CommandResult::message(
                "Last message loaded into composer — edit and press Enter to resubmit",
            )
        }
        None => CommandResult::message("No previous message to edit"),
    }
}

/// Show git diff output since session start.
///
/// Runs `git diff --stat` and `git diff --name-only` in the workspace
/// directory. Displays which files have changed and a stat summary. If no
/// changes exist or git fails, returns an appropriate message.
pub fn diff(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let Some(mut name_only_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let Some(mut stat_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let name_only_output = name_only_cmd
        .args(["diff", "--name-only"])
        .current_dir(&workspace)
        .output();
    let stat_output = stat_cmd
        .args(["diff", "--stat"])
        .current_dir(&workspace)
        .output();

    match (name_only_output, stat_output) {
        (Ok(name_only), Ok(stat)) => {
            let name_stdout = String::from_utf8_lossy(&name_only.stdout);
            let stat_stdout = String::from_utf8_lossy(&stat.stdout);

            if name_stdout.trim().is_empty() {
                return CommandResult::message("No changes since session start");
            }

            let files: Vec<&str> = name_stdout.lines().filter(|l| !l.is_empty()).collect();
            let file_count = files.len();
            let file_list = files.join("\n");

            // Detect rename entries (e.g. "foo -> bar") and exclude them
            // from the file-count header so the user sees only actual
            // modifications.
            let renamed_count = files.iter().filter(|f| f.contains(" -> ")).count();
            let summary = if renamed_count > 0 {
                format!("Changed files ({file_count}, {renamed_count} renamed):\n{file_list}")
            } else {
                format!("Changed files ({file_count}):\n{file_list}")
            };

            let stat_str = stat_stdout.trim();
            let mut message = summary;
            if !stat_str.is_empty() {
                message.push_str("\n\n── Stat ──\n");
                message.push_str(stat_str);
            }
            CommandResult::message(message)
        }
        (Err(e), _) | (_, Err(e)) => {
            CommandResult::message(format!("Git diff failed — is this a git repository?\n{e}"))
        }
    }
}

/// Retry last request - remove last exchange and re-send the user's message
pub fn retry(app: &mut App) -> CommandResult {
    let last_user_input = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user_input {
        Some(input) => {
            undo_conversation(app);
            let display_input = if input.len() > 50 {
                let truncate_at = input
                    .char_indices()
                    .take_while(|(i, _)| *i <= 50)
                    .last()
                    .map_or(0, |(i, _)| i);
                format!("{}...", &input[..truncate_at])
            } else {
                input.clone()
            };
            CommandResult::with_message_and_action(
                format!("Retrying: {display_input}"),
                AppAction::SendMessage(input),
            )
        }
        None => CommandResult::error("No previous request to retry"),
    }
}
