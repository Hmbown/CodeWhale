//! Tool wrapper for executing multiple tool calls in parallel.
//!
//! NOTE: this meta-tool is intentionally no longer registered with the
//! agent (see `ToolRegistryBuilder::with_parallel_tool`). DeepSeek-V4
//! supports native parallel `tool_calls` in a single assistant turn, and
//! advertising the OpenAI-internal name `multi_tool_use.parallel` made
//! the model hallucinate ChatGPT-style XML wrappers. The struct stays
//! around so the engine compatibility dispatcher and historical sessions
//! still resolve it cleanly.

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use async_trait::async_trait;
use serde_json::{Value, json};

#[allow(dead_code)] // Multi-tool parallel tool spec; reserved for Anthropic parallel tool use (see #3490)
pub struct MultiToolUseParallelTool;

#[async_trait]
impl ToolSpec for MultiToolUseParallelTool {
    fn name(&self) -> &'static str {
        "multi_tool_use.parallel"
    }

    fn description(&self) -> &'static str {
        "Execute multiple tool calls in parallel and return their results."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_uses": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "recipient_name": { "type": "string" },
                            "parameters": { "type": "object" }
                        },
                        "required": ["recipient_name", "parameters"]
                    }
                }
            },
            "required": ["tool_uses"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "multi_tool_use.parallel must be handled by the engine",
        ))
    }
}
