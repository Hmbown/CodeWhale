use ratatui::{Frame, layout::Rect, style::Style, text::Span};
use std::time::Instant;
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

use crate::localization::{Locale, MessageId};
use crate::palette;
use crate::tools::subagent::SubAgentStatus;
use crate::tui::app::{App, TaskPanelEntryKind};
use crate::tui::format_helpers;
use crate::tui::history::{HistoryCell, ToolCell, ToolStatus, summarize_tool_output};
use crate::tui::key_shortcuts;
use crate::tui::subagent_routing::{active_fanout_counts, running_agent_count};
use crate::tui::ui::{
    active_foreground_shell_running, context_usage_snapshot, selected_detail_footer_label,
    status_color,
};
use crate::tui::ui_text::{concise_shell_command_label, truncate_line_to_width};
use crate::tui::widgets::tool_card::tool_activity_label_for_name;
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
    // sub-agents in flight. The spout strip and dot-pulse fallback are gated
    // on `fancy_animations` (the "do I want animated chrome" knob);
    // `low_motion` governs streaming pacing and redraw cadence.
    if footer_working_strip_active(app) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let dot_frame = footer_working_label_frame(now_ms, app.fancy_animations);
        // Surface one compact live status row in the footer whenever a turn
        // is live. Tool turns get the current action plus active/done counts;
        // non-tool work falls back to a descriptive label with elapsed time.
        let elapsed_secs = app
            .turn_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let active_subagent_label = active_subagent_status_label(app);
        let mut label = active_subagent_label
            .clone()
            .or_else(|| active_tool_status_label(app))
            .unwrap_or_else(|| {
                // Show the working label during active turns (loading, compacting, etc.).
                let base = crate::tui::widgets::footer_working_label(dot_frame, app.ui_locale);
                if elapsed_secs > 0 {
                    format!("{base} ({elapsed_secs}s)")
                } else {
                    base.to_string()
                }
            });
        // Append stall reason when the turn has been running > 30 s.
        if let Some(reason) = stall_reason(app) {
            label = format!("{label}  ({reason})");
        }
        props.state_label = label;
        if active_subagent_label.is_some() {
            props.agents.clear();
        }
        props.state_color = palette::DEEPSEEK_SKY;

        // Water-spout frame source: wall-clock milliseconds. The sine-wave
        // math in `footer_working_strip_glyph_at` was tuned for this cadence
        // (`t = frame / 1000.0`, primary term × 8.0 ≈ 1.3 Hz at 1 ms ticks),
        // so frame must advance at ~1000 units/sec to produce the intended
        // animation feel. `fancy_animations = false` hides the strip and pins
        // the textual fallback to `working`.
        if app.fancy_animations {
            props.working_strip_frame = Some(now_ms);
        }
    } else if matches!(props.state_label.as_str(), "idle" | "ready")
        && let Some(label) = selected_detail_footer_label(app)
    {
        props.state_label = label;
        props.state_color = palette::TEXT_MUTED;
    }

    let widget = FooterWidget::new(props);
    let buf = f.buffer_mut();
    widget.render(area, buf);
}

/// Classify why a turn that has been running for > 30 s might appear stalled.
/// Returns a short human-readable reason string, or `None` when the turn has
/// not been running long enough to classify as stalled.
pub(crate) fn stall_reason(app: &App) -> Option<String> {
    let elapsed = app.turn_started_at?.elapsed();
    if elapsed.as_secs() < 30 {
        return None;
    }
    if app.is_compacting {
        return Some("compacting context".to_string());
    }
    if app.is_loading {
        return Some(provider_wait_reason(app));
    }
    if running_agent_count(app) > 0 {
        return Some("sub-agents working".to_string());
    }
    if app.task_panel.iter().any(|task| task.status == "running") {
        return Some("background jobs running".to_string());
    }
    let active = app.active_cell.as_ref()?;
    if active.entries().iter().any(|cell| match cell {
        crate::tui::history::HistoryCell::Tool(tool) => match tool {
            crate::tui::history::ToolCell::Exec(exec) => {
                exec.status == crate::tui::history::ToolStatus::Running
            }
            crate::tui::history::ToolCell::Exploring(explore) => explore
                .entries
                .iter()
                .any(|e| e.status == crate::tui::history::ToolStatus::Running),
            _ => false,
        },
        _ => false,
    }) {
        return Some("tools executing".to_string());
    }
    if app.runtime_turn_status.as_deref() == Some("in_progress") {
        return Some("waiting - no recent activity".to_string());
    }
    None
}

