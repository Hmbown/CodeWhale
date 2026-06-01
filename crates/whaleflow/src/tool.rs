/// Schema for the workflow_run tool that DeepSeek calls.
pub const WORKFLOW_RUN_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "config": {
      "type": "object",
      "description": "The workflow configuration",
      "properties": {
        "goal": {"type": "string", "description": "Human-readable goal"},
        "max_concurrent": {"type": "integer", "default": 6, "description": "Max concurrent agents"},
        "phases": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "name": {"type": "string"},
              "depends_on": {"type": "array", "items": {"type": "string"}},
              "parallel": {"type": "boolean", "default": true},
              "on_failure": {"type": "string", "enum": ["skip_continue", "abort"], "default": "skip_continue"},
              "tasks": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "id": {"type": "string", "description": "Unique task id"},
                    "prompt": {"type": "string", "description": "Prompt for the sub-agent"},
                    "agent_type": {"type": "string", "description": "explore, review, implementer, verifier, or omit for default"},
                    "depends_on_results": {"type": "array", "items": {"type": "string"}, "description": "Task IDs whose results feed into this task"},
                    "mode": {"type": "string", "enum": ["read_only", "read_write"], "default": "read_only"},
                    "file_scope": {"type": "array", "items": {"type": "string"}, "description": "Glob patterns for files this task touches"},
                    "isolation": {"type": "string", "enum": ["shared", "worktree"], "default": "shared"}
                  },
                  "required": ["id", "prompt"]
                }
              }
            },
            "required": ["name", "tasks"]
          }
        }
      },
      "required": ["goal", "phases"]
    }
  },
  "required": ["config"]
}"#;

/// Run a workflow and return the result.
pub async fn execute_workflow(
    config_json: &str,
    spawner: std::sync::Arc<dyn crate::AgentSpawner>,
) -> Result<String, String> {
    let config: crate::config::WorkflowConfig = serde_json::from_str(config_json)
        .map_err(|e| format!("Invalid config JSON: {e}"))?;

    // Validate
    config.validate().map_err(|errors| errors.join("\n"))?;

    // Check conflicts
    let conflicts = config.detect_conflicts();
    if !conflicts.is_empty() {
        let msg = conflicts
            .iter()
            .map(|c| format!("⚠ {}: {}", c.kind_name(), c.description))
            .collect::<Vec<_>>()
            .join("\n");
        // Don't fail — warn and continue. The scheduler will serialize if needed.
        tracing::warn!("workflow conflicts detected:\n{msg}");
    }

    // Run
    let mut scheduler = crate::Scheduler::new(config, spawner);
    let result = scheduler.run().await;

    // Return structured result as JSON
    serde_json::to_string_pretty(&result)
        .map_err(|e| format!("Failed to serialize result: {e}"))
}
