//! Safe, bounded Agent Details projection (#2889).
//!
//! The default route intentionally does not expose the child transcript. Exact
//! evidence remains behind an explicit artifact-first action.

use std::path::{Component, Path};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::tools::subagent::{SubAgentResult, SubAgentStatus, localized_whale_display_names};
use crate::tui::app::{
    AgentCurrentActivityStatus, AgentProgressMeta, App, bound_agent_activity_text,
};
use crate::tui::pager::PagerView;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

pub(crate) struct AgentDetailsProjection {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) transcript_available: bool,
}

/// Pager-backed details view with a distinct close receipt and an explicit
/// exact-transcript action.
pub(crate) struct AgentDetailsView {
    pager: PagerView,
    agent_id: String,
    transcript_available: bool,
}

impl AgentDetailsView {
    fn new(projection: AgentDetailsProjection, agent_id: impl Into<String>, width: u16) -> Self {
        let agent_id = agent_id.into();
        let pager =
            PagerView::from_text(projection.title, &projection.body, width.saturating_sub(2))
                .with_copy_text(projection.body);
        Self {
            pager,
            agent_id,
            transcript_available: projection.transcript_available,
        }
    }

    #[cfg(test)]
    fn body_text(&self) -> String {
        self.pager.body_text()
    }

    #[cfg(test)]
    fn title(&self) -> &str {
        self.pager.title()
    }
}