/// Seconds the current turn has gone without observable stream activity.
pub(crate) fn provider_wait_idle_secs(app: &App) -> u64 {
    app.turn_last_activity_at
        .or(app.turn_started_at)
        .map(|at| at.elapsed().as_secs())
        .unwrap_or(0)
}

/// Idle threshold (seconds) above which the footer surfaces the elapsed
/// idle time during a provider wait.  Below this threshold the footer shows
/// only the concise label without a running counter (#3189).
const PROVIDER_WAIT_IDLE_SHOW_SECS: u64 = 60;

/// `waiting for model` reason — kept short by default: only the label when
/// the idle time is below [`PROVIDER_WAIT_IDLE_SHOW_SECS`].  Once the idle
/// exceeds that threshold the elapsed seconds appear, and when the idle
/// approaches the stream-idle budget the full `Ns/Ms idle timeout` detail
/// surfaces so the user knows the stream is at risk of timing out (#3189).
/// Provider and model stay in the header bar; the structured incident logger
/// (`maybe_log_provider_wait_incident`) captures full diagnostics regardless
/// of the footer copy.
fn provider_wait_reason(app: &App) -> String {
    let idle = provider_wait_idle_secs(app);
    let budget = app.stream_chunk_timeout_secs;

    if running_agent_count(app) == 0 {
        if let Some((0, total)) = active_fanout_counts(app) {
            return format!("waiting · fanout 0/{total}");
        } else if app.pending_subagent_dispatch.is_some() {
            return "waiting · dispatch pending".to_string();
        }
    }

    // Normal wait — no countdown noise.
    if idle < PROVIDER_WAIT_IDLE_SHOW_SECS {
        return "waiting for model".to_string();
    }

    // Significant idle — surface the elapsed seconds so the user can judge
    // whether the stream is making progress.
    let near_timeout = budget > 0 && idle >= budget.saturating_mul(3) / 4; // ≥ 75%
    if near_timeout {
        format!("waiting for model · {idle}s/{budget}s idle timeout")
    } else {
        format!("waiting for model · {idle}s")
    }
}

/// Threshold after which a provider wait with a planned fanout is logged as
/// a structured incident (once per turn).
const PROVIDER_WAIT_INCIDENT_SECS: u64 = 120;

/// Log a compact structured incident when the parent turn has spent a long
/// time in provider wait while a sub-agent fanout plan is present (#3095).
pub(crate) fn maybe_log_provider_wait_incident(app: &mut App) {
    if app.provider_wait_incident_logged || !app.is_loading {
        return;
    }
    let elapsed = match app.turn_started_at {
        Some(at) => at.elapsed().as_secs(),
        None => return,
    };
    if elapsed < PROVIDER_WAIT_INCIDENT_SECS {
        return;
    }
    let fanout = active_fanout_counts(app);
    let pending_dispatch = app.pending_subagent_dispatch.is_some();
    if fanout.is_none() && !pending_dispatch {
        return;
    }
    let (fanout_running, fanout_total) = fanout.unwrap_or((0, 0));
    app.provider_wait_incident_logged = true;
    crate::logging::warn(format!(
        "provider-wait incident: provider={} model={} elapsed_secs={elapsed} \
         idle_secs={} stream_idle_budget_secs={} max_subagents={} \
         fanout_running={fanout_running} fanout_total={fanout_total} \
         running_agents={} pending_dispatch={pending_dispatch}",
        app.api_provider.as_str(),
        app.model,
        provider_wait_idle_secs(app),
        app.stream_chunk_timeout_secs,
        app.max_subagents,
        running_agent_count(app),
    ));
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
    app.is_loading
        || app.is_compacting
        || app.is_purging
        || running_agent_count(app) > 0
        || turn_in_progress
}

