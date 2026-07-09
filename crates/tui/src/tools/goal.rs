//! Goal tools for the model-visible LLM-as-judge loop.
//!
//! The TUI already has a `/goal` command and passes its objective into the
//! engine prompt. This module keeps the runtime slice separate: a small
//! session-scoped state object plus tools the model can use to inspect and
//! close out that state.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// Maximum number of automatic goal-continuation prompt injections in one
/// engine turn. This is intra-turn granularity only — it prevents a stuck spin
/// within a single turn from making no progress. The cross-turn loop has **no
/// cap**: a goal runs until complete/blocked/paused, or an optional budget is
/// exhausted. See `goal_loop::decide_continuation`.
pub const MAX_GOAL_CONTINUATIONS_PER_TURN: u32 = 3;

/// Shared reference to the current runtime goal.
pub type SharedGoalState = Arc<Mutex<GoalState>>;

/// Create an empty shared goal state.
#[must_use]
pub fn new_shared_goal_state() -> SharedGoalState {
    Arc::new(Mutex::new(GoalState::default()))
}

/// Create shared state seeded from the host goal surface with an explicit status.
#[must_use]
pub fn new_shared_goal_state_from_host_status(
    objective: Option<String>,
    token_budget: Option<u32>,
    status: GoalStatus,
) -> SharedGoalState {
    let mut state = GoalState::default();
    state.sync_from_host_status(objective.as_deref(), token_budget, status);
    Arc::new(Mutex::new(state))
}

/// Runtime status for a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Paused,
    Complete,
    Blocked,
}

impl GoalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }
}

/// Session-local goal state. `Instant` stays runtime-only; snapshots expose
/// elapsed seconds so tool output remains serializable and stable.
#[derive(Debug, Clone, Default)]
pub struct GoalState {
    objective: Option<String>,
    token_budget: Option<u32>,
    status: Option<GoalStatus>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    evidence: Option<String>,
    blocker: Option<String>,
    completion_verification: Option<GoalCompletionVerification>,
}

impl GoalState {
    #[must_use]
    pub fn objective(&self) -> Option<&str> {
        self.objective.as_deref()
    }

    #[must_use]
    pub fn token_budget(&self) -> Option<u32> {
        self.token_budget
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == Some(GoalStatus::Active)
    }

    pub fn sync_from_host_status(
        &mut self,
        objective: Option<&str>,
        token_budget: Option<u32>,
        status: GoalStatus,
    ) {
        let objective = objective.map(str::trim).filter(|value| !value.is_empty());
        match objective {
            Some(objective) => {
                let changed = self.objective.as_deref() != Some(objective);
                let status_changed = self.status != Some(status);
                if changed {
                    self.objective = Some(objective.to_string());
                    self.token_budget = token_budget;
                    self.tokens_used = 0;
                    self.time_used_seconds = 0;
                    self.continuation_count = 0;
                    self.started_at = Some(Instant::now());
                    self.evidence = None;
                    self.blocker = None;
                    self.completion_verification = None;
                } else if self.token_budget != token_budget {
                    self.token_budget = token_budget;
                }

                if changed || status_changed || self.status.is_none() {
                    self.status = Some(status);
                    self.finished_at = if status == GoalStatus::Active {
                        None
                    } else {
                        Some(Instant::now())
                    };
                }
            }
            None => self.clear(),
        }
    }

    pub fn create(&mut self, objective: String, token_budget: Option<u32>) {
        self.objective = Some(objective);
        self.token_budget = token_budget;
        self.status = Some(GoalStatus::Active);
        self.tokens_used = 0;
        self.time_used_seconds = 0;
        self.continuation_count = 0;
        self.started_at = Some(Instant::now());
        self.finished_at = None;
        self.evidence = None;
        self.blocker = None;
        self.completion_verification = None;
    }

    pub fn record_usage(&mut self, token_delta: u64, time_delta_seconds: u64) {
        if self.is_active() {
            self.tokens_used = self.tokens_used.saturating_add(token_delta);
            self.time_used_seconds = self.time_used_seconds.saturating_add(time_delta_seconds);
        }
    }

    pub fn record_continuation(&mut self) {
        if self.is_active() {
            self.continuation_count = self.continuation_count.saturating_add(1);
        }
    }

