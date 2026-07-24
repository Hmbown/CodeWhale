//! Todo list tool and supporting data structures.

use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === Types ===

/// Status for a todo item.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    #[serde(alias = "canceled")]
    Cancelled,
}

impl TodoStatus {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }

    /// Parse a string into a todo status.
    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" | "inprogress" => Some(TodoStatus::InProgress),
            "completed" | "done" => Some(TodoStatus::Completed),
            "cancelled" | "canceled" => Some(TodoStatus::Cancelled),
            _ => None,
        }
    }

    /// Whether this item has reached a terminal outcome. Cancellation settles
    /// work without misreporting it as successful completion.
    #[must_use]
    pub fn is_settled(self) -> bool {
        matches!(self, TodoStatus::Completed | TodoStatus::Cancelled)
    }
}

/// A single todo item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub id: u32,
    pub content: String,
    pub status: TodoStatus,
}

/// Snapshot of a todo list for display or serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoListSnapshot {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<TodoItem>,
    #[serde(default)]
    pub completion_pct: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_progress_id: Option<u32>,
}

impl TodoListSnapshot {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Plain-text rendering for headless and accessibility surfaces. Styled
    /// panes consume the same `items`; this keeps status wording and ordering
    /// owned by the canonical list rather than a second progress model.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.items
            .iter()
            .map(|item| {
                let marker = match item.status {
                    TodoStatus::Pending => "[ ]",
                    TodoStatus::InProgress => "[~]",
                    TodoStatus::Completed => "[x]",
                    TodoStatus::Cancelled => "[-]",
                };
                format!("{marker} #{} {}", item.id, item.content)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Mutable list of todo items with helper operations.
#[derive(Debug, Clone, Default)]
pub struct TodoList {
    items: Vec<TodoItem>,
    next_id: u32,
}

impl TodoList {
    /// Create an empty todo list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
        }
    }

    /// Return a snapshot of the list with computed metrics.
    #[must_use]
    pub fn snapshot(&self) -> TodoListSnapshot {
        TodoListSnapshot {
            items: self.items.clone(),
            completion_pct: self.completion_percentage(),
            in_progress_id: self.in_progress_id(),
        }
    }

    /// Rebuild a mutable list from a persisted snapshot.
    ///
    /// Derived snapshot fields are deliberately recomputed. IDs and the
    /// single-in-progress invariant are validated before any live state is
    /// replaced, so malformed session data cannot leave a half-restored list.
    pub fn from_snapshot(snapshot: &TodoListSnapshot) -> Result<Self, String> {
        let mut seen = std::collections::HashSet::with_capacity(snapshot.items.len());
        let mut in_progress_count = 0usize;
        let mut max_id = 0u32;
        let mut items = Vec::with_capacity(snapshot.items.len());

        for item in &snapshot.items {
            if item.id == 0 {
                return Err("To-do item IDs must be greater than zero".to_string());
            }
            if !seen.insert(item.id) {
                return Err(format!("Duplicate To-do item ID {}", item.id));
            }
            if item.status == TodoStatus::InProgress {
                in_progress_count += 1;
                if in_progress_count > 1 {
                    return Err("Only one To-do item may be in progress".to_string());
                }
            }
            max_id = max_id.max(item.id);
            items.push(TodoItem {
                id: item.id,
                content: item.content.clone(),
                status: item.status,
            });
        }

        let next_id = if items.is_empty() {
            1
        } else {
            max_id
                .checked_add(1)
                .ok_or_else(|| "To-do item IDs are exhausted".to_string())?
        };
        Ok(Self { items, next_id })
    }

    /// Add a new todo item.
    pub fn add(&mut self, content: String, status: TodoStatus) -> TodoItem {
        let status = match status {
            TodoStatus::InProgress => {
                self.set_single_in_progress(None);
                TodoStatus::InProgress
            }
            other => other,
        };

        let item = TodoItem {
            id: self.next_id,
            content,
            status,
        };
        self.next_id += 1;
        self.items.push(item.clone());
        item
    }

    /// Update an item's status by id.
    pub fn update_status(&mut self, id: u32, status: TodoStatus) -> Option<TodoItem> {
        let mut updated: Option<TodoItem> = None;
        if status == TodoStatus::InProgress {
            self.set_single_in_progress(Some(id));
        }
        for item in &mut self.items {
            if item.id == id {
                item.status = status;
                updated = Some(item.clone());
                break;
            }
        }
        updated
    }

    /// Compute completion percentage for the list.
    #[must_use]
    pub fn completion_percentage(&self) -> u8 {
        if self.items.is_empty() {
            return 0;
        }
        let total = self.items.len();
        let settled = self
            .items
            .iter()
            .filter(|item| item.status.is_settled())
            .count();
        let percent = settled.saturating_mul(100);
        let percent = (percent + total / 2) / total;
        u8::try_from(percent).unwrap_or(u8::MAX)
    }

    /// Return the id of the in-progress item, if any.
    #[must_use]
    pub fn in_progress_id(&self) -> Option<u32> {
        self.items
            .iter()
            .find(|item| item.status == TodoStatus::InProgress)
            .map(|item| item.id)
    }

    /// Clear all todo items.
    pub fn clear(&mut self) {
        self.items.clear();
        self.next_id = 1;
    }

    fn set_single_in_progress(&mut self, allow_id: Option<u32>) {
        for item in &mut self.items {
            if Some(item.id) != allow_id && item.status == TodoStatus::InProgress {
                item.status = TodoStatus::Pending;
            }
        }
    }
}

// === TodoWriteTool - ToolSpec implementation ===

/// Shared reference to a `TodoList` for use across tools
pub type SharedTodoList = Arc<Mutex<TodoList>>;

/// Create a new shared `TodoList`
pub fn new_shared_todo_list() -> SharedTodoList {
    Arc::new(Mutex::new(TodoList::new()))
}

const CANONICAL_WORK_SURFACE: &str = "work";
const CANONICAL_PROGRESS_TOOL: &str = "work_update";
const DURABLE_WORK_OWNER: &str = "fleet_workflow_ledger";

/// Tool for writing and updating the todo list
pub struct TodoWriteTool {
    todo_list: SharedTodoList,
    tool_name: &'static str,
}

impl TodoWriteTool {
    /// Canonical model-facing progress surface (#4132).
    pub fn work_update(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: CANONICAL_PROGRESS_TOOL,
        }
    }

