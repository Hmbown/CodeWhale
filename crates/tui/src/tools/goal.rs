//! Goal tools for the model-visible LLM-as-judge loop.
//!
//! The TUI already has a `/goal` command and passes its objective into the
//! engine prompt. This module keeps the runtime slice separate: a small
//! session-scoped state object plus tools the model can use to inspect and
//! close out that state.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// Maximum number of automatic goal-continuation prompt injections in one
/// engine turn. This prevents a missing `update_goal` call from becoming an
/// unbounded local loop.
pub const MAX_GOAL_CONTINUATIONS_PER_TURN: u32 = 3;

/// Shared reference to the current runtime goal.
pub type SharedGoalState = Arc<Mutex<GoalState>>;

/// Create an empty shared goal state.
#[must_use]
pub fn new_shared_goal_state() -> SharedGoalState {
    Arc::new(Mutex::new(GoalState::default()))
}

/// Create shared state seeded from the existing `/goal` surface.
#[must_use]
pub fn new_shared_goal_state_from_host(
    objective: Option<String>,
    token_budget: Option<u32>,
    completed: bool,
) -> SharedGoalState {
    let mut state = GoalState::default();
    state.sync_from_host(objective.as_deref(), token_budget, completed);
    Arc::new(Mutex::new(state))
}

/// Runtime status for a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Complete,
    Blocked,
}

impl GoalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
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
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    evidence: Option<String>,
    blocker: Option<String>,
}

impl GoalState {
    #[must_use]
    pub fn objective(&self) -> Option<&str> {
        self.objective.as_deref()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == Some(GoalStatus::Active)
    }

    pub fn sync_from_host(
        &mut self,
        objective: Option<&str>,
        token_budget: Option<u32>,
        completed: bool,
    ) {
        let objective = objective.map(str::trim).filter(|value| !value.is_empty());
        match objective {
            Some(objective) => {
                let changed = self.objective.as_deref() != Some(objective);
                if changed {
                    self.objective = Some(objective.to_string());
                    self.token_budget = token_budget;
                    self.started_at = Some(Instant::now());
                    self.evidence = None;
                    self.blocker = None;
                } else if token_budget.is_some() {
                    self.token_budget = token_budget;
                }

                if changed || self.status.is_none() {
                    self.status = Some(if completed {
                        GoalStatus::Complete
                    } else {
                        GoalStatus::Active
                    });
                    self.finished_at = completed.then(Instant::now);
                }
            }
            None => self.clear(),
        }
    }

    pub fn create(&mut self, objective: String, token_budget: Option<u32>) {
        self.objective = Some(objective);
        self.token_budget = token_budget;
        self.status = Some(GoalStatus::Active);
        self.started_at = Some(Instant::now());
        self.finished_at = None;
        self.evidence = None;
        self.blocker = None;
    }

    pub fn resume(&mut self, objective: Option<String>) -> Result<(), &'static str> {
        if let Some(objective) = objective {
            self.create(objective, self.token_budget);
            return Ok(());
        }
        if self.objective.is_none() {
            return Err("No goal exists to resume.");
        }
        self.status = Some(GoalStatus::Active);
        self.finished_at = None;
        self.evidence = None;
        self.blocker = None;
        Ok(())
    }

    pub fn mark_complete(&mut self, evidence: String) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to complete.");
        }
        self.status = Some(GoalStatus::Complete);
        self.finished_at = Some(Instant::now());
        self.evidence = Some(evidence);
        self.blocker = None;
        Ok(())
    }

    pub fn mark_blocked(&mut self, blocker: String) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to block.");
        }
        self.status = Some(GoalStatus::Blocked);
        self.finished_at = Some(Instant::now());
        self.blocker = Some(blocker);
        Ok(())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn snapshot(&self) -> GoalSnapshot {
        GoalSnapshot {
            objective: self.objective.clone(),
            status: self
                .status
                .map(GoalStatus::as_str)
                .unwrap_or("none")
                .to_string(),
            token_budget: self.token_budget,
            elapsed_seconds: self.started_at.map(|started| started.elapsed().as_secs()),
            evidence: self.evidence.clone(),
            blocker: self.blocker.clone(),
        }
    }
}

