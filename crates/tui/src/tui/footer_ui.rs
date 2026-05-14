use ratatui::{Frame, layout::Rect, style::Style, text::Span};
use std::time::Instant;
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

use crate::core::coherence::CoherenceState;
use crate::palette;
use crate::tools::subagent::SubAgentStatus;
use crate::tui::app::App;
use crate::tui::format_helpers;
use crate::tui::history::{HistoryCell, ToolCell, ToolStatus, summarize_tool_output};
use crate::tui::key_shortcuts;
use crate::tui::subagent_routing::{active_fanout_counts, running_agent_count};
use crate::tui::ui::{
    active_foreground_shell_running, context_usage_snapshot, selected_detail_footer_label,
    status_color,
};
use crate::tui::ui_text::{concise_shell_command_label, truncate_line_to_width};
use crate::tui::widgets::{FooterProps, FooterToast, FooterWidget, Renderable};
use crate::tui::workspace_context;

pub(crate) fn render_footer(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Pull in the toast first so we don't re-borrow `app` mutably mid-build,
    // then build the FooterProps once. The widget itself is a pure render —
    // it owns no `App` knowledge; all width-aware layout lives in the widget.
    //
    // The quit-confirmation prompt takes precedence over normal status toasts
    // because it represents a transient instruction the user must respond to
    // within ~2s. Mirrors codex-rs's `FooterMode::QuitShortcutReminder`.
    let quit_prompt = if app.quit_is_armed() {
        Some(FooterToast {
            text: crate::localization::tr(
                app.ui_locale,
                crate::localization::MessageId::FooterPressCtrlCAgain,
            )
            .to_string(),
            color: palette::STATUS_WARNING,
        })
    } else {
        None
    };
    let toast = quit_prompt.or_else(|| {
        app.active_status_toast().map(|toast| FooterToast {
            text: toast.text,
            color: status_color(toast.level),
        })
    });

    // Drive every cluster from the user's configured `status_items`. Mode
    // and Model are always rendered by `FooterProps` itself (their position
    // is structural — cluster gating is handled by the widget), so we only
    // gate the optional clusters here. If a variant is missing from
    // `status_items`, its span vec stays empty and the footer hides it.
    let mut props = render_footer_from(app, &app.status_items, toast);
    // FooterProps is mut so the working-strip animation can layer on top.

    // Animate the spacer between the left status line and the right-hand
    // chips whenever a turn is live: model loading/streaming, compacting, or
    // sub-agents in flight. The spout strip is gated on `fancy_animations`
    // (the "do I want a whale at all" knob); `low_motion` now governs only
    // streaming pacing (typewriter vs upstream), not the spout. Dot-pulse
    // counter ticks every 400 ms so `working` → `working...` reads at a
    // calm pace regardless of motion mode.
    if footer_working_strip_active(app) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let dot_frame = now_ms / 400;
        // Surface one compact live status row in the footer whenever a turn
        // is live. Tool turns get the current action plus active/done counts;
        // non-tool work falls back to the existing dot-pulse label.
        props.state_label = active_subagent_status_label(app)
            .or_else(|| active_tool_status_label(app))
            .unwrap_or_else(|| crate::tui::widgets::footer_working_label(dot_frame, app.ui_locale));
        props.state_color = palette::DEEPSEEK_SKY;

        // Water-spout frame source: wall-clock milliseconds. The sine-wave
        // math in `footer_working_strip_glyph_at` was tuned for this cadence
        // (`t = frame / 1000.0`, primary term × 8.0 ≈ 1.3 Hz at 1 ms ticks),
        // so frame must advance at ~1000 units/sec to produce the intended
        // animation feel. `fancy_animations = false` hides the strip
        // entirely; the textual `working...` pulse still keeps a heartbeat
        // regardless.
        if app.fancy_animations {
            props.working_strip_frame = Some(now_ms);
        }
    } else if props.state_label == "ready"
        && let Some(label) = selected_detail_footer_label(app)
    {
        props.state_label = label;
        props.state_color = palette::TEXT_MUTED;
    }

    let widget = FooterWidget::new(props);
    let buf = f.buffer_mut();
    widget.render(area, buf);
}

