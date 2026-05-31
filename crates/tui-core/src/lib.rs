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
