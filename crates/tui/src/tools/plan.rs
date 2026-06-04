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
///
/// Backward-compatible: legacy callers that only send `plan` + optional
/// `explanation` still work. New v0.9.0 fields are all `Option` and
/// default to `None` / empty.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdatePlanArgs {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub context_summary: Option<String>,
    #[serde(default)]
    pub sources_used: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub recommended_approach: Option<String>,
    #[serde(default)]
    pub plan: Vec<PlanItemArg>,
    #[serde(default)]
    pub verification_plan: Option<String>,
    #[serde(default)]
    pub risks_and_unknowns: Option<String>,
    #[serde(default)]
    pub handoff_packet: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_summary: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources_used: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_approach: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub items: Vec<PlanItemArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risks_and_unknowns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff_packet: Option<String>,
}

/// State tracking for the current plan
#[derive(Debug, Clone, Default)]
pub struct PlanState {
    title: Option<String>,
    objective: Option<String>,
    context_summary: Option<String>,
    sources_used: Vec<String>,
    constraints: Vec<String>,
    recommended_approach: Option<String>,
    steps: Vec<PlanStep>,
    verification_plan: Option<String>,
    risks_and_unknowns: Option<String>,
    handoff_packet: Option<String>,
    explanation: Option<String>,
}