    /// Legacy spelling kept for transcript replay and older prompts.
    pub fn checklist(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "checklist_write",
        }
    }

    /// Pre-checklist `todo_*` spelling kept for transcript replay.
    pub fn todo(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "todo_write",
        }
    }
}

/// Tool for adding a single todo item (legacy compatibility).
pub struct TodoAddTool {
    todo_list: SharedTodoList,
    tool_name: &'static str,
}

impl TodoAddTool {
    pub fn checklist(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "checklist_add",
        }
    }

    pub fn todo(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "todo_add",
        }
    }
}

#[async_trait]
impl ToolSpec for TodoAddTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn description(&self) -> &'static str {
        if self.tool_name == "todo_add" {
            "Compatibility alias for work_update/checklist_add. Adds one To-do item on the active thread/task."
        } else {
            "Compatibility alias for work_update. Adds one To-do item on the active thread/task. Prefer work_update to replace the full list."
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The task description"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                    "description": "Task status (default: pending)"
                }
            },
            "required": ["content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn model_visible(&self) -> bool {
        // Granular add stays callable for replay; models should use work_update.
        false
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("Missing 'content'"))?;
        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(TodoStatus::from_str)
            .unwrap_or(TodoStatus::Pending);

        let current = current_todo_snapshot(context, &self.todo_list).await?;
        let mut desired = TodoList::from_snapshot(&current).map_err(ToolError::execution_failed)?;
        let item = desired.add(content.to_string(), status);
        let snapshot =
            publish_todo_snapshot(context, &self.todo_list, self.tool_name, desired.snapshot())
                .await?;

        let result = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());
        Ok(ToolResult::success(format!(
            "Added todo #{} ({})\n{}",
            item.id,
            item.status.as_str(),
            result
        ))
        .with_metadata(work_progress_metadata(&snapshot, self.tool_name)))
    }
}