/// Whether the footer should animate the water-spout strip. Driven by the
/// underlying live-work flags so the strip stays visible for the *entire*
/// turn — not just the moments where bytes are streaming. `is_loading` can
/// flicker off between LLM rounds within a single turn (tool execution,
/// reasoning replay, capacity refresh, etc.), so we ALSO gate on the turn
/// itself still being in flight via `runtime_turn_status == "in_progress"`.
/// Without that, the user sees the strip vanish for seconds at a time even
/// though the agent is still working.
pub(crate) fn footer_working_strip_active(app: &App) -> bool {
    let turn_in_progress = app.runtime_turn_status.as_deref() == Some("in_progress");
    app.is_loading || app.is_compacting || running_agent_count(app) > 0 || turn_in_progress
}

pub(crate) fn is_noisy_subagent_progress(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    status.contains("requesting model response")
}

pub(crate) fn subagent_objective_summary(app: &App, id: &str) -> Option<String> {
    app.subagent_cache
        .iter()
        .find(|agent| agent.agent_id == id)
        .map(|agent| summarize_tool_output(&agent.assignment.objective))
        .filter(|summary| !summary.is_empty())
}

pub(crate) fn friendly_subagent_progress(app: &App, id: &str, status: &str) -> String {
    if !is_noisy_subagent_progress(status) {
        return summarize_tool_output(status);
    }

    if let Some(summary) = subagent_objective_summary(app, id) {
        return format!("working on {summary}");
    }
    if let Some(existing) = app.agent_progress.get(id)
        && !is_noisy_subagent_progress(existing)
        && existing != "working"
    {
        return existing.clone();
    }
    "working".to_string()
}

pub(crate) fn active_subagent_status_label(app: &App) -> Option<String> {
    let running = running_agent_count(app);
    let fanout = active_fanout_counts(app);
    let (display_running, total) = if let Some((fanout_running, fanout_total)) = fanout {
        if fanout_running == 0 {
            return None;
        }
        (fanout_running, fanout_total)
    } else {
        if running == 0 {
            return None;
        }
        (running, running)
    };
    let detail = app
        .subagent_cache
        .iter()
        .find(|agent| matches!(agent.status, SubAgentStatus::Running))
        .map(|agent| summarize_tool_output(&agent.assignment.objective))
        .filter(|summary| !summary.is_empty())
        .or_else(|| {
            app.agent_progress
                .values()
                .find(|value| !is_noisy_subagent_progress(value) && value.as_str() != "working")
                .cloned()
        })
        .unwrap_or_else(|| "working".to_string());
    let detail = truncate_line_to_width(&detail, 34);
    let elapsed = app
        .agent_activity_started_at
        .or(app.turn_started_at)
        .map(|started| format!("{}s", started.elapsed().as_secs()));

    let mut parts = vec![format!("agents {display_running}/{total}"), detail];
    if let Some(elapsed) = elapsed {
        parts.push(elapsed);
    }
    parts.push("Alt+4".to_string());
    Some(parts.join(" \u{00B7} "))
}

#[derive(Default)]
struct ActiveToolStatusSnapshot {
    primary_running: Option<String>,
    primary_any: Option<String>,
    running: usize,
    completed: usize,
    started_at: Option<Instant>,
}

impl ActiveToolStatusSnapshot {
    fn record(&mut self, label: String, status: ToolStatus, started_at: Option<Instant>) {
        if self.primary_any.is_none() {
            self.primary_any = Some(label.clone());
        }
        if status == ToolStatus::Running {
            self.running += 1;
            if self.primary_running.is_none() {
                self.primary_running = Some(label);
            }
        } else {
            self.completed += 1;
        }
        if let Some(started) = started_at {
            self.started_at = Some(match self.started_at {
                Some(current) => current.min(started),
                None => started,
            });
        }
    }

    fn total(&self) -> usize {
        self.running + self.completed
    }
}

pub(crate) fn active_tool_status_label(app: &App) -> Option<String> {
    let active = app.active_cell.as_ref()?;
    if active.is_empty() {
        return None;
    }

    let mut snapshot = ActiveToolStatusSnapshot::default();
    for cell in active.entries() {
        collect_active_tool_status(cell, &mut snapshot);
    }
    if snapshot.total() == 0 {
        return None;
    }

    let primary = snapshot
        .primary_running
        .or(snapshot.primary_any)
        .unwrap_or_else(|| "tools".to_string());
    let primary = truncate_line_to_width(&primary, 30);
    let elapsed = snapshot
        .started_at
        .or(app.turn_started_at)
        .map(|started| format!("{}s", started.elapsed().as_secs()));

    let mut parts = vec![
        primary,
        format!("{} active", snapshot.running),
        format!("{} done", snapshot.completed),
    ];
    if let Some(elapsed) = elapsed {
        parts.push(elapsed);
    }
    if active_foreground_shell_running(app) {
        parts.push("Ctrl+B shell".to_string());
    }
    parts.push(key_shortcuts::tool_details_shortcut_label().to_string());
    Some(parts.join(" \u{00B7} "))
}