impl ModalView for AgentDetailsView {
    fn kind(&self) -> ModalKind {
        ModalKind::Pager
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if matches!(key.code, KeyCode::Char('v' | 'V'))
            && key.modifiers.contains(KeyModifiers::ALT)
            && self.transcript_available
        {
            return ViewAction::Emit(ViewEvent::OpenAgentTranscript {
                agent_id: self.agent_id.clone(),
            });
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Left)
            || (key.code == KeyCode::Char('q') && key.modifiers.is_empty())
        {
            return ViewAction::EmitAndClose(ViewEvent::AgentDetailsClosed {
                agent_id: self.agent_id.clone(),
            });
        }
        self.pager.handle_key(key)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.pager.handle_paste(text)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        self.pager.handle_mouse(mouse)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.pager.render(area, buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

pub(crate) fn open_agent_details(app: &mut App, agent_id: &str) -> bool {
    let Some(projection) = project_agent_details(app, agent_id) else {
        return false;
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    app.view_stack
        .push(AgentDetailsView::new(projection, agent_id, width));
    true
}

pub(crate) fn safe_agent_display_name(app: &App, agent_id: &str) -> String {
    let generated = localized_whale_display_names(
        app.subagent_cache
            .iter()
            .map(|agent| (agent.agent_id.as_str(), agent.nickname.as_deref())),
        app.ui_locale.tag(),
    );
    generated
        .get(agent_id)
        .cloned()
        .or_else(|| app.agent_label_map.get(agent_id).cloned())
        .and_then(|name| safe_child_value(app, &name))
        .unwrap_or_else(|| "Agent".to_string())
}

pub(crate) fn project_agent_details(app: &App, agent_id: &str) -> Option<AgentDetailsProjection> {
    let agent = app
        .subagent_cache
        .iter()
        .find(|agent| agent.agent_id == agent_id);
    let meta = app.agent_progress_meta.get(agent_id);
    if agent.is_none() && meta.is_none() && !app.agent_progress.contains_key(agent_id) {
        return None;
    }

    let display_name = safe_agent_display_name(app, agent_id);
    let mut lines = Vec::new();

    if let Some(agent) = agent {
        push_safe_line(app, &mut lines, "Assignment", &agent.assignment.objective);

        if let Some(role) = agent.assignment.role.as_deref() {
            push_safe_line(app, &mut lines, "Role", role);
        }
        push_safe_line(app, &mut lines, "Profile", agent.agent_type.as_str());
        lines.push(format!("Parent: {}", safe_parent_name(app, agent)));
    } else {
        lines.push(format!("Parent: {}", safe_parent_from_meta(app, meta)));
    }

    let status = typed_status(agent, meta);
    let steps = agent.map_or(0, |agent| agent.steps_taken);
    let mut state = vec![activity_status_label(status).to_string()];
    if let Some(agent) = agent {
        state.push(format!(
            "elapsed {}",
            crate::elapsed::format_elapsed_ms(agent.duration_ms)
        ));
    }
    state.push(format!(
        "{steps} {}",
        if steps == 1 { "step" } else { "steps" }
    ));
    lines.push(format!("State: {}", state.join(" · ")));

    if let Some(meta) = meta
        && let Some(provider) = meta.resolved_provider.as_deref()
    {
        push_safe_line(app, &mut lines, "Provider", provider);
    }
    let model = meta
        .and_then(|meta| meta.resolved_model.as_deref())
        .or_else(|| agent.map(|agent| agent.model.as_str()));
    if let Some(model) = model {
        push_safe_line(app, &mut lines, "Model", model);
    }

    if let Some(agent) = agent {
        if let Some(workspace) = agent.workspace.as_deref()
            && let Some(workspace) = safe_workspace(app, workspace)
        {
            lines.push(format!("Workspace: {workspace}"));
        }
        if let Some(branch) = agent.git_branch.as_deref() {
            push_safe_line(app, &mut lines, "Branch", branch);
        }
    }

    if let Some(activity) = meta.and_then(|meta| meta.current_activity.as_ref())
        && !matches!(
            activity.status,
            AgentCurrentActivityStatus::Done
                | AgentCurrentActivityStatus::Failed
                | AgentCurrentActivityStatus::Canceled
                | AgentCurrentActivityStatus::Interrupted
                | AgentCurrentActivityStatus::Waiting
        )
    {
        let mut current = vec![activity_status_label(activity.status).to_string()];
        if let Some(tool) = activity
            .current_tool
            .as_deref()
            .and_then(|tool| safe_child_value(app, tool))
        {
            current.push(tool);
        }
        if let Some(step) = activity.step {
            current.push(format!("step {step}"));
        }
        if let Some(detail) = activity
            .detail
            .as_deref()
            .and_then(|detail| safe_child_value(app, detail))
            && !detail.starts_with("started ")
            && !current.iter().any(|part| part == &detail)
        {
            current.push(detail);
        }
        lines.push(format!("Current: {}", current.join(" · ")));
    }

    if let Some(meta) = meta {
        for action in meta.recent_actions.iter().rev().take(3).rev() {
            if let Some(tool) = safe_child_value(app, &action.tool) {
                lines.push(format!(
                    "Recent: {} {tool} · step {}",
                    if action.ok { "✓" } else { "!" },
                    action.step
                ));
            }
        }
    }

    if let Some(question) = pending_question(agent, meta)
        && let Some(question) = safe_child_value(app, question)
    {
        lines.push(format!("Pending question: {question}"));
    }
    if let Some(blocker) = blocker(agent, meta)
        && let Some(blocker) = safe_child_value(app, blocker)
    {
        lines.push(format!("Blocker: {blocker}"));
    }
    if let Some(summary) = terminal_summary(agent, meta)
        && let Some(summary) = safe_child_value(app, summary)
    {
        lines.push(format!("Summary: {summary}"));
    }

    let transcript_available =
        crate::tui::mouse_ui::agent_transcript_evidence_available(app, agent_id);
    if transcript_available {
        lines.push("Exact evidence: available · Alt/⌥V opens transcript".to_string());
    } else {
        lines.push("Exact evidence: unavailable".to_string());
    }

    Some(AgentDetailsProjection {
        title: format!("Agent Details — {display_name}"),
        body: lines.join("\n"),
        transcript_available,
    })
}

fn push_safe_line(app: &App, lines: &mut Vec<String>, label: &str, value: &str) {
    if let Some(value) = safe_child_value(app, value) {
        lines.push(format!("{label}: {value}"));
    }
}

fn safe_child_value(app: &App, value: &str) -> Option<String> {
    let bounded = bound_agent_activity_text(value);
    let scrubbed = scrub_raw_agent_ids(app, &bounded);
    let trimmed = scrubbed.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "none" | "(none)" | "n/a" | "unknown" | "not set" | "not available" | "-"
        )
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn scrub_raw_agent_ids(app: &App, value: &str) -> String {
    let mut scrubbed = value.to_string();
    let mut ids: Vec<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .chain(
            app.subagent_cache
                .iter()
                .filter_map(|agent| agent.parent_run_id.as_deref()),
        )
        .chain(app.agent_progress_meta.keys().map(String::as_str))
        .chain(
            app.agent_progress_meta
                .values()
                .filter_map(|meta| meta.parent_run_id.as_deref()),
        )
        .chain(app.agent_progress.keys().map(String::as_str))
        .chain(app.agent_label_map.keys().map(String::as_str))
        .collect();
    ids.sort_unstable_by_key(|id| std::cmp::Reverse(id.len()));
    ids.dedup();
    for id in ids {
        if !id.is_empty() {
            scrubbed = scrubbed.replace(id, "agent");
        }
    }

    let mut output = String::with_capacity(scrubbed.len());
    let mut token = String::new();
    let flush = |token: &mut String, output: &mut String| {
        if token.starts_with("agent_")
            || token.starts_with("agent-")
            || token.starts_with("worker:agent_")
            || token.starts_with("worker:agent-")
        {
            output.push_str("agent");
        } else {
            output.push_str(token);
        }
        token.clear();
    };
    for ch in scrubbed.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':') {
            token.push(ch);
        } else {
            flush(&mut token, &mut output);
            output.push(ch);
        }
    }
    flush(&mut token, &mut output);
    output
}

fn safe_parent_name(app: &App, agent: &SubAgentResult) -> String {
    match agent.parent_run_id.as_deref() {
        Some(parent_id)
            if app
                .subagent_cache
                .iter()
                .any(|candidate| candidate.agent_id == parent_id) =>
        {
            safe_agent_display_name(app, parent_id)
        }
        Some(_) if agent.spawn_depth > 1 => "parent agent".to_string(),
        _ => "primary session".to_string(),
    }
}

fn safe_parent_from_meta(app: &App, meta: Option<&AgentProgressMeta>) -> String {
    match meta.and_then(|meta| meta.parent_run_id.as_deref()) {
        Some(parent_id)
            if app
                .subagent_cache
                .iter()
                .any(|candidate| candidate.agent_id == parent_id) =>
        {
            safe_agent_display_name(app, parent_id)
        }
        Some(_) => "parent agent".to_string(),
        None => "primary session".to_string(),
    }
}

fn typed_status(
    agent: Option<&SubAgentResult>,
    meta: Option<&AgentProgressMeta>,
) -> AgentCurrentActivityStatus {
    if let Some(status) = meta
        .and_then(|meta| meta.current_activity.as_ref())
        .map(|activity| activity.status)
    {
        return status;
    }
    if let Some(status) = agent.and_then(|agent| agent.worker_status) {
        return status.into();
    }
    match agent.map(|agent| &agent.status) {
        Some(SubAgentStatus::Running) | None => AgentCurrentActivityStatus::Running,
        Some(SubAgentStatus::Completed) => AgentCurrentActivityStatus::Done,
        Some(SubAgentStatus::Interrupted(_)) => AgentCurrentActivityStatus::Interrupted,
        Some(SubAgentStatus::Failed(_) | SubAgentStatus::BudgetExhausted) => {
            AgentCurrentActivityStatus::Failed
        }
        Some(SubAgentStatus::Cancelled) => AgentCurrentActivityStatus::Canceled,
    }
}

fn activity_status_label(status: AgentCurrentActivityStatus) -> &'static str {
    match status {
        AgentCurrentActivityStatus::Queued => "queued",
        AgentCurrentActivityStatus::Starting => "starting",
        AgentCurrentActivityStatus::Running => "running",
        AgentCurrentActivityStatus::ModelWait => "waiting for model",
        AgentCurrentActivityStatus::RunningTool => "running tool",
        AgentCurrentActivityStatus::Waiting => "waiting for input",
        AgentCurrentActivityStatus::Done => "completed",
        AgentCurrentActivityStatus::Failed => "failed",
        AgentCurrentActivityStatus::Canceled => "canceled",
        AgentCurrentActivityStatus::Interrupted => "interrupted",
    }
}

fn pending_question<'a>(
    agent: Option<&'a SubAgentResult>,
    meta: Option<&'a AgentProgressMeta>,
) -> Option<&'a str> {
    agent
        .and_then(|agent| agent.needs_input.as_ref())
        .map(|needs_input| needs_input.question.as_str())
        .or_else(|| {
            meta.and_then(|meta| meta.current_activity.as_ref())
                .filter(|activity| activity.status == AgentCurrentActivityStatus::Waiting)
                .and_then(|activity| activity.detail.as_deref())
        })
        .or_else(|| match agent.map(|agent| &agent.status) {
            Some(SubAgentStatus::Interrupted(reason)) => Some(reason.as_str()),
            _ => None,
        })
}