/// Serializable tool output and prompt input for the current goal.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub objective: Option<String>,
    pub status: String,
    pub token_budget: Option<u32>,
    pub elapsed_seconds: Option<u64>,
    pub evidence: Option<String>,
    pub blocker: Option<String>,
}

impl GoalSnapshot {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == GoalStatus::Active.as_str()
    }
}

/// Render the bounded continuation prompt injected when a goal is still active
/// after an assistant message has no tool calls.
#[must_use]
pub fn render_continuation_prompt(
    snapshot: &GoalSnapshot,
    continuation_index: u32,
    max_continuations: u32,
) -> String {
    let goal_json = serde_json::to_string_pretty(snapshot).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{}\n\n## Active Goal State\n\n```json\n{}\n```\n\nContinuation pass: {}/{}.\nIf the goal is complete, call `update_goal` with `status: \"complete\"` and concrete evidence. If it is blocked, call `update_goal` with `status: \"blocked\"` and the blocker. Otherwise continue making progress toward the objective.",
        crate::prompts::GOAL_CONTINUATION_PROMPT.trim(),
        goal_json,
        continuation_index,
        max_continuations,
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
        "Create or replace the current runtime goal. Use this when the user asks for a persistent goal that should be audited before the turn is allowed to finish."
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
        "Update the runtime goal. This is the LLM-as-judge completion gate: only mark complete when the objective has been verified against concrete current-state evidence."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["active", "complete", "blocked"],
                    "description": "Use complete only when the goal is fully satisfied; blocked when meaningful progress cannot continue; active to resume or revise the objective."
                },
                "evidence": {
                    "type": "string",
                    "description": "Required when status is complete. Briefly cite the proof that the goal is done."
                },
                "blocker": {
                    "type": "string",
                    "description": "Required when status is blocked. Explain the condition preventing progress."
                },
                "objective": {
                    "type": "string",
                    "description": "Optional replacement objective when status is active."
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
                    state
                        .mark_complete(evidence)
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
                "active" => {
                    let objective = input
                        .get("objective")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    state.resume(objective).map_err(ToolError::invalid_input)?;
                }
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "unsupported goal status '{other}'"
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
    use serde_json::json;

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
        assert!(created.content.contains("\"status\": \"active\""));

        let get = GetGoalTool::new(state.clone());
        let current = get.execute(json!({}), &ctx).await.expect("get goal");
        assert!(current.content.contains("ship the runtime slice"));
        assert!(current.content.contains("\"token_budget\": 1200"));

        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "focused tests passed"
                }),
                &ctx,
            )
            .await
            .expect("complete goal");
        assert!(completed.content.contains("\"status\": \"complete\""));
        assert!(completed.content.contains("focused tests passed"));
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_completion_evidence() {
        let state =
            new_shared_goal_state_from_host(Some("prove completion".to_string()), None, false);
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "complete"}), &ToolContext::new("."))
            .await
            .expect_err("missing evidence should fail");

        assert!(err.to_string().contains("evidence is required"));
    }

    #[test]
    fn continuation_prompt_includes_bound_and_goal_state() {
        let snapshot = GoalSnapshot {
            objective: Some("finish issue 2199".to_string()),
            status: "active".to_string(),
            token_budget: None,
            elapsed_seconds: Some(5),
            evidence: None,
            blocker: None,
        };

        let prompt = render_continuation_prompt(&snapshot, 2, 3);
        assert!(prompt.contains("Goal Continuation"));
        assert!(prompt.contains("finish issue 2199"));
        assert!(prompt.contains("Continuation pass: 2/3"));
    }
}
