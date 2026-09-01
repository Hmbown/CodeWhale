//! Task-panel and shell projection: task-panel refresh, shell live-output
//! reconciliation, detached-job projection, and RLM task entries
//! (TUI_MODULARIZATION.md slice 5). Pure projection — no dispatch here.

use super::*;
use crate::tui::automation_panel::AutomationScan;

pub(super) async fn refresh_active_task_panel(
    app: &mut App,
    task_manager: &SharedTaskManager,
) -> bool {
    let tasks = match app.current_session_id.as_deref() {
        Some(session_id) => {
            task_manager
                .list_tasks_for_owner(None, None, session_id)
                .await
        }
        None => Vec::new(),
    };
    let previously_active_durable_ids = app
        .task_panel
        .iter()
        .filter(|entry| matches!(entry.status.as_str(), "queued" | "running"))
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let durable_background_completed = newly_completed_id(
        previously_active_durable_ids,
        tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .map(|task| task.id.as_str()),
    );
    let mut lifecycle_changed = false;
    if let (Some(work), Some(session_id)) = (
        app.runtime_services.work.as_ref(),
        app.current_session_id.as_deref(),
    ) {
        for task in &tasks {
            let external = format!("task:{}", task.id);
            if !work.has_operation_binding(Some(session_id), &external) {
                continue;
            }
            match work.reconcile_operation(
                session_id,
                task_owner_snapshot(
                    &task.id,
                    task.status,
                    task.lifecycle_seq,
                    task.created_at,
                    task.started_at,
                    task.ended_at,
                ),
            ) {
                Ok(changed) => lifecycle_changed |= changed,
                Err(err) => {
                    tracing::warn!(task_id = %task.id, error = %err, "failed to reconcile durable task lifecycle");
                }
            }
        }
    }
    if lifecycle_changed && let Err(err) = persist_pending_work_checkpoint(app).await {
        tracing::warn!(error = %err, "durable task lifecycle checkpoint remains pending");
    }
    let session_started_at = app.session_started_at;
    let mut entries: Vec<TaskPanelEntry> =
        select_work_sidebar_tasks(tasks, session_started_at, app.current_session_id.as_deref())
            .into_iter()
            .map(task_summary_to_panel_entry)
            .collect();

    entries.extend(active_rlm_task_entries(app));

    // #3804: this is a render-only read of shell jobs and must not block the
    // async UI loop on the shell manager's std::sync Mutex. Use try_lock; on
    // contention, retain the previous frame's background shell entries so
    // running shells don't flicker out of the Work panel. Shell ownership,
    // cancellation, approval state, and output capture never depend on this
    // refresh succeeding.
    let prev_shell_entries: Vec<TaskPanelEntry> = app
        .task_panel
        .iter()
        .filter(|entry| matches!(entry.kind, TaskPanelEntryKind::Background))
        .cloned()
        .collect();
    let prev_shell_ids = prev_shell_entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let (shell_entries, shell_background_completed): (Vec<TaskPanelEntry>, bool) = match app
        .runtime_services
        .shell_manager
        .as_ref()
    {
        Some(shell_mgr) => match shell_mgr.try_lock() {
            Ok(mut mgr) => {
                let jobs = mgr
                    .list_jobs_for_session(app.current_session_id.as_deref().unwrap_or_default());
                let completed = newly_completed_id(
                    prev_shell_ids.iter().map(String::as_str).collect(),
                    jobs.iter()
                        .filter(|job| {
                            matches!(job.status, crate::tools::shell::ShellStatus::Completed)
                        })
                        .map(|job| job.id.as_str()),
                );
                let entries = jobs
                    .into_iter()
                    .filter(|job| matches!(job.status, crate::tools::shell::ShellStatus::Running))
                    .map(|job| TaskPanelEntry {
                        id: job.id,
                        status: "running".to_string(),
                        prompt_summary: format!("shell: {}", job.command),
                        duration_ms: Some(job.elapsed_ms),
                        kind: TaskPanelEntryKind::Background,
                        stale: job.stale,
                        elapsed_since_output_ms: job.elapsed_since_output_ms,
                        owner_agent_id: job.owner_agent_id,
                        owner_agent_name: job.owner_agent_name,
                        current_tool: None,
                        role: None,
                        files_touched: 0,
                    })
                    .collect();
                (entries, completed)
            }
            // Contended: keep the last known snapshot rather than blocking.
            // A retained frame could belong to the session that was just
            // replaced. Fail closed on contention instead of showing it
            // in the new conversation.
            Err(_) => (Vec::new(), false),
        },
        None => (Vec::new(), false),
    };
    entries.extend(shell_entries);

    // Report whether anything visible changed so the idle tick can skip the
    // redraw: an unconditional 2.5 s repaint kept the app from ever going
    // quiescent (#3757).
    let changed = lifecycle_changed || app.task_panel != entries;
    app.task_panel = entries;
    let tip_shown = (durable_background_completed || shell_background_completed)
        && app.maybe_show_behavioral_tip(
            crate::tui::behavioral_tips::BehavioralTip::BackgroundJobReceipt,
        );
    changed || tip_shown
}

