//! Model-visible automation tools over `AutomationManager`.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::automation_manager::{
    AutomationStatus, CreateAutomationRequest, UpdateAutomationRequest, run_now_shared,
};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, optional_u64, required_str,
};

pub struct AutomationCreateTool;
pub struct AutomationListTool;
pub struct AutomationReadTool;
pub struct AutomationUpdateTool;
pub struct AutomationPauseTool;
pub struct AutomationResumeTool;
pub struct AutomationDeleteTool;
pub struct AutomationRunTool;

#[async_trait]
impl ToolSpec for AutomationCreateTool {
    fn name(&self) -> &'static str {
        "automation_create"
    }

    fn description(&self) -> &'static str {
        "Create a durable scheduled automation. Creation requires approval and recurrence is constrained to supported HOURLY/WEEKLY RRULE forms. Runs enqueue normal durable tasks."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "rrule": {
                    "type": "string",
                    "description": "Supported: FREQ=HOURLY;INTERVAL=N[;BYDAY=MO,TU] or FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=30"
                },
                "cwds": { "type": "array", "items": { "type": "string" } },
                "mode": { "type": "string", "description": "Task mode for scheduled runs. Defaults to agent when omitted." },
                "allow_shell": { "type": "boolean", "default": false },
                "trust_mode": { "type": "boolean", "default": false },
                "auto_approve": { "type": "boolean", "default": true },
                "paused": { "type": "boolean", "default": false }
            },
            "required": ["name", "prompt", "rrule"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let req = CreateAutomationRequest {
            name: required_str(&input, "name")?.to_string(),
            prompt: required_str(&input, "prompt")?.to_string(),
            rrule: required_str(&input, "rrule")?.to_string(),
            cwds: string_array(&input, "cwds")?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            mode: optional_str(&input, "mode").map(ToString::to_string),
            allow_shell: optional_bool_value(&input, "allow_shell"),
            trust_mode: optional_bool_value(&input, "trust_mode"),
            auto_approve: optional_bool_value(&input, "auto_approve"),
            status: Some(
                if input
                    .get("paused")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    AutomationStatus::Paused
                } else {
                    AutomationStatus::Active
                },
            ),
        };
        let automation = manager
            .create_automation(req)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&automation).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for AutomationListTool {
    fn name(&self) -> &'static str {
        "automation_list"
    }

    fn description(&self) -> &'static str {
        "List durable automations with status, next run, and last run timestamps."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let mut automations = manager
            .list_automations()
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        automations.truncate(optional_u64(&input, "limit", 50).clamp(1, 100) as usize);
        ToolResult::json(&automations).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for AutomationReadTool {
    fn name(&self) -> &'static str {
        "automation_read"
    }

    fn description(&self) -> &'static str {
        "Read one durable automation plus recent run records."
    }

    fn input_schema(&self) -> Value {
        automation_id_schema(true)
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let id = required_str(&input, "automation_id")?;
        let automation = manager
            .get_automation(id)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let runs = manager
            .list_runs(id, Some(20))
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&json!({ "automation": automation, "recent_runs": runs }))
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for AutomationUpdateTool {
    fn name(&self) -> &'static str {
        "automation_update"
    }

    fn description(&self) -> &'static str {
        "Update a durable automation. Requires approval; recurrence remains constrained to supported RRULE forms."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "automation_id": { "type": "string" },
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "rrule": { "type": "string" },
                "cwds": { "type": "array", "items": { "type": "string" } },
                "mode": { "type": "string", "description": "Task mode for scheduled runs. Defaults to agent when omitted." },
                "allow_shell": { "type": "boolean" },
                "trust_mode": { "type": "boolean" },
                "auto_approve": { "type": "boolean" },
                "status": { "type": "string", "enum": ["active", "paused"] }
            },
            "required": ["automation_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let status = optional_str(&input, "status").map(|value| match value {
            "paused" => AutomationStatus::Paused,
            _ => AutomationStatus::Active,
        });
        let req = UpdateAutomationRequest {
            name: optional_str(&input, "name").map(ToString::to_string),
            prompt: optional_str(&input, "prompt").map(ToString::to_string),
            rrule: optional_str(&input, "rrule").map(ToString::to_string),
            cwds: if input.get("cwds").is_some() {
                Some(
                    string_array(&input, "cwds")?
                        .into_iter()
                        .map(PathBuf::from)
                        .collect(),
                )
            } else {
                None
            },
            mode: optional_str(&input, "mode").map(ToString::to_string),
            allow_shell: optional_bool_value(&input, "allow_shell"),
            trust_mode: optional_bool_value(&input, "trust_mode"),
            auto_approve: optional_bool_value(&input, "auto_approve"),
            status,
        };
        let automation = manager
            .update_automation(required_str(&input, "automation_id")?, req)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&automation).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

macro_rules! write_automation_tool {
    ($ty:ident, $name:literal, $desc:literal, $method:ident) => {
        #[async_trait]
        impl ToolSpec for $ty {
            fn name(&self) -> &'static str {
                $name
            }
            fn description(&self) -> &'static str {
                $desc
            }
            fn input_schema(&self) -> Value {
                automation_id_schema(true)
            }
            fn capabilities(&self) -> Vec<ToolCapability> {
                vec![ToolCapability::RequiresApproval]
            }
            fn approval_requirement(&self) -> ApprovalRequirement {
                ApprovalRequirement::Required
            }
            async fn execute(
                &self,
                input: Value,
                context: &ToolContext,
            ) -> Result<ToolResult, ToolError> {
                let manager =
                    context.runtime.automations.as_ref().ok_or_else(|| {
                        ToolError::not_available("AutomationManager is not attached")
                    })?;
                let manager = manager.lock().await;
                let automation = manager
                    .$method(required_str(&input, "automation_id")?)
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                ToolResult::json(&automation)
                    .map_err(|e| ToolError::execution_failed(e.to_string()))
            }
        }
    };
}

write_automation_tool!(
    AutomationPauseTool,
    "automation_pause",
    "Pause a durable automation. Requires approval.",
    pause_automation
);
write_automation_tool!(
    AutomationResumeTool,
    "automation_resume",
    "Resume a paused durable automation. Requires approval.",
    resume_automation
);
write_automation_tool!(
    AutomationDeleteTool,
    "automation_delete",
    "Delete a durable automation and its run history. Requires approval.",
    delete_automation
);

#[async_trait]
impl ToolSpec for AutomationRunTool {
    fn name(&self) -> &'static str {
        "automation_run"
    }

    fn description(&self) -> &'static str {
        "Run an automation now. The run enqueues a normal durable task and returns linked task/thread/turn ids as they become available."
    }

    fn input_schema(&self) -> Value {
        automation_id_schema(true)
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let task_manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        // run_now_shared handles its own lock phases so the manager mutex is
        // never held across the task-manager await.
        let run = run_now_shared(
            manager,
            required_str(&input, "automation_id")?,
            task_manager,
        )
        .await
        .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&run).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

fn automation_id_schema(require_id: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "automation_id": { "type": "string" }
        },
        "additionalProperties": false
    });
    if require_id {
        schema["required"] = json!(["automation_id"]);
    }
    schema
}

fn string_array(input: &Value, field: &str) -> Result<Vec<String>, ToolError> {
    Ok(input
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default())
}

fn optional_bool_value(input: &Value, field: &str) -> Option<bool> {
    input.get(field).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolSpec;

    #[test]
    fn create_schema_exposes_rrule() {
        let schema = AutomationCreateTool.input_schema();
        assert!(schema["properties"]["rrule"].is_object());
        assert_eq!(schema["required"][0], "name");
    }
}
