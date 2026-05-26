use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use codewhale_protocol::{ToolKind, ToolOutput, ToolPayload};
use codewhale_tools::{
    ToolCall, ToolCallSource, ToolHandler, ToolInvocation, ToolRegistry, ToolSpec,
};
use serde_json::json;
use tokio::sync::Notify;

struct EchoHandler;

#[async_trait]
impl ToolHandler for EchoHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn is_mutating(&self) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, codewhale_tools::FunctionCallError> {
        Ok(ToolOutput::Function {
            body: Some(json!({
                "tool": invocation.tool_name,
                "call_id": invocation.call_id
            })),
            success: true,
        })
    }
}

struct BlockingHandler {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ToolHandler for BlockingHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, codewhale_tools::FunctionCallError> {
        self.started.notify_waiters();
        self.release.notified().await;
        Ok(ToolOutput::Function {
            body: Some(json!({
                "tool": invocation.tool_name,
                "call_id": invocation.call_id
            })),
            success: true,
        })
    }
}

struct ReentrantHandler {
    registry: Arc<OnceLock<Arc<ToolRegistry>>>,
}

#[async_trait]
impl ToolHandler for ReentrantHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, codewhale_tools::FunctionCallError> {
        let registry = self.registry.get().expect("registry initialized").clone();
        registry
            .dispatch(
                ToolCall {
                    name: "inner".to_string(),
                    payload: ToolPayload::Function {
                        arguments: "{}".to_string(),
                    },
                    source: ToolCallSource::Direct,
                    raw_tool_call_id: Some("inner-call".to_string()),
                },
                true,
            )
            .await
    }
}

#[tokio::test]
async fn dispatches_function_tool_with_parallel_flag() {
    let mut registry = ToolRegistry::default();
    registry
        .register(
            ToolSpec {
                name: "echo".to_string(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":"object"}),
                supports_parallel_tool_calls: true,
                timeout_ms: Some(1000),
            },
            Arc::new(EchoHandler),
        )
        .expect("register tool");

    let output = registry
        .dispatch(
            ToolCall {
                name: "echo".to_string(),
                payload: ToolPayload::Function {
                    arguments: "{\"message\":\"hi\"}".to_string(),
                },
                source: ToolCallSource::Direct,
                raw_tool_call_id: Some("call-1".to_string()),
            },
            true,
        )
        .await
        .expect("dispatch tool");
    match output {
        ToolOutput::Function { success, .. } => assert!(success),
        other => panic!("unexpected output: {other:?}"),
    }
}

#[tokio::test]
async fn serial_tool_waits_for_running_parallel_tool() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut registry = ToolRegistry::default();
    registry
        .register(
            ToolSpec {
                name: "slow_read".to_string(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":"object"}),
                supports_parallel_tool_calls: true,
                timeout_ms: Some(1000),
            },
            Arc::new(BlockingHandler {
                started: started.clone(),
                release: release.clone(),
            }),
        )
        .expect("register slow read");
    registry
        .register(
            ToolSpec {
                name: "serial".to_string(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":"object"}),
                supports_parallel_tool_calls: false,
                timeout_ms: Some(1000),
            },
            Arc::new(EchoHandler),
        )
        .expect("register serial");

    let registry = Arc::new(registry);
    let started_wait = started.notified();
    let parallel_registry = registry.clone();
    let parallel = tokio::spawn(async move {
        parallel_registry
            .dispatch(
                ToolCall {
                    name: "slow_read".to_string(),
                    payload: ToolPayload::Function {
                        arguments: "{}".to_string(),
                    },
                    source: ToolCallSource::Direct,
                    raw_tool_call_id: Some("parallel-call".to_string()),
                },
                true,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), started_wait)
        .await
        .expect("parallel tool started");

    let serial_registry = registry.clone();
    let mut serial = tokio::spawn(async move {
        serial_registry
            .dispatch(
                ToolCall {
                    name: "serial".to_string(),
                    payload: ToolPayload::Function {
                        arguments: "{}".to_string(),
                    },
                    source: ToolCallSource::Direct,
                    raw_tool_call_id: Some("serial-call".to_string()),
                },
                true,
            )
            .await
    });

    tokio::select! {
        _ = &mut serial => panic!("serial tool overlapped a running parallel tool"),
        () = tokio::time::sleep(Duration::from_millis(50)) => {}
    }

    release.notify_waiters();
    serial
        .await
        .expect("serial task panicked")
        .expect("serial ran");
    parallel
        .await
        .expect("parallel task panicked")
        .expect("parallel ran");
}

#[tokio::test]
async fn serial_tool_can_reenter_registry_without_deadlock() {
    let registry_cell = Arc::new(OnceLock::new());
    let mut registry = ToolRegistry::default();
    registry
        .register(
            ToolSpec {
                name: "outer".to_string(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":"object"}),
                supports_parallel_tool_calls: false,
                timeout_ms: Some(1000),
            },
            Arc::new(ReentrantHandler {
                registry: registry_cell.clone(),
            }),
        )
        .expect("register outer");
    registry
        .register(
            ToolSpec {
                name: "inner".to_string(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({"type":"object"}),
                supports_parallel_tool_calls: false,
                timeout_ms: Some(1000),
            },
            Arc::new(EchoHandler),
        )
        .expect("register inner");

    let registry = Arc::new(registry);
    assert!(registry_cell.set(registry.clone()).is_ok());

    let output = tokio::time::timeout(
        Duration::from_secs(1),
        registry.dispatch(
            ToolCall {
                name: "outer".to_string(),
                payload: ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
                source: ToolCallSource::Direct,
                raw_tool_call_id: Some("outer-call".to_string()),
            },
            true,
        ),
    )
    .await
    .expect("outer dispatch timed out")
    .expect("outer dispatch failed");

    match output {
        ToolOutput::Function { success, .. } => assert!(success),
        other => panic!("unexpected output: {other:?}"),
    }
}
