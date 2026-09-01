//! Model-visible automation tools over `AutomationManager`.
//!
//! Unified surface (piagent phase B): the model sees one tool, `automation`,
//! with an `action` parameter routing to the per-action logic. The legacy
//! `automation_*` execution aliases were removed in v0.9.3.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::automation_manager::{
    AUTOMATION_WATCHER_NO_REPORT_SENTINEL, AutomationDeliveryMode, AutomationStatus,
    CreateAutomationRequest, UpdateAutomationRequest, run_now_shared,
};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, optional_u64, required_str,
};

/// Read-only actions — these are the only ones the Plan-mode surface exposes.
const READ_ACTIONS: &[&str] = &["list", "read"];
const ALL_ACTIONS: &[&str] = &[
    "create", "list", "read", "update", "pause", "resume", "delete", "run",
];

/// Unified automation tool.
///
/// One struct, one input schema per surface: the canonical `automation`
/// tool (all actions, or the read-only subset via [`AutomationTool::read_only`])
/// plus hidden legacy aliases carrying a `forced_action`.
pub struct AutomationTool {
    name: &'static str,
    forced_action: Option<&'static str>,
    read_only: bool,
}

impl AutomationTool {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: false,
        }
    }

    /// Plan-mode variant: only the read-only actions are advertised and routed.
    pub const fn read_only(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: true,
        }
    }

    #[cfg(test)]
    pub const fn alias(name: &'static str, action: &'static str) -> Self {
        Self {
            name,
            forced_action: Some(action),
            read_only: false,
        }
    }

    fn allowed_actions(&self) -> &'static [&'static str] {
        if self.read_only {
            READ_ACTIONS
        } else {
            ALL_ACTIONS
        }
    }

    fn resolve_action<'a>(&'a self, input: &'a Value) -> Result<&'a str, ToolError> {
        let action = match self.forced_action {
            Some(action) => action,
            None => input.get("action").and_then(Value::as_str).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "automation: missing `action` (one of: {})",
                    self.allowed_actions().join(", ")
                ))
            })?,
        };
        if self.allowed_actions().contains(&action) {
            Ok(action)
        } else {
            Err(ToolError::invalid_input(format!(
                "automation: invalid action `{action}` (one of: {})",
                self.allowed_actions().join(", ")
            )))
        }
    }

    fn action_is_read(action: &str) -> bool {
        READ_ACTIONS.contains(&action)
    }
}

