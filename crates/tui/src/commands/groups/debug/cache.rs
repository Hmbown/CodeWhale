//! `/cache` command — per-turn prefix-cache telemetry and inspection.

use std::time::Instant;

use super::CommandResult;
use crate::client::{CacheWarmupKey, PromptInspection, inspect_prompt_for_request};
use crate::localization::{Locale, MessageId, tr};
use crate::models::MessageRequest;
use crate::tui::app::{App, AppAction, TurnCacheRecord};

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

    let Some(target) = app.cache_replay_target() else {
        return "cache inspect: Auto has no concrete route yet; send a turn first".to_string();
    };
    let Some(replay_base_url) = target.base_url.as_deref() else {
        return "cache inspect: the restored Auto route has no captured endpoint; send a turn first"
            .to_string();
    };
    let reasoning_effort = app
        .reasoning_effort_api_value_for_replay(target.provider, replay_base_url, &target.model)
        .map(str::to_string);
    let request = MessageRequest {
        model: target.model.clone(),
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
        &target.provider_identity,
        &target.model,
        replay_base_url,
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

pub(crate) fn format_warmup_status(
    last_warmup: Option<&CacheWarmupKey>,
    current: &CacheWarmupKey,
) -> String {
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

            let drift = app.prefix_drift_count;
            if changes == 0 {
                out.push_str(&format!(
                    "  Stability: {pct}% ({stable_checks}/{checks} checks)\n"
                ));
                out.push_str("  Status:    stable (no prefix changes this session)\n");
                if app.prefix_context_updates > 0 {
                    out.push_str(&format!(
                        "  Context updates: {} (workspace drift delivered as history, header unchanged)\n",
                        app.prefix_context_updates
                    ));
                }
            } else {
                out.push_str(&format!(
                    "  Stability: {pct}% ({stable_checks}/{checks} checks, {changes} change{})\n",
                    if changes == 1 { "" } else { "s" }
                ));
                if drift == 0 {
                    out.push_str(
                        "  Status:    stable (all changes were declared header changes)\n",
                    );
                } else {
                    out.push_str(&format!(
                        "  Status:    WARNING — {drift} undeclared drift{}\n",
                        if drift == 1 { "" } else { "s" }
                    ));
                }
                if let Some(ref reason) = app.prefix_pin_reason {
                    out.push_str(&format!("  Pin reason: {reason}\n"));
                }
                if app.prefix_context_updates > 0 {
                    out.push_str(&format!(
                        "  Context updates: {} (workspace drift delivered as history, header unchanged)\n",
                        app.prefix_context_updates
                    ));
                }
                if let Some(ref reason) = app.prefix_last_miss_reason {
                    out.push_str(&format!("  Last miss:  {reason}\n"));
                }
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
            if app.prefix_drift_count > 0 {
                out.push_str("  Drift:       WARNING — undeclared hash change this session\n");
                out.push_str(&format!(
                    "               ({change} change{plural} detected, {drift} undeclared)\n",
                    change = app.prefix_change_count,
                    plural = if app.prefix_change_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                    drift = app.prefix_drift_count,
                ));
            } else if app.prefix_change_count > 0 {
                out.push_str("  Drift:       none (all changes were declared)\n");
                out.push_str(&format!(
                    "               ({change} change{plural} detected)\n",
                    change = app.prefix_change_count,
                    plural = if app.prefix_change_count == 1 {
                        ""
                    } else {
                        "s"
                    },
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
pub(crate) fn format_tokens(n: u64) -> String {
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

/// Column header for the per-turn cache/cost table. The widths here must match
/// the row format strings below.
const TURN_CACHE_ROW_HEADER: &str = "turn  route                        in    out    hit   miss  write  replay    ratio        cost   age";

/// Rule width for the table. Sized to the header above.
const TURN_CACHE_TABLE_WIDTH: usize = 106;

/// Render one turn's cost cell, collecting the reason when it has none.
///
/// A turn with no route provenance (legacy or synthetic record) and a turn on a
/// route that is not money-metered both render as `—` — neither is a real
/// zero-dollar charge.
fn turn_cost_cell(
    rec: &TurnCacheRecord,
    currency: crate::pricing::CostCurrency,
    unpriced_notes: &mut std::collections::BTreeSet<&'static str>,
) -> String {
    let Some(audit) = rec.cost_audit.as_ref() else {
        return "—".to_string();
    };
    if audit.is_priced_in(currency)
        && let Some(estimate) = audit.estimate
    {
        return crate::pricing::format_cost_amount_precise(estimate.amount(currency), currency);
    }
    if let Some(reason) = audit.unpriced_reason {
        unpriced_notes.insert(reason.label());
    }
    for class in &audit.unpriced_classes {
        unpriced_notes.insert(class.label());
    }
    "—".to_string()
}

fn format_cache_history(app: &App, count: usize, locale: Locale) -> String {
    let total = app.session.turn_cache_history.len();
    let start = total.saturating_sub(count);
    let rows: Vec<&TurnCacheRecord> = app.session.turn_cache_history.iter().skip(start).collect();

    let mut totals_input: u64 = 0;
    let mut totals_hit: u64 = 0;
    let mut totals_miss: u64 = 0;
    let mut totals_write: u64 = 0;
    let mut totals_reasoning: u64 = 0;
    let currency = app.cost_display_currency(app.cost_currency);
    // Non-secret audit trail for turns whose spend is missing from the session
    // total, so a `—` in the cost column is always explainable.
    let mut unpriced_notes: std::collections::BTreeSet<&'static str> =
        std::collections::BTreeSet::new();
    let mut header = tr(locale, MessageId::CmdCacheHeader)
        .replace("{count}", &rows.len().to_string())
        .replace("{total}", &total.to_string())
        .replace("{model}", &app.model);
    header.push_str(&"─".repeat(TURN_CACHE_TABLE_WIDTH));
    header.push('\n');
    header.push_str(TURN_CACHE_ROW_HEADER);
    header.push('\n');
    header.push_str(&"─".repeat(TURN_CACHE_TABLE_WIDTH));
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
        let classes = crate::pricing::token_usage_for_pricing(&crate::models::Usage {
            input_tokens: rec.input_tokens,
            output_tokens: rec.output_tokens,
            prompt_cache_hit_tokens: rec.cache_hit_tokens,
            prompt_cache_miss_tokens: rec.cache_miss_tokens,
            prompt_cache_write_tokens: rec.cache_write_tokens,
            reasoning_tokens: rec.reasoning_tokens,
            reasoning_replay_tokens: rec.reasoning_replay_tokens,
            server_tool_use: None,
        });
        let write = u32::try_from(classes.cache_write).unwrap_or(u32::MAX);
        let write_cell = rec
            .cache_write_tokens
            .map_or_else(|| "—".to_string(), |_| write.to_string());
        totals_write += classes.cache_write;
        totals_reasoning += u64::from(rec.reasoning_tokens.unwrap_or(0));
        let cost_cell = turn_cost_cell(rec, currency, &mut unpriced_notes);
        let route_cell = format_turn_cache_route(rec);
        let age = humanize_age(now.saturating_duration_since(rec.recorded_at));

        // No cache telemetry → render `—` everywhere and don't pollute totals
        // with inferred zeros. Some providers (and some routes inside DeepSeek)
        // skip the cache fields; including a synthesized 0/N for those turns
        // would make every aggregate ratio look broken.
        if rec.cache_hit_tokens.is_none()
            && rec.cache_miss_tokens.is_none()
            && rec.cache_write_tokens.is_none()
        {
            body.push_str(&format!(
                "{turn:>4}  {route:<24}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {write:>5}  {replay:>6}   {ratio:>6}   {cost:>9}   {age}\n",
                turn = turn_index,
                route = route_cell,
                input = rec.input_tokens,
                output = rec.output_tokens,
                hit = "—",
                miss = "—",
                write = write_cell,
                replay = replay_cell,
                ratio = "—",
                cost = cost_cell,
                age = age,
            ));
            continue;
        }

        let miss_reported = rec.cache_miss_tokens;
        let hit = u32::try_from(classes.cache_read).unwrap_or(u32::MAX);
        let miss = u32::try_from(classes.input).unwrap_or(u32::MAX);
        // Use the same mutually-exclusive hit/miss/write partition as pricing.
        // Inferring `input - hit` here and then adding write counted creation
        // tokens twice in exactly the turns with a write premium.
        let accounted = u64::from(hit) + u64::from(miss) + u64::from(write);
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
            "{turn:>4}  {route:<24}  {input:>5}  {output:>5}  {hit:>5}  {miss:>5}  {write:>5}  {replay:>6}   {ratio}   {cost:>9}   {age}\n",
            turn = turn_index,
            route = route_cell,
            input = rec.input_tokens,
            output = rec.output_tokens,
            hit = hit,
            miss = miss_cell,
            write = write_cell,
            replay = replay_cell,
            ratio = ratio,
            cost = cost_cell,
            age = age,
        ));
    }

    // Anthropic-normalized aggregate: hit / (hit + miss + write).
    let totals_accounted = totals_hit + totals_miss + totals_write;
    let avg_ratio = if totals_accounted == 0 {
        "—".to_string()
    } else {
        format!(
            "{:.1}%",
            100.0 * totals_hit as f64 / totals_accounted as f64
        )
    };

    let mut footer = String::new();
    footer.push_str(&"─".repeat(TURN_CACHE_TABLE_WIDTH));
    footer.push('\n');
    // Reasoning is reported separately from `sum_out` on purpose: providers
    // count it *inside* the completion tokens they bill, so adding the two
    // would double-count it.
    footer.push_str(&format!(
        "sum_write: {totals_write}  sum_reasoning: {totals_reasoning} (already inside out)\n"
    ));
    footer.push_str(
        &tr(locale, MessageId::CmdCacheTotals)
            .replace("{sum_in}", &totals_input.to_string())
            .replace("{sum_hit}", &totals_hit.to_string())
            .replace("{sum_miss}", &totals_miss.to_string())
            .replace("{avg}", &avg_ratio),
    );
    footer.push_str(&tr(locale, MessageId::CmdCacheFootnote));
    if !unpriced_notes.is_empty() {
        footer.push_str(&format!(
            "cost — = no authoritative price for that turn; it is missing from the session estimate ({}).\n",
            unpriced_notes.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    footer.push_str(&tr(locale, MessageId::CmdCacheAdvice));

    format!("{header}{body}{footer}")
}

fn format_turn_cache_route(rec: &TurnCacheRecord) -> String {
    let Some(model) = rec.model.as_deref().filter(|model| !model.is_empty()) else {
        return "—".to_string();
    };
    let provider = rec
        .provider_identity
        .as_deref()
        .filter(|provider| !provider.trim().is_empty())
        .or_else(|| rec.provider.map(|provider| provider.as_str()))
        .unwrap_or("?");
    let route = if rec.auto_model {
        format!("auto:{provider}/{model}")
    } else {
        format!("{provider}/{model}")
    };
    truncate_route_cell(&route, 24)
}

fn truncate_route_cell(route: &str, max_chars: usize) -> String {
    if route.chars().count() <= max_chars {
        return route.to_string();
    }
    if max_chars <= 3 {
        return route.chars().take(max_chars).collect();
    }
    let mut out: String = route.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

fn humanize_age(d: std::time::Duration) -> String {
    crate::elapsed::format_elapsed_secs(d.as_secs())
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn cache_route_keeps_exact_named_custom_identity() {
        let record = TurnCacheRecord {
            provider: Some(crate::config::ApiProvider::Custom),
            provider_identity: Some("lm-studio".to_string()),
            model: Some("local-code-model".to_string()),
            auto_model: false,
            input_tokens: 1,
            output_tokens: 1,
            cache_hit_tokens: None,
            cache_miss_tokens: None,
            reasoning_replay_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_audit: None,
            recorded_at: Instant::now(),
        };

        assert_eq!(format_turn_cache_route(&record), "lm-studio/local-code-...");
    }
}
