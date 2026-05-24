//! Plan tool implementation with step tracking and validation

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === Types ===

/// Status of a plan step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

impl StepStatus {
    #[allow(dead_code)]
    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "pending" => Some(StepStatus::Pending),
            "in_progress" | "inprogress" => Some(StepStatus::InProgress),
            "completed" | "done" => Some(StepStatus::Completed),
            _ => None,
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn symbol(&self) -> &'static str {
        match self {
            StepStatus::Pending => "○",
            StepStatus::InProgress => "◎",
            StepStatus::Completed => "●",
        }
    }
}

/// Input representation for a plan item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItemArg {
    pub step: String,
    pub status: StepStatus,
}

/// Update payload used by the plan tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePlanArgs {
    #[serde(default)]
    pub explanation: Option<String>,
    pub plan: Vec<PlanItemArg>,
}

// === Plan State ===

/// A plan step with timing information
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
    /// When the step was started (transitioned to `InProgress`)
    pub started_at: Option<Instant>,
    /// When the step was completed
    pub completed_at: Option<Instant>,
}

impl PlanStep {
    /// Create a new plan step.
    pub fn new(text: String, status: StepStatus) -> Self {
        Self {
            text,
            status,
            started_at: None,
            completed_at: None,
        }
    }

    /// Get the elapsed time if the step has timing info
    #[must_use]
    pub fn elapsed(&self) -> Option<Duration> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) => Some(end.duration_since(start)),
            (Some(start), None) if self.status == StepStatus::InProgress => Some(start.elapsed()),
            _ => None,
        }
    }

    /// Format elapsed time for display
    #[must_use]
    pub fn elapsed_str(&self) -> String {
        match self.elapsed() {
            Some(d) => {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{secs}s")
                } else if secs < 3600 {
                    format!("{}m {}s", secs / 60, secs % 60)
                } else {
                    format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
                }
            }
            None => String::new(),
        }
    }
}

/// Serializable snapshot for display
#[derive(Debug, Clone, Serialize)]
pub struct PlanSnapshot {
    pub explanation: Option<String>,
    pub items: Vec<PlanItemArg>,
}

/// State tracking for the current plan
#[derive(Debug, Clone, Default)]
pub struct PlanState {
    explanation: Option<String>,
    steps: Vec<PlanStep>,
}

impl PlanState {
    /// Check whether the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.explanation.as_deref().unwrap_or("").is_empty()
    }

    pub fn update(&mut self, args: UpdatePlanArgs) {
        self.explanation = args.explanation.filter(|s| !s.trim().is_empty());

        let now = Instant::now();
        let mut new_steps = Vec::new();
        let mut in_progress_seen = false;

        for item in args.plan {
            // Try to find existing step to preserve timing
            let existing = self.steps.iter().find(|s| s.text == item.step);

            let mut status = item.status;
            // Enforce single in_progress
            if status == StepStatus::InProgress {
                if in_progress_seen {
                    status = StepStatus::Pending;
                } else {
                    in_progress_seen = true;
                }
            }

            let step = if let Some(old) = existing {
                let mut s = old.clone();
                let old_status = s.status.clone();
                s.status = status.clone();

                // Track timing transitions
                if old_status == StepStatus::Pending && status == StepStatus::InProgress {
                    s.started_at = Some(now);
                }
                if old_status == StepStatus::InProgress && status == StepStatus::Completed {
                    s.completed_at = Some(now);
                }

                s
            } else {
                let mut s = PlanStep::new(item.step, status.clone());
                if status == StepStatus::InProgress {
                    s.started_at = Some(now);
                }
                s
            };

            new_steps.push(step);
        }

        self.steps = new_steps;
    }

    pub fn snapshot(&self) -> PlanSnapshot {
        PlanSnapshot {
            explanation: self.explanation.clone(),
            items: self
                .steps
                .iter()
                .map(|s| PlanItemArg {
                    step: s.text.clone(),
                    status: s.status.clone(),
                })
                .collect(),
        }
    }

    pub fn explanation(&self) -> Option<&str> {
        self.explanation.as_deref()
    }

    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }

    /// Get counts of steps by status
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut pending = 0;
        let mut in_progress = 0;
        let mut completed = 0;
        for s in &self.steps {
            match s.status {
                StepStatus::Pending => pending += 1,
                StepStatus::InProgress => in_progress += 1,
                StepStatus::Completed => completed += 1,
            }
        }
        (pending, in_progress, completed)
    }

    /// Get progress as a percentage
    pub fn progress_percent(&self) -> u8 {
        if self.steps.is_empty() {
            return 0;
        }
        let completed = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count();
        let percent = completed.saturating_mul(100) / self.steps.len();
        u8::try_from(percent).unwrap_or(u8::MAX)
    }
}