pub(crate) fn footer_working_label_frame(now_ms: u64, fancy_animations: bool) -> u64 {
    if fancy_animations { now_ms / 400 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::{
        active_subagent_status_label, footer_state_label, footer_working_label_frame,
        one_line_summary,
    };
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    #[test]
    fn footer_working_label_frame_is_static_without_fancy_animations() {
        assert_eq!(footer_working_label_frame(0, false), 0);
        assert_eq!(footer_working_label_frame(399, false), 0);
        assert_eq!(footer_working_label_frame(1_600, false), 0);
        assert_eq!(footer_working_label_frame(1_600, true), 4);
    }

    #[test]
    fn one_line_summary_strips_ansi_before_collapsing_text() {
        let summary = one_line_summary("read \x1b[38;2;6;174;242mfile.rs\x1b[0m", 80);
        assert_eq!(summary, "read file.rs");
        assert!(!summary.contains("38;2"));
    }

    #[test]
    fn active_subagent_status_label_is_descriptive_without_shortcut_or_timer() {
        let mut app = create_test_app();
        app.agent_progress.insert(
            "agent_live".to_string(),
            "reading summary files".to_string(),
        );

        let label = active_subagent_status_label(&app).expect("active agent label");

        assert_eq!(label, "agents 1/1 running · reading summary files");
        assert!(!label.contains("Ctrl+Alt+4"));
        assert!(!label.contains("0s"));
    }

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
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
        App::new(options, &Config::default())
    }

    #[test]
    fn footer_state_label_reports_paused_when_command_is_on_hold() {
        let mut app = create_test_app();
        app.is_loading = false;
        app.paused = false;
        app.paused_quarry = Some("Scan nested git repositories".to_string());

        let (label, _) = footer_state_label(&app);
        assert_eq!(
            label, "paused \u{23F8}",
            "footer should surface a paused command once the turn has drained, got {label:?}"
        );
    }

    #[test]
    fn footer_state_label_reports_paused_via_app_flag_even_without_quarry() {
        let mut app = create_test_app();
        app.is_loading = false;
        app.paused = true;
        app.paused_quarry = None;

        let (label, _) = footer_state_label(&app);
        assert_eq!(
            label, "paused \u{23F8}",
            "footer should honor app.paused directly, got {label:?}"
        );
    }

    #[test]
    fn footer_state_label_prefers_busy_while_pausing_and_loading() {
        // While the turn is still draining the pause request, the coarse
        // footer stays "busy"; the finer Pausing/Paused split lives in the
        // sidebar. This guards against reintroducing a redundant vocabulary.
        let mut app = create_test_app();
        app.is_loading = true;
        app.paused = true;
        app.paused_quarry = Some("Deploy to staging".to_string());

        let (label, _) = footer_state_label(&app);
        assert_eq!(label, "busy");
    }

    #[test]
    fn footer_state_label_falls_back_to_idle_at_rest() {
        let app = create_test_app();
        let (label, _) = footer_state_label(&app);
        assert_eq!(label, "idle");
    }

    // #3189: provider-wait reason thresholds

    #[test]
    fn provider_wait_reason_fresh_show_only_label() {
        let mut app = create_test_app();
        app.stream_chunk_timeout_secs = 300;
        app.turn_started_at = Some(std::time::Instant::now()); // < 60s
        let reason = super::provider_wait_reason(&app);
        assert_eq!(reason, "waiting for model");
        assert!(!reason.contains("idle"));
        assert!(!reason.contains("s/"));
    }

    #[test]
    fn provider_wait_reason_thresholded_show_idle_seconds() {
        let mut app = create_test_app();
        app.stream_chunk_timeout_secs = 300;
        // Simulate idle >= 60s
        app.turn_started_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(60));
        let reason = super::provider_wait_reason(&app);
        assert!(reason.contains("waiting for model"));
        assert!(reason.contains("60s"));
        // Should NOT show the full timeout budget yet (<75% of 300s = 225s)
        assert!(!reason.contains("/300s"));
    }

    #[test]
    fn provider_wait_reason_near_timeout_show_full_idle_budget() {
        let mut app = create_test_app();
        app.stream_chunk_timeout_secs = 300;
        // ≥ 75% of 300s = 225s
        app.turn_started_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(240));
        let reason = super::provider_wait_reason(&app);
        assert!(reason.contains("waiting for model"));
        assert!(reason.contains("/300s idle timeout"));
        assert!(reason.contains("240s"));
    }

    #[test]
    fn provider_wait_reason_dispatch_pending() {
        let mut app = create_test_app();
        app.stream_chunk_timeout_secs = 300;
        app.turn_started_at = Some(std::time::Instant::now());
        app.pending_subagent_dispatch = Some("test".to_string());
        let reason = super::provider_wait_reason(&app);
        assert_eq!(reason, "waiting · dispatch pending");
    }
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
    Some(format!(
        "agents {display_running}/{total} running \u{00B7} {detail}"
    ))
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
        collect_active_tool_status(cell, &mut snapshot, app.ui_locale);
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
    parts.push(key_shortcuts::tool_details_shortcut_hint_label().to_string());
    Some(parts.join(" \u{00B7} "))
}

fn collect_active_tool_status(
    cell: &HistoryCell,
    snapshot: &mut ActiveToolStatusSnapshot,
    locale: Locale,
) {
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
            if generic.name == "agent" {
                return;
            }
            snapshot.record(
                tool_activity_label_for_name(&generic.name, locale),
                generic.status,
                None,
            );
        }
    }
}

