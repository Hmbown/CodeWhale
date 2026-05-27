use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use codewhale_protocol::{ToolKind, ToolOutput, ToolPayload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

tokio::task_local! {
    static TOOL_EXECUTION_LOCK_HELD: ();
}

/// Capabilities that a tool may have or require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCapability {
    /// Tool only reads data, never modifies state.
    ReadOnly,
    /// Tool writes to the filesystem.
    WritesFiles,
    /// Tool executes arbitrary shell commands.
    ExecutesCode,
    /// Tool makes network requests.
    Network,
    /// Tool can be run in a sandbox.
    Sandboxable,
    /// Tool requires user approval before execution.
    RequiresApproval,
}

/// Approval requirement for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalRequirement {
    /// Never needs approval: safe read-only operations.
    #[default]
    Auto,
    /// Suggest approval but allow user to skip.
    Suggest,
    /// Always require explicit user approval.
    Required,
}

/// Errors that can occur during tool execution.
#[derive(Debug, Clone)]
pub enum ToolError {
    InvalidInput { message: String },
    MissingField { field: String },
    PathEscape { path: PathBuf },
    ExecutionFailed { message: String },
    Timeout { seconds: u64 },
    NotAvailable { message: String },
    PermissionDenied { message: String },
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput { message } => {
                write!(f, "Failed to validate input: {message}")
            }
            Self::MissingField { field } => {
                write!(
                    f,
                    "Failed to validate input: missing required field '{field}'"
                )
            }
            Self::PathEscape { path } => {
                write!(
                    f,
                    "Failed to resolve path '{}': path escapes workspace",
                    path.display()
                )
            }
            Self::ExecutionFailed { message } => {
                write!(f, "Failed to execute tool: {message}")
            }
            Self::Timeout { seconds } => {
                write!(
                    f,
                    "Failed to execute tool: operation timed out after {seconds}s"
                )
            }
            Self::NotAvailable { message } => {
                write!(f, "Failed to locate tool: {message}")
            }
            Self::PermissionDenied { message } => {
                write!(f, "Failed to authorize tool execution: {message}")
            }
        }
    }
}

impl std::error::Error for ToolError {}

impl ToolError {
    #[must_use]
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn missing_field(field: impl Into<String>) -> Self {
        Self::MissingField {
            field: field.into(),
        }
    }

    #[must_use]
    pub fn execution_failed(msg: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn path_escape(path: impl Into<PathBuf>) -> Self {
        Self::PathEscape { path: path.into() }
    }

    #[must_use]
    pub fn not_available(msg: impl Into<String>) -> Self {
        Self::NotAvailable {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn permission_denied(msg: impl Into<String>) -> Self {
        Self::PermissionDenied {
            message: msg.into(),
        }
    }
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The output content, which may be JSON or plain text.
    pub content: String,
    /// Whether the execution was successful.
    pub success: bool,
    /// Optional structured metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    /// Create a successful result with content.
    #[must_use]
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            metadata: None,
        }
    }

    /// Create an error result with message.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            success: false,
            metadata: None,
        }
    }

    /// Create a successful result from JSON.
    pub fn json<T: Serialize>(value: &T) -> std::result::Result<Self, serde_json::Error> {
        Ok(Self {
            content: serde_json::to_string_pretty(value)?,
            success: true,
            metadata: None,
        })
    }

    /// Add metadata to the result.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Helper to extract a required string field from JSON input.
pub fn required_str<'a>(input: &'a Value, field: &str) -> std::result::Result<&'a str, ToolError> {
    input.get(field).and_then(Value::as_str).ok_or_else(|| {
        // When the field is missing, list the fields the caller *did*
        // supply so the model can spot the mismatch without a retry.
        let provided: Vec<&str> = input
            .as_object()
            .map(|obj| obj.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        if provided.is_empty() {
            ToolError::missing_field(field)
        } else {
            let hint = format!(
                "missing required field '{field}'. Input provided: {}",
                provided.join(", ")
            );
            ToolError::invalid_input(hint)
        }
    })
}

/// Helper to extract an optional string field from JSON input.
#[must_use]
pub fn optional_str<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input.get(field).and_then(Value::as_str)
}

/// Helper to extract a required u64 field from JSON input.
pub fn required_u64(input: &Value, field: &str) -> std::result::Result<u64, ToolError> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::missing_field(field))
}

/// Helper to extract an optional u64 field with default.
#[must_use]
pub fn optional_u64(input: &Value, field: &str, default: u64) -> u64 {
    input.get(field).and_then(Value::as_u64).unwrap_or(default)
}

