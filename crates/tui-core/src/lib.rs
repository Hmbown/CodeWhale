/// Identifies which pane is currently focused in the TUI layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// Main chat/conversation pane.
    Chat,
    /// Diff viewer pane.
    Diff,
    /// Task list pane.
    Tasks,
    /// Sub-agent list pane.
    Agents,
    /// Status overview pane.
    Status,
    /// Background jobs pane.
    Jobs,
}

/// Events fed into the UI state machine from user input or runtime updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// A key was pressed by the user.
    KeyPressed(char),
    /// The user submitted a prompt string.
    PromptSubmitted(String),
    /// A partial response arrived from the model.
    ResponseDelta(String),
    /// A tool began executing.
    ToolStarted(String),
    /// A tool finished executing.
    ToolFinished(String),
    /// A background job was queued.
    JobQueued(String),
    /// A background job reported progress.
    JobProgress { job_id: String, progress: u8 },
    /// A background job completed.
    JobCompleted(String),
    /// An exec approval was requested from the user.
    ApprovalRequested(String),
    /// An exec approval was resolved.
    ApprovalResolved(String),
    /// The user requested a pause.
    PauseRequested,
    /// The user requested a resume.
    ResumeRequested,
    /// Periodic tick for background work scheduling.
    Tick,
}

/// Side effects emitted by the state machine in response to events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEffect {
    /// The UI should re-render.
    Render,
    /// A checkpoint should be persisted to the state store.
    PersistCheckpoint,
    /// A background refresh should be scheduled.
    ScheduleBackgroundRefresh,
    /// A status line message should be emitted to the footer.
    EmitStatusLine(String),
}

/// The complete UI state, driven by [`UiEvent`] via [`UiState::reduce`].
#[derive(Debug, Clone)]
pub struct UiState {
    /// Currently active/focused pane.
    pub active_pane: Pane,
    /// Whether the UI is paused (no new work dispatched).
    pub paused: bool,
    /// Most recent partial response delta from the model.
    pub last_response_delta: Option<String>,
    /// Name of the currently executing tool, if any.
    pub active_tool: Option<String>,
    /// Number of tasks waiting to be processed.
    pub pending_tasks: usize,
    /// Number of active background jobs.
    pub active_jobs: usize,
    /// Number of pending approval requests.
    pub pending_approvals: usize,
    /// Current status line text shown in the footer.
    pub status_line: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            active_pane: Pane::Chat,
            paused: false,
            last_response_delta: None,
            active_tool: None,
            pending_tasks: 0,
            active_jobs: 0,
            pending_approvals: 0,
            status_line: "ready".to_string(),
        }
    }
}