pub(crate) fn one_line_summary(text: &str, max_width: usize) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    truncate_line_to_width(
        &cleaned.split_whitespace().collect::<Vec<_>>().join(" "),
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
    let balance = if has(S::Balance) {
        footer_balance_spans(app)
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
        agents,
        reasoning_replay,
        cache,
        cost,
        balance,
    );
    if !has(S::Mode) {
        props.mode_label = "";
    }
    if !has(S::Model) {
        props.model.clear();
    }

    // Shell-running chip: visible whenever foreground or background shell work
    // is active, regardless of user-configured status items.
    let shell_chip = footer_shell_spans(app);

    // Right-cluster extension chips: append in `items` order so user
    // ordering is preserved across the new variants.
    let mut extra: Vec<Span<'static>> = Vec::new();
    if !shell_chip.is_empty() {
        extra.extend(shell_chip);
    }
    for item in items {
        let chip = match *item {
            S::PrefixStability => prefix_stability.clone(),
            S::Cache => cache_chip.clone(),
            S::ContextPercent => footer_context_percent_spans(app),
            S::GitBranch => footer_git_branch_spans(app),
            S::LastToolElapsed | S::RateLimit => Vec::new(),
            S::Tokens => footer_session_tokens_spans(app),
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
    // Identity is sourced strictly from workspace/git detection (the cached
    // "branch | status" context and the workspace path) — never from
    // provider/model/config text (#3188). The cached context being `None`
    // means "not a git repo", which we surface as an explicit non-repo state
    // rather than an empty `Repo:` label.
    //
    // We render the full `Repo: <name> @ <branch>` identity and let the footer
    // widget clip the whole bar to the real terminal width (matching the prior
    // branch-only chip, which also emitted its full string). The width-aware
    // `format_repo_identity` truncation policy is exercised in unit tests with
    // explicit widths; here we pass an effectively unbounded budget so a normal
    // branch name is never dropped on a wide terminal.
    let identity =
        workspace_context::identity_from_context(&app.workspace, app.workspace_context.as_deref());
    let label = workspace_context::format_repo_identity(&identity, usize::MAX);
    if label.is_empty() {
        return Vec::new();
    }
    vec![Span::styled(
        label,
        Style::default().fg(app.ui_theme.text_muted),
    )]
}

fn footer_shell_spans(app: &App) -> Vec<Span<'static>> {
    if let Some(label) = active_foreground_shell_label(app) {
        return crate::tui::widgets::footer_shell_label_chip(label);
    }

    let mut running = app.task_panel.iter().filter(|task| {
        task.kind == TaskPanelEntryKind::Background
            && task.status == "running"
            && task.id.starts_with("shell_")
    });
    let Some(first) = running.next() else {
        return Vec::new();
    };
    let extra = running.count();
    let command = first
        .prompt_summary
        .strip_prefix("shell: ")
        .unwrap_or(first.prompt_summary.as_str());
    let label = if extra == 0 {
        format!("shell bg: {}", concise_shell_command_label(command, 48))
    } else {
        format!("shell bg: {} jobs", extra + 1)
    };
    crate::tui::widgets::footer_shell_label_chip(label)
}

fn active_foreground_shell_label(app: &App) -> Option<String> {
    let active = app.active_cell.as_ref()?;
    active.entries().iter().find_map(|cell| {
        let HistoryCell::Tool(ToolCell::Exec(exec)) = cell else {
            return None;
        };
        if exec.status == ToolStatus::Running && exec.interaction.is_none() {
            Some(format!(
                "shell fg: {}",
                concise_shell_command_label(&exec.command, 48)
            ))
        } else {
            None
        }
    })
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
    let mut spans = vec![Span::styled(
        app.format_cost_amount(displayed_cost),
        Style::default().fg(palette::TEXT_MUTED),
    )];
    // Append cache-savings hint when the last turn had cache hits that
    // saved money (#2038).
    if let Some(saved) = app.last_turn_cache_savings()
        && saved > 0.0
    {
        spans.push(Span::styled(
            format!(" · saved {}", app.format_cost_amount(saved)),
            Style::default().fg(palette::STATUS_SUCCESS),
        ));
    }
    spans
}

pub(crate) fn footer_balance_spans(app: &App) -> Vec<Span<'static>> {
    let balance = match app.balance_cell.lock() {
        Ok(guard) => guard,
        Err(_) => return Vec::new(),
    };
    let info = match balance.as_ref() {
        Some(info) => info,
        None => return Vec::new(),
    };
    let total = match info.total_balance_f64() {
        Some(total) if total > 0.0 => total,
        _ => return Vec::new(),
    };
    let currency = match info.currency.as_str() {
        "CNY" | "cny" => "¥",
        _ => "$",
    };
    let prefix = app.tr(MessageId::FooterBalancePrefix);
    let label = if total >= 1000.0 {
        format!("{prefix} {currency}{total:.0}")
    } else if total >= 10.0 {
        format!("{prefix} {currency}{total:.1}")
    } else {
        format!("{prefix} {currency}{total:.2}")
    };
    vec![Span::styled(
        label,
        Style::default().fg(palette::TEXT_MUTED),
    )]
}