    pub fn mark_complete(
        &mut self,
        evidence: String,
        verification: GoalCompletionVerification,
    ) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to complete.");
        }
        self.status = Some(GoalStatus::Complete);
        self.finished_at = Some(Instant::now());
        self.evidence = Some(evidence);
        self.blocker = None;
        self.completion_verification = Some(verification);
        Ok(())
    }

    pub fn mark_blocked(&mut self, blocker: String) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to block.");
        }
        self.status = Some(GoalStatus::Blocked);
        self.finished_at = Some(Instant::now());
        self.blocker = Some(blocker);
        self.evidence = None;
        self.completion_verification = None;
        Ok(())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn snapshot(&self) -> GoalSnapshot {
        // Once the goal is terminal, freeze elapsed at the finish time so the
        // sidebar timer (and any tool snapshot) stops growing after completion.
        let elapsed_seconds = match (self.started_at, self.finished_at) {
            (Some(started), Some(finished)) => {
                Some(finished.saturating_duration_since(started).as_secs())
            }
            (Some(started), None) => Some(started.elapsed().as_secs()),
            (None, _) => None,
        };
        GoalSnapshot {
            objective: self.objective.clone(),
            status: self
                .status
                .map(GoalStatus::as_str)
                .unwrap_or("none")
                .to_string(),
            token_budget: self.token_budget,
            tokens_used: self.tokens_used,
            time_used_seconds: self.time_used_seconds,
            continuation_count: self.continuation_count,
            elapsed_seconds,
            evidence: self.evidence.clone(),
            blocker: self.blocker.clone(),
            completion_verification: self.completion_verification.clone(),
        }
    }
}

/// Serializable tool output and prompt input for the current goal.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub objective: Option<String>,
    pub status: String,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuation_count: u32,
    pub elapsed_seconds: Option<u64>,
    pub evidence: Option<String>,
    pub blocker: Option<String>,
    pub completion_verification: Option<GoalCompletionVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalCompletionVerification {
    pub status: String,
    pub check: String,
    pub summary: String,
}

impl GoalSnapshot {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == GoalStatus::Active.as_str()
    }

    #[must_use]
    pub fn from_thread_goal(goal: &codewhale_protocol::ThreadGoal) -> Self {
        Self {
            objective: Some(goal.objective.clone()),
            status: thread_goal_status_as_goal_status(goal.status.clone())
                .as_str()
                .to_string(),
            token_budget: goal
                .token_budget
                .and_then(|value| u32::try_from(value.max(0)).ok()),
            tokens_used: u64::try_from(goal.tokens_used.max(0)).unwrap_or(u64::MAX),
            time_used_seconds: u64::try_from(goal.time_used_seconds.max(0)).unwrap_or(u64::MAX),
            continuation_count: u32::try_from(goal.continuation_count.max(0)).unwrap_or(u32::MAX),
            elapsed_seconds: None,
            evidence: None,
            blocker: None,
            completion_verification: None,
        }
    }
}

#[must_use]
pub fn thread_goal_status_as_goal_status(
    status: codewhale_protocol::ThreadGoalStatus,
) -> GoalStatus {
    match status {
        codewhale_protocol::ThreadGoalStatus::Active => GoalStatus::Active,
        codewhale_protocol::ThreadGoalStatus::Paused => GoalStatus::Paused,
        codewhale_protocol::ThreadGoalStatus::Complete => GoalStatus::Complete,
        codewhale_protocol::ThreadGoalStatus::Blocked
        | codewhale_protocol::ThreadGoalStatus::UsageLimited
        | codewhale_protocol::ThreadGoalStatus::BudgetLimited => GoalStatus::Blocked,
    }
}