/// Helper to extract an optional bool field with default.
#[must_use]
pub fn optional_bool(input: &Value, field: &str, default: bool) -> bool {
    input.get(field).and_then(Value::as_bool).unwrap_or(default)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub supports_parallel_tool_calls: bool,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredToolSpec {
    pub spec: ToolSpec,
    pub supports_parallel_tool_calls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallSource {
    Direct,
    JsRepl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub payload: ToolPayload,
    pub source: ToolCallSource,
    pub raw_tool_call_id: Option<String>,
}

impl ToolCall {
    pub fn execution_subject(&self, fallback_cwd: &str) -> (String, String, &'static str) {
        match &self.payload {
            ToolPayload::LocalShell { params } => (
                params.command.clone(),
                params
                    .cwd
                    .clone()
                    .unwrap_or_else(|| fallback_cwd.to_string()),
                "shell",
            ),
            _ => (self.name.clone(), fallback_cwd.to_string(), "tool"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub call_id: String,
    pub tool_name: String,
    pub payload: ToolPayload,
    pub source: ToolCallSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FunctionCallError {
    ToolNotFound { name: String },
    KindMismatch { expected: ToolKind, got: ToolKind },
    MutatingToolRejected { name: String },
    TimedOut { name: String, timeout_ms: u64 },
    Cancelled { name: String },
    ExecutionFailed { name: String, error: String },
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn kind(&self) -> ToolKind;
    fn matches_kind(&self, kind: ToolKind) -> bool {
        self.kind() == kind
    }
    fn is_mutating(&self) -> bool {
        false
    }
    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, FunctionCallError>;
}

#[derive(Debug)]
pub struct ToolCallRuntime {
    /// Preserve read/write tool execution semantics: parallel-safe tools may
    /// overlap, while serial tools run exclusively.
    execution_lock: Arc<RwLock<()>>,
}

impl Default for ToolCallRuntime {
    fn default() -> Self {
        Self {
            execution_lock: Arc::new(RwLock::new(())),
        }
    }
}

#[derive(Debug)]
enum ToolExecutionGuard {
    Parallel(#[allow(dead_code)] OwnedRwLockReadGuard<()>),
    Serial(#[allow(dead_code)] OwnedRwLockWriteGuard<()>),
    Reentrant,
}

impl ToolCallRuntime {
    async fn acquire(&self, supports_parallel: bool) -> ToolExecutionGuard {
        if TOOL_EXECUTION_LOCK_HELD.try_with(|_| ()).is_ok() {
            return ToolExecutionGuard::Reentrant;
        }

        if supports_parallel {
            ToolExecutionGuard::Parallel(self.execution_lock.clone().read_owned().await)
        } else {
            ToolExecutionGuard::Serial(self.execution_lock.clone().write_owned().await)
        }
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: HashMap<String, ConfiguredToolSpec>,
    runtime: ToolCallRuntime,
}

impl ToolRegistry {
    pub fn register(&mut self, spec: ToolSpec, handler: Arc<dyn ToolHandler>) -> Result<()> {
        let name = spec.name.clone();
        self.specs.insert(
            name.clone(),
            ConfiguredToolSpec {
                supports_parallel_tool_calls: spec.supports_parallel_tool_calls,
                spec,
            },
        );
        self.handlers.insert(name, handler);
        Ok(())
    }

    pub fn list_specs(&self) -> Vec<ConfiguredToolSpec> {
        self.specs.values().cloned().collect()
    }

    pub async fn dispatch(
        &self,
        call: ToolCall,
        allow_mutating: bool,
    ) -> std::result::Result<ToolOutput, FunctionCallError> {
        let handler = self.handlers.get(&call.name).cloned().ok_or_else(|| {
            FunctionCallError::ToolNotFound {
                name: call.name.clone(),
            }
        })?;
        let configured =
            self.specs
                .get(&call.name)
                .cloned()
                .ok_or_else(|| FunctionCallError::ToolNotFound {
                    name: call.name.clone(),
                })?;

        let payload_kind = tool_payload_kind(&call.payload);
        let expected = handler.kind();
        if !handler.matches_kind(payload_kind) {
            return Err(FunctionCallError::KindMismatch {
                expected,
                got: payload_kind,
            });
        }
        if handler.is_mutating() && !allow_mutating {
            return Err(FunctionCallError::MutatingToolRejected { name: call.name });
        }

        let invocation = ToolInvocation {
            call_id: call
                .raw_tool_call_id
                .clone()
                .unwrap_or_else(|| format!("tool-call-{}", uuid::Uuid::new_v4())),
            tool_name: call.name.clone(),
            payload: call.payload,
            source: call.source,
        };

        let _guard = self
            .runtime
            .acquire(configured.supports_parallel_tool_calls)
            .await;

        TOOL_EXECUTION_LOCK_HELD
            .scope(
                (),
                self.execute_with_timeout(handler, configured.spec.timeout_ms, invocation),
            )
            .await
    }

    async fn execute_with_timeout(
        &self,
        handler: Arc<dyn ToolHandler>,
        timeout_ms: Option<u64>,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, FunctionCallError> {
        if let Some(timeout_ms) = timeout_ms {
            let name = invocation.tool_name.clone();
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                handler.handle(invocation),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(FunctionCallError::TimedOut { name, timeout_ms }),
            }
        } else {
            handler.handle(invocation).await
        }
    }
}

fn tool_payload_kind(payload: &ToolPayload) -> ToolKind {
    match payload {
        ToolPayload::Mcp { .. } => ToolKind::Mcp,
        ToolPayload::Function { .. }
        | ToolPayload::Custom { .. }
        | ToolPayload::LocalShell { .. } => ToolKind::Function,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_result_json_round_trips_content() {
        let result = ToolResult::json(&json!({"ok": true})).expect("json");
        assert!(result.success);
        assert!(result.content.contains("\"ok\": true"));
    }

    #[test]
    fn helper_extractors_validate_shape() {
        let input = json!({"name": "demo", "count": 7, "enabled": true});
        assert_eq!(required_str(&input, "name").expect("name"), "demo");
        assert_eq!(optional_u64(&input, "count", 0), 7);
        assert!(optional_bool(&input, "enabled", false));
        assert!(matches!(
            required_u64(&input, "name"),
            Err(ToolError::MissingField { .. })
        ));
    }

    #[test]
    fn required_str_reports_provided_fields_on_missing_required_field() {
        let input = json!({"path": "src/lib.rs", "content": "new body"});
        let err = required_str(&input, "replace").expect_err("replace is missing");
        let message = err.to_string();
        assert!(message.contains("missing required field 'replace'"));
        assert!(message.contains("Input provided:"));
        assert!(message.contains("path"));
        assert!(message.contains("content"));
    }

    #[test]
    fn tool_error_display_matches_legacy_text() {
        let err = ToolError::missing_field("path");
        assert_eq!(
            err.to_string(),
            "Failed to validate input: missing required field 'path'"
        );
    }
}