pub(crate) fn should_show_footer_cost(displayed_cost: f64) -> bool {
    displayed_cost.is_finite() && displayed_cost > 0.0
}

/// Session token-usage chip for the footer right cluster.
///
/// Renders a compact accumulated token count for the current runtime session.
/// Detailed cache stats live in the separate `cache` chip.
pub(crate) fn footer_session_tokens_spans(app: &App) -> Vec<Span<'static>> {
    let session = &app.session;
    if session.total_input_tokens == 0 && session.total_output_tokens == 0 {
        return Vec::new();
    }
    let total = u64::from(session.total_input_tokens)
        .saturating_add(u64::from(session.total_output_tokens));
    let text = format!("tok {}", format_token_count_compact(total));
    vec![Span::styled(text, Style::default().fg(palette::TEXT_MUTED))]
}

/// Test-only helper retained as a parity reference for `FooterWidget`'s
/// auxiliary-span composition. Production rendering is performed by the
/// widget itself; the existing footer parity tests still exercise this
/// function directly to guard against drift.
#[cfg(test)]
pub(crate) fn footer_auxiliary_spans(app: &App, max_width: usize) -> Vec<Span<'static>> {
    // Context % is already shown in the header signal bar — don't
    // duplicate it in the footer. The footer carries unique info only:
    // prefix stability, in-flight sub-agents, reasoning replay tokens, cache
    // hit rate, and session cost.
    let agents_spans =
        crate::tui::widgets::footer_agents_chip(running_agent_count(app), app.ui_locale);
    let replay_spans = footer_reasoning_replay_spans(app);
    let cache_spans = footer_cache_spans(app);
    let cost_spans = footer_cost_spans(app);
    let prefix_spans = app
        .prefix_stability_pct
        .map(|_| {
            let (label, color) = format_helpers::prefix_stability_chip(app).unwrap_or((
                "cache prefix --".to_string(),
                ratatui::style::Color::DarkGray,
            ));
            vec![Span::styled(label, Style::default().fg(color))]
        })
        .unwrap_or_default();

    let shell_spans = footer_shell_spans(app);

    let parts: Vec<&Vec<Span<'static>>> = [
        &agents_spans,
        &replay_spans,
        &prefix_spans,
        &cache_spans,
        &cost_spans,
        &shell_spans,
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
        format!("Cache: {percent:.1}% hit | hit {hit_tokens} | miss {miss_tokens}"),
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
    if app.is_fallback_active() {
        return ("fallback ->", app.ui_theme.status_warning);
    }
    if app.is_compacting {
        return ("compacting \u{238B}", app.ui_theme.status_warning);
    }
    if app.is_purging {
        return ("purging \u{238B}", app.ui_theme.status_warning);
    }
    if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        return ("busy", app.ui_theme.status_working);
    }
    // Note: we deliberately do NOT show a "thinking" label for live turns.
    // Busy can mean model bytes, tool calls, approval waits, or sub-agents;
    // the label should be a state indicator, not an invented activity.
    // Sub-agents still surface "working" because that's a distinct lifecycle
    // the user can act on (open `/agents`).
    if running_agent_count(app) > 0 {
        return ("working", app.ui_theme.status_working);
    }
    // A paused pausable command is an actionable state even after the turn's
    // tools have drained: the user can resume or ESC-to-cancel. Without this
    // branch the footer would read "idle" while a command is on hold, so the
    // pause state would only be visible in the Work sidebar. The sidebar's
    // `live_pause_indicator` keeps the finer "(Pausing)" vs "(Paused)" split;
    // here we surface a single coarse "paused" state because the `busy` branch
    // above already covers the draining transition. `paused_quarry` is checked
    // alongside `app.paused` so the label survives the turn-end window where
    // `app.paused` has been cleared but the hold is still resumable.
    if app.paused || app.paused_quarry.is_some() {
        return ("paused \u{23F8}", app.ui_theme.status_warning);
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

    ("idle", app.ui_theme.status_ready)
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