fn collect_active_tool_status(cell: &HistoryCell, snapshot: &mut ActiveToolStatusSnapshot) {
    let HistoryCell::Tool(tool) = cell else {
        return;
    };
    match tool {
        ToolCell::Exec(exec) => snapshot.record(
            concise_shell_command_label(&exec.command, 80),
            exec.status,
            exec.started_at,
        ),
        ToolCell::Exploring(explore) => {
            for entry in &explore.entries {
                snapshot.record(
                    format!("read {}", one_line_summary(&entry.label, 80)),
                    entry.status,
                    None,
                );
            }
        }
        ToolCell::PlanUpdate(plan) => {
            snapshot.record("update plan".to_string(), plan.status, None);
        }
        ToolCell::PatchSummary(patch) => {
            snapshot.record(format!("patch {}", patch.path), patch.status, None);
        }
        ToolCell::Review(review) => {
            let target = one_line_summary(&review.target, 80);
            let label = if target.is_empty() {
                "review".to_string()
            } else {
                format!("review {target}")
            };
            snapshot.record(label, review.status, None);
        }
        ToolCell::DiffPreview(diff) => {
            snapshot.record(format!("diff {}", diff.title), ToolStatus::Success, None);
        }
        ToolCell::Mcp(mcp) => snapshot.record(format!("tool {}", mcp.tool), mcp.status, None),
        ToolCell::ViewImage(image) => snapshot.record(
            format!("image {}", image.path.display()),
            ToolStatus::Success,
            None,
        ),
        ToolCell::WebSearch(search) => {
            snapshot.record(format!("search {}", search.query), search.status, None);
        }
        ToolCell::Generic(generic) => {
            // Sub-agent dispatch represents itself through the DelegateCard
            // + Agents sidebar. Counting it again here would duplicate the
            // status. RLM is different today: it is a foreground tool call,
            // so keep it in the live tool footer until the async RLM
            // workbench lands (#513).
            if matches!(generic.name.as_str(), "agent_open" | "agent_spawn") {
                return;
            }
            snapshot.record(format!("tool {}", generic.name), generic.status, None);
        }
    }
}

pub(crate) fn one_line_summary(text: &str, max_width: usize) -> String {
    truncate_line_to_width(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        max_width,
    )
}