impl UiState {
    /// Process a UI event, updating internal state and returning side effects.
    pub fn reduce(&mut self, event: UiEvent) -> Vec<UiEffect> {
        match event {
            UiEvent::KeyPressed('1') => {
                self.active_pane = Pane::Chat;
                vec![UiEffect::Render]
            }
            UiEvent::KeyPressed('2') => {
                self.active_pane = Pane::Diff;
                vec![UiEffect::Render]
            }
            UiEvent::KeyPressed('3') => {
                self.active_pane = Pane::Tasks;
                vec![UiEffect::Render]
            }
            UiEvent::KeyPressed('4') => {
                self.active_pane = Pane::Agents;
                vec![UiEffect::Render]
            }
            UiEvent::KeyPressed('5') => {
                self.active_pane = Pane::Jobs;
                vec![UiEffect::Render]
            }
            UiEvent::PromptSubmitted(_) => {
                self.pending_tasks = self.pending_tasks.saturating_add(1);
                self.status_line = "prompt submitted".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::PersistCheckpoint,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ResponseDelta(delta) => {
                self.last_response_delta = Some(delta);
                self.status_line = "streaming response".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ToolStarted(name) => {
                self.active_tool = Some(name.clone());
                self.status_line = format!("tool running: {name}");
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ToolFinished(name) => {
                self.active_tool = None;
                self.pending_tasks = self.pending_tasks.saturating_sub(1);
                self.status_line = format!("tool finished: {name}");
                vec![
                    UiEffect::Render,
                    UiEffect::PersistCheckpoint,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::JobQueued(_) => {
                self.active_jobs = self.active_jobs.saturating_add(1);
                self.status_line = "job queued".to_string();
                vec![UiEffect::Render, UiEffect::PersistCheckpoint]
            }
            UiEvent::JobProgress { progress, .. } => {
                self.status_line = format!("job progress: {}%", progress.min(100));
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::JobCompleted(_) => {
                self.active_jobs = self.active_jobs.saturating_sub(1);
                self.status_line = "job completed".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::PersistCheckpoint,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ApprovalRequested(_) => {
                self.pending_approvals = self.pending_approvals.saturating_add(1);
                self.status_line = "approval requested".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ApprovalResolved(_) => {
                self.pending_approvals = self.pending_approvals.saturating_sub(1);
                self.status_line = "approval resolved".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::PersistCheckpoint,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::PauseRequested => {
                self.paused = true;
                self.status_line = "paused".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::ResumeRequested => {
                self.paused = false;
                self.status_line = "resumed".to_string();
                vec![
                    UiEffect::Render,
                    UiEffect::EmitStatusLine(self.status_line.clone()),
                ]
            }
            UiEvent::Tick => vec![UiEffect::ScheduleBackgroundRefresh],
            UiEvent::KeyPressed(_) => Vec::new(),
        }
    }

    /// Produce a human-readable summary of the current state for debugging.
    pub fn snapshot(&self) -> String {
        format!(
            "pane={:?};paused={};pending_tasks={};active_jobs={};pending_approvals={};active_tool={};status={}",
            self.active_pane,
            self.paused,
            self.pending_tasks,
            self.active_jobs,
            self.pending_approvals,
            self.active_tool.clone().unwrap_or_default(),
            self.status_line
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default state ──────────────────────────────────────────────────

    #[test]
    fn default_state_is_chat_pane_and_ready() {
        let state = UiState::default();
        assert_eq!(state.active_pane, Pane::Chat);
        assert!(!state.paused);
        assert_eq!(state.last_response_delta, None);
        assert_eq!(state.active_tool, None);
        assert_eq!(state.pending_tasks, 0);
        assert_eq!(state.active_jobs, 0);
        assert_eq!(state.pending_approvals, 0);
        assert_eq!(state.status_line, "ready");
    }

    // ── Key navigation ─────────────────────────────────────────────────

    #[test]
    fn key_1_switches_to_chat_pane() {
        let mut state = UiState::default();
        let effects = state.reduce(UiEvent::KeyPressed('1'));
        assert_eq!(state.active_pane, Pane::Chat);
        assert_eq!(effects, vec![UiEffect::Render]);
    }

    #[test]
    fn key_2_switches_to_diff_pane() {
        let mut state = UiState::default();
        state.reduce(UiEvent::KeyPressed('2'));
        assert_eq!(state.active_pane, Pane::Diff);
    }

    #[test]
    fn key_3_switches_to_tasks_pane() {
        let mut state = UiState::default();
        state.reduce(UiEvent::KeyPressed('3'));
        assert_eq!(state.active_pane, Pane::Tasks);
    }

    #[test]
    fn key_4_switches_to_agents_pane() {
        let mut state = UiState::default();
        state.reduce(UiEvent::KeyPressed('4'));
        assert_eq!(state.active_pane, Pane::Agents);
    }

    #[test]
    fn key_5_switches_to_jobs_pane() {
        let mut state = UiState::default();
        state.reduce(UiEvent::KeyPressed('5'));
        assert_eq!(state.active_pane, Pane::Jobs);
    }

    #[test]
    fn unknown_key_produces_no_effects() {
        let mut state = UiState::default();
        let effects = state.reduce(UiEvent::KeyPressed('x'));
        assert!(effects.is_empty());
        assert_eq!(state.active_pane, Pane::Chat);
    }

    // ── Prompt lifecycle ───────────────────────────────────────────────

    #[test]
    fn prompt_submitted_increments_pending_tasks() {
        let mut state = UiState::default();
        let effects = state.reduce(UiEvent::PromptSubmitted("hello".to_string()));
        assert_eq!(state.pending_tasks, 1);
        assert_eq!(state.status_line, "prompt submitted");
        assert!(effects.contains(&UiEffect::Render));
        assert!(effects.contains(&UiEffect::PersistCheckpoint));
    }

    #[test]
    fn response_delta_updates_last_delta() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ResponseDelta("partial".to_string()));
        assert_eq!(state.last_response_delta, Some("partial".to_string()));
        assert_eq!(state.status_line, "streaming response");
    }

    // ── Tool lifecycle ─────────────────────────────────────────────────

    #[test]
    fn tool_started_sets_active_tool() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ToolStarted("shell".to_string()));
        assert_eq!(state.active_tool, Some("shell".to_string()));
        assert_eq!(state.status_line, "tool running: shell");
    }

    #[test]
    fn tool_finished_clears_active_tool_and_decrements_tasks() {
        let mut state = UiState::default();
        state.reduce(UiEvent::PromptSubmitted("test".to_string()));
        assert_eq!(state.pending_tasks, 1);
        state.reduce(UiEvent::ToolFinished("shell".to_string()));
        assert_eq!(state.active_tool, None);
        assert_eq!(state.pending_tasks, 0);
        assert_eq!(state.status_line, "tool finished: shell");
    }

    #[test]
    fn tool_finished_saturates_at_zero() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ToolFinished("shell".to_string()));
        assert_eq!(state.pending_tasks, 0);
    }

    // ── Job lifecycle ──────────────────────────────────────────────────

    #[test]
    fn job_queued_increments_active_jobs() {
        let mut state = UiState::default();
        state.reduce(UiEvent::JobQueued("build".to_string()));
        assert_eq!(state.active_jobs, 1);
        assert_eq!(state.status_line, "job queued");
    }

    #[test]
    fn job_progress_updates_status_line() {
        let mut state = UiState::default();
        state.reduce(UiEvent::JobProgress {
            job_id: "j1".to_string(),
            progress: 75,
        });
        assert_eq!(state.status_line, "job progress: 75%");
    }

    #[test]
    fn job_progress_clamps_over_100() {
        let mut state = UiState::default();
        state.reduce(UiEvent::JobProgress {
            job_id: "j1".to_string(),
            progress: 150,
        });
        assert_eq!(state.status_line, "job progress: 100%");
    }

    #[test]
    fn job_completed_decrements_active_jobs() {
        let mut state = UiState::default();
        state.reduce(UiEvent::JobQueued("build".to_string()));
        assert_eq!(state.active_jobs, 1);
        state.reduce(UiEvent::JobCompleted("build".to_string()));
        assert_eq!(state.active_jobs, 0);
        assert_eq!(state.status_line, "job completed");
    }

    #[test]
    fn job_completed_saturates_at_zero() {
        let mut state = UiState::default();
        state.reduce(UiEvent::JobCompleted("build".to_string()));
        assert_eq!(state.active_jobs, 0);
    }

    // ── Approval lifecycle ─────────────────────────────────────────────

    #[test]
    fn approval_requested_increments_pending() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ApprovalRequested("exec".to_string()));
        assert_eq!(state.pending_approvals, 1);
        assert_eq!(state.status_line, "approval requested");
    }

    #[test]
    fn approval_resolved_decrements_pending() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ApprovalRequested("exec".to_string()));
        state.reduce(UiEvent::ApprovalResolved("exec".to_string()));
        assert_eq!(state.pending_approvals, 0);
        assert_eq!(state.status_line, "approval resolved");
    }

    #[test]
    fn approval_resolved_saturates_at_zero() {
        let mut state = UiState::default();
        state.reduce(UiEvent::ApprovalResolved("exec".to_string()));
        assert_eq!(state.pending_approvals, 0);
    }

    // ── Pause/Resume ───────────────────────────────────────────────────

    #[test]
    fn pause_sets_paused_flag() {
        let mut state = UiState::default();
        state.reduce(UiEvent::PauseRequested);
        assert!(state.paused);
        assert_eq!(state.status_line, "paused");
    }

    #[test]
    fn resume_clears_paused_flag() {
        let mut state = UiState::default();
        state.reduce(UiEvent::PauseRequested);
        state.reduce(UiEvent::ResumeRequested);
        assert!(!state.paused);
        assert_eq!(state.status_line, "resumed");
    }

    // ── Tick ────────────────────────────────────────────────────────────

    #[test]
    fn tick_schedules_background_refresh() {
        let mut state = UiState::default();
        let effects = state.reduce(UiEvent::Tick);
        assert_eq!(effects, vec![UiEffect::ScheduleBackgroundRefresh]);
    }

    // ── Snapshot ────────────────────────────────────────────────────────

    #[test]
    fn snapshot_contains_all_fields() {
        let state = UiState::default();
        let snap = state.snapshot();
        assert!(snap.contains("pane=Chat"));
        assert!(snap.contains("paused=false"));
        assert!(snap.contains("pending_tasks=0"));
        assert!(snap.contains("active_jobs=0"));
        assert!(snap.contains("pending_approvals=0"));
        assert!(snap.contains("status=ready"));
    }

    #[test]
    fn snapshot_reflects_state_changes() {
        let mut state = UiState::default();
        state.reduce(UiEvent::KeyPressed('2'));
        state.reduce(UiEvent::PauseRequested);
        state.reduce(UiEvent::PromptSubmitted("test".to_string()));
        let snap = state.snapshot();
        assert!(snap.contains("pane=Diff"));
        assert!(snap.contains("paused=true"));
        assert!(snap.contains("pending_tasks=1"));
    }
}
