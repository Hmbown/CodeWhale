//! Persistent RLM session tools.
//!
//! v0.8.33 replaces the old one-shot `rlm` tool with a head/hands surface:
//! `rlm_open` creates a named Python kernel over a large context,
//! `rlm_eval` runs bounded probes against it, `rlm_configure` adjusts runtime
//! feedback, and `rlm_close` tears it down.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::client::DeepSeekClient;
use crate::repl::PythonRuntime;
use crate::rlm::RlmBridge;
use crate::rlm::session::{
    ContextMeta, OutputFeedback, RlmSession, derive_session_name, write_context_file,
};
use crate::tools::fetch_url::FetchUrlTool;
use crate::tools::handle::VarHandle;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

const DEFAULT_CHILD_MODEL: &str = "deepseek-v4-flash";
const MAX_INLINE_CONTENT_CHARS: usize = 200_000;
const FULL_STDOUT_HEAD_CHARS: usize = 4_096;
const FULL_STDOUT_TAIL_CHARS: usize = 1_024;
const HARD_SUB_RLM_DEPTH_CAP: u32 = 3;

pub struct RlmOpenTool;

#[async_trait]
impl ToolSpec for RlmOpenTool {
    fn name(&self) -> &'static str {
        "rlm_open"
    }

    fn description(&self) -> &'static str {
        "Open a persistent RLM context. Loads `file_path`, `content`, or `url` \
         into a named Python kernel and returns only metadata: name, length, \
         preview, and sha256. Use this for large or unfamiliar inputs so the \
         parent transcript holds a handle, not the body."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Caller-chosen context name, unique within this parent session. Defaults to a slug from the source."
                },
                "file_path": {
                    "type": "string",
                    "description": "Workspace-relative file to load."
                },
                "content": {
                    "type": "string",
                    "description": "Inline content to load. Capped at 200k chars."
                },
                "url": {
                    "type": "string",
                    "description": "HTTP/HTTPS URL to fetch through fetch_url and load."
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ReadOnly,
            ToolCapability::Network,
            ToolCapability::ExecutesCode,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let source_count = rlm_open_source_count(&input);
        if source_count != 1 {
            return Err(ToolError::invalid_input(
                "rlm_open: provide exactly one of `file_path`, `content`, or `url`",
            ));
        }

        let (body, source_type, source_hint) = load_source(&input, context).await?;
        if body.trim().is_empty() {
            return Err(ToolError::invalid_input(
                "rlm_open: input is empty after loading",
            ));
        }

        let name = input
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| derive_session_name(source_hint.as_deref()));

        {
            let sessions = context.runtime.rlm_sessions.lock().await;
            if sessions.contains_key(&name) {
                return Err(ToolError::invalid_input(format!(
                    "rlm_open: context name `{name}` already exists"
                )));
            }
        }

        let context_path = write_context_file(&body).map_err(|e| {
            ToolError::execution_failed(format!("rlm_open: failed to stage context: {e}"))
        })?;
        let kernel = PythonRuntime::spawn_with_context(&context_path)
            .await
            .map_err(|e| ToolError::execution_failed(format!("rlm_open: {e}")))?;
        let context_meta = ContextMeta::from_body(&body, source_type);
        let session = RlmSession::new(name.clone(), kernel, context_meta.clone(), context_path);
        let id = session.id.clone();

        let mut sessions = context.runtime.rlm_sessions.lock().await;
        sessions.insert(name.clone(), Arc::new(tokio::sync::Mutex::new(session)));

        ToolResult::json(&json!({
            "name": name,
            "id": id,
            "length": context_meta.length,
            "type": context_meta.type_name,
            "preview_500": context_meta.preview_500,
            "sha256": context_meta.sha256,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

pub struct RlmEvalTool {
    client: Option<DeepSeekClient>,
}

impl RlmEvalTool {
    #[must_use]
    pub fn new(client: Option<DeepSeekClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolSpec for RlmEvalTool {
    fn name(&self) -> &'static str {
        "rlm_eval"
    }

    fn description(&self) -> &'static str {
        "Run one Python REPL block against a named RLM context. Returns a \
         bounded projection of stdout/stderr plus metadata. If the code calls \
         FINAL/finalize, the final value is stored as a var_handle retrievable \
         with handle_read instead of copied unbounded into the parent context. \
         Batch child helpers require dependency_mode='independent'; use \
         sub_query_sequence or a sequential loop for dependent work."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name", "code"],
            "properties": {
                "name": { "type": "string", "description": "RLM context name from rlm_open." },
                "code": { "type": "string", "description": "Python code to execute. Do not include markdown fences." }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network, ToolCapability::ExecutesCode]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(&input, "name")?;
        let code = required_non_empty_str(&input, "code")?;
        let session = get_session(context, name).await?;
        let mut session = session.lock().await;
        let config = session.config.clone();

        let Some(kernel) = session.kernel.as_mut() else {
            return Err(ToolError::invalid_input(format!(
                "rlm_eval: context `{name}` is closed"
            )));
        };

        let started = Instant::now();
        let (round, child_usage) = if let Some(client) = self.client.clone() {
            let bridge = RlmBridge::new(
                Arc::new(client),
                DEFAULT_CHILD_MODEL.to_string(),
                config.sub_rlm_max_depth.min(HARD_SUB_RLM_DEPTH_CAP),
            );
            let usage_handle = bridge.usage_handle();
            let round = kernel
                .run(code, Some(&bridge))
                .await
                .map_err(|e| ToolError::execution_failed(format!("rlm_eval: {e}")))?;
            let usage = usage_handle.lock().await.clone();
            (round, usage)
        } else {
            let round = kernel
                .run(code, None::<&RlmBridge>)
                .await
                .map_err(|e| ToolError::execution_failed(format!("rlm_eval: {e}")))?;
            (round, Default::default())
        };

        session.rpc_count = session.rpc_count.saturating_add(round.rpc_count);
        session.total_duration += round.elapsed;
        session.last_used_at = Instant::now();

        let final_handle = if let Some(value_json) = round.final_json.clone() {
            session.final_count = session.final_count.saturating_add(1);
            let handle_name = format!("final_{}", session.final_count);
            let handle = {
                let mut store = context.runtime.handle_store.lock().await;
                match value_json {
                    Value::String(value) => {
                        store.insert_text(session.id.clone(), handle_name, value)
                    }
                    other => store.insert_json(session.id.clone(), handle_name, other),
                }
            };
            Some(handle)
        } else {
            None
        };

        let had_error = round.has_error;
        let rpc_count = round.rpc_count;
        let duration_ms = round.elapsed.as_millis() as u64;
        let stdout_preview = match config.output_feedback {
            OutputFeedback::Full => Some(preview_output(&round.full_stdout)),
            OutputFeedback::Metadata => None,
        };
        let stderr_preview = match config.output_feedback {
            OutputFeedback::Full if !round.stderr.is_empty() => Some(preview_output(&round.stderr)),
            _ => None,
        };

        let mut output = json!({
            "name": session.name,
            "id": session.id,
            "duration_ms": duration_ms,
            "rpc_count": rpc_count,
            "had_error": had_error,
            "new_vars": [],
            "final": final_handle,
        });
        if let Some(stdout_preview) = stdout_preview {
            output["stdout_preview"] = json!(stdout_preview);
        }
        if let Some(stderr_preview) = stderr_preview {
            output["stderr_preview"] = json!(stderr_preview);
        }
        if let Some(confidence) = round.final_confidence.clone() {
            output["confidence"] = confidence;
        }

        let metadata = json!({
            "tool": "rlm_eval",
            "duration_ms": started.elapsed().as_millis() as u64,
            "child_input_tokens": child_usage.input_tokens,
            "child_output_tokens": child_usage.output_tokens,
            "child_prompt_cache_hit_tokens": child_usage.prompt_cache_hit_tokens,
            "child_prompt_cache_miss_tokens": child_usage.prompt_cache_miss_tokens,
            "child_model": DEFAULT_CHILD_MODEL,
        });

        Ok(ToolResult::json(&output)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?
            .with_metadata(metadata))
    }
}

pub struct RlmConfigureTool;

#[async_trait]
impl ToolSpec for RlmConfigureTool {
    fn name(&self) -> &'static str {
        "rlm_configure"
    }

    fn description(&self) -> &'static str {
        "Configure a named RLM context: output feedback, child query timeout, \
         recursive sub-RLM depth, and explicit session sharing."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" },
                "output_feedback": { "type": "string", "enum": ["full", "metadata"] },
                "sub_query_timeout_secs": { "type": "integer" },
                "sub_rlm_max_depth": { "type": "integer", "minimum": 0, "maximum": 3 },
                "share_session": { "type": "boolean" }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(&input, "name")?;
        let session = get_session(context, name).await?;
        let mut session = session.lock().await;

        if let Some(value) = input.get("output_feedback").and_then(Value::as_str) {
            session.config.output_feedback = match value {
                "full" => OutputFeedback::Full,
                "metadata" => OutputFeedback::Metadata,
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "rlm_configure: invalid output_feedback `{other}`"
                    )));
                }
            };
        }
        if let Some(timeout) = input.get("sub_query_timeout_secs").and_then(Value::as_u64) {
            session.config.sub_query_timeout_secs = timeout.clamp(1, 600);
        }
        if let Some(depth) = input.get("sub_rlm_max_depth").and_then(Value::as_u64) {
            session.config.sub_rlm_max_depth = (depth as u32).min(HARD_SUB_RLM_DEPTH_CAP);
        }
        if let Some(share) = input.get("share_session").and_then(Value::as_bool) {
            session.config.share_session = share;
        }

        ToolResult::json(&json!({
            "name": session.name,
            "current_config": session.config,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

pub struct RlmCloseTool;

#[async_trait]
impl ToolSpec for RlmCloseTool {
    fn name(&self) -> &'static str {
        "rlm_close"
    }

    fn description(&self) -> &'static str {
        "Close a named RLM context, tear down its Python kernel, and return \
         usage/lifecycle metadata."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "RLM context name from rlm_open." }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(&input, "name")?;
        let removed = {
            let mut sessions = context.runtime.rlm_sessions.lock().await;
            sessions.remove(name)
        };
        let Some(session) = removed else {
            return Err(ToolError::invalid_input(format!(
                "rlm_close: unknown context `{name}`"
            )));
        };

        let mut session = session.lock().await;
        let kernel = session.kernel.take();
        let output = json!({
            "name": session.name,
            "id": session.id,
            "rpc_count": session.rpc_count,
            "total_duration_ms": session.total_duration.as_millis() as u64,
            "peak_var_count": session.peak_var_count,
            "created_ms_ago": session.created_at.elapsed().as_millis() as u64,
            "context_path": session.context_path,
        });
        drop(session);

        if let Some(kernel) = kernel {
            kernel.shutdown().await;
        }

        ToolResult::json(&output).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

async fn load_source(
    input: &Value,
    context: &ToolContext,
) -> Result<(String, String, Option<String>), ToolError> {
    if let Some(path) = rlm_open_source_field(input, "file_path").map(str::trim) {
        let resolved = context.resolve_path(path)?;
        let body = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
            ToolError::execution_failed(format!("rlm_open: read {}: {e}", resolved.display()))
        })?;
        return Ok((body, "file".to_string(), Some(path.to_string())));
    }

    if let Some(content) = rlm_open_source_field(input, "content") {
        if content.chars().count() > MAX_INLINE_CONTENT_CHARS {
            return Err(ToolError::invalid_input(format!(
                "rlm_open: inline content is {} chars (cap {MAX_INLINE_CONTENT_CHARS})",
                content.chars().count()
            )));
        }
        return Ok((content.to_string(), "content".to_string(), None));
    }

    let url = rlm_open_source_field(input, "url")
        .map(str::trim)
        .ok_or_else(|| ToolError::invalid_input("rlm_open: missing source"))?;
    let result = FetchUrlTool
        .execute(json!({"url": url, "format": "raw"}), context)
        .await?;
    let parsed: Value = serde_json::from_str(&result.content).map_err(|e| {
        ToolError::execution_failed(format!("rlm_open: fetch_url returned invalid JSON: {e}"))
    })?;
    let body = parsed
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::execution_failed("rlm_open: fetched body missing content"))?
        .to_string();
    let source_type = parsed
        .get("content_type")
        .and_then(Value::as_str)
        .unwrap_or("url")
        .to_string();
    Ok((body, source_type, Some(url.to_string())))
}

