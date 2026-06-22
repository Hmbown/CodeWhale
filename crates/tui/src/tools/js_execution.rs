//! `js_execution` tool — execute model-provided JavaScript via a local
//! Node.js runtime, returning stdout / stderr / exit code as JSON.
//!
//! Mirrors the shape of `code_execution` (Python) so the model sees a
//! single consistent surface for "run this snippet locally and tell me
//! what it printed." The split into a dedicated module (rather than
//! living inline in `core::engine::tool_catalog` next to
//! `execute_code_execution_tool`) keeps the dependency-probe and
//! tempfile-spawn logic isolated for the test pin.
//!
//! Registration is gated by [`crate::dependencies::resolve_node`]:
//! when Node is missing the tool is simply not advertised, so the
//! model never sees a runtime it can't actually use. See
//! `core::engine::tool_catalog::ensure_advanced_tooling` for the
//! catalog-side dispatch.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::dependencies::ExternalTool;
use serde_json::{Value, json};

use crate::models::Tool;
use crate::tools::spec::{ToolError, ToolResult, required_str};

/// Tool name surfaced to the model. Held alongside `code_execution`
/// in the deferred-tool dispatcher.
pub const JS_EXECUTION_TOOL_NAME: &str = "js_execution";
/// Tool-type tag — uses the same `code_execution_*` family the
/// Anthropic message API expects so the wire shape stays stable
/// across the two interpreters.
const JS_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
const NODE_USE_ENV_PROXY: &str = "NODE_USE_ENV_PROXY";
const NODE_PROXY_PAIRS: &[(&str, &str)] =
    &[("HTTP_PROXY", "http_proxy"), ("HTTPS_PROXY", "https_proxy")];