/// Build [`FooterProps`] from a user-configured `status_items` slice.
///
/// Variants are routed to their structural cluster: `Mode` and `Model` are
/// always emitted (the widget needs them to lay out the line correctly even
/// when the user toggled them off the picker — we honour the toggle by
/// blanking their visible content rather than collapsing the layout).
/// `Cost` and `Status` belong in the left cluster; the rest in the right.
///
/// A variant absent from `items` produces an empty span vec, which the
/// footer widget already hides cleanly. This keeps the renderer fully
/// data-driven without changing `FooterProps`'s public shape.
pub(crate) fn render_footer_from(
    app: &App,
    items: &[crate::config::StatusItem],
    toast: Option<FooterToast>,
) -> FooterProps {
    use crate::config::StatusItem as S;
    let has = |item: S| items.contains(&item);

    let (state_label, state_color) = if has(S::Status) {
        footer_state_label(app)
    } else {
        // "ready" is the sentinel the widget uses to skip the status segment;
        // pair it with theme text_muted for visual neutrality.
        ("ready", app.ui_theme.text_muted)
    };

    let coherence = if has(S::Coherence) {
        footer_coherence_spans(app)
    } else {
        Vec::new()
    };
    let agents = if has(S::Agents) {
        crate::tui::widgets::footer_agents_chip(running_agent_count(app), app.ui_locale)
    } else {
        Vec::new()
    };
    let reasoning_replay = if has(S::ReasoningReplay) {
        footer_reasoning_replay_spans(app)
    } else {
        Vec::new()
    };
    let cache = Vec::new();
    let cache_chip = if has(S::Cache) {
        footer_cache_spans(app)
    } else {
        Vec::new()
    };
    let prefix_stability = if has(S::PrefixStability) {
        footer_prefix_stability_spans(app)
    } else {
        Vec::new()
    };
    let cost = if has(S::Cost) {
        footer_cost_spans(app)
    } else {
        Vec::new()
    };

    // Build the props; `Mode` and `Model` toggles modulate downstream by
    // blanking the rendered text rather than restructuring the widget — the
    // user is opting out of the chip, not destroying the bar.
    let mut props = FooterProps::from_app(
        app,
        toast,
        state_label,
        state_color,
        coherence,
        agents,
        reasoning_replay,
        cache,
        cost,
    );
    if !has(S::Mode) {
        props.mode_label = "";
    }
    if !has(S::Model) {
        props.model.clear();
    }

    // Right-cluster extension chips: append in `items` order so user
    // ordering is preserved across the new variants.
    let mut extra: Vec<Span<'static>> = Vec::new();
    for item in items {
        let chip = match *item {
            S::PrefixStability => prefix_stability.clone(),
            S::Cache => cache_chip.clone(),
            S::ContextPercent => footer_context_percent_spans(app),
            S::GitBranch => footer_git_branch_spans(app),
            S::LastToolElapsed | S::RateLimit => Vec::new(),
            _ => continue,
        };
        if chip.is_empty() {
            continue;
        }
        if !extra.is_empty() {
            extra.push(Span::raw("  "));
        }
        extra.extend(chip);
    }
    if !extra.is_empty() {
        // Stack into the cache slot — last existing right-cluster pipe — so
        // they appear adjacent without changing FooterProps's API. Chips are
        // appended in `items` order, so users can place prefix stability next
        // to cache telemetry without adding another FooterProps field.
        if !props.cache.is_empty() {
            props.cache.push(Span::raw("  "));
        }
        props.cache.extend(extra);
    }

    props
}

pub(crate) fn footer_git_branch_spans(app: &App) -> Vec<Span<'static>> {
    let Some(branch) = workspace_context::branch(&app.workspace) else {
        return Vec::new();
    };
    vec![Span::styled(
        branch,
        Style::default().fg(app.ui_theme.text_muted),
    )]
}

pub(crate) fn footer_prefix_stability_spans(app: &App) -> Vec<Span<'static>> {
    let Some((label, color)) = format_helpers::prefix_stability_chip(app) else {
        return Vec::new();
    };
    vec![Span::styled(label, Style::default().fg(color))]
}

/// Spans for the "context %" footer chip. Mirrors the header colour ramp so
/// the two surfaces stay visually consistent when both are enabled.
pub(crate) fn footer_context_percent_spans(app: &App) -> Vec<Span<'static>> {
    let Some((_, _, percent)) = context_usage_snapshot(app) else {
        return Vec::new();
    };
    let color = if percent >= 95.0 {
        palette::STATUS_ERROR
    } else if percent >= 85.0 {
        palette::STATUS_WARNING
    } else {
        palette::TEXT_MUTED
    };
    vec![Span::styled(
        format!("active ctx {percent:.0}%"),
        Style::default().fg(color),
    )]
}

pub(crate) fn footer_cost_spans(app: &App) -> Vec<Span<'static>> {
    let displayed_cost = app.displayed_session_cost_for_currency(app.cost_currency);
    if !should_show_footer_cost(displayed_cost) {
        return Vec::new();
    }
    vec![Span::styled(
        app.format_cost_amount(displayed_cost),
        Style::default().fg(palette::TEXT_MUTED),
    )]
}

pub(crate) fn should_show_footer_cost(displayed_cost: f64) -> bool {
    displayed_cost.is_finite() && displayed_cost > 0.0
}

