use codewhale_tui_core::{Pane, UiEvent, UiState};

#[test]
fn reducer_produces_stable_snapshot_for_core_workflow() {
    let mut state = UiState::default();
    state.reduce(UiEvent::PromptSubmitted("hello".to_string()));
    state.reduce(UiEvent::ToolStarted("web.search".to_string()));
    state.reduce(UiEvent::ResponseDelta("partial".to_string()));
    state.reduce(UiEvent::ToolFinished("web.search".to_string()));
    state.reduce(UiEvent::ApprovalRequested("approval-1".to_string()));
    state.reduce(UiEvent::ApprovalResolved("approval-1".to_string()));
    state.reduce(UiEvent::JobQueued("job-1".to_string()));
    state.reduce(UiEvent::JobProgress {
        job_id: "job-1".to_string(),
        progress: 60,
    });
    state.reduce(UiEvent::JobCompleted("job-1".to_string()));
    state.reduce(UiEvent::KeyPressed('5'));

    assert_eq!(state.active_pane, Pane::Jobs);
    assert_eq!(
        state.snapshot(),
        "pane=Jobs;paused=false;pending_tasks=0;active_jobs=0;pending_approvals=0;active_tool=;status=job completed"
    );
}
