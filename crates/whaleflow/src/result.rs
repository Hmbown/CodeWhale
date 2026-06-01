//! Structured result returned to the orchestrator model after a workflow run.

use serde::{Deserialize, Serialize};

/// Outcome of a complete workflow run, returned to the model as the
/// tool result for `workflow_run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    /// The goal from the original config.
    pub goal: String,

    /// Overall status.
    pub status: WorkflowStatus,

    /// Results per phase, in execution order.
    pub phases: Vec<PhaseResult>,

    /// Aggregate counts.
    pub counts: TaskCounts,

    /// Human-readable summary suitable for model consumption.
    pub summary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// All tasks completed successfully.
    Completed,
    /// Some tasks failed but workflow continued (SkipContinue policy).
    Partial,
    /// Workflow was aborted due to a failure (Abort policy).
    Aborted,
    /// Workflow was cancelled by user interrupt.
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    pub name: String,
    pub status: TaskStatus,
    pub tasks: Vec<TaskResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub id: String,
    pub status: TaskStatus,
    /// Summary from the agent output (truncated).
    pub summary: Option<String>,
    /// Files the agent touched.
    pub files_touched: Vec<String>,
    /// Error message if the task failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct TaskCounts {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub pending: usize,
}

impl WorkflowResult {
    /// Build a human-readable summary for the orchestrator model.
    pub fn build_summary(&mut self) {
        let counts = &self.counts;
        let status_str = match self.status {
            WorkflowStatus::Completed => "completed",
            WorkflowStatus::Partial => "completed with failures",
            WorkflowStatus::Aborted => "aborted",
            WorkflowStatus::Cancelled => "cancelled",
        };

        let mut parts = vec![format!(
            "Workflow '{}' {}. {} total tasks: {} completed, {} failed, {} skipped.",
            self.goal,
            status_str,
            counts.total,
            counts.completed,
            counts.failed,
            counts.skipped,
        )];

        for phase in &self.phases {
            parts.push(format!("\n## Phase: {}", phase.name));
            for task in &phase.tasks {
                let icon = match task.status {
                    TaskStatus::Completed => "✓",
                    TaskStatus::Failed => "✗",
                    TaskStatus::Skipped => "⊘",
                    _ => "○",
                };
                let summary = task.summary.as_deref().unwrap_or("(no summary)");
                let files = if task.files_touched.is_empty() {
                    String::new()
                } else {
                    format!(" [files: {}]", task.files_touched.join(", "))
                };
                parts.push(format!("  {} {}: {}{}", icon, task.id, summary, files));
                if let Some(ref err) = task.error {
                    parts.push(format!("      error: {}", err));
                }
            }
        }

        self.summary = parts.join("\n");
    }
}
