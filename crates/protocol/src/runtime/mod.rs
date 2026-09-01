use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_API_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventEnvelope {
    #[serde(default = "default_runtime_event_envelope_schema_version")]
    pub schema_version: u32,
    pub seq: u64,
    pub event: String,
    pub kind: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    pub payload: Value,
    #[serde(default)]
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_runtime_event_envelope_schema_version() -> u32 {
    RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION
}

// ---------------------------------------------------------------------------
// Capability advertisement
// ---------------------------------------------------------------------------

/// Fixed capability map advertised by `GET /v1/runtime/info`.
///
/// All fields are required on serialization so clients can rely on the shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    #[serde(default)]
    pub account_session: bool,
    pub threads: bool,
    pub turns: bool,
    /// `POST /v1/threads/{id}/turns` accepts a durable, thread-scoped
    /// `operation_key` and returns the original turn for exact retries.
    #[serde(default)]
    pub turn_operation_idempotency: bool,
    pub turn_steer: bool,
    pub turn_interrupt: bool,
    pub event_replay: bool,
    pub external_tools: bool,
    pub environments: bool,
    pub worker_runtime: bool,
    #[serde(default)]
    pub fleet_run_create: bool,
    #[serde(default)]
    pub fleet_run_start: bool,
    #[serde(default)]
    pub fleet_event_replay: bool,
    #[serde(default)]
    pub fleet_event_stream: bool,
    #[serde(default)]
    pub fleet_local_target: bool,
    /// `GET/PUT/DELETE /v1/threads/{id}/goal` and the `complete`/`block`
    /// lifecycle actions are available.
    #[serde(default)]
    pub thread_goals: bool,
    /// `GET /v1/memory` and `GET /v1/memory/{id}` are available for
    /// bounded inspection of the native memory store.  `POST /v1/memory`
    /// and `DELETE /v1/memory` are also available (auth-gated via the
    /// standard route layer) for lifecycle controls.
    #[serde(default)]
    pub memory: bool,
    /// Whether the runtime supports create/update/enable/disable/reconnect/delete
    /// operations on MCP server configuration via the `POST|GET|PATCH|DELETE
    /// /v1/apps/mcp/servers` family of endpoints.
    #[serde(default)]
    pub mcp_server_management: bool,
    /// Skill lifecycle operations (install, update, uninstall, trust, audit)
    /// are available via the HTTP API.
    #[serde(default)]
    pub skill_lifecycle: bool,
    /// Durable, workspace-scoped cross-task Agent Mail endpoints and events.
    #[serde(default)]
    pub agent_mail: bool,
}

/// Experimental opt-in flags advertised by `GET /v1/runtime/info`.
///
/// Fields are additive and default to `false` when omitted by older servers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeExperimentalCapabilities {
    #[serde(default)]
    pub environments: bool,
}

// ---------------------------------------------------------------------------
// External Tool Bridge protocol types
// ---------------------------------------------------------------------------

/// Specification for a dynamic external tool registered by a runtime client.
///
/// Example JSON from the spec:
///
/// ```json
/// {
///   "namespace": "tau_bench",
///   "name": "get_reservation",
///   "description": "Look up an airline reservation.",
///   "input_schema": {
///     "type": "object",
///     "properties": {
///       "reservation_id": { "type": "string" }
///     },
///     "required": ["reservation_id"],
///     "additionalProperties": false
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolSpec {
    /// Optional namespace that groups related tools (e.g. `"tau_bench"`).
    /// When present, the runtime may expose the tool as
    /// `<namespace>::<name>` to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Short tool name. Combined with `namespace` it forms a unique tool id.
    pub name: String,

    /// Human-readable description exposed to the model.
    pub description: String,

    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Value,

    /// If true, the runtime may defer schema validation / tool loading until
    /// the model actually calls the tool.
    ///
    /// Defaults to `false` so that older clients omitting this field still
    /// behave the same way.
    #[serde(default)]
    pub defer_loading: bool,
}