/// Tool for updating a todo item's status (legacy compatibility).
pub struct TodoUpdateTool {
    todo_list: SharedTodoList,
    tool_name: &'static str,
}

impl TodoUpdateTool {
    pub fn checklist(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "checklist_update",
        }
    }

    pub fn todo(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "todo_update",
        }
    }
}

#[async_trait]
impl ToolSpec for TodoUpdateTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn description(&self) -> &'static str {
        if self.tool_name == "todo_update" {
            "Compatibility alias for work_update/checklist_update. Updates one To-do item by id on the active thread/task."
        } else {
            "Compatibility alias for work_update. Updates one To-do item's status by id on the active thread/task."
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "Todo item id"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                    "description": "New status"
                }
            },
            "required": ["id", "status"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn model_visible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = input
            .get("id")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| ToolError::invalid_input("Missing or invalid 'id'"))?;
        let status = input
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(TodoStatus::from_str)
            .ok_or_else(|| ToolError::invalid_input("Missing or invalid 'status'"))?;

        let current = current_todo_snapshot(context, &self.todo_list).await?;
        let mut desired = TodoList::from_snapshot(&current).map_err(ToolError::execution_failed)?;
        let updated = desired.update_status(id, status);
        let snapshot = if updated.is_some() {
            publish_todo_snapshot(context, &self.todo_list, self.tool_name, desired.snapshot())
                .await?
        } else {
            current
        };
        let result = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());

        match updated {
            Some(item) => Ok(ToolResult::success(format!(
                "Updated todo #{} to {}\n{}",
                item.id,
                item.status.as_str(),
                result
            ))
            .with_metadata(work_progress_metadata(&snapshot, self.tool_name))),
            None => Ok(ToolResult::error(format!("Todo id {id} not found"))),
        }
    }
}

/// Tool for listing current todos (legacy compatibility).
pub struct TodoListTool {
    todo_list: SharedTodoList,
    tool_name: &'static str,
}

impl TodoListTool {
    pub fn checklist(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "checklist_list",
        }
    }

    pub fn todo(todo_list: SharedTodoList) -> Self {
        Self {
            todo_list,
            tool_name: "todo_list",
        }
    }
}

#[async_trait]
impl ToolSpec for TodoListTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn description(&self) -> &'static str {
        if self.tool_name == "todo_list" {
            "Compatibility alias for work_update/checklist_list. Lists current To-do progress."
        } else {
            "Compatibility alias for work_update. Lists current To-do progress for the active thread/task."
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn model_visible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = current_todo_snapshot(context, &self.todo_list).await?;
        let result = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());
        Ok(ToolResult::success(format!(
            "Todo list ({} items, {}% settled)\n{}",
            snapshot.items.len(),
            snapshot.completion_pct,
            result
        ))
        .with_metadata(work_progress_metadata(&snapshot, self.tool_name)))
    }
}