/// Render the continuation prompt injected when a goal is still active after a
/// turn. There is no run-level cap, so this shows progress (turn count, tokens)
/// rather than a "N/max" meter — the loop runs until done, blocked, or paused.
#[must_use]
pub fn render_continuation_prompt(snapshot: &GoalSnapshot, continuation_index: u32) -> String {
    let goal_json = serde_json::to_string_pretty(snapshot).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{}\n\n## Active Goal State\n\n```json\n{}\n```\n\nContinuation pass #{}.\nIf the goal is complete, first run or cite a concrete verifier/check when one applies, then call `update_goal` with `status: \"complete\"`, concrete evidence, and `verification: {{\"status\":\"passed\",\"check\":\"...\",\"summary\":\"...\"}}`. For non-verifiable work (docs, research, writing), use `verification: {{\"status\":\"not_applicable\",\"check\":\"...\",\"summary\":\"...\"}}` with a clear rationale instead of fabricating a verifier receipt. If it is blocked, call `update_goal` with `status: \"blocked\"` and the blocker. Otherwise continue making progress toward the objective.",
        crate::prompts::GOAL_CONTINUATION_PROMPT.trim(),
        goal_json,
        continuation_index,
    )
}

fn lock_goal_state(
    state: &SharedGoalState,
) -> Result<std::sync::MutexGuard<'_, GoalState>, ToolError> {
    state
        .lock()
        .map_err(|_| ToolError::execution_failed("goal state lock poisoned"))
}

fn parse_token_budget(input: &Value) -> Result<Option<u32>, ToolError> {
    let Some(raw) = input.get("token_budget") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(value) = raw.as_u64() else {
        return Err(ToolError::invalid_input(
            "token_budget must be a non-negative integer",
        ));
    };
    u32::try_from(value)
        .map(Some)
        .map_err(|_| ToolError::invalid_input("token_budget is too large"))
}

fn parse_completion_verification(input: &Value) -> Result<GoalCompletionVerification, ToolError> {
    let Some(raw) = input.get("verification") else {
        return Err(ToolError::invalid_input(
            "verification is required when status is complete; run a verifier/check and pass verification: {status, check, summary}",
        ));
    };
    let verification: GoalCompletionVerification = serde_json::from_value(raw.clone())
        .map_err(|err| ToolError::invalid_input(format!("invalid verification: {err}")))?;
    let status = verification.status.trim();
    let normalized_status = match status {
        "passed" | "not_applicable" => status,
        other => {
            return Err(ToolError::invalid_input(format!(
                "verification.status must be 'passed' or 'not_applicable' before update_goal can mark a goal complete; got '{other}'"
            )));
        }
    };
    if verification.check.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.check is required"));
    }
    if verification.summary.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.summary is required"));
    }
    Ok(GoalCompletionVerification {
        status: normalized_status.to_string(),
        check: verification.check.trim().to_string(),
        summary: verification.summary.trim().to_string(),
    })
}

fn json_result(snapshot: &GoalSnapshot) -> Result<ToolResult, ToolError> {
    ToolResult::json(snapshot).map_err(|err| ToolError::execution_failed(err.to_string()))
}

pub struct CreateGoalTool {
    goal_state: SharedGoalState,
}