pub(super) fn newly_completed_id<'a>(
    previously_active_ids: HashSet<&'a str>,
    completed_ids: impl IntoIterator<Item = &'a str>,
) -> bool {
    completed_ids
        .into_iter()
        .any(|id| previously_active_ids.contains(id))
}

/// Newest runs scanned per automation when refreshing the automation
/// projection. Live runs sit at the head of the newest-first listing, and a
/// failure once seen is held unacknowledged by the projection until the
/// operator engages the automation surface, so the band never needs a full
/// run-history scan on the render cadence.
const AUTOMATION_PANEL_RUN_SCAN: usize = 25;

/// Refresh the activity band's scheduled-work projection
/// (AUTOMATION-VISIBILITY-SPEC §2.1) from the durable automation store.
///
/// The store is files on disk — every definition plus up to
/// `AUTOMATION_PANEL_RUN_SCAN` run files per definition — so the scan never
/// runs on the async UI loop: it is taken on a blocking thread, and this
/// tick folds whatever scan has finished, then starts the next one. At most
/// one scan is in flight; a slow disk costs the band latency, never the
/// frame. Returns whether the visible state changed, so the idle tick can
/// skip the redraw (#3757).
pub(super) async fn refresh_automation_panel(app: &mut App) -> bool {
    let mut changed = false;
    if let Some(scan) = app.automation_scan.take() {
        if scan.is_finished() {
            match scan.await {
                Ok(scan) => changed = fold_automation_scan(app, &scan),
                Err(err) => {
                    tracing::warn!(error = %err, "automation panel scan task failed");
                }
            }
        } else {
            app.automation_scan = Some(scan);
            return false;
        }
    }
    app.automation_scan = start_automation_scan(app, false);
    changed
}

/// Startup variant: take one scan and wait for it, so the first frame
/// already carries the band count (the task panel gets the same courtesy).
/// The startup pass is FULL — every run file — so a long-running task that
/// already sits behind more than a window of newer runs is visible from the
/// first frame. The manager lock is retried briefly: the scheduler tick
/// holds it only while persisting, and giving up here would blank the band
/// on a contended startup.
pub(super) async fn refresh_automation_panel_blocking(app: &mut App) -> bool {
    let scan = 'retry: {
        for _ in 0..STARTUP_SCAN_LOCK_RETRIES {
            if let Some(scan) = start_automation_scan(app, true) {
                break 'retry Some(scan);
            }
            tokio::time::sleep(STARTUP_SCAN_LOCK_RETRY_DELAY).await;
        }
        None
    };
    let Some(scan) = scan else {
        return false;
    };
    match scan.await {
        Ok(scan) => fold_automation_scan(app, &scan),
        Err(err) => {
            tracing::warn!(error = %err, "automation panel scan task failed");
            false
        }
    }
}

