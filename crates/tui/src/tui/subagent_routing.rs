//! Sub-agent and background-task routing helpers for the TUI loop.

use std::time::{Duration, Instant};

use crate::task_manager::{TaskRecord, TaskStatus, TaskSummary};
use crate::tools::subagent::{MailboxMessage, SubAgentResult, SubAgentStatus};
use crate::tui::app::{AgentProgressMeta, App, AppMode, TaskPanelEntry, TaskPanelEntryKind};
use crate::tui::history::{HistoryCell, SubAgentCell, summarize_tool_output};
use crate::tui::pager::PagerView;
use crate::tui::tool_routing::refreshes_workspace_context_on_completion;
use crate::tui::widgets::agent_card::{
    AgentLifecycle, DelegateCard, FanoutCard, apply_to_delegate, apply_to_fanout,
};
use crate::tui::workspace_context;

const SUBAGENT_TERMINAL_CARD_TTL: Duration = Duration::from_secs(5 * 60);
const SUBAGENT_TERMINAL_CARD_MAX_RETAINED: usize = 24;

pub(super) fn running_agent_count(app: &App) -> usize {
    let mut ids: std::collections::HashSet<&str> =
        app.agent_progress.keys().map(String::as_str).collect();
    for agent in app
        .subagent_cache
        .iter()
        .filter(|agent| matches!(agent.status, SubAgentStatus::Running))
    {
        ids.insert(agent.agent_id.as_str());
    }
    ids.len()
}

pub(super) fn active_fanout_counts(app: &App) -> Option<(usize, usize)> {
    // Read running count from the canonical slot states on the active
    // FanoutCard, if one exists. Used by `rlm` and any future multi-child
    // dispatch the parent agent makes via repeated `agent`.
    if let Some(idx) = app.last_fanout_card_index
        && let Some(HistoryCell::SubAgent(SubAgentCell::Fanout(card))) = app.history.get(idx)
    {
        let running = card
            .workers
            .iter()
            .filter(|slot| matches!(slot.status, AgentLifecycle::Running))
            .count();
        return Some((running, card.worker_count()));
    }
    None
}

pub(super) fn reconcile_subagent_activity_state(app: &mut App) {
    reconcile_subagent_activity_state_at(app, Instant::now());
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn reconcile_subagent_activity_state_at(app: &mut App, now: Instant) {
    reconcile_terminal_subagent_card_retention(app, now);

    let running_agents: Vec<(String, String)> = app
        .subagent_cache
        .iter()
        .filter(|agent| matches!(agent.status, SubAgentStatus::Running))
        .map(|agent| {
            (
                agent.agent_id.clone(),
                summarize_tool_output(&agent.assignment.objective),
            )
        })
        .collect();

    let running_ids: std::collections::HashSet<String> =
        running_agents.iter().map(|(id, _)| id.clone()).collect();
    app.agent_progress
        .retain(|id, _| running_ids.contains(id.as_str()));
    app.agent_progress_meta
        .retain(|id, _| running_ids.contains(id.as_str()));
    for (id, objective) in running_agents {
        app.agent_progress.entry(id.clone()).or_insert(objective);
        if let Some(agent) = app.subagent_cache.iter().find(|agent| agent.agent_id == id) {
            app.agent_progress_meta
                .entry(id.clone())
                .or_insert_with(|| AgentProgressMeta {
                    parent_run_id: agent.parent_run_id.clone(),
                    spawn_depth: agent.spawn_depth,
                });
        }
    }

    if running_ids.is_empty() {
        app.agent_activity_started_at = None;
    } else if app.agent_activity_started_at.is_none() {
        app.agent_activity_started_at = Some(Instant::now());
    }

    reconcile_cards_with_snapshots(app);
}

fn reconcile_terminal_subagent_card_retention(app: &mut App, now: Instant) {
    let current_ids: std::collections::HashSet<String> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.clone())
        .collect();
    app.subagent_terminal_seen_at
        .retain(|id, _| current_ids.contains(id));

    for agent in &app.subagent_cache {
        if matches!(agent.status, SubAgentStatus::Running) {
            app.subagent_terminal_seen_at.remove(&agent.agent_id);
        } else {
            app.subagent_terminal_seen_at
                .entry(agent.agent_id.clone())
                .or_insert(now);
        }
    }

    app.subagent_cache.retain(|agent| {
        if matches!(agent.status, SubAgentStatus::Running) {
            return true;
        }
        app.subagent_terminal_seen_at
            .get(&agent.agent_id)
            .and_then(|seen_at| now.checked_duration_since(*seen_at))
            .is_none_or(|age| age <= SUBAGENT_TERMINAL_CARD_TTL)
    });

    let mut terminal_seen: Vec<(String, Instant)> = app
        .subagent_cache
        .iter()
        .filter(|agent| !matches!(agent.status, SubAgentStatus::Running))
        .filter_map(|agent| {
            app.subagent_terminal_seen_at
                .get(&agent.agent_id)
                .map(|seen_at| (agent.agent_id.clone(), *seen_at))
        })
        .collect();
    terminal_seen.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let keep_terminal_ids: std::collections::HashSet<String> = terminal_seen
        .into_iter()
        .take(SUBAGENT_TERMINAL_CARD_MAX_RETAINED)
        .map(|(id, _)| id)
        .collect();
    app.subagent_cache.retain(|agent| {
        matches!(agent.status, SubAgentStatus::Running)
            || keep_terminal_ids.contains(agent.agent_id.as_str())
    });

    let kept_ids: std::collections::HashSet<String> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.clone())
        .collect();
    app.subagent_terminal_seen_at
        .retain(|id, _| kept_ids.contains(id));
}