fn first_non_empty_env_from(
    keys: &[&str],
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<OsString> {
    keys.iter()
        .filter_map(|key| env(key))
        .find(|value| !value.is_empty())
}

fn node_proxy_env_overrides_from(
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    let all_proxy = first_non_empty_env_from(&["ALL_PROXY", "all_proxy"], &env);
    let proxy_configured = all_proxy.is_some()
        || NODE_PROXY_PAIRS
            .iter()
            .any(|(upper, lower)| first_non_empty_env_from(&[upper, lower], &env).is_some());

    let mut overrides = Vec::new();
    if proxy_configured && first_non_empty_env_from(&[NODE_USE_ENV_PROXY], &env).is_none() {
        overrides.push((NODE_USE_ENV_PROXY, OsString::from("1")));
    }

    for (upper, lower) in NODE_PROXY_PAIRS {
        if first_non_empty_env_from(&[upper], &env).is_none()
            && let Some(value) =
                first_non_empty_env_from(&[lower], &env).or_else(|| all_proxy.clone())
        {
            overrides.push((*upper, value));
        }
    }

    if first_non_empty_env_from(&["NO_PROXY"], &env).is_none()
        && let Some(value) = first_non_empty_env_from(&["no_proxy"], &env)
    {
        overrides.push(("NO_PROXY", value));
    }

    overrides
}

fn node_proxy_env_overrides() -> Vec<(&'static str, OsString)> {
    node_proxy_env_overrides_from(|key| std::env::var_os(key))
}

fn apply_node_proxy_env(cmd: &mut tokio::process::Command) {
    for (key, value) in node_proxy_env_overrides() {
        cmd.env(key, value);
    }
}

/// Build the `Tool` definition the catalog should advertise when
/// Node.js is present on the host. Kept as a constructor (rather
/// than a `static`) so the input schema can stay declarative
/// without a `lazy_static!`-style indirection.
#[must_use]
pub fn js_execution_tool_definition() -> Tool {
    Tool {
        tool_type: Some(JS_EXECUTION_TOOL_TYPE.to_string()),
        name: JS_EXECUTION_TOOL_NAME.to_string(),
        description:
            "Execute JavaScript code in a local sandboxed Node.js runtime and return stdout/stderr/return_code as JSON."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "JavaScript source code to execute." }
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

/// Run the model-provided JavaScript and return the captured
/// stdout / stderr / return_code payload. Mirrors
/// `execute_code_execution_tool` exactly — same tempfile pattern,
/// same 120-second timeout, same error shape — so the surfaces
/// stay interchangeable from the model's point of view.
///
/// Tempfile lives only for the duration of this execution; `Drop`
/// removes it. We use the `.js` extension so any source-map /
/// shebang / encoding-sniffer logic in the interpreter behaves
/// normally.
pub async fn execute_js_execution_tool(
    input: &Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // Resolve the Node runtime via ExternalTool. If it's absent now
    // tokio_command() returns None and we fail fast with a clear message.

    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("js_execution.js");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = crate::dependencies::Node::tokio_command().ok_or_else(|| {
        ToolError::execution_failed("js_execution: Node.js runtime became unavailable".to_string())
    })?;
    // Recent Node releases use this startup env to make fetch/http(s) honor
    // standard proxy variables; older runtimes ignore it and keep prior behavior.
    apply_node_proxy_env(&mut cmd);
    cmd.arg(&script_path).current_dir(workspace);

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
    use std::ffi::OsString;
    use tempfile::tempdir;

    /// Skip helper — `js_execution` is a no-op on hosts without Node.
    /// The tool simply isn't advertised in that case, so happy-path
    /// tests don't fail; they just don't exercise the spawn path.
    fn node_present() -> bool {
        crate::dependencies::resolve_node().is_some()
    }

    fn proxy_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find_map(|(name, value)| (*name == key).then(|| OsString::from(value)))
        }
    }

    #[test]
    fn tool_definition_advertises_js_execution_name_and_required_code_field() {
        let tool = js_execution_tool_definition();
        assert_eq!(tool.name, JS_EXECUTION_TOOL_NAME);
        assert_eq!(tool.tool_type.as_deref(), Some(JS_EXECUTION_TOOL_TYPE));
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

    #[test]
    fn node_proxy_overrides_enable_env_proxy_when_proxy_env_is_present() {
        let overrides =
            node_proxy_env_overrides_from(proxy_env(&[("HTTPS_PROXY", "http://127.0.0.1:20499")]));

        assert_eq!(
            overrides,
            vec![(NODE_USE_ENV_PROXY, OsString::from("1"))],
            "uppercase proxy vars are inherited by the child; only Node's env-proxy flag is needed"
        );
    }

    #[test]
    fn node_proxy_overrides_mirror_lowercase_proxy_vars() {
        let overrides = node_proxy_env_overrides_from(proxy_env(&[
            ("https_proxy", "http://127.0.0.1:20499"),
            ("no_proxy", "localhost"),
        ]));

        assert_eq!(
            overrides,
            vec![
                (NODE_USE_ENV_PROXY, OsString::from("1")),
                ("HTTPS_PROXY", OsString::from("http://127.0.0.1:20499")),
                ("NO_PROXY", OsString::from("localhost")),
            ]
        );
    }

    #[tokio::test]
    async fn execute_js_runs_node_and_returns_stdout_payload() {
        if !node_present() {
            // Catalog-build skips the tool entirely on hosts without
            // Node — match that behaviour in the test rather than
            // failing the suite for users without Node installed.
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "process.stdout.write('hello from node')" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(result.success, "successful node run must report success");
        assert!(
            result.content.contains("hello from node"),
            "stdout payload must surface the printed text; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_surfaces_runtime_error_with_nonzero_exit() {
        if !node_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "throw new Error('intentional fail')" }),
            tmp.path(),
        )
        .await
        .expect("execute should not Err — runtime errors land in stderr/exit code");
        assert!(
            !result.success,
            "non-zero exit must report success=false in the result payload"
        );
        assert!(
            result.content.contains("intentional fail"),
            "stderr payload must surface the error message; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_rejects_input_without_code_field() {
        let tmp = tempdir().expect("tempdir");
        let err = execute_js_execution_tool(&json!({}), tmp.path())
            .await
            .expect_err("missing `code` must reject before any node spawn");
        let msg = err.to_string();
        assert!(
            msg.contains("code"),
            "error must name the missing `code` field; got {msg}"
        );
    }
}