fn rlm_open_source_count(input: &Value) -> usize {
    ["file_path", "content", "url"]
        .iter()
        .filter(|field| rlm_open_source_field(input, field).is_some())
        .count()
}

fn rlm_open_source_field<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

async fn get_session(
    context: &ToolContext,
    name: &str,
) -> Result<Arc<tokio::sync::Mutex<RlmSession>>, ToolError> {
    let sessions = context.runtime.rlm_sessions.lock().await;
    sessions.get(name).cloned().ok_or_else(|| {
        ToolError::invalid_input(format!("unknown RLM context `{name}`; call rlm_open first"))
    })
}

fn required_non_empty_str<'a>(input: &'a Value, field: &str) -> Result<&'a str, ToolError> {
    let value = input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::missing_field(field))?
        .trim();
    if value.is_empty() {
        return Err(ToolError::invalid_input(format!(
            "rlm: `{field}` must not be empty"
        )));
    }
    Ok(value)
}

fn preview_output(text: &str) -> String {
    let total = text.chars().count();
    if total <= FULL_STDOUT_HEAD_CHARS + FULL_STDOUT_TAIL_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(FULL_STDOUT_HEAD_CHARS).collect();
    let tail: String = text
        .chars()
        .skip(total.saturating_sub(FULL_STDOUT_TAIL_CHARS))
        .collect();
    format!(
        "{head}\n... [{} chars truncated, retrieve via handle_read when returned as a handle] ...\n{tail}",
        total.saturating_sub(FULL_STDOUT_HEAD_CHARS + FULL_STDOUT_TAIL_CHARS)
    )
}