/// Validation result for plan transitions
#[derive(Debug)]
#[allow(dead_code)]
pub enum PlanValidation {
    Ok,
    Warning(String),
    Error(String),
}

/// Validate a plan update
#[allow(dead_code)]
pub fn validate_plan_update(current: &PlanState, update: &UpdatePlanArgs) -> PlanValidation {
    let current_steps: std::collections::HashMap<_, _> = current
        .steps()
        .iter()
        .map(|s| (s.text.clone(), &s.status))
        .collect();

    for item in &update.plan {
        if let Some(old_status) = current_steps.get(&item.step) {
            // Check for invalid transitions
            match (old_status, &item.status) {
                (StepStatus::Completed, StepStatus::Pending) => {
                    return PlanValidation::Warning(format!(
                        "Step '{}' was completed but is now pending",
                        item.step
                    ));
                }
                (StepStatus::Completed, StepStatus::InProgress) => {
                    return PlanValidation::Warning(format!(
                        "Step '{}' was completed but is now in progress",
                        item.step
                    ));
                }
                _ => {}
            }
        }
    }

    PlanValidation::Ok
}

// === UpdatePlanTool - ToolSpec implementation ===

/// Shared reference to `PlanState` for use across tools
pub type SharedPlanState = Arc<Mutex<PlanState>>;

/// Create a new shared `PlanState`
pub fn new_shared_plan_state() -> SharedPlanState {
    Arc::new(Mutex::new(PlanState::default()))
}

/// Tool for updating the implementation plan
pub struct UpdatePlanTool {
    plan_state: SharedPlanState,
}

impl UpdatePlanTool {
    pub fn new(plan_state: SharedPlanState) -> Self {
        Self { plan_state }
    }
}

#[async_trait]
impl ToolSpec for UpdatePlanTool {
    fn name(&self) -> &'static str {
        "update_plan"
    }

    fn description(&self) -> &'static str {
        "Update optional high-level strategy metadata for complex initiatives. Use checklist_write for primary Work progress; update_plan should capture phase-level approach changes, not duplicate checklist items. Each strategy step has a description and status (pending, in_progress, completed). Optionally include an explanation of the overall approach."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional high-level explanation of the plan or approach"
                },
                "plan": {
                    "type": "array",
                    "description": "List of plan steps",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {
                                "type": "string",
                                "description": "Description of the step"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Step status"
                            }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let explanation = input
            .get("explanation")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);

        let plan_items = input
            .get("plan")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::invalid_input("Missing or invalid 'plan' array"))?;

        let mut plan_args = Vec::new();
        for item in plan_items {
            let step = item
                .get("step")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::invalid_input("Plan item missing 'step'"))?;

            let status_str = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");

            let status = StepStatus::from_str(status_str).unwrap_or(StepStatus::Pending);

            plan_args.push(PlanItemArg {
                step: step.to_string(),
                status,
            });
        }

        let args = UpdatePlanArgs {
            explanation,
            plan: plan_args,
        };

        let mut state = self.plan_state.lock().await;

        state.update(args);

        let snapshot = state.snapshot();
        let (pending, in_progress, completed) = state.counts();
        let progress = state.progress_percent();

        let result = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());

        Ok(ToolResult::success(format!(
            "Plan updated: {pending} pending, {in_progress} in progress, {completed} completed ({progress}% done)\n{result}"
        )))
    }
}