/// Startup lock-retry budget: the scheduler's persist phase is short; ten
/// 50 ms attempts covers it without parking startup on a stuck lock.
const STARTUP_SCAN_LOCK_RETRIES: usize = 10;
const STARTUP_SCAN_LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// Start one store scan on a blocking thread. The manager is cloned out
/// from under its tokio Mutex (`try_lock`, same rule as the shell snapshot:
/// the scheduler tick holds that lock while persisting, and the UI loop
/// must not park behind it; on contention the next tick retries) so the
/// scan holds no lock while it reads — the store's writes are atomic
/// renames, so a concurrent read sees a whole file either way.
///
/// `full` reads every run file (startup only). The cadence scan reads the
/// newest `AUTOMATION_PANEL_RUN_SCAN` runs per automation PLUS the runs
/// this session already watched go live, wherever they sit in history — a
/// frequent automation can stack newer runs behind a long-running task,
/// and neither the live count nor the settle receipt may depend on the
/// task staying inside the newest window.
fn start_automation_scan(app: &App, full: bool) -> Option<tokio::task::JoinHandle<AutomationScan>> {
    let automations = app.runtime_services.automations.as_ref()?;
    let manager = automations.try_lock().ok()?.clone();
    let live_owners = app.automation_panel.live_run_owners();
    Some(tokio::task::spawn_blocking(move || {
        let records = match manager.list_automations() {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(error = %err, "automation panel refresh could not list automations");
                return AutomationScan::default();
            }
        };
        let mut runs = Vec::new();
        for record in &records {
            let limit = if full {
                None
            } else {
                Some(AUTOMATION_PANEL_RUN_SCAN)
            };
            match manager.list_runs(&record.id, limit) {
                Ok(recent) => runs.extend(recent),
                Err(err) => {
                    tracing::warn!(automation_id = %record.id, error = %err, "automation panel refresh could not list runs");
                }
            }
            if !full {
                let wanted: std::collections::BTreeSet<String> = live_owners
                    .iter()
                    .filter(|(_, owner)| owner.as_str() == record.id.as_str())
                    .map(|(run_id, _)| run_id.clone())
                    .collect();
                if !wanted.is_empty() {
                    match manager.get_runs_by_ids(&record.id, &wanted) {
                        Ok(found) => runs.extend(found),
                        Err(err) => {
                            tracing::warn!(automation_id = %record.id, error = %err, "automation panel refresh could not re-read live runs");
                        }
                    }
                }
            }
        }
        // A re-read live run may also sit inside the newest window; the
        // fold counts by id, so dedupe before handing the scan over.
        let mut seen = std::collections::BTreeSet::new();
        runs.retain(|run| seen.insert(run.id.clone()));
        AutomationScan { records, runs }
    }))
}

/// Fold a finished scan into the projection and post the typed receipt
/// (spec §2.2) for every run this session watched go live and settle — the
/// transcript learns about background work from the same scan that
/// repaints the band.
fn fold_automation_scan(app: &mut App, scan: &AutomationScan) -> bool {
    let session_started_at = app.session_started_at;
    let delta = app
        .automation_panel
        .fold_scan(&scan.records, &scan.runs, session_started_at);
    let locale = app.ui_locale;
    for run in &delta.settled {
        app.add_message(crate::tui::automation_routing::settled_run_receipt(
            locale, run,
        ));
    }
    delta.changed || !delta.settled.is_empty()
}