#[async_trait]
impl ToolSpec for TodoWriteTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn description(&self) -> &'static str {
        match self.tool_name {
            "todo_write" => {
                "Compatibility alias for work_update. Replace the active thread/task To-do list; durable tasks are the real executable work object."
            }
            "checklist_write" => {
                "Compatibility alias for work_update. Replace the active thread/task To-do list; durable tasks are the real executable work object."
            }
            _ => {
                "Replace the active thread/task To-do list (concrete current work items). This is the canonical progress surface the user watches, so keep it live while you work: mark an item in_progress before starting it (exactly one at a time), and call work_update again the moment an item finishes so it shows completed — never batch completions at the end. Durable tasks remain the real executable work object."
            }
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete list of To-do items. This replaces the existing list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "The task description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Task status"
                            }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn model_visible(&self) -> bool {
        // Only the canonical work_update spelling is advertised to models.
        self.tool_name == CANONICAL_PROGRESS_TOOL
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let todos = input
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::invalid_input("Missing or invalid 'todos' array"))?;

        let mut list = TodoList::new();

        for item in todos {
            let content = item
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::invalid_input("Todo item missing 'content'"))?;

            let status_str = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");

            let status = TodoStatus::from_str(status_str).unwrap_or(TodoStatus::Pending);

            list.add(content.to_string(), status);
        }

        let snapshot =
            publish_todo_snapshot(context, &self.todo_list, self.tool_name, list.snapshot())
                .await?;
        let result = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());

        Ok(ToolResult::success(format!(
            "Todo list updated ({} items, {}% settled)\n{}",
            snapshot.items.len(),
            snapshot.completion_pct,
            result
        ))
        .with_metadata(work_progress_metadata(&snapshot, self.tool_name)))
    }
}

fn is_compat_alias(tool_name: &str) -> bool {
    tool_name != CANONICAL_PROGRESS_TOOL
}

async fn current_todo_snapshot(
    context: &ToolContext,
    todo_list: &SharedTodoList,
) -> Result<TodoListSnapshot, ToolError> {
    if let Some(work) = context.runtime.work.as_ref()
        && work.matches_todos(todo_list)
    {
        return work
            .current_todos()
            .await
            .map_err(ToolError::execution_failed);
    }
    Ok(todo_list.lock().await.snapshot())
}

async fn publish_todo_snapshot(
    context: &ToolContext,
    todo_list: &SharedTodoList,
    tool_name: &str,
    desired: TodoListSnapshot,
) -> Result<TodoListSnapshot, ToolError> {
    if let Some(work) = context.runtime.work.as_ref()
        && work.matches_todos(todo_list)
    {
        return work
            .apply_todo_update(&context.state_namespace, tool_name, &desired)
            .await
            .map_err(ToolError::execution_failed);
    }
    *todo_list.lock().await =
        TodoList::from_snapshot(&desired).map_err(ToolError::execution_failed)?;
    Ok(desired)
}