#[async_trait]
impl ToolSpec for AutomationTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        self.forced_action.is_none()
    }

    fn description(&self) -> &'static str {
        match self.forced_action {
            Some("create") => {
                "Create a durable scheduled automation. Creation requires approval. Supported schedules: FREQ=ONCE;AT=YYYY-MM-DDTHH:MM[:SS] (local time) or RFC3339, FREQ=HOURLY..., FREQ=WEEKLY..., and FREQ=CRON;EXPR=<standard 5-field local cron>. delivery_mode=watcher is for condition checks: return EXACTLY NOTHING_TO_REPORT when there is no change."
            }
            Some("list") => {
                "List durable automations with status, next run, and last run timestamps."
            }
            Some("read") => "Read one durable automation plus recent run records.",
            Some("update") => {
                "Update a durable automation. Requires approval; schedules support ONCE, HOURLY, WEEKLY, and 5-field CRON forms."
            }
            Some("pause") => "Pause a durable automation. Requires approval.",
            Some("resume") => "Resume a paused durable automation. Requires approval.",
            Some("delete") => "Delete a durable automation and its run history. Requires approval.",
            Some("run") => {
                "Run an automation now. The run enqueues a normal durable task and returns linked task/thread/turn ids as they become available."
            }
            _ if self.read_only => {
                "Inspect durable scheduled automations. Actions: \"list\" (status, next run, last run) and \"read\" (one automation plus recent run records)."
            }
            _ => {
                "Manage durable scheduled automations. Actions: \"create\" (approval; schedules support ONCE, HOURLY, WEEKLY, and 5-field CRON forms; watcher mode uses EXACT NOTHING_TO_REPORT for no-change checks), \"list\", \"read\", \"update\" (approval), \"pause\" (approval), \"resume\" (approval), \"delete\" (approval), \"run\" (approval)."
            }
        }
    }

    fn input_schema(&self) -> Value {
        if let Some(action) = self.forced_action {
            return legacy_action_schema(action);
        }
        let actions: Vec<&str> = self.allowed_actions().to_vec();
        let mut properties = serde_json::Map::new();
        properties.insert(
            "action".to_string(),
            json!({
                "type": "string",
                "enum": actions,
                "description": "Action to perform."
            }),
        );
        if !self.read_only {
            properties.insert(
                "name".to_string(),
                json!({ "type": "string", "description": "Automation name (action=create/update)." }),
            );
            properties.insert(
                "prompt".to_string(),
                json!({ "type": "string", "description": "Prompt for scheduled runs (action=create/update)." }),
            );
            properties.insert(
                "rrule".to_string(),
                json!({
                    "type": "string",
                    "description": "Supported: FREQ=ONCE;AT=2026-08-03T14:30 (local time or RFC3339), FREQ=HOURLY;INTERVAL=N[;BYDAY=MO,TU][;BYHOUR=9][;BYMINUTE=30], FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=30, or FREQ=CRON;EXPR=*/17 * * * *. Cron uses standard 5-field local time. For HOURLY, BYHOUR/BYMINUTE choose the initial local wall-clock anchor and INTERVAL advances from that anchor; BYHOUR is not a daily-only filter. Anchored wall times skip nonexistent clock times and use the first occurrence of ambiguous clock times. (action=create/update)"
                }),
            );
            properties.insert(
                "cwds".to_string(),
                json!({ "type": "array", "items": { "type": "string" }, "description": "Working directories for scheduled runs (action=create/update)." }),
            );
            properties.insert(
                "model".to_string(),
                json!({ "type": "string", "description": "Model id for scheduled runs (action=create/update)." }),
            );
            properties.insert(
                "mode".to_string(),
                json!({ "type": "string", "description": "Task mode for scheduled runs. Defaults to agent when omitted. (action=create/update)" }),
            );
            properties.insert(
                "allow_shell".to_string(),
                json!({ "type": "boolean", "default": false, "description": "(action=create/update)" }),
            );
            properties.insert(
                "trust_mode".to_string(),
                json!({ "type": "boolean", "default": false, "description": "(action=create/update)" }),
            );
            properties.insert(
                "auto_approve".to_string(),
                json!({ "type": "boolean", "default": false, "description": "(action=create/update)" }),
            );
            properties.insert(
                "delivery_mode".to_string(),
                json!({
                    "type": "string",
                    "enum": ["task", "watcher"],
                    "default": "task",
                    "description": format!("Delivery mode for scheduled checks. \"task\" creates a normal durable background run. \"watcher\" is for condition-shaped prompts; when there is no change, return EXACTLY {AUTOMATION_WATCHER_NO_REPORT_SENTINEL}. (action=create/update)")
                }),
            );
            properties.insert(
                "paused".to_string(),
                json!({ "type": "boolean", "default": false, "description": "Create the automation paused (action=create)." }),
            );
            properties.insert(
                "status".to_string(),
                json!({ "type": "string", "enum": ["active", "paused"], "description": "(action=update)" }),
            );
        }
        properties.insert(
            "automation_id".to_string(),
            json!({ "type": "string", "description": "Target automation id (action=read/update/pause/resume/delete/run)." }),
        );
        properties.insert(
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 50, "description": "(action=list)" }),
        );
        json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        match self.forced_action {
            Some(action) if Self::action_is_read(action) => vec![ToolCapability::ReadOnly],
            // `run` executes a stored automation now; the other mutating
            // actions schedule one to execute later, with its own prompt, cwd,
            // and task mode. Declaring only `RequiresApproval` described the
            // *approval* consequence and hid the *execution* one, which left
            // every capability-derived policy — including the child execution
            // envelope — unable to see that this family spawns agent runs.
            Some(_) => vec![
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ],
            None if self.read_only => vec![ToolCapability::ReadOnly],
            None => vec![
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ],
        }
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        match self.forced_action {
            Some(action) if Self::action_is_read(action) => ApprovalRequirement::Auto,
            Some(_) => ApprovalRequirement::Required,
            None if self.read_only => ApprovalRequirement::Auto,
            None => ApprovalRequirement::Required,
        }
    }

    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match self.resolve_action(input) {
            Ok(action) if Self::action_is_read(action) => ApprovalRequirement::Auto,
            _ => ApprovalRequirement::Required,
        }
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        match self.resolve_action(input) {
            Ok(action) => Self::action_is_read(action),
            Err(_) => self.is_read_only(),
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        match self.resolve_action(&input)? {
            "create" => self.execute_create(&input, context).await,
            "list" => self.execute_list(&input, context).await,
            "read" => self.execute_read(&input, context).await,
            "update" => self.execute_update(&input, context).await,
            "pause" => self.execute_simple(context, &input, "pause").await,
            "resume" => self.execute_simple(context, &input, "resume").await,
            "delete" => self.execute_simple(context, &input, "delete").await,
            "run" => self.execute_run(&input, context).await,
            action => Err(ToolError::invalid_input(format!(
                "automation: invalid action `{action}`"
            ))),
        }
    }
}