impl PlanState {
    /// Check whether the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
            && self.explanation.as_deref().unwrap_or("").is_empty()
            && self.title.as_deref().unwrap_or("").is_empty()
            && self.objective.as_deref().unwrap_or("").is_empty()
            && self.context_summary.as_deref().unwrap_or("").is_empty()
            && self
                .recommended_approach
                .as_deref()
                .unwrap_or("")
                .is_empty()
    }

    pub fn update(&mut self, args: UpdatePlanArgs) {
        self.title = args.title.filter(|s| !s.trim().is_empty());
        self.objective = args.objective.filter(|s| !s.trim().is_empty());
        self.context_summary = args.context_summary.filter(|s| !s.trim().is_empty());
        self.sources_used = args.sources_used;
        self.constraints = args.constraints;
        self.recommended_approach = args.recommended_approach.filter(|s| !s.trim().is_empty());
        self.verification_plan = args.verification_plan.filter(|s| !s.trim().is_empty());
        self.risks_and_unknowns = args.risks_and_unknowns.filter(|s| !s.trim().is_empty());
        self.handoff_packet = args.handoff_packet.filter(|s| !s.trim().is_empty());
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
            title: self.title.clone(),
            objective: self.objective.clone(),
            context_summary: self.context_summary.clone(),
            sources_used: self.sources_used.clone(),
            constraints: self.constraints.clone(),
            recommended_approach: self.recommended_approach.clone(),
            explanation: self.explanation.clone(),
            items: self
                .steps
                .iter()
                .map(|s| PlanItemArg {
                    step: s.text.clone(),
                    status: s.status.clone(),
                })
                .collect(),
            verification_plan: self.verification_plan.clone(),
            risks_and_unknowns: self.risks_and_unknowns.clone(),
            handoff_packet: self.handoff_packet.clone(),
        }
    }

    #[allow(dead_code)]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[allow(dead_code)]
    pub fn objective(&self) -> Option<&str> {
        self.objective.as_deref()
    }

    #[allow(dead_code)]
    pub fn context_summary(&self) -> Option<&str> {
        self.context_summary.as_deref()
    }

    #[allow(dead_code)]
    pub fn sources_used(&self) -> &[String] {
        &self.sources_used
    }

    #[allow(dead_code)]
    pub fn constraints(&self) -> &[String] {
        &self.constraints
    }

    #[allow(dead_code)]
    pub fn recommended_approach(&self) -> Option<&str> {
        self.recommended_approach.as_deref()
    }

    pub fn explanation(&self) -> Option<&str> {
        self.explanation.as_deref()
    }

    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }

    #[allow(dead_code)]
    pub fn verification_plan(&self) -> Option<&str> {
        self.verification_plan.as_deref()
    }

    #[allow(dead_code)]
    pub fn risks_and_unknowns(&self) -> Option<&str> {
        self.risks_and_unknowns.as_deref()
    }

    #[allow(dead_code)]
    pub fn handoff_packet(&self) -> Option<&str> {
        self.handoff_packet.as_deref()
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
        "Publish a structured implementation plan as a reviewable artifact. Include the investigation findings (sources, constraints, risks) so the user can make an informed decision. All fields except `plan` are optional — only provide what you actually discovered."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title summarizing the plan"
                },
                "objective": {
                    "type": "string",
                    "description": "What this plan aims to achieve"
                },
                "context_summary": {
                    "type": "string",
                    "description": "Brief summary of the investigation context"
                },
                "sources_used": {
                    "type": "array",
                    "description": "Files, docs, commands, or sub-agents consulted",
                    "items": { "type": "string" }
                },
                "constraints": {
                    "type": "array",
                    "description": "User rules, repo constitution, mode limits, safety constraints",
                    "items": { "type": "string" }
                },
                "recommended_approach": {
                    "type": "string",
                    "description": "High-level approach and rationale"
                },
                "plan": {
                    "type": "array",
                    "description": "Ordered execution steps",
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
                },
                "verification_plan": {
                    "type": "string",
                    "description": "How to verify the plan was executed correctly"
                },
                "risks_and_unknowns": {
                    "type": "string",
                    "description": "Known risks, assumptions, and open questions"
                },
                "handoff_packet": {
                    "type": "string",
                    "description": "Compact execution-ready summary for handoff to Agent mode"
                },
                "explanation": {
                    "type": "string",
                    "description": "Optional high-level explanation (legacy, prefer recommended_approach)"
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
        let opt_str = |key: &str| -> Option<String> {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let opt_strs = |key: &str| -> Vec<String> {
            input
                .get(key)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };

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
            title: opt_str("title"),
            objective: opt_str("objective"),
            context_summary: opt_str("context_summary"),
            sources_used: opt_strs("sources_used"),
            constraints: opt_strs("constraints"),
            recommended_approach: opt_str("recommended_approach"),
            plan: plan_args,
            verification_plan: opt_str("verification_plan"),
            risks_and_unknowns: opt_str("risks_and_unknowns"),
            handoff_packet: opt_str("handoff_packet"),
            explanation: opt_str("explanation"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_update_plan_still_works() {
        let mut state = PlanState::default();
        state.update(UpdatePlanArgs {
            explanation: Some("legacy plan".into()),
            plan: vec![PlanItemArg {
                step: "step one".into(),
                status: StepStatus::Pending,
            }],
            ..Default::default()
        });
        assert_eq!(state.explanation(), Some("legacy plan"));
        assert_eq!(state.steps().len(), 1);
        assert!(state.title().is_none());
    }

    #[test]
    fn full_artifact_fields_parsed_and_stored() {
        let mut state = PlanState::default();
        state.update(UpdatePlanArgs {
            title: Some("Test Plan".into()),
            objective: Some("Test objective".into()),
            context_summary: Some("Investigated X".into()),
            sources_used: vec!["file_a.rs".into(), "docs/guide.md".into()],
            constraints: vec!["no shell access".into()],
            recommended_approach: Some("Use incremental refactor".into()),
            plan: vec![PlanItemArg {
                step: "step one".into(),
                status: StepStatus::InProgress,
            }],
            verification_plan: Some("Run tests".into()),
            risks_and_unknowns: Some("Might break old API".into()),
            handoff_packet: Some("Compact summary".into()),
            ..Default::default()
        });

        assert_eq!(state.title(), Some("Test Plan"));
        assert_eq!(state.objective(), Some("Test objective"));
        assert_eq!(state.context_summary(), Some("Investigated X"));
        assert_eq!(state.sources_used(), &["file_a.rs", "docs/guide.md"]);
        assert_eq!(state.constraints(), &["no shell access"]);
        assert_eq!(
            state.recommended_approach(),
            Some("Use incremental refactor")
        );
        assert_eq!(state.steps().len(), 1);
        assert_eq!(state.verification_plan(), Some("Run tests"));
        assert_eq!(state.risks_and_unknowns(), Some("Might break old API"));
        assert_eq!(state.handoff_packet(), Some("Compact summary"));
    }

    #[test]
    fn snapshot_skips_empty_fields() {
        let mut state = PlanState::default();
        state.update(UpdatePlanArgs {
            plan: vec![PlanItemArg {
                step: "only step".into(),
                status: StepStatus::Completed,
            }],
            ..Default::default()
        });

        let snapshot = state.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        // Verify that only non-empty fields appear.
        assert!(json.contains("\"items\""));
        assert!(json.contains("\"step\""));
        // Empty/Optional fields should be absent.
        assert!(!json.contains("\"title\""));
        assert!(!json.contains("\"sources_used\""));
    }

    #[test]
    fn snapshot_includes_new_fields_when_present() {
        let mut state = PlanState::default();
        state.update(UpdatePlanArgs {
            title: Some("Rich Plan".into()),
            verification_plan: Some("Verify with CI".into()),
            plan: vec![],
            ..Default::default()
        });

        let snapshot = state.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"title\""));
        assert!(json.contains("Rich Plan"));
        assert!(json.contains("\"verification_plan\""));
    }

    #[test]
    fn is_empty_respects_new_fields() {
        let mut state = PlanState::default();
        assert!(state.is_empty());

        state.update(UpdatePlanArgs {
            title: Some("Non-empty".into()),
            ..Default::default()
        });
        assert!(!state.is_empty(), "title should make plan non-empty");
    }

    #[test]
    fn default_update_plan_args_is_usable() {
        let args = UpdatePlanArgs::default();
        assert!(args.plan.is_empty());
        assert!(args.title.is_none());
        assert!(args.sources_used.is_empty());
    }
}