/// Lifecycle status of a dynamic tool item shown in thread detail and event
/// payloads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DynamicToolItemStatus {
    InProgress,
    Completed,
    Failed,
}

/// Parameters identifying a dynamic tool call request emitted by the runtime.
///
/// This is the typed payload for `tool_call.requested` events and also the
/// natural identifier used when the runtime looks up a pending call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolCallParams {
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,

    /// Optional namespace that was registered with the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Tool name that the model invoked.
    pub tool: String,

    /// Arguments supplied by the model, validated against `input_schema`.
    pub arguments: Value,
}

/// Result submitted by a runtime client after executing a dynamic tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolCallResult {
    /// Whether the client-side tool execution succeeded.
    pub success: bool,

    /// Content fragments returned by the tool.
    ///
    /// Defaults to an empty vector when omitted so clients can send a minimal
    /// `{ "success": false }` payload.
    #[serde(default)]
    pub content: Vec<DynamicToolCallContent>,
}

/// A single content fragment inside a [`DynamicToolCallResult`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DynamicToolCallContent {
    InputText { text: String },
    InputImage { image_url: String },
}

// ---------------------------------------------------------------------------
// Environment targeting protocol types
// ---------------------------------------------------------------------------

/// Environment target selected for a turn's shell/filesystem work.
///
/// Example JSON:
///
/// ```json
/// {
///   "environment_id": "local",
///   "cwd": "/workspace"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnEnvironmentParams {
    pub environment_id: String,
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dynamic_tool_spec_roundtrip() {
        let spec = DynamicToolSpec {
            namespace: Some("tau_bench".into()),
            name: "get_reservation".into(),
            description: "Look up an airline reservation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "reservation_id": { "type": "string" }
                },
                "required": ["reservation_id"],
                "additionalProperties": false
            }),
            defer_loading: false,
        };

        let serialized = serde_json::to_string(&spec).unwrap();
        let deserialized: DynamicToolSpec = serde_json::from_str(&serialized).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn dynamic_tool_spec_omits_defer_loading_defaults_false() {
        let json = r#"{
            "namespace": "tau_bench",
            "name": "get_reservation",
            "description": "Look up an airline reservation.",
            "input_schema": { "type": "object" }
        }"#;

        let spec: DynamicToolSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.namespace, Some("tau_bench".into()));
        assert_eq!(spec.name, "get_reservation");
        assert!(!spec.defer_loading);
    }

    #[test]
    fn dynamic_tool_item_status_snake_case() {
        assert_eq!(
            serde_json::to_string(&DynamicToolItemStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::from_str::<DynamicToolItemStatus>("\"completed\"").unwrap(),
            DynamicToolItemStatus::Completed
        );
        assert_eq!(
            serde_json::from_str::<DynamicToolItemStatus>("\"failed\"").unwrap(),
            DynamicToolItemStatus::Failed
        );
    }

    #[test]
    fn dynamic_tool_call_params_roundtrip() {
        let params = DynamicToolCallParams {
            thread_id: "thr_123".into(),
            turn_id: "turn_456".into(),
            call_id: "call_abc".into(),
            namespace: Some("tau_bench".into()),
            tool: "get_reservation".into(),
            arguments: json!({ "reservation_id": "ABC123" }),
        };

        let serialized = serde_json::to_string(&params).unwrap();
        let deserialized: DynamicToolCallParams = serde_json::from_str(&serialized).unwrap();
        assert_eq!(params, deserialized);
    }

    #[test]
    fn dynamic_tool_call_content_roundtrip() {
        let content = vec![
            DynamicToolCallContent::InputText {
                text: "{\"status\":\"confirmed\"}".into(),
            },
            DynamicToolCallContent::InputImage {
                image_url: "http://example.com/receipt.png".into(),
            },
        ];

        let value = serde_json::to_value(&content).unwrap();
        let deserialized: Vec<DynamicToolCallContent> = serde_json::from_value(value).unwrap();
        assert_eq!(content, deserialized);

        // Verify the exact JSON tag names expected by the spec.
        assert_eq!(
            serde_json::to_string(&DynamicToolCallContent::InputText { text: "x".into() }).unwrap(),
            r#"{"type":"input_text","text":"x"}"#
        );
        assert_eq!(
            serde_json::to_string(&DynamicToolCallContent::InputImage {
                image_url: "y".into()
            })
            .unwrap(),
            r#"{"type":"input_image","image_url":"y"}"#
        );
    }

    #[test]
    fn dynamic_tool_call_result_defaults_empty_content() {
        let json = r#"{ "success": false }"#;
        let result: DynamicToolCallResult = serde_json::from_str(json).unwrap();
        assert!(!result.success);
        assert!(result.content.is_empty());
    }

    #[test]
    fn dynamic_tool_call_result_roundtrip_with_content() {
        let result = DynamicToolCallResult {
            success: true,
            content: vec![DynamicToolCallContent::InputText {
                text: "done".into(),
            }],
        };

        let serialized = serde_json::to_string(&result).unwrap();
        let deserialized: DynamicToolCallResult = serde_json::from_str(&serialized).unwrap();
        assert_eq!(result, deserialized);
    }

    #[test]
    fn turn_environment_params_roundtrip() {
        let env = TurnEnvironmentParams {
            environment_id: "local".into(),
            cwd: PathBuf::from("/workspace"),
        };

        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: TurnEnvironmentParams = serde_json::from_str(&serialized).unwrap();
        assert_eq!(env, deserialized);

        // Verify JSON from the spec deserializes directly.
        let from_spec = r#"{
            "environment_id": "local",
            "cwd": "/workspace"
        }"#;
        let parsed: TurnEnvironmentParams = serde_json::from_str(from_spec).unwrap();
        assert_eq!(parsed.environment_id, "local");
        assert_eq!(parsed.cwd, PathBuf::from("/workspace"));
    }

    #[test]
    fn runtime_capabilities_serializes_expected_shape() {
        let caps = RuntimeCapabilities {
            account_session: true,
            threads: true,
            turns: true,
            turn_operation_idempotency: true,
            turn_steer: true,
            turn_interrupt: true,
            event_replay: true,
            external_tools: false,
            environments: false,
            worker_runtime: false,
            fleet_run_create: true,
            fleet_run_start: true,
            fleet_event_replay: true,
            fleet_event_stream: true,
            fleet_local_target: true,
            thread_goals: true,
            memory: true,
            mcp_server_management: false,
            skill_lifecycle: false,
            agent_mail: true,
        };
        let value = serde_json::to_value(&caps).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.get("threads").unwrap(), &json!(true));
        assert_eq!(obj.get("account_session").unwrap(), &json!(true));
        assert_eq!(obj.get("turn_operation_idempotency").unwrap(), &json!(true));
        assert_eq!(obj.get("external_tools").unwrap(), &json!(false));
        assert!(obj.contains_key("worker_runtime"));
        assert_eq!(obj.get("fleet_run_create").unwrap(), &json!(true));
        assert_eq!(obj.get("fleet_event_stream").unwrap(), &json!(true));
        assert_eq!(obj.get("thread_goals").unwrap(), &json!(true));
        assert_eq!(obj.get("memory").unwrap(), &json!(true));
        assert_eq!(obj.get("agent_mail").unwrap(), &json!(true));
    }

    #[test]
    fn runtime_event_envelope_schema_version_default() {
        let json = r#"{
            "seq": 1,
            "event": "test",
            "kind": "test",
            "thread_id": "thr_1",
            "timestamp": "2026-06-12T00:00:00Z",
            "payload": {}
        }"#;
        let envelope: RuntimeEventEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(
            envelope.schema_version,
            RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION
        );
    }
}