impl AutomationTool {
    async fn execute_create(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let req = CreateAutomationRequest {
            name: required_str(input, "name")?.to_string(),
            prompt: required_str(input, "prompt")?.to_string(),
            rrule: required_str(input, "rrule")?.to_string(),
            cwds: string_array(input, "cwds")?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            model: optional_str(input, "model")?.map(ToString::to_string),
            mode: optional_str(input, "mode")?.map(ToString::to_string),
            allow_shell: optional_bool_value(input, "allow_shell"),
            trust_mode: optional_bool_value(input, "trust_mode"),
            auto_approve: optional_bool_value(input, "auto_approve"),
            delivery_mode: optional_delivery_mode(input)?,
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

    async fn execute_list(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let mut automations = manager
            .list_automations()
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        automations.truncate(optional_u64(input, "limit", 50)?.clamp(1, 100) as usize);
        ToolResult::json(&automations).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_read(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let id = required_str(input, "automation_id")?;
        let automation = manager
            .get_automation(id)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let runs = manager
            .list_runs(id, Some(20))
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&json!({ "automation": automation, "recent_runs": runs }))
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_update(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let status = optional_str(input, "status")?
            .map(parse_automation_status)
            .transpose()?;
        let req = UpdateAutomationRequest {
            name: optional_str(input, "name")?.map(ToString::to_string),
            prompt: optional_str(input, "prompt")?.map(ToString::to_string),
            rrule: optional_str(input, "rrule")?.map(ToString::to_string),
            cwds: if input.get("cwds").is_some() {
                Some(
                    string_array(input, "cwds")?
                        .into_iter()
                        .map(PathBuf::from)
                        .collect(),
                )
            } else {
                None
            },
            model: optional_str(input, "model")?.map(ToString::to_string),
            mode: optional_str(input, "mode")?.map(ToString::to_string),
            allow_shell: optional_bool_value(input, "allow_shell"),
            trust_mode: optional_bool_value(input, "trust_mode"),
            auto_approve: optional_bool_value(input, "auto_approve"),
            delivery_mode: optional_delivery_mode(input)?,
            status,
        };
        let automation = manager
            .update_automation(required_str(input, "automation_id")?, req)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&automation).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    /// pause / resume / delete share the same shape: one id in, automation out.
    async fn execute_simple(
        &self,
        context: &ToolContext,
        input: &Value,
        action: &str,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .automations
            .as_ref()
            .ok_or_else(|| ToolError::not_available("AutomationManager is not attached"))?;
        let manager = manager.lock().await;
        let automation = match action {
            "pause" => manager.pause_automation(required_str(input, "automation_id")?),
            "resume" => manager.resume_automation(required_str(input, "automation_id")?),
            "delete" => manager.delete_automation(required_str(input, "automation_id")?),
            _ => unreachable!("execute_simple only routes pause/resume/delete"),
        }
        .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&automation).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_run(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
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
        let run = run_now_shared(manager, required_str(input, "automation_id")?, task_manager)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        ToolResult::json(&run).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

/// The exact schema the legacy per-action tool exposed, kept so hidden alias
/// registrations report an identical contract to the pre-unification tools.
fn legacy_action_schema(action: &str) -> Value {
    match action {
        "create" => json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "rrule": {
                    "type": "string",
                    "description": "Supported: FREQ=ONCE;AT=2026-08-03T14:30 (local time or RFC3339), FREQ=HOURLY;INTERVAL=N[;BYDAY=MO,TU][;BYHOUR=9][;BYMINUTE=30], FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=30, or FREQ=CRON;EXPR=*/17 * * * *. Cron uses standard 5-field local time. For HOURLY, BYHOUR/BYMINUTE choose the initial local wall-clock anchor and INTERVAL advances from that anchor; BYHOUR is not a daily-only filter. Anchored wall times skip nonexistent clock times and use the first occurrence of ambiguous clock times."
                },
                "cwds": { "type": "array", "items": { "type": "string" } },
                "model": { "type": "string", "description": "Model id for scheduled runs." },
                "mode": { "type": "string", "description": "Task mode for scheduled runs. Defaults to agent when omitted." },
                "allow_shell": { "type": "boolean", "default": false },
                "trust_mode": { "type": "boolean", "default": false },
                "auto_approve": { "type": "boolean", "default": false },
                "delivery_mode": {
                    "type": "string",
                    "enum": ["task", "watcher"],
                    "default": "task",
                    "description": "Delivery mode. watcher prompts must return EXACTLY NOTHING_TO_REPORT when there is no change."
                },
                "paused": { "type": "boolean", "default": false }
            },
            "required": ["name", "prompt", "rrule"],
            "additionalProperties": false
        }),
        "list" => json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }
            },
            "additionalProperties": false
        }),
        "update" => json!({
            "type": "object",
            "properties": {
                "automation_id": { "type": "string" },
                "name": { "type": "string" },
                "prompt": { "type": "string" },
                "rrule": { "type": "string" },
                "cwds": { "type": "array", "items": { "type": "string" } },
                "model": { "type": "string", "description": "Model id for scheduled runs." },
                "mode": { "type": "string", "description": "Task mode for scheduled runs. Defaults to agent when omitted." },
                "allow_shell": { "type": "boolean" },
                "trust_mode": { "type": "boolean" },
                "auto_approve": { "type": "boolean" },
                "delivery_mode": { "type": "string", "enum": ["task", "watcher"] },
                "status": { "type": "string", "enum": ["active", "paused"] }
            },
            "required": ["automation_id"],
            "additionalProperties": false
        }),
        // read / pause / resume / delete / run share the id-only schema.
        _ => automation_id_schema(true),
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