fn blocker<'a>(
    agent: Option<&'a SubAgentResult>,
    meta: Option<&'a AgentProgressMeta>,
) -> Option<&'a str> {
    match agent.map(|agent| &agent.status) {
        Some(SubAgentStatus::Failed(error) | SubAgentStatus::Interrupted(error)) => {
            Some(error.as_str())
        }
        Some(SubAgentStatus::BudgetExhausted) => Some("worker budget exhausted"),
        _ => meta
            .and_then(|meta| meta.current_activity.as_ref())
            .filter(|activity| activity.status == AgentCurrentActivityStatus::Failed)
            .and_then(|activity| activity.detail.as_deref()),
    }
}

fn terminal_summary<'a>(
    agent: Option<&'a SubAgentResult>,
    meta: Option<&'a AgentProgressMeta>,
) -> Option<&'a str> {
    let agent = agent?;
    if !matches!(
        agent.status,
        SubAgentStatus::Completed | SubAgentStatus::Cancelled
    ) {
        return None;
    }
    agent.result.as_deref().or_else(|| {
        meta.and_then(|meta| meta.current_activity.as_ref())
            .and_then(|activity| activity.detail.as_deref())
    })
}

fn safe_workspace(app: &App, workspace: &Path) -> Option<String> {
    if workspace.as_os_str().is_empty() {
        return None;
    }
    if workspace == app.workspace {
        return Some(".".to_string());
    }
    if let Ok(relative) = workspace.strip_prefix(&app.workspace) {
        let parts: Vec<String> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => part.to_str().map(ToString::to_string),
                _ => None,
            })
            .collect();
        if !parts.is_empty() {
            return safe_child_value(app, &parts.join("/"));
        }
    }
    workspace
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| safe_child_value(app, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{buffer::Buffer, layout::Rect};
    use serde_json::json;
    use tempfile::tempdir;

    use crate::config::Config;
    use crate::tools::subagent::{
        AgentWorkerStatus, FleetRole, SubAgentAssignment, SubAgentNeedsInput,
    };
    use crate::tui::app::{
        AgentCurrentActivity, AgentRecentAction, MAX_AGENT_RECENT_ACTIONS, TuiOptions,
    };

    fn test_app(workspace: PathBuf) -> App {
        App::new(
            TuiOptions {
                model: "test-model".to_string(),
                use_mouse_capture: true,
                max_subagents: 4,
                ..crate::test_support::test_tui_options(workspace)
            },
            &Config::default(),
        )
    }

    fn agent(agent_id: &str, status: SubAgentStatus) -> SubAgentResult {
        SubAgentResult {
            name: agent_id.to_string(),
            agent_id: agent_id.to_string(),
            context_mode: "isolated".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: FleetRole::Builder,
            assignment: SubAgentAssignment {
                objective: "Implement the bounded details route".to_string(),
                role: Some("worker".to_string()),
            },
            model: "deepseek-v4-pro".to_string(),
            nickname: Some("Blue Whale".to_string()),
            status,
            worker_status: None,
            parent_run_id: None,
            spawn_depth: 1,
            result: None,
            steps_taken: 2,
            checkpoint: None,
            needs_input: None,
            duration_ms: 2_500,
            from_prior_session: false,
        }
    }

    fn body_for(
        status: SubAgentStatus,
        worker_status: AgentWorkerStatus,
        detail: Option<&str>,
    ) -> String {
        let tmp = tempdir().expect("tempdir");
        let mut app = test_app(tmp.path().to_path_buf());
        let agent_id = "agent_matrix_subject";
        let mut child = agent(agent_id, status);
        child.worker_status = Some(worker_status);
        match worker_status {
            AgentWorkerStatus::WaitingForUser => {
                child.needs_input = Some(SubAgentNeedsInput {
                    question: detail.unwrap_or("Which path should I use?").to_string(),
                });
            }
            AgentWorkerStatus::Completed => child.result = detail.map(str::to_string),
            _ => {}
        }
        app.subagent_cache.push(child);
        app.agent_progress_meta.insert(
            agent_id.to_string(),
            AgentProgressMeta {
                current_activity: Some(AgentCurrentActivity::bounded(
                    worker_status.into(),
                    detail.map(str::to_string),
                    (worker_status == AgentWorkerStatus::RunningTool)
                        .then(|| "read_file".to_string()),
                    Some(2),
                )),
                ..AgentProgressMeta::default()
            },
        );
        project_agent_details(&app, agent_id)
            .expect("projection")
            .body
    }

    #[test]
    fn provider_free_status_matrix_is_typed_and_bounded() {
        let running = body_for(
            SubAgentStatus::Running,
            AgentWorkerStatus::RunningTool,
            None,
        );
        assert!(running.contains("State: running tool · elapsed 2s · 2 steps"));
        assert!(running.contains("Current: running tool · read_file · step 2"));
        assert!(!running.contains("Provider:"));

        let waiting = body_for(
            SubAgentStatus::Running,
            AgentWorkerStatus::WaitingForUser,
            Some("Which path should I use?"),
        );
        assert!(waiting.contains("State: waiting for input"));
        assert!(waiting.contains("Pending question: Which path should I use?"));

        let failed = body_for(
            SubAgentStatus::Failed("verification failed".to_string()),
            AgentWorkerStatus::Failed,
            Some("verification failed"),
        );
        assert!(failed.contains("State: failed"));
        assert!(failed.contains("Blocker: verification failed"));

        let completed = body_for(
            SubAgentStatus::Completed,
            AgentWorkerStatus::Completed,
            Some("all checks passed"),
        );
        assert!(completed.contains("State: completed"));
        assert!(completed.contains("Summary: all checks passed"));
    }

    #[test]
    fn projection_redacts_child_strings_and_never_exposes_raw_ids_or_none() {
        let tmp = tempdir().expect("tempdir");
        let mut app = test_app(tmp.path().to_path_buf());
        let agent_id = "agent_secret_child";
        let parent_id = "agent_raw_parent";
        let mut parent = agent(parent_id, SubAgentStatus::Running);
        parent.nickname = Some("Parent Whale".to_string());
        let mut child = agent(agent_id, SubAgentStatus::Running);
        child.parent_run_id = Some(parent_id.to_string());
        child.spawn_depth = 2;
        child.assignment.objective = format!(
            "\u{1b}[31minspect {agent_id}\u{1b}[0m with api_key=sk-agent-details-secret-1234567890"
        );
        child.git_branch = Some(format!("work/{parent_id}"));
        child.model.clear();
        child.nickname = Some(format!("\u{1b}[35m{agent_id}\u{1b}[0m"));
        app.subagent_cache.extend([parent, child]);

        let projection = project_agent_details(&app, agent_id).expect("projection");
        let all = format!("{}\n{}", projection.title, projection.body);
        assert!(!all.contains(agent_id), "{all}");
        assert!(!all.contains(parent_id), "{all}");
        assert!(!all.contains("sk-agent-details-secret"), "{all}");
        assert!(!all.contains('\u{1b}'), "{all:?}");
        assert!(!all.contains("None"), "{all}");
        assert!(all.contains("[redacted]"), "{all}");
    }

    #[test]
    fn external_workspace_is_basename_safe_and_branch_is_bounded() {
        let mut app = test_app(PathBuf::from("/repo/main"));
        let agent_id = "agent_external_workspace";
        let mut child = agent(agent_id, SubAgentStatus::Running);
        child.workspace = Some(PathBuf::from("/private/customer/secret/repo-child"));
        child.git_branch = Some("codex/details".to_string());
        app.subagent_cache.push(child);

        let body = project_agent_details(&app, agent_id)
            .expect("projection")
            .body;
        assert!(body.contains("Workspace: repo-child"), "{body}");
        assert!(!body.contains("/private/customer/secret"), "{body}");
        assert!(body.contains("Branch: codex/details"), "{body}");
    }

    #[test]
    fn recent_actions_are_bounded_and_render_only_structured_outcomes() {
        let tmp = tempdir().expect("tempdir");
        let mut app = test_app(tmp.path().to_path_buf());
        let agent_id = "agent_recent_actions";
        app.subagent_cache
            .push(agent(agent_id, SubAgentStatus::Running));
        let mut meta = AgentProgressMeta::default();
        for step in 1..=MAX_AGENT_RECENT_ACTIONS as u32 {
            meta.recent_actions.push_back(AgentRecentAction::bounded(
                if step == 2 {
                    "apply_patch"
                } else {
                    "read_file"
                },
                step,
                step != 2,
            ));
        }
        app.agent_progress_meta.insert(agent_id.to_string(), meta);

        let body = project_agent_details(&app, agent_id)
            .expect("projection")
            .body;
        assert_eq!(body.matches("Recent:").count(), 3, "{body}");
        assert!(body.contains("Recent: ! apply_patch · step 2"), "{body}");
    }

    #[test]
    fn alt_v_is_truthful_for_present_and_absent_evidence() {
        let tmp = tempdir().expect("tempdir");
        let agent_id = "agent_evidence";
        let mut app = test_app(tmp.path().to_path_buf());
        app.subagent_cache
            .push(agent(agent_id, SubAgentStatus::Running));
        let absent = project_agent_details(&app, agent_id).expect("projection");
        assert!(!absent.transcript_available);
        assert!(!absent.body.contains("Alt/⌥V"));
        let mut absent_view = AgentDetailsView::new(absent, agent_id, 80);
        assert!(matches!(
            absent_view.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)),
            ViewAction::None
        ));

        {
            let mut store = app
                .runtime_services
                .handle_store
                .try_lock()
                .expect("handle store");
            let _ = store.insert_json(
                format!("agent:{agent_id}"),
                "full_transcript",
                json!({
                    "message_count": 1,
                    "messages": [{
                        "role": "assistant",
                        "content": [{
                            "type": "text",
                            "text": "exact evidence",
                            "cache_control": null
                        }]
                    }]
                }),
            );
        }
        let present = project_agent_details(&app, agent_id).expect("projection");
        assert!(present.transcript_available);
        assert!(present.body.contains("Alt/⌥V opens transcript"));
        let mut present_view = AgentDetailsView::new(present, agent_id, 80);
        assert!(matches!(
            present_view.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)),
            ViewAction::Emit(ViewEvent::OpenAgentTranscript { agent_id: ref id }) if id == agent_id
        ));

        assert!(open_agent_details(&mut app, agent_id));
        let events = app
            .view_stack
            .handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT));
        assert!(matches!(
            events.as_slice(),
            [ViewEvent::OpenAgentTranscript { agent_id: id }] if id == agent_id
        ));
        assert!(crate::tui::mouse_ui::open_agent_chat_pager(
            &mut app, agent_id
        ));
        assert!(
            app.view_stack
                .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                .is_empty(),
            "transcript Esc closes only the top pager"
        );
        assert!(matches!(
            app.view_stack
                .handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT))
                .as_slice(),
            [ViewEvent::OpenAgentTranscript { agent_id: id }] if id == agent_id
        ));
    }

    #[test]
    fn close_keys_emit_receipt_and_80x24_render_stays_safe() {
        let tmp = tempdir().expect("tempdir");
        let agent_id = "agent_render_80x24";
        let mut app = test_app(tmp.path().to_path_buf());
        app.subagent_cache
            .push(agent(agent_id, SubAgentStatus::Running));
        let projection = project_agent_details(&app, agent_id).expect("projection");
        let mut view = AgentDetailsView::new(projection, agent_id, 80);
        assert!(view.title().starts_with("Agent Details — "));
        assert!(view.body_text().contains("Exact evidence: unavailable"));

        let area = Rect::new(0, 0, 80, 24);
        let mut buffer = Buffer::empty(area);
        view.render(area, &mut buffer);
        let rendered = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .map(|point| buffer[point].symbol())
            .collect::<String>();
        assert!(rendered.contains("Agent Details"), "{rendered}");
        assert!(rendered.contains("Assignment"), "{rendered}");
        assert!(!rendered.contains(agent_id), "{rendered}");

        for code in [KeyCode::Esc, KeyCode::Left] {
            assert!(matches!(
                view.handle_key(KeyEvent::new(code, KeyModifiers::NONE)),
                ViewAction::EmitAndClose(ViewEvent::AgentDetailsClosed { agent_id: ref id })
                    if id == agent_id
            ));
        }
    }
}