/// Sync in-transcript card slots that still render as running against the
/// canonical manager snapshot statuses. A card can miss its terminal mailbox
/// envelope (e.g. API-timeout interruption observed only via `AgentList`),
/// which would otherwise leave the fanout/delegate UI counting the agent as
/// running indefinitely.
fn reconcile_cards_with_snapshots(app: &mut App) {
    let non_running: Vec<(String, AgentLifecycle)> = app
        .subagent_cache
        .iter()
        .filter_map(|agent| {
            let lifecycle = match &agent.status {
                SubAgentStatus::Running => return None,
                SubAgentStatus::Interrupted(_) => AgentLifecycle::Interrupted,
                SubAgentStatus::Completed => AgentLifecycle::Completed,
                SubAgentStatus::Failed(_) => AgentLifecycle::Failed,
                SubAgentStatus::Cancelled => AgentLifecycle::Cancelled,
                SubAgentStatus::BudgetExhausted => AgentLifecycle::Failed,
            };
            Some((agent.agent_id.clone(), lifecycle))
        })
        .collect();
    for (agent_id, lifecycle) in non_running {
        let Some(&idx) = app.subagent_card_index.get(&agent_id) else {
            continue;
        };
        let updated = match app.history.get_mut(idx) {
            Some(HistoryCell::SubAgent(SubAgentCell::Delegate(card)))
                if card.agent_id == agent_id
                    && matches!(
                        card.status,
                        AgentLifecycle::Pending | AgentLifecycle::Running
                    ) =>
            {
                card.status = lifecycle;
                true
            }
            Some(HistoryCell::SubAgent(SubAgentCell::Fanout(card))) => {
                match card.workers.iter_mut().find(|slot| {
                    slot.agent_id == agent_id
                        && matches!(
                            slot.status,
                            AgentLifecycle::Pending | AgentLifecycle::Running
                        )
                }) {
                    Some(slot) => {
                        slot.status = lifecycle;
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if updated {
            app.bump_history_cell(idx);
        }
    }
}

fn subagent_status_rank(status: &SubAgentStatus) -> u8 {
    match status {
        SubAgentStatus::Running => 0,
        SubAgentStatus::Interrupted(_) => 1,
        SubAgentStatus::Failed(_) => 2,
        SubAgentStatus::Completed => 3,
        SubAgentStatus::Cancelled => 4,
        SubAgentStatus::BudgetExhausted => 2,
    }
}

pub(super) fn sort_subagents_in_place(agents: &mut [SubAgentResult]) {
    agents.sort_by(|a, b| {
        subagent_status_rank(&a.status)
            .cmp(&subagent_status_rank(&b.status))
            .then_with(|| a.agent_type.as_str().cmp(b.agent_type.as_str()))
            .then_with(|| a.agent_id.cmp(&b.agent_id))
    });
}

pub(super) fn subagent_message_refreshes_workspace_context(message: &MailboxMessage) -> bool {
    matches!(
        message,
        MailboxMessage::ToolCallCompleted { tool_name, .. }
            if refreshes_workspace_context_on_completion(tool_name)
    )
}

/// Route a `MailboxMessage` envelope to the matching in-transcript card,
/// allocating a `DelegateCard` or `FanoutCard` on first sight (issue #128).
pub(super) fn handle_subagent_mailbox(app: &mut App, seq: u64, message: &MailboxMessage) -> bool {
    // Accumulate sub-agent token costs for the real-time footer counter (#166).
    if let MailboxMessage::TokenUsage { model, usage, .. } = message {
        if app.session.subagent_cost_event_seqs.insert(seq)
            && let Some(cost) =
                crate::pricing::calculate_turn_cost_estimate_from_usage(model, usage)
        {
            app.accrue_subagent_cost_estimate(cost);
        }
        return false; // No card visual change needed; the footer handles display.
    }

    // Resolve (or allocate) the target cell for this envelope. ChildSpawned
    // is special — it always belongs to the active fanout card if one
    // exists; otherwise it seeds a new one.
    let agent_id = message.agent_id().to_string();
    if subagent_message_refreshes_workspace_context(message) {
        workspace_context::refresh_now(app, Instant::now());
    }

    if matches!(message, MailboxMessage::ChildSpawned { .. })
        && let Some(idx) = app.last_fanout_card_index
        && let Some(HistoryCell::SubAgent(SubAgentCell::Fanout(card))) = app.history.get_mut(idx)
    {
        let updated = apply_to_fanout(card, message);
        app.subagent_card_index.insert(agent_id, idx);
        if updated {
            app.bump_history_cell(idx);
        }
        return updated;
    }

    // Existing card for this agent_id? Mutate in place.
    if let Some(&idx) = app.subagent_card_index.get(&agent_id) {
        let updated = match app.history.get_mut(idx) {
            Some(HistoryCell::SubAgent(SubAgentCell::Delegate(card))) => {
                apply_to_delegate(card, message)
            }
            Some(HistoryCell::SubAgent(SubAgentCell::Fanout(card))) => {
                apply_to_fanout(card, message)
            }
            _ => false,
        };
        if updated {
            // idx is already in scope from the outer
            // `if let Some(&idx) = app.subagent_card_index.get(&agent_id)`.
            app.bump_history_cell(idx);
        }
        return updated;
    }

    // No existing card — only `Started` reasonably opens one. Anything else
    // for an unknown agent_id is dropped (likely arrived after the cell was
    // cleared, e.g. session-resume edge cases).
    let MailboxMessage::Started { agent_type, .. } = message else {
        return false;
    };

    let dispatch_kind = app.pending_subagent_dispatch.as_deref();
    let is_fanout = matches!(dispatch_kind, Some("rlm_open" | "rlm_eval" | "rlm"));

    if is_fanout {
        // Reuse the active fanout card for sibling spawns; otherwise create
        // one anchored at this position so subsequent siblings join it.
        if let Some(idx) = app.last_fanout_card_index
            && let Some(HistoryCell::SubAgent(SubAgentCell::Fanout(card))) =
                app.history.get_mut(idx)
        {
            let updated = card.claim_pending_worker(&agent_id, AgentLifecycle::Running);
            app.subagent_card_index.insert(agent_id, idx);
            if updated {
                app.bump_history_cell(idx);
            }
            updated
        } else {
            let mut card = FanoutCard::new(
                dispatch_kind.unwrap_or("rlm_eval").to_string(),
                app.ui_locale,
            );
            card.upsert_worker(&agent_id, AgentLifecycle::Running);
            app.add_message(HistoryCell::SubAgent(SubAgentCell::Fanout(card)));
            let idx = app.history.len().saturating_sub(1);
            app.last_fanout_card_index = Some(idx);
            app.subagent_card_index.insert(agent_id, idx);
            app.bump_history_cell(idx);
            true
        }
    } else {
        let mut card = DelegateCard::new(agent_id.clone(), agent_type.clone());
        apply_to_delegate(&mut card, message);
        app.add_message(HistoryCell::SubAgent(SubAgentCell::Delegate(card)));
        let idx = app.history.len().saturating_sub(1);
        app.subagent_card_index.insert(agent_id.clone(), idx);
        // Single delegate consumes the pending dispatch label so a follow-on
        // tool call doesn't accidentally inherit it.
        app.pending_subagent_dispatch = None;
        // idx was just inserted on the line above — no need to re-query.
        app.bump_history_cell(idx);
        true
    }
}

pub(super) fn task_mode_label(mode: AppMode) -> &'static str {
    mode.as_setting()
}

pub(super) fn task_summary_to_panel_entry(summary: TaskSummary) -> TaskPanelEntry {
    TaskPanelEntry {
        id: summary.id,
        status: task_status_label(summary.status).to_string(),
        prompt_summary: summary.prompt_summary,
        duration_ms: summary.duration_ms,
        kind: TaskPanelEntryKind::Background,
        stale: false,
        elapsed_since_output_ms: None,
        owner_agent_id: None,
        owner_agent_name: None,
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Canceled => "canceled",
    }
}

pub(super) fn format_task_list(tasks: &[TaskSummary]) -> String {
    if tasks.is_empty() {
        return "No tasks found.".to_string();
    }

    let mut lines = vec![
        format!("Tasks ({})", tasks.len()),
        "ID             Status        Time  Title".to_string(),
        "------------------------------------------------------------".to_string(),
    ];
    for task in tasks {
        let duration = task
            .duration_ms
            .map(|ms| format!("{:.2}s", ms as f64 / 1000.0))
            .unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{:<13}  {:<9}  {:>8}  {}",
            task.id,
            task_status_label(task.status),
            duration,
            task.prompt_summary
        ));
    }
    lines.push("Use /task show <id> for timeline details.".to_string());
    lines.join("\n")
}

pub(super) fn open_task_pager(app: &mut App, task: &TaskRecord) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(100)
        .saturating_sub(4);
    app.view_stack.push(PagerView::from_text(
        format!("Task {}", task.id),
        &format_task_detail(task),
        width.max(60),
    ));
}