fn work_progress_metadata(snapshot: &TodoListSnapshot, tool_name: &str) -> serde_json::Value {
    let items = snapshot
        .items
        .iter()
        .map(|item| {
            json!({
                "id": item.id,
                "content": item.content,
                "status": item.status.as_str(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "canonical_tool": CANONICAL_PROGRESS_TOOL,
        "invoked_as": tool_name,
        "compat_alias": is_compat_alias(tool_name),
        "work_surface": {
            "canonical": CANONICAL_WORK_SURFACE,
            "model_visible": tool_name == CANONICAL_PROGRESS_TOOL,
            "durable_owner": DURABLE_WORK_OWNER,
            "progress_key": "task_updates.checklist"
        },
        "task_updates": {
            "checklist": {
                "items": items,
                "completion_pct": snapshot.completion_pct,
                "in_progress_id": snapshot.in_progress_id,
                "updated_at": null
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn work_update_description_teaches_live_upkeep() {
        // 2026-07-23 user report: models wrote the list once and never
        // updated it while working. The canonical tool description must
        // carry the upkeep contract every provider sees.
        let tool = super::TodoWriteTool::work_update(super::new_shared_todo_list());
        let description = crate::tools::spec::ToolSpec::description(&tool);
        for phrase in [
            "keep it live while you work",
            "in_progress before starting it (exactly one at a time)",
            "the moment an item finishes",
            "never batch completions",
        ] {
            assert!(
                description.contains(phrase),
                "work_update description missing {phrase:?}: {description}"
            );
        }
    }

    use super::*;

    #[test]
    fn cancelled_is_a_terminal_round_trippable_todo_state() {
        assert_eq!(
            TodoStatus::from_str("cancelled"),
            Some(TodoStatus::Cancelled)
        );
        assert_eq!(
            TodoStatus::from_str("canceled"),
            Some(TodoStatus::Cancelled)
        );

        let mut list = TodoList::new();
        list.add("abandoned approach".to_string(), TodoStatus::Cancelled);
        let snapshot = list.snapshot();
        assert_eq!(snapshot.completion_pct, 100);
        assert_eq!(snapshot.in_progress_id, None);
        assert_eq!(snapshot.plain_text(), "[-] #1 abandoned approach");
        assert_eq!(
            serde_json::to_value(snapshot.items[0].status).expect("serialize"),
            serde_json::json!("cancelled")
        );

        let schema = TodoWriteTool::work_update(new_shared_todo_list()).input_schema();
        let statuses = &schema["properties"]["todos"]["items"]["properties"]["status"]["enum"];
        assert!(statuses.as_array().is_some_and(|values| {
            values
                .iter()
                .any(|value| value.as_str() == Some("cancelled"))
        }));
    }

    #[test]
    fn persisted_snapshot_restores_ids_status_and_recomputes_metrics() {
        let snapshot = TodoListSnapshot {
            items: vec![
                TodoItem {
                    id: 4,
                    content: " inspect ".to_string(),
                    status: TodoStatus::Completed,
                },
                TodoItem {
                    id: 9,
                    content: "patch".to_string(),
                    status: TodoStatus::InProgress,
                },
            ],
            completion_pct: 0,
            in_progress_id: None,
        };

        let mut restored = TodoList::from_snapshot(&snapshot).expect("restore");
        let restored_snapshot = restored.snapshot();
        assert_eq!(restored_snapshot.items[0].id, 4);
        assert_eq!(restored_snapshot.items[0].content, " inspect ");
        assert_eq!(restored_snapshot.items[1].id, 9);
        assert_eq!(restored_snapshot.completion_pct, 50);
        assert_eq!(restored_snapshot.in_progress_id, Some(9));
        assert_eq!(
            restored.add("verify".to_string(), TodoStatus::Pending).id,
            10
        );
    }

    #[test]
    fn malformed_persisted_snapshot_is_rejected_deterministically() {
        let duplicate = TodoListSnapshot {
            items: vec![
                TodoItem {
                    id: 1,
                    content: "one".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: 1,
                    content: "two".to_string(),
                    status: TodoStatus::Pending,
                },
            ],
            ..TodoListSnapshot::default()
        };
        assert_eq!(
            TodoList::from_snapshot(&duplicate).unwrap_err(),
            "Duplicate To-do item ID 1"
        );

        let multiple_active = TodoListSnapshot {
            items: vec![
                TodoItem {
                    id: 1,
                    content: "one".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: 2,
                    content: "two".to_string(),
                    status: TodoStatus::InProgress,
                },
            ],
            ..TodoListSnapshot::default()
        };
        assert_eq!(
            TodoList::from_snapshot(&multiple_active).unwrap_err(),
            "Only one To-do item may be in progress"
        );
    }

    #[tokio::test]
    async fn work_update_returns_canonical_task_update_metadata() {
        let tool = TodoWriteTool::work_update(new_shared_todo_list());
        let context = ToolContext::new(std::env::temp_dir());
        let result = tool
            .execute(
                json!({
                    "todos": [
                        { "content": "wire durable task tools", "status": "in_progress" },
                        { "content": "run gates", "status": "pending" }
                    ]
                }),
                &context,
            )
            .await
            .expect("work_update succeeds");

        assert!(tool.model_visible());
        let metadata = result.metadata.expect("metadata");
        assert_eq!(metadata["canonical_tool"], "work_update");
        assert_eq!(metadata["invoked_as"], "work_update");
        assert_eq!(metadata["compat_alias"], false);
        assert_eq!(metadata["work_surface"]["canonical"], "work");
        assert_eq!(metadata["work_surface"]["model_visible"], true);
        assert_eq!(
            metadata["work_surface"]["durable_owner"],
            "fleet_workflow_ledger"
        );
        assert_eq!(
            metadata["work_surface"]["progress_key"],
            "task_updates.checklist"
        );
        assert_eq!(
            metadata["task_updates"]["checklist"]["in_progress_id"],
            json!(1)
        );
        assert_eq!(
            metadata["task_updates"]["checklist"]["items"][0]["content"],
            "wire durable task tools"
        );
    }

    #[tokio::test]
    async fn work_update_and_hidden_alias_route_through_attached_graph() {
        let todos = new_shared_todo_list();
        let plan = crate::tools::plan::new_shared_plan_state();
        let work = crate::work_graph::new_shared_work_runtime(todos.clone(), plan);
        let mut context = ToolContext::new(std::env::temp_dir());
        context.runtime.work = Some(work.clone());

        TodoWriteTool::work_update(todos.clone())
            .execute(
                json!({"todos": [
                    {"content": "Graph-owned", "status": "in_progress"},
                    {"content": "Discarded branch", "status": "cancelled"}
                ]}),
                &context,
            )
            .await
            .expect("work_update");
        assert!(todos.lock().await.snapshot().is_empty());
        TodoUpdateTool::checklist(todos.clone())
            .execute(json!({"id": 1, "status": "completed"}), &context)
            .await
            .expect("hidden alias update");

        let state = work
            .capture(Some(&context.state_namespace))
            .expect("capture")
            .expect("graph state");
        assert_eq!(state.todos.items[0].status, TodoStatus::Completed);
        assert_eq!(state.todos.items[1].status, TodoStatus::Cancelled);
        assert_eq!(state.todos.completion_pct, 100);
        let node = state
            .graph
            .node(&state.graph.compat.todos[0].node)
            .expect("projected node");
        assert_eq!(node.state, crate::work_graph::NodeState::Completed);
        let cancelled_node = state
            .graph
            .node(&state.graph.compat.todos[1].node)
            .expect("cancelled projected node");
        assert_eq!(
            cancelled_node.state,
            crate::work_graph::NodeState::Cancelled
        );
        assert!(todos.lock().await.snapshot().is_empty());
        assert_eq!(work.publish_pending().await, Ok(true));
        assert_eq!(
            todos.lock().await.snapshot().items[0].status,
            TodoStatus::Completed
        );
        assert_eq!(
            todos.lock().await.snapshot().items[1].status,
            TodoStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn checklist_write_compat_alias_still_replays() {
        let tool = TodoWriteTool::checklist(new_shared_todo_list());
        let context = ToolContext::new(std::env::temp_dir());
        let result = tool
            .execute(
                json!({
                    "todos": [
                        { "content": "legacy checklist payload", "status": "completed" }
                    ]
                }),
                &context,
            )
            .await
            .expect("checklist_write compat succeeds");

        assert!(!tool.model_visible());
        let metadata = result.metadata.expect("metadata");
        assert_eq!(metadata["canonical_tool"], "work_update");
        assert_eq!(metadata["invoked_as"], "checklist_write");
        assert_eq!(metadata["compat_alias"], true);
        assert_eq!(metadata["work_surface"]["canonical"], "work");
        assert_eq!(metadata["work_surface"]["model_visible"], false);
        assert_eq!(
            metadata["task_updates"]["checklist"]["items"][0]["content"],
            "legacy checklist payload"
        );
    }

    #[tokio::test]
    async fn todo_write_compat_alias_still_replays() {
        let tool = TodoWriteTool::todo(new_shared_todo_list());
        let context = ToolContext::new(std::env::temp_dir());
        let result = tool
            .execute(
                json!({
                    "todos": [
                        { "content": "legacy todo payload", "status": "pending" }
                    ]
                }),
                &context,
            )
            .await
            .expect("todo_write compat succeeds");

        assert!(!tool.model_visible());
        let metadata = result.metadata.expect("metadata");
        assert_eq!(metadata["canonical_tool"], "work_update");
        assert_eq!(metadata["invoked_as"], "todo_write");
        assert_eq!(metadata["compat_alias"], true);
    }
}