impl CreateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for CreateGoalTool {
    fn name(&self) -> &'static str {
        "create_goal"
    }

    fn description(&self) -> &'static str {
        "Create the current runtime goal. Use this only when the user explicitly asks to pursue a persistent objective."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "objective": {
                    "type": "string",
                    "description": "The full objective to pursue. Keep the complete user goal, not a shortened one-turn version."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional soft token budget for the goal."
                }
            },
            "required": ["objective"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let objective = required_str(&input, "objective")?.trim().to_string();
        if objective.is_empty() {
            return Err(ToolError::invalid_input("objective cannot be empty"));
        }
        let token_budget = parse_token_budget(&input)?;
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            state.create(objective, token_budget);
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

pub struct GetGoalTool {
    goal_state: SharedGoalState,
}

impl GetGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for GetGoalTool {
    fn name(&self) -> &'static str {
        "get_goal"
    }

    fn description(&self) -> &'static str {
        "Inspect the current runtime goal state, including objective, status, token budget, elapsed time, evidence, and blocker."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = {
            let state = lock_goal_state(&self.goal_state)?;
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

pub struct UpdateGoalTool {
    goal_state: SharedGoalState,
}

impl UpdateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for UpdateGoalTool {
    fn name(&self) -> &'static str {
        "update_goal"
    }

    fn description(&self) -> &'static str {
        "Update the runtime goal completion gate. Only mark complete when the objective has verified evidence; mark blocked only after a real blocker prevents progress."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["complete", "blocked"],
                    "description": "Use complete only when the goal is fully satisfied; blocked when meaningful progress cannot continue. Pause, resume, and budget-limit states are controlled by the user or system."
                },
                "evidence": {
                    "type": "string",
                    "description": "Required when status is complete. Briefly cite the proof that the goal is done."
                },
                "verification": {
                    "type": "object",
                    "description": "Required when status is complete. A verifier-as-judge receipt from a concrete check, such as run_verifiers or an equivalent project-specific gate.",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["passed", "not_applicable"],
                            "description": "Use passed when a concrete verifier/check succeeded; not_applicable when no automated verifier applies."
                        },
                        "check": {
                            "type": "string",
                            "description": "The verifier/check that passed."
                        },
                        "summary": {
                            "type": "string",
                            "description": "Brief result summary from the verifier/check."
                        }
                    },
                    "required": ["status", "check", "summary"],
                    "additionalProperties": false
                },
                "blocker": {
                    "type": "string",
                    "description": "Required when status is blocked. Explain the condition preventing progress."
                },
                "objective": {
                    "type": "string",
                    "description": "Reserved for future host-controlled goal edits; ignored by update_goal."
                }
            },
            "required": ["status"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let status = required_str(&input, "status")?.trim().to_ascii_lowercase();
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            match status.as_str() {
                "complete" => {
                    let evidence = input
                        .get("evidence")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if evidence.is_empty() {
                        return Err(ToolError::invalid_input(
                            "evidence is required when status is complete",
                        ));
                    }
                    let verification = parse_completion_verification(&input)?;
                    state
                        .mark_complete(evidence, verification)
                        .map_err(ToolError::invalid_input)?;
                }
                "blocked" => {
                    let blocker = input
                        .get("blocker")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if blocker.is_empty() {
                        return Err(ToolError::invalid_input(
                            "blocker is required when status is blocked",
                        ));
                    }
                    state
                        .mark_blocked(blocker)
                        .map_err(ToolError::invalid_input)?;
                }
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "unsupported goal status '{other}'; update_goal can only mark complete or blocked"
                    )));
                }
            }
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[tokio::test]
    async fn create_get_and_complete_goal() {
        let state = new_shared_goal_state();
        let ctx = ToolContext::new(".");

        let create = CreateGoalTool::new(state.clone());
        let created = create
            .execute(
                json!({
                    "objective": "ship the runtime slice",
                    "token_budget": 1200
                }),
                &ctx,
            )
            .await
            .expect("create goal");
        assert!(created.success);
        let created_json: Value = serde_json::from_str(&created.content).expect("created json");
        assert_eq!(
            created_json.get("status").and_then(Value::as_str),
            Some("active")
        );

        let get = GetGoalTool::new(state.clone());
        let current = get.execute(json!({}), &ctx).await.expect("get goal");
        assert!(current.content.contains("ship the runtime slice"));
        let current_json: Value = serde_json::from_str(&current.content).expect("current json");
        assert_eq!(
            current_json.get("token_budget").and_then(Value::as_u64),
            Some(1200)
        );

        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "focused tests passed",
                    "verification": {
                        "status": "passed",
                        "check": "cargo test -p codewhale-tui goal_loop",
                        "summary": "focused tests passed"
                    }
                }),
                &ctx,
            )
            .await
            .expect("complete goal");
        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert!(completed.content.contains("focused tests passed"));
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_completion_evidence() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "complete"}), &ToolContext::new("."))
            .await
            .expect_err("missing evidence should fail");

        assert!(err.to_string().contains("evidence is required"));
    }

    #[tokio::test]
    async fn update_goal_accepts_not_applicable_verification_for_non_verifiable_goals() {
        let state = new_shared_goal_state_from_host_status(
            Some("write the release notes".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "release notes drafted and reviewed in thread",
                    "verification": {
                        "status": "not_applicable",
                        "check": "no automated verifier applies",
                        "summary": "writing task completed with evidence in thread"
                    }
                }),
                &ToolContext::new("."),
            )
            .await
            .expect("non-verifiable goal should complete");

        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert_eq!(
            completed_json
                .get("completion_verification")
                .and_then(|verification| verification.get("status"))
                .and_then(Value::as_str),
            Some("not_applicable")
        );
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_passed_verification_to_complete() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let err = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "all checks look good"
                }),
                &ToolContext::new("."),
            )
            .await
            .expect_err("missing verifier gate should fail");

        assert!(err.to_string().contains("verification is required"));
        assert!(state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_rejects_model_resume() {
        let state = new_shared_goal_state_from_host_status(
            Some("pause remains host controlled".to_string()),
            None,
            GoalStatus::Paused,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "active"}), &ToolContext::new("."))
            .await
            .expect_err("model resume should fail");

        assert!(err.to_string().contains("complete or blocked"));
    }

    #[test]
    fn paused_host_goal_is_not_active() {
        let state = new_shared_goal_state_from_host_status(
            Some("wait for user".to_string()),
            Some(42),
            GoalStatus::Paused,
        );
        let snapshot = state.lock().expect("goal lock").snapshot();

        assert_eq!(snapshot.status, "paused");
        assert_eq!(snapshot.token_budget, Some(42));
        assert!(!snapshot.is_active());
    }

    #[test]
    fn goal_state_projects_usage_and_continuations() {
        let state = new_shared_goal_state_from_host_status(
            Some("persist accounting".to_string()),
            Some(1_000),
            GoalStatus::Active,
        );
        {
            let mut goal = state.lock().expect("goal lock");
            goal.record_usage(300, 12);
            goal.record_continuation();
        }

        let snapshot = state.lock().expect("goal lock").snapshot();
        assert_eq!(snapshot.tokens_used, 300);
        assert_eq!(snapshot.time_used_seconds, 12);
        assert_eq!(snapshot.continuation_count, 1);
    }

    #[test]
    fn completed_goal_snapshot_freezes_elapsed() {
        // Regression: a completed goal's snapshot elapsed_seconds must not keep
        // growing. Before the fix, snapshot() always used started_at.elapsed(),
        // so a finished goal's elapsed kept ticking in the sidebar/tool output.
        let state = new_shared_goal_state_from_host_status(
            Some("freeze on completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let first = {
            let mut goal = state.lock().expect("goal lock");
            goal.mark_complete(
                "evidence".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test".to_string(),
                    summary: "ok".to_string(),
                },
            )
            .expect("mark complete");
            goal.snapshot()
        };
        let elapsed_at_completion = first.elapsed_seconds.expect("elapsed present");

        // Sleep past a whole-second boundary. Under the old (buggy) code,
        // snapshot() returned started_at.elapsed().as_secs(), so this would
        // tick up by at least one second and the assertion below would fail.
        // With the freeze, the completed snapshot stays at the captured value.
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let second = state.lock().expect("goal lock").snapshot();
        assert_eq!(second.status, "complete");
        assert_eq!(
            second.elapsed_seconds,
            Some(elapsed_at_completion),
            "completed goal elapsed must be frozen, not keep ticking"
        );
    }

    #[test]
    fn protocol_thread_goal_converts_to_runtime_snapshot() {
        let snapshot = GoalSnapshot::from_thread_goal(&codewhale_protocol::ThreadGoal {
            thread_id: "thread-1".to_string(),
            goal_id: "goal-1".to_string(),
            objective: "Bridge the goal models".to_string(),
            status: codewhale_protocol::ThreadGoalStatus::Active,
            token_budget: Some(2_000),
            tokens_used: 750,
            time_used_seconds: 44,
            continuation_count: 3,
            created_at: 1,
            updated_at: 2,
        });

        assert_eq!(
            snapshot.objective.as_deref(),
            Some("Bridge the goal models")
        );
        assert_eq!(snapshot.status, "active");
        assert_eq!(snapshot.token_budget, Some(2_000));
        assert_eq!(snapshot.tokens_used, 750);
        assert_eq!(snapshot.time_used_seconds, 44);
        assert_eq!(snapshot.continuation_count, 3);
    }

    #[test]
    fn continuation_prompt_includes_bound_and_goal_state() {
        let snapshot = GoalSnapshot {
            objective: Some("finish issue 2199".to_string()),
            status: "active".to_string(),
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            continuation_count: 0,
            elapsed_seconds: Some(5),
            evidence: None,
            blocker: None,
            completion_verification: None,
        };

        let prompt = render_continuation_prompt(&snapshot, 2);
        assert!(prompt.contains("Goal Continuation"));
        assert!(prompt.contains("finish issue 2199"));
        assert!(prompt.contains("Continuation pass #2"));
    }
}