/// Test-only helper retained as a parity reference for `FooterWidget`'s
/// auxiliary-span composition. Production rendering is performed by the
/// widget itself; the existing footer parity tests still exercise this
/// function directly to guard against drift.
#[cfg(test)]
pub(crate) fn footer_auxiliary_spans(app: &App, max_width: usize) -> Vec<Span<'static>> {
    // Context % is already shown in the header signal bar — don't
    // duplicate it in the footer. The footer carries unique info only:
    // prefix stability, coherence, in-flight sub-agents, reasoning
    // replay tokens, cache hit rate, and session cost.
    let coherence_spans = footer_coherence_spans(app);
    let agents_spans =
        crate::tui::widgets::footer_agents_chip(running_agent_count(app), app.ui_locale);
    let replay_spans = footer_reasoning_replay_spans(app);
    let cache_spans = footer_cache_spans(app);
    let cost_spans = footer_cost_spans(app);
    let prefix_spans = app
        .prefix_stability_pct
        .map(|_| {
            let (label, color) = format_helpers::prefix_stability_chip(app)
                .unwrap_or(("P --".to_string(), ratatui::style::Color::DarkGray));
            vec![Span::styled(label, Style::default().fg(color))]
        })
        .unwrap_or_default();

    let parts: Vec<&Vec<Span<'static>>> = [
        &coherence_spans,
        &agents_spans,
        &replay_spans,
        &prefix_spans,
        &cache_spans,
        &cost_spans,
    ]
    .iter()
    .filter(|spans| !spans.is_empty())
    .copied()
    .collect();

    // Try to fit as many parts as possible, dropping from the end.
    for end in (0..=parts.len()).rev() {
        let mut combined = Vec::new();
        for (i, part) in parts[..end].iter().enumerate() {
            if i > 0 {
                combined.push(Span::raw("  "));
            }
            combined.extend(part.iter().cloned());
        }
        if spans_width(&combined) <= max_width {
            return combined;
        }
    }
    Vec::new()
}

pub(crate) fn footer_coherence_spans(app: &App) -> Vec<Span<'static>> {
    // Only surface coherence when the engine is actively intervening — the
    // user-facing signal is "we're doing something different now," not
    // "your conversation is getting complex," which the context-percent
    // header already covers. `GettingCrowded` is just a soft hint, so we
    // suppress it; the active interventions get their own visible label.
    let (label, color) = match app.coherence_state {
        CoherenceState::Healthy | CoherenceState::GettingCrowded => return Vec::new(),
        CoherenceState::RefreshingContext => ("refreshing context", palette::STATUS_WARNING),
        CoherenceState::VerifyingRecentWork => ("verifying", palette::DEEPSEEK_SKY),
        CoherenceState::ResettingPlan => ("resetting plan", palette::STATUS_ERROR),
    };

    vec![Span::styled(label.to_string(), Style::default().fg(color))]
}

pub(crate) fn footer_cache_spans(app: &App) -> Vec<Span<'static>> {
    if app.session.last_prompt_tokens.is_none() && app.session.last_completion_tokens.is_none() {
        return Vec::new();
    };
    let Some(hit_tokens) = app.session.last_prompt_cache_hit_tokens else {
        return vec![Span::styled(
            "Cache: unavailable",
            Style::default().fg(palette::TEXT_MUTED),
        )];
    };
    let miss_tokens = app
        .session
        .last_prompt_cache_miss_tokens
        .unwrap_or_else(|| {
            app.session
                .last_prompt_tokens
                .unwrap_or(0)
                .saturating_sub(hit_tokens)
        });
    let total = hit_tokens.saturating_add(miss_tokens);
    let percent = if total == 0 {
        0.0
    } else {
        (f64::from(hit_tokens) / f64::from(total) * 100.0).clamp(0.0, 100.0)
    };
    // Threshold-based coloring for cache hit rate (#396):
    //   >80%: green (good cache utilization)
    //   40-80%: yellow/warning
    //   <40%: red/dimmed only when the stable prefix is also suspect.
    //
    // A stable prefix with a low hit rate usually means the latest request
    // contains a large new tail (tool results, sub-agent summaries, or fresh
    // user input), not that the cacheable prefix is churning.
    let prefix_is_stable = app
        .prefix_stability_pct
        .is_some_and(|pct| pct >= 95 && app.prefix_change_count == 0);
    let color = if percent > 80.0 {
        palette::STATUS_SUCCESS
    } else if percent >= 40.0 {
        palette::STATUS_WARNING
    } else if prefix_is_stable {
        palette::TEXT_MUTED
    } else {
        palette::STATUS_ERROR
    };
    vec![Span::styled(
        format!(
            "Cache: {:.1}% hit | hit {hit_tokens} | miss {miss_tokens}",
            percent
        ),
        Style::default().fg(color),
    )]
}

