//! Row builders for the dock views that are not work rows: files, notepad,
//! context, git, and price. Each returns ordinary [`WorkRow`]s so the one
//! row/hitbox pipeline in `render/` paints, selects, and clicks them exactly
//! like a to-do or a sub-agent — a view is a subset of one row grammar, not
//! a second widget system.

use crate::tui::agent_roster::format_tokens;
use crate::tui::app::{App, SidebarRowAction};

use super::model::{RailPanel, WorkRow, WorkRowId, WorkTone};

/// Views whose tab always shows once the dock is up: the fact views have
/// something to say in every session, so they are always one click away.
pub(super) const fn view_always_has_content(panel: RailPanel) -> bool {
    matches!(
        panel,
        RailPanel::Context | RailPanel::Git | RailPanel::Price
    )
}

pub(super) fn files_touched_count(_app: &mut App) -> usize {
    0
}

pub(super) fn notepad_has_text(_app: &App) -> bool {
    false
}

pub(super) fn files_rows(_app: &mut App) -> Vec<WorkRow> {
    Vec::new()
}

pub(super) fn notepad_rows(_app: &mut App) -> Vec<WorkRow> {
    Vec::new()
}

/// The context view: the budget, not a fact list. Used/limit and the
/// compaction threshold from the same snapshot the footer meter reads, a
/// gauge, then the breakdown the accounting can already give per frame —
/// system prompt, tool schemas, conversation, tool output, files read — and
/// the one action that exists as a command, `/compact`.
pub(super) fn context_rows(app: &mut App) -> Vec<WorkRow> {
    let Some((used, max, percent)) = crate::tui::ui::context_usage_snapshot(app) else {
        return Vec::new();
    };
    let used = u64::try_from(used).unwrap_or(0);
    let threshold = app.auto_compact_threshold_percent.round().clamp(0.0, 100.0) as u8;
    let fact = |id: &str, label: String, detail: &str| WorkRow {
        id: WorkRowId(format!("context:{id}")),
        mark: "·",
        label,
        detail: detail.to_string(),
        tone: WorkTone::Muted,
        selectable: false,
        primary_action: None,
        agent: None,
    };
    let mut out = vec![WorkRow {
        id: WorkRowId("context:budget".to_string()),
        mark: "◔",
        label: format!(
            "{} of {} · {}% · compacts at {threshold}%",
            format_tokens(used),
            format_tokens(u64::from(max)),
            percent.round() as u8
        ),
        detail: "/context for the full source map".to_string(),
        tone: if percent >= f64::from(threshold) {
            WorkTone::Attention
        } else {
            WorkTone::Live
        },
        selectable: true,
        primary_action: Some(SidebarRowAction::Command("/context".to_string())),
        agent: None,
    }];
    out.push(fact("gauge", gauge(percent, 24), ""));

    // Breakdown from what is already counted per frame: the system prompt
    // estimate the footer meter uses and the per-message token cache. No
    // text is re-scanned here; a message is tool output when every block in
    // it is a tool result.
    let system_tokens =
        crate::compaction::estimate_input_tokens_conservative(&[], app.system_prompt.as_ref());
    let tools = app
        .session
        .last_tool_catalog
        .as_ref()
        .map(|catalog| catalog.len())
        .unwrap_or(0);
    let (conversation, tool_output, messages) = message_split(app);
    out.push(fact(
        "system",
        format!(
            "system + tools · {} · {tools} tools",
            format_tokens(system_tokens as u64)
        ),
        "",
    ));
    out.push(fact(
        "conversation",
        format!(
            "conversation · {messages} · {}",
            format_tokens(conversation)
        ),
        "",
    ));
    out.push(fact(
        "tool-output",
        format!("tool output · {}", format_tokens(tool_output)),
        "",
    ));
    let read = super::model::settled_file_activity(app).read;
    if !read.is_empty() {
        out.push(fact(
            "files",
            format!("files read · {}", read.len()),
            &read.join(", "),
        ));
    }
    out.push(WorkRow {
        id: WorkRowId("context:compact".to_string()),
        mark: "▸",
        label: "compact now".to_string(),
        detail: "/compact".to_string(),
        tone: WorkTone::Live,
        selectable: true,
        primary_action: Some(SidebarRowAction::Command("/compact".to_string())),
        agent: None,
    });
    app.work_surface.latest_rows = out.clone();
    out
}

/// `████████░░░░░░░░` — `width` cells, filled to `percent`.
fn gauge(percent: f64, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    let mut bar = String::with_capacity(width * 3);
    for _ in 0..filled {
        bar.push('█');
    }
    for _ in filled..width {
        bar.push('░');
    }
    bar
}

