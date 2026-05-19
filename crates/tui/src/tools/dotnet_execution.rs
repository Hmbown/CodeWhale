//! `dotnet_execution` tool — execute model-provided C# code via a local
//! .NET SDK, returning stdout / stderr / exit code as JSON.
//!
//! Starting with .NET 6, `dotnet run file.cs` can run a single C# file
//! as a top-level-statements script — no project, no Main(), no class
//! wrapper needed. This tool writes the model-provided code to a temp
//! `.cs` file and spawns `dotnet run` against it, mirroring the shape
//! of `code_execution` (Python) and `js_execution` (Node).
//!
//! Registration is gated by [`crate::dependencies::DotNet::resolve`]:
//! when the .NET SDK is missing the tool is simply not advertised.

use std::path::Path;
use std::time::Duration;

use crate::dependencies::ExternalTool;
use serde_json::{Value, json};

use crate::models::Tool;
use crate::tools::spec::{ToolError, ToolResult, required_str};

/// Tool name surfaced to the model.
pub const DOTNET_EXECUTION_TOOL_NAME: &str = "dotnet_execution";
/// Tool-type tag — same `code_execution_*` family as Python/Node so
/// the wire shape stays stable across interpreters.
const DOTNET_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";

/// Build the `Tool` definition the catalog should advertise when
/// the .NET SDK is present on the host.
#[must_use]
pub fn dotnet_execution_tool_definition() -> Tool {
    Tool {
        tool_type: Some(DOTNET_EXECUTION_TOOL_TYPE.to_string()),
        name: DOTNET_EXECUTION_TOOL_NAME.to_string(),
        description:
            "Execute C# code in a local .NET SDK sandbox and return stdout/stderr/return_code as JSON. \
             Requires `dotnet` (NET 6+ SDK) on PATH. Code runs as a single-file top-level-statements script — \
             no project or Main() wrapper needed."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "C# source code to execute. Use top-level statements (no class or Main needed)." }
            },
            "required": ["code"]
        }),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

/// Run the model-provided C# code and return the captured
/// stdout / stderr / return_code payload.
///
/// Writes code to a temp `.cs` file, spawns `dotnet run <file>`,
/// and collects the output. 120-second timeout, same error shape
/// as `code_execution` and `js_execution`.
pub async fn execute_dotnet_execution_tool(
    input: &Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // Resolve the .NET SDK via ExternalTool. If absent, fail fast.
    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("dotnet_execution.cs");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = crate::dependencies::DotNet::tokio_command().ok_or_else(|| {
        ToolError::execution_failed(
            "dotnet_execution: .NET SDK became unavailable".to_string()
        )
    })?;
    // `dotnet run file.cs` works from .NET 6 onwards.
    // The current_dir must be the temp_dir so `dotnet run` can
    // create its temporary project artefacts there.
    cmd.arg("run")
        .arg(&script_path)
        .current_dir(temp_dir.path());

    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| ToolError::Timeout { seconds: 120 })
        .and_then(|res| res.map_err(|e| ToolError::execution_failed(e.to_string())))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Skip helper — `dotnet_execution` is a no-op on hosts without .NET SDK.
    fn dotnet_present() -> bool {
        crate::dependencies::DotNet::available()
    }

    #[test]
    fn tool_definition_advertises_dotnet_execution_name_and_required_code_field() {
        let tool = dotnet_execution_tool_definition();
        assert_eq!(tool.name, DOTNET_EXECUTION_TOOL_NAME);
        assert_eq!(tool.tool_type.as_deref(), Some(DOTNET_EXECUTION_TOOL_TYPE));
        let required = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("schema must declare a `required` array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("code")),
            "input_schema must require `code`",
        );
    }

    #[tokio::test]
    async fn execute_dotnet_runs_and_returns_stdout_payload() {
        if !dotnet_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_dotnet_execution_tool(
            &json!({ "code": "System.Console.WriteLine(\"hello from dotnet\");" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(result.success, "successful dotnet run must report success");
        assert!(
            result.content.contains("hello from dotnet"),
            "stdout payload must surface the printed text; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_dotnet_surfaces_runtime_error_with_nonzero_exit() {
        if !dotnet_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_dotnet_execution_tool(
            &json!({ "code": "throw new System.Exception(\"intentional fail\");" }),
            tmp.path(),
        )
        .await
        .expect("execute should not Err — runtime errors land in stderr/exit code");
        assert!(
            !result.success,
            "non-zero exit must report success=false"
        );
        assert!(
            result.content.contains("intentional fail"),
            "stderr payload must surface the error message; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_dotnet_rejects_input_without_code_field() {
        let tmp = tempdir().expect("tempdir");
        let err = execute_dotnet_execution_tool(&json!({}), tmp.path())
            .await
            .expect_err("missing `code` must reject before any dotnet spawn");
        let msg = err.to_string();
        assert!(
            msg.contains("code"),
            "error must name the missing `code` field; got {msg}"
        );
    }
}