/// Render a footer chip showing the size of the `reasoning_content` block
/// replayed on the most recent thinking-mode tool-calling turn (#30).
///
/// Stays hidden when the count is zero (non-thinking models, first turn, or
/// turns with no tool calls). When replay tokens dominate the input budget
/// (>50%), the chip turns warning-coloured so users notice that thinking
/// replay is the main consumer of context.
pub(crate) fn footer_reasoning_replay_spans(app: &App) -> Vec<Span<'static>> {
    let Some(replay) = app.session.last_reasoning_replay_tokens else {
        return Vec::new();
    };
    if replay == 0 {
        return Vec::new();
    }
    let label = format!("rsn {}", format_token_count_compact(u64::from(replay)));
    let color = match app.session.last_prompt_tokens {
        Some(input) if input > 0 && f64::from(replay) / f64::from(input) > 0.5 => {
            palette::STATUS_WARNING
        }
        _ => palette::TEXT_MUTED,
    };
    vec![Span::styled(label, Style::default().fg(color))]
}

#[cfg(test)]
pub(crate) fn footer_status_line_spans(app: &App, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }

    let (mode_label, mode_color) = footer_mode_style(app);
    let (status_label, status_color) = footer_state_label(app);
    let sep = " \u{00B7} ";
    let show_status = status_label != "ready";

    let fixed_width = mode_label.width()
        + sep.width()
        + if show_status {
            sep.width() + status_label.width()
        } else {
            0
        };

    if max_width <= mode_label.width() {
        return vec![Span::styled(
            truncate_line_to_width(mode_label, max_width),
            Style::default().fg(mode_color),
        )];
    }

    let model_budget = max_width.saturating_sub(fixed_width).max(1);
    let model_label = truncate_line_to_width(&app.model, model_budget);

    let mut spans = vec![
        Span::styled(mode_label.to_string(), Style::default().fg(mode_color)),
        Span::styled(sep.to_string(), Style::default().fg(app.ui_theme.text_dim)),
        Span::styled(model_label, Style::default().fg(app.ui_theme.text_hint)),
    ];

    if show_status {
        spans.push(Span::styled(
            sep.to_string(),
            Style::default().fg(app.ui_theme.text_dim),
        ));
        spans.push(Span::styled(
            status_label.to_string(),
            Style::default().fg(status_color),
        ));
    }

    spans
}

pub(crate) fn footer_state_label(app: &App) -> (&'static str, ratatui::style::Color) {
    if app.is_compacting {
        return ("compacting \u{238B}", app.ui_theme.status_warning);
    }
    // Note: we deliberately do NOT show a "thinking" label for `is_loading`.
    // The animated water-spout strip in the footer's spacer is the visual
    // signal that the model is live; "thinking" was misleading because it
    // fired for every kind of in-flight work (tool calls, streaming, etc.),
    // not strictly reasoning. Sub-agents still surface "working" because
    // that's a distinct lifecycle the user can act on (open `/agents`).
    if running_agent_count(app) > 0 {
        return ("working", app.ui_theme.status_working);
    }
    if app.queued_draft.is_some() {
        return ("draft", app.ui_theme.text_muted);
    }

    if !app.view_stack.is_empty() {
        return ("overlay", app.ui_theme.text_muted);
    }

    if !app.input.is_empty() {
        return ("draft", app.ui_theme.text_muted);
    }

    ("ready", app.ui_theme.status_ready)
}

#[cfg(test)]
pub(crate) fn footer_mode_style(app: &App) -> (&'static str, ratatui::style::Color) {
    let label = app.mode.as_setting();
    let color = match app.mode {
        crate::tui::app::AppMode::Agent => app.ui_theme.mode_agent,
        crate::tui::app::AppMode::Yolo => app.ui_theme.mode_yolo,
        crate::tui::app::AppMode::Plan => app.ui_theme.mode_plan,
    };
    (label, color)
}

pub(crate) fn format_token_count_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
pub(crate) fn format_context_budget(used: i64, max: u32) -> String {
    let max_u64 = u64::from(max);
    let max_i64 = i64::from(max);

    if used > max_i64 {
        return format!(
            ">{}/{}",
            format_token_count_compact(max_u64),
            format_token_count_compact(max_u64)
        );
    }

    let used_u64 = u64::try_from(used.max(0)).unwrap_or(0);
    format!(
        "{}/{}",
        format_token_count_compact(used_u64),
        format_token_count_compact(max_u64)
    )
}

#[cfg(test)]
pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}