/// Parse an `automation_update` status. #5123-class: unknown statuses used to
/// coerce to Active — the opposite of pause intent, and run-scheduling.
fn parse_automation_status(value: &str) -> Result<AutomationStatus, ToolError> {
    match value {
        "active" => Ok(AutomationStatus::Active),
        "paused" => Ok(AutomationStatus::Paused),
        other => Err(ToolError::invalid_input(format!(
            "unknown automation status '{other}'; expected 'active' or 'paused'"
        ))),
    }
}

fn optional_delivery_mode(input: &Value) -> Result<Option<AutomationDeliveryMode>, ToolError> {
    match optional_str(input, "delivery_mode")? {
        None => Ok(None),
        Some("task") => Ok(Some(AutomationDeliveryMode::Task)),
        Some("watcher") => Ok(Some(AutomationDeliveryMode::Watcher)),
        Some(other) => Err(ToolError::invalid_input(format!(
            "automation: invalid delivery_mode `{other}` (expected task or watcher)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolSpec;

    #[test]
    fn create_schema_exposes_rrule() {
        let schema = AutomationTool::alias("automation_create", "create").input_schema();
        assert!(schema["properties"]["rrule"].is_object());
        assert_eq!(schema["required"][0], "name");
    }

    #[test]
    fn update_status_rejects_unknown_values_instead_of_coercing_to_active() {
        assert!(matches!(
            parse_automation_status("active"),
            Ok(AutomationStatus::Active)
        ));
        assert!(matches!(
            parse_automation_status("paused"),
            Ok(AutomationStatus::Paused)
        ));
        for bad in ["pause", "disabled", "off", "stopped", ""] {
            let err = parse_automation_status(bad).expect_err("must not coerce");
            assert!(
                err.to_string().contains("expected 'active' or 'paused'"),
                "{err}"
            );
        }
    }

    #[test]
    fn create_schema_auto_approve_defaults_to_false() {
        let schema = AutomationTool::alias("automation_create", "create").input_schema();
        let auto_approve = &schema["properties"]["auto_approve"];
        assert_eq!(auto_approve["type"], "boolean");
        assert_eq!(auto_approve["default"], false);
    }

    #[test]
    fn create_schema_exposes_delivery_mode_and_new_schedule_forms() {
        let schema = AutomationTool::alias("automation_create", "create").input_schema();
        assert!(schema["properties"]["model"].is_object());
        assert_eq!(
            schema["properties"]["delivery_mode"]["enum"],
            json!(["task", "watcher"])
        );
        let description = schema["properties"]["rrule"]["description"]
            .as_str()
            .expect("rrule description");
        assert!(description.contains("FREQ=ONCE"));
        assert!(description.contains("FREQ=CRON"));
    }

    #[test]
    fn canonical_schema_lists_all_actions_and_union_fields() {
        let schema = AutomationTool::new("automation").input_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        for action in [
            "create", "list", "read", "update", "pause", "resume", "delete", "run",
        ] {
            assert!(
                actions.iter().any(|value| value.as_str() == Some(action)),
                "canonical schema must offer action {action}"
            );
        }
        for field in [
            "name",
            "prompt",
            "rrule",
            "model",
            "delivery_mode",
            "automation_id",
            "limit",
        ] {
            assert!(
                schema["properties"][field].is_object(),
                "canonical schema must carry union field {field}"
            );
        }
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn read_only_variant_only_offers_read_actions() {
        let tool = AutomationTool::read_only("automation");
        let schema = tool.input_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert_eq!(actions, &vec![json!("list"), json!("read")]);
        assert!(!schema["properties"]["rrule"].is_object());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
        assert!(tool.is_read_only());
    }

    #[test]
    fn aliases_hide_from_model_and_force_action() {
        let create = AutomationTool::alias("automation_create", "create");
        assert!(!create.model_visible());
        assert_eq!(create.name(), "automation_create");
        assert_eq!(create.approval_requirement(), ApprovalRequirement::Required);

        let list = AutomationTool::alias("automation_list", "list");
        assert!(!list.model_visible());
        assert_eq!(list.approval_requirement(), ApprovalRequirement::Auto);
        assert!(list.is_read_only_for(&json!({})));

        let canonical = AutomationTool::new("automation");
        assert!(canonical.model_visible());
        // Approval routing stays per action: read actions auto, writes required.
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "list"})),
            ApprovalRequirement::Auto
        );
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "delete"})),
            ApprovalRequirement::Required
        );
        assert!(canonical.is_read_only_for(&json!({"action": "read"})));
        assert!(!canonical.is_read_only_for(&json!({"action": "create"})));
    }

    #[test]
    fn canonical_rejects_unknown_or_missing_action() {
        let tool = AutomationTool::new("automation");
        let err = tool
            .resolve_action(&json!({}))
            .expect_err("missing action must fail");
        assert!(err.to_string().contains("missing `action`"));
        let err = tool
            .resolve_action(&json!({"action": "explode"}))
            .expect_err("unknown action must fail");
        assert!(err.to_string().contains("invalid action"));

        let read_only = AutomationTool::read_only("automation");
        let err = read_only
            .resolve_action(&json!({"action": "delete"}))
            .expect_err("read-only surface must reject write actions");
        assert!(err.to_string().contains("invalid action"));
    }
}