pub(super) fn refresh_shell_exec_live_output(app: &mut App) -> bool {
    let Some(shell_mgr) = app.runtime_services.shell_manager.as_ref().cloned() else {
        return false;
    };
    // #3804: render-only read — try_lock so a contended shell Mutex can never
    // block the async UI loop; skip this frame's live-output update on
    // contention (the next refresh picks it up).
    let jobs = {
        let Ok(mut mgr) = shell_mgr.try_lock() else {
            return false;
        };
        mgr.list_jobs_for_session(app.current_session_id.as_deref().unwrap_or_default())
            .into_iter()
            .map(|job| (job.id.clone(), job))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let mut changed = false;
    for index in 0..app.virtual_cell_count() {
        let Some(ShellExecLiveUpdate {
            task_id,
            status: next_status,
            output: next_live,
            duration_ms: next_duration,
            finalized,
            stale_elapsed_since_output_ms,
        }) = shell_exec_live_update(app, index, &jobs)
        else {
            continue;
        };
        let Some(HistoryCell::Tool(ToolCell::Exec(exec))) = app.cell_at_virtual_index_mut(index)
        else {
            continue;
        };
        if exec.output.is_some() || exec.shell_task_id.as_deref() != Some(task_id.as_str()) {
            continue;
        }
        exec.status = next_status;
        exec.duration_ms = Some(next_duration);
        exec.stale_elapsed_since_output_ms = stale_elapsed_since_output_ms;
        if finalized {
            exec.output = next_live;
            exec.output_summary = exec
                .output
                .as_deref()
                .map(crate::tui::history::summarize_tool_output);
            exec.live_output = None;
            exec.stale_elapsed_since_output_ms = None;
        } else {
            exec.live_output = next_live;
        }
        changed = true;
    }
    changed
}

pub(super) struct ShellExecLiveUpdate {
    pub(super) task_id: String,
    pub(super) status: ToolStatus,
    pub(super) output: Option<String>,
    pub(super) duration_ms: u64,
    pub(super) finalized: bool,
    pub(super) stale_elapsed_since_output_ms: Option<u64>,
}

pub(super) fn shell_exec_live_update(
    app: &App,
    index: usize,
    jobs: &std::collections::HashMap<String, ShellJobSnapshot>,
) -> Option<ShellExecLiveUpdate> {
    let HistoryCell::Tool(ToolCell::Exec(exec)) = app.cell_at_virtual_index(index)? else {
        return None;
    };
    if exec.output.is_some() {
        return None;
    }
    let task_id = exec.shell_task_id.as_deref()?;
    let Some(job) = jobs.get(task_id) else {
        return Some(ShellExecLiveUpdate {
            task_id: task_id.to_string(),
            status: ToolStatus::Failed,
            output: detached_shell_job_output(task_id, exec),
            duration_ms: exec.duration_ms.unwrap_or_default(),
            finalized: true,
            stale_elapsed_since_output_ms: None,
        });
    };
    let next_status = shell_job_tool_status(&job.status);
    let next_live = shell_job_live_output(job).or_else(|| exec.live_output.clone());
    let finalized = !matches!(job.status, ShellStatus::Running);
    let stale_elapsed_since_output_ms = if matches!(job.status, ShellStatus::Running) && job.stale {
        Some(job.elapsed_since_output_ms.unwrap_or(0))
    } else {
        None
    };
    if exec.status == next_status
        && exec.live_output == next_live
        && exec.duration_ms == Some(job.elapsed_ms)
        && exec.stale_elapsed_since_output_ms == stale_elapsed_since_output_ms
    {
        return None;
    }
    Some(ShellExecLiveUpdate {
        task_id: task_id.to_string(),
        status: next_status,
        output: next_live,
        duration_ms: job.elapsed_ms,
        finalized,
        stale_elapsed_since_output_ms,
    })
}

pub(super) fn detached_shell_job_output(task_id: &str, exec: &ExecCell) -> Option<String> {
    let mut output = exec.live_output.clone().unwrap_or_default();
    if !output.trim().is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(&format!(
        "Shell job `{task_id}` is no longer attached to this TUI session."
    ));
    Some(output)
}

pub(super) fn shell_job_tool_status(status: &ShellStatus) -> ToolStatus {
    match status {
        ShellStatus::Running => ToolStatus::Running,
        ShellStatus::Completed => ToolStatus::Success,
        ShellStatus::Failed | ShellStatus::Killed | ShellStatus::TimedOut => ToolStatus::Failed,
    }
}

pub(super) fn shell_job_live_output(job: &ShellJobSnapshot) -> Option<String> {
    match (job.stdout_tail.is_empty(), job.stderr_tail.is_empty()) {
        (true, true) => None,
        (false, true) => Some(job.stdout_tail.clone()),
        (true, false) => Some(format!("STDERR:\n{}", job.stderr_tail)),
        (false, false) => Some(format!(
            "{}\n\nSTDERR:\n{}",
            job.stdout_tail, job.stderr_tail
        )),
    }
}

pub(super) fn active_rlm_task_entries(app: &App) -> Vec<TaskPanelEntry> {
    let Some(active) = app.active_cell.as_ref() else {
        return Vec::new();
    };
    let duration_ms = app
        .turn_started_at
        .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    active
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let HistoryCell::Tool(ToolCell::Generic(generic)) = entry else {
                return None;
            };
            if !matches!(
                generic.name.as_str(),
                "rlm_open" | "rlm_eval" | "rlm_configure" | "rlm_close" | "rlm"
            ) || generic.status != ToolStatus::Running
            {
                return None;
            }
            let summary = generic
                .input_summary
                .as_deref()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or("running chunked analysis");
            Some(TaskPanelEntry {
                id: format!("rlm-{}", idx + 1),
                status: "running".to_string(),
                prompt_summary: format!("RLM: {summary}"),
                duration_ms,
                kind: TaskPanelEntryKind::Background,
                stale: false,
                elapsed_since_output_ms: None,
                owner_agent_id: None,
                owner_agent_name: None,
                current_tool: None,
                role: None,
                files_touched: 0,
            })
        })
        .collect()
}