/// `(conversation tokens, tool-output tokens, message count)` from the
/// per-message estimate cache the meter already maintains. Messages the
/// cache has not seen yet count as zero rather than being re-estimated on
/// the render path.
fn message_split(app: &App) -> (u64, u64, usize) {
    let cache = app.context_token_cache.borrow();
    let mut conversation = 0u64;
    let mut tool_output = 0u64;
    for (index, message) in app.api_messages.iter().enumerate() {
        let tokens = cache
            .message_tokens
            .get(index)
            .map(|tokens| (*tokens as u64).saturating_mul(3).div_ceil(2))
            .unwrap_or(0);
        let all_tool_results = !message.content.is_empty()
            && message.content.iter().all(|block| {
                matches!(
                    block,
                    crate::models::ContentBlock::ToolResult { .. }
                        | crate::models::ContentBlock::ToolSearchToolResult { .. }
                        | crate::models::ContentBlock::CodeExecutionToolResult { .. }
                )
            });
        if all_tool_results {
            tool_output = tool_output.saturating_add(tokens);
        } else {
            conversation = conversation.saturating_add(tokens);
        }
    }
    (conversation, tool_output, app.api_messages.len())
}

pub(super) fn git_rows(_app: &mut App) -> Vec<WorkRow> {
    Vec::new()
}

/// The price view. One number everywhere: the session total is the same
/// `displayed_session_cost_for_currency` the footer chip prints, through the
/// same `App::format_cost_amount`; per-agent rows come from the roster's
/// usage receipts (`cost_microusd`, absent = no receipt, never `0`).
pub(super) fn price_rows(app: &mut App) -> Vec<WorkRow> {
    let mut out = Vec::new();
    let session = app.session_cost_label();
    out.push(WorkRow {
        id: WorkRowId("price:session".to_string()),
        mark: "$",
        label: format!("session · {session}"),
        detail: "/cost for the full ledger".to_string(),
        tone: WorkTone::Live,
        selectable: true,
        primary_action: Some(SidebarRowAction::Command("/cost".to_string())),
        agent: None,
    });
    let roster = app.agent_roster.clone();
    let priced: Vec<_> = roster
        .iter()
        .filter(|row| row.cost_microusd.is_some())
        .collect();
    if !priced.is_empty() {
        let total = priced
            .iter()
            .filter_map(|row| row.cost_microusd)
            .fold(0u64, u64::saturating_add);
        out.push(WorkRow {
            id: WorkRowId("price:agents".to_string()),
            mark: "·",
            label: format!(
                "agents · {}",
                app.format_cost_amount(total as f64 / 1_000_000.0)
            ),
            detail: format!("{} of {} agents priced", priced.len(), roster.len()),
            tone: WorkTone::Muted,
            selectable: false,
            primary_action: None,
            agent: None,
        });
        for row in priced {
            let cost = row.cost_microusd.unwrap_or(0) as f64 / 1_000_000.0;
            out.push(WorkRow {
                id: WorkRowId(format!("price:agent:{}", row.worker_id)),
                mark: row.state.glyph(),
                label: format!("  {} · {}", row.display_name, app.format_cost_amount(cost)),
                detail: row.model.clone(),
                tone: WorkTone::Muted,
                selectable: true,
                primary_action: Some(SidebarRowAction::OpenAgentDetail {
                    agent_id: row.worker_id.clone(),
                }),
                agent: None,
            });
        }
    }
    let metrics = crate::tui::session_metrics::snapshot_from_app(app);
    if let Some(pct) = metrics.cache_hit_percent {
        out.push(WorkRow {
            id: WorkRowId("price:cache".to_string()),
            mark: "·",
            label: format!("cache hit · {pct}%"),
            detail: "/cache for per-turn cache telemetry".to_string(),
            tone: WorkTone::Muted,
            selectable: true,
            primary_action: Some(SidebarRowAction::Command("/cache".to_string())),
            agent: None,
        });
    }
    if let Some(rate) = crate::pricing::model_rate_label(
        app.api_provider,
        &app.model,
        app.cost_display_currency(app.cost_currency),
    ) {
        out.push(WorkRow {
            id: WorkRowId("price:rate".to_string()),
            mark: "·",
            label: format!("{} · {rate}", app.model),
            detail: "per million tokens, in / out".to_string(),
            tone: WorkTone::Muted,
            selectable: false,
            primary_action: None,
            agent: None,
        });
    }
    app.work_surface.latest_rows = out.clone();
    out
}