#[allow(dead_code)]
fn _assert_var_handle_shape(_: Option<VarHandle>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::handle::HandleReadTool;
    use crate::tools::spec::ToolContext;

    fn ctx() -> ToolContext {
        ToolContext::new(".")
    }

    #[test]
    fn schema_uses_new_tool_names() {
        assert_eq!(RlmOpenTool.name(), "rlm_open");
        assert_eq!(RlmEvalTool::new(None).name(), "rlm_eval");
        assert_eq!(RlmConfigureTool.name(), "rlm_configure");
        assert_eq!(RlmCloseTool.name(), "rlm_close");
    }

    #[test]
    fn rlm_open_source_count_ignores_empty_string_defaults() {
        assert_eq!(
            rlm_open_source_count(
                &json!({"name": "url-doc", "file_path": "", "content": "", "url": "https://example.com/doc"})
            ),
            1
        );
        assert_eq!(
            rlm_open_source_count(
                &json!({"name": "inline-doc", "file_path": "", "content": "body", "url": ""})
            ),
            1
        );
        assert_eq!(
            rlm_open_source_count(&json!({"content": "body", "url": "https://example.com/doc"})),
            2
        );
    }

    #[tokio::test]
    async fn rlm_open_ignores_blank_source_defaults_from_schema_fillers() {
        let ctx = ctx();
        RlmOpenTool
            .execute(
                json!({"name": "blank-defaults", "file_path": "", "content": "body", "url": ""}),
                &ctx,
            )
            .await
            .expect("open with blank sibling source fields");

        RlmCloseTool
            .execute(json!({"name": "blank-defaults"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_session_open_eval_close_lifecycle() {
        let ctx = ctx();
        RlmOpenTool
            .execute(
                json!({"name": "sample", "content": "alpha\nbeta\ngamma"}),
                &ctx,
            )
            .await
            .expect("open");

        let eval = RlmEvalTool::new(None)
            .execute(json!({"name": "sample", "code": "print('ok')"}), &ctx)
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        let stdout_preview = eval_json["stdout_preview"]
            .as_str()
            .expect("stdout_preview")
            .replace("\r\n", "\n");
        assert_eq!(stdout_preview, "ok\n");

        let close = RlmCloseTool
            .execute(json!({"name": "sample"}), &ctx)
            .await
            .expect("close");
        assert!(close.content.contains("sample"));
    }

    #[tokio::test]
    async fn rlm_eval_final_returns_handle() {
        let ctx = ctx();
        RlmOpenTool
            .execute(json!({"name": "finals", "content": "body"}), &ctx)
            .await
            .expect("open");

        let eval = RlmEvalTool::new(None)
            .execute(
                json!({"name": "finals", "code": "finalize('done', confidence=0.8)"}),
                &ctx,
            )
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        assert_eq!(eval_json["final"]["kind"], "var_handle");
        assert_eq!(eval_json["final"]["name"], "final_1");
        assert_eq!(eval_json["confidence"], 0.8);

        RlmCloseTool
            .execute(json!({"name": "finals"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_eval_final_preserves_json_handle() {
        let ctx = ctx();
        RlmOpenTool
            .execute(json!({"name": "json-final", "content": "body"}), &ctx)
            .await
            .expect("open");

        let eval = RlmEvalTool::new(None)
            .execute(
                json!({"name": "json-final", "code": "finalize({'answer': 42, 'items': ['a', 'b']})"}),
                &ctx,
            )
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        assert_eq!(eval_json["final"]["kind"], "var_handle");
        assert_eq!(eval_json["final"]["type"], "dict");
        assert_eq!(eval_json["final"]["length"], 2);

        let read = HandleReadTool
            .execute(
                json!({"handle": eval_json["final"].clone(), "jsonpath": "$.items[*]"}),
                &ctx,
            )
            .await
            .expect("read final handle");
        let read_json: Value = serde_json::from_str(&read.content).expect("read json");
        assert_eq!(read_json["matches"], json!(["a", "b"]));

        RlmCloseTool
            .execute(json!({"name": "json-final"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_configure_metadata_omits_stdout() {
        let ctx = ctx();
        RlmOpenTool
            .execute(json!({"name": "quiet", "content": "body"}), &ctx)
            .await
            .expect("open");
        RlmConfigureTool
            .execute(
                json!({"name": "quiet", "output_feedback": "metadata", "sub_rlm_max_depth": 99}),
                &ctx,
            )
            .await
            .expect("configure");

        let eval = RlmEvalTool::new(None)
            .execute(json!({"name": "quiet", "code": "print('hidden')"}), &ctx)
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        assert!(eval_json.get("stdout_preview").is_none());

        RlmCloseTool
            .execute(json!({"name": "quiet"}), &ctx)
            .await
            .expect("close");
    }
}