fn format_task_detail(task: &TaskRecord) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Task: {}", task.id));
    lines.push(format!("Status: {}", task_status_label(task.status)));
    lines.push(format!("Mode: {}", task.mode));
    lines.push(format!("Model: {}", task.model));
    lines.push(format!(
        "Workspace: {}",
        crate::utils::display_path(&task.workspace)
    ));
    if let Some(thread_id) = task.thread_id.as_ref() {
        lines.push(format!("Runtime Thread: {thread_id}"));
    }
    if let Some(turn_id) = task.turn_id.as_ref() {
        lines.push(format!("Runtime Turn: {turn_id}"));
    }
    if task.runtime_event_count > 0 {
        lines.push(format!("Runtime Events: {}", task.runtime_event_count));
    }
    lines.push(format!("Created: {}", task.created_at));
    if let Some(started_at) = task.started_at {
        lines.push(format!("Started: {started_at}"));
    }
    if let Some(ended_at) = task.ended_at {
        lines.push(format!("Ended: {ended_at}"));
    }
    if let Some(duration) = task.duration_ms {
        lines.push(format!("Duration: {:.2}s", duration as f64 / 1000.0));
    }
    lines.push(String::new());
    lines.push("Prompt:".to_string());
    lines.push(task.prompt.clone());

    if let Some(summary) = task.result_summary.as_ref() {
        lines.push(String::new());
        lines.push("Result Summary:".to_string());
        lines.push(summary.clone());
    }
    if let Some(path) = task.result_detail_path.as_ref() {
        lines.push(format!("Result Artifact: {}", path.display()));
    }
    if let Some(error) = task.error.as_ref() {
        lines.push(String::new());
        lines.push(format!("Error: {error}"));
    }

    lines.push(String::new());
    lines.push("Tool Calls:".to_string());
    if task.tool_calls.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for tool in &task.tool_calls {
            let status = match tool.status {
                crate::task_manager::TaskToolStatus::Running => "running",
                crate::task_manager::TaskToolStatus::Success => "success",
                crate::task_manager::TaskToolStatus::Failed => "failed",
                crate::task_manager::TaskToolStatus::Canceled => "canceled",
            };
            let mut line = format!(
                "- {} [{}] {}",
                tool.name,
                status,
                tool.output_summary.as_deref().unwrap_or("(no summary)")
            );
            if let Some(duration) = tool.duration_ms {
                line.push_str(&format!(" ({:.2}s)", duration as f64 / 1000.0));
            }
            lines.push(line);
            if let Some(path) = tool.detail_path.as_ref() {
                lines.push(format!("  detail: {}", path.display()));
            }
            if let Some(path) = tool.patch_ref.as_ref() {
                lines.push(format!("  patch: {}", path.display()));
            }
        }
    }

    lines.push(String::new());
    lines.push("Timeline:".to_string());
    if task.timeline.is_empty() {
        lines.push("- (none)".to_string());
    } else {
        for entry in &task.timeline {
            lines.push(format!(
                "- [{}] {}: {}",
                entry.timestamp, entry.kind, entry.summary
            ));
            if let Some(path) = entry.detail_path.as_ref() {
                lines.push(format!("  detail: {}", path.display()));
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::task_manager::{TaskStatus, TaskSummary};
    use crate::tools::subagent::SubAgentType;
    use crate::tui::app::{InitialInput, TuiOptions};
    use chrono::Utc;
    use std::path::PathBuf;

    fn test_options() -> TuiOptions {
        TuiOptions {
            model: "test-model".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: true,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 4,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None::<InitialInput>,
        }
    }

    fn task_summary(id: &str, status: TaskStatus, duration_ms: Option<u64>) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            status,
            prompt_summary: "Fix task list output".to_string(),
            model: "deepseek-v4-pro".to_string(),
            mode: "agent".to_string(),
            created_at: Utc::now(),
            started_at: None,
            ended_at: None,
            duration_ms,
            error: None,
            thread_id: None,
            turn_id: None,
        }
    }

    #[test]
    fn task_list_includes_title_header_and_time_column() {
        let output = format_task_list(&[
            task_summary("task_12345678", TaskStatus::Running, None),
            task_summary("task_abcdef12", TaskStatus::Completed, Some(1234)),
        ]);

        assert!(output.contains("ID             Status        Time  Title"));
        assert!(output.contains("task_12345678  running           -  Fix task list output"));
        assert!(output.contains("task_abcdef12  completed     1.23s  Fix task list output"));
    }

    #[test]
    fn mailbox_progress_reports_transcript_change_only_for_visible_card_updates() {
        let mut app = App::new(test_options(), &Config::default());
        let started = MailboxMessage::started("agent_live", SubAgentType::General);
        assert!(
            handle_subagent_mailbox(&mut app, 1, &started),
            "first started envelope creates a visible card"
        );

        let progress =
            MailboxMessage::progress("agent_live", "step 1/100: requesting model response");
        assert!(
            !handle_subagent_mailbox(&mut app, 2, &progress),
            "low-signal progress for an already-running card is a no-op"
        );

        let tool = MailboxMessage::ToolCallStarted {
            agent_id: "agent_live".to_string(),
            tool_name: "read_file".to_string(),
            step: 1,
        };
        assert!(
            handle_subagent_mailbox(&mut app, 3, &tool),
            "tool progress still updates the visible transcript card"
        );
    }
}
