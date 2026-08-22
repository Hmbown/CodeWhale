//! Compatibility persistent-RLM session tools.
//!
//! v0.8.33 replaces the old one-shot `rlm` tool with a head/hands surface:
//! `rlm_open` creates a named Python kernel over a large context,
//! `rlm_eval` runs bounded probes against it, `rlm_configure` adjusts runtime
//! feedback, and `rlm_close` tears it down.
//!
//! The normal Agent path now owns one session-persistent `repl` kernel. This
//! action-shaped surface stays registered for explicit compatibility and saved
//! transcript replay, but is hidden from new model turns. Its `rlm_*` aliases
//! force the action so old transcripts replay correctly — the pattern
//! `BashTool` established for `exec_shell*` in #4625.

use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// When `rlm_eval` stdout exceeds this many characters the full body is
/// stored as a `var_handle` instead of inlined into the parent transcript.
/// The model retrieves the body via `handle_read` using the returned handle.
const STDOUT_HANDLE_THRESHOLD_CHARS: usize = 1_000;
const HARD_SUB_RLM_DEPTH_CAP: u32 = 3;

const ALL_ACTIONS: &[&str] = &["session_objects", "open", "eval", "configure", "close"];

fn rlm_kernel_error_result(
    error: &str,
    elapsed: Duration,
    route: &crate::cost_status::EffectiveRouteEnvelope,
    usage: &crate::models::Usage,
) -> ToolResult {
    let mut metadata = json!({
        // The registered tool is `rlm`; `eval` is its action. Naming a
        // retired `rlm_eval` tool here taught the model a call it cannot
        // make (2026-08-04 audit).
        "tool": "rlm",
        "action": "eval",
        "duration_ms": elapsed.as_millis() as u64,
        "kernel_error": true,
    });
    crate::cost_status::attach_child_usage_metadata(&mut metadata, route, usage);
    ToolResult::error(format!("rlm action='eval': {error}")).with_metadata(metadata)
}

/// Unified RLM session tool.
///
/// One struct and one input schema for the canonical `rlm` tool. `client` is
/// only exercised by the `eval` action (child sub-RLM queries); other actions
/// ignore it.
pub struct RlmTool {
    name: &'static str,
    forced_action: Option<&'static str>,
    client: Option<DeepSeekClient>,
    /// Kept only for replay-compatible explicit RLM sessions. New normal
    /// agent work uses the session kernel and inherits its route there.
    root_model: String,
}

impl RlmTool {
    #[must_use]
    pub fn new(name: &'static str, client: Option<DeepSeekClient>) -> Self {
        Self {
            name,
            forced_action: None,
            client,
            root_model: DEFAULT_CHILD_MODEL.to_string(),
        }
    }

    /// Bind an explicit compatibility session to the active parent route.
    /// This prevents a saved/manual RLM invocation from silently falling back
    /// to an unrelated legacy child model.
    #[must_use]
    pub fn with_root_model(mut self, root_model: String) -> Self {
        self.root_model = root_model;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn alias(name: &'static str, action: &'static str, client: Option<DeepSeekClient>) -> Self {
        Self {
            name,
            forced_action: Some(action),
            client,
            root_model: DEFAULT_CHILD_MODEL.to_string(),
        }
    }

    fn resolve_action<'a>(&'a self, input: &'a Value) -> Result<&'a str, ToolError> {
        let action = match self.forced_action {
            Some(action) => action,
            None => input.get("action").and_then(Value::as_str).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "rlm: missing `action` (one of: {})",
                    ALL_ACTIONS.join(", ")
                ))
            })?,
        };
        if ALL_ACTIONS.contains(&action) {
            Ok(action)
        } else {
            Err(ToolError::invalid_input(format!(
                "rlm: invalid action `{action}` (one of: {})",
                ALL_ACTIONS.join(", ")
            )))
        }
    }

    /// Mirror of the legacy per-tool approval contract: only `rlm_eval`
    /// required approval (it is the non-bypassable code-eval surface, #3866).
    fn action_requires_approval(action: &str) -> bool {
        action == "eval"
    }

    /// Mirror of the legacy per-tool read-only contract (capability-derived):
    /// `rlm_open` carries `ExecutesCode`, so only session_objects / configure /
    /// close counted as read-only.
    fn action_is_read_only(action: &str) -> bool {
        matches!(action, "session_objects" | "configure" | "close")
    }

    fn action_capabilities(action: &str) -> Vec<ToolCapability> {
        match action {
            "session_objects" => vec![ToolCapability::ReadOnly],
            "open" => vec![
                ToolCapability::ReadOnly,
                ToolCapability::Network,
                ToolCapability::ExecutesCode,
            ],
            "eval" => vec![
                ToolCapability::Network,
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ],
            // configure / close
            _ => vec![ToolCapability::ReadOnly],
        }
    }
}

#[async_trait]
impl ToolSpec for RlmTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        // The normal Agent path owns a session-scoped `repl` kernel. Keep the
        // old action fan-out registered for replay and explicit compatibility,
        // but do not teach a second RLM workflow to new model turns.
        false
    }

    fn description(&self) -> &'static str {
        match self.forced_action {
            Some("session_objects") => {
                "List active prompt/history/session symbolic objects as compact cards. \
                 Pass one of the returned `id` values to `rlm_open` as \
                 `session_object` to inspect it inside an RLM REPL without copying the \
                 full prompt or transcript into the parent context."
            }
            Some("open") => {
                "Open a persistent RLM context. Loads `file_path`, `content`, `url`, \
                 or `session_object` into a named Python kernel and returns only \
                 metadata: name, length, preview, and sha256. Use this for large or \
                 unfamiliar inputs so the parent transcript holds a handle, not the \
                 body."
            }
            Some("eval") => {
                "Run one Python REPL block against a named RLM context. Returns a \
                 bounded projection of stdout/stderr plus metadata. If the code calls \
                 FINAL/finalize, the final value is stored as a var_handle retrievable \
                 with handle_read instead of copied unbounded into the parent context. \
                 Large stdout/stderr payloads (>1k chars) are also stored as \
                 var_handles (returned in stdout_handle / stderr_handle) to keep the \
                 parent transcript lean. Batch child helpers require \
                 dependency_mode='independent'; use sub_query_sequence or a \
                 sequential loop for dependent work."
            }
            Some("configure") => {
                "Configure a named RLM context: output feedback, child query timeout, \
                 recursive sub-RLM depth, and explicit session sharing."
            }
            Some("close") => {
                "Close a named RLM context, tear down its Python kernel, and return \
                 usage/lifecycle metadata."
            }
            _ => {
                "Persistent RLM sessions over large contexts. Actions: \"session_objects\" \
                 (list active prompt/history/session symbolic objects as compact cards), \
                 \"open\" (load file_path/content/url/session_object into a named Python \
                 kernel; returns only metadata so the parent transcript holds a handle, \
                 not the body), \"eval\" (run one bounded Python REPL block against a \
                 named context; approval required; FINAL/finalize values and large \
                 stdout/stderr become var_handles retrievable with handle_read), \
                 \"configure\" (output feedback, child timeout, sub-RLM depth, session \
                 sharing), \"close\" (tear down the kernel and return usage metadata)."
            }
        }
    }

    fn input_schema(&self) -> Value {
        if let Some(action) = self.forced_action {
            return legacy_action_schema(action);
        }
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ALL_ACTIONS,
                    "description": "Action to perform."
                },
                "name": {
                    "type": "string",
                    "description": "RLM context name, unique within this parent session (action=open: optional, defaults to a slug from the source). Required for action=eval/configure/close."
                },
                "file_path": {
                    "type": "string",
                    "description": "Workspace-relative file to load (action=open; exactly one of file_path/content/url/session_object)."
                },
                "content": {
                    "type": "string",
                    "description": "Inline content to load. Capped at 200k chars. (action=open)"
                },
                "url": {
                    "type": "string",
                    "description": "HTTP/HTTPS URL to fetch (through the same path as Web action=\"fetch\") and load. (action=open)"
                },
                "session_object": {
                    "type": "string",
                    "description": "Stable symbolic active-session ref from action=session_objects, for example session://active/system_prompt or session://active/messages/0. (action=open)"
                },
                "code": {
                    "type": "string",
                    "description": "Raw Python executed against the context (no markdown fences). The loaded source is in scope as `content`; call FINAL(value)/finalize(...) to return a result handle. Example: print(len(content)). (action=eval)"
                },
                "output_feedback": {
                    "type": "string",
                    "enum": ["full", "metadata"],
                    "description": "(action=configure)"
                },
                "sub_query_timeout_secs": {
                    "type": "integer",
                    "description": "(action=configure)"
                },
                "sub_rlm_max_depth": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3,
                    "description": "(action=configure)"
                },
                "share_session": {
                    "type": "boolean",
                    "description": "(action=configure)"
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        match self.forced_action {
            Some(action) => Self::action_capabilities(action),
            None => vec![
                ToolCapability::Network,
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ],
        }
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        match self.forced_action {
            Some(action) if Self::action_requires_approval(action) => ApprovalRequirement::Required,
            Some(_) => ApprovalRequirement::Auto,
            None => ApprovalRequirement::Required,
        }
    }

    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match self.resolve_action(input) {
            Ok(action) if Self::action_requires_approval(action) => ApprovalRequirement::Required,
            Ok(_) => ApprovalRequirement::Auto,
            Err(_) => self.approval_requirement(),
        }
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        match self.resolve_action(input) {
            Ok(action) => Self::action_is_read_only(action),
            Err(_) => self.is_read_only(),
        }
    }

    fn supports_parallel(&self) -> bool {
        matches!(self.forced_action, Some("session_objects"))
    }

    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(self.resolve_action(input), Ok("session_objects"))
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        match self.resolve_action(&input)? {
            "session_objects" => self.execute_session_objects(context).await,
            "open" => self.execute_open(&input, context).await,
            "eval" => self.execute_eval(&input, context).await,
            "configure" => self.execute_configure(&input, context).await,
            "close" => self.execute_close(&input, context).await,
            action => Err(ToolError::invalid_input(format!(
                "rlm: invalid action `{action}`"
            ))),
        }
    }
}

impl RlmTool {
    async fn execute_session_objects(
        &self,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = context.session_objects.as_ref().ok_or_else(|| {
            ToolError::not_available("rlm_session_objects: active session snapshot unavailable")
        })?;
        ToolResult::json(&json!({
            "objects": snapshot.object_cards(),
            "open_with": {
                "tool": "rlm",
                "action": "open",
                "field": "session_object",
                "example": {
                    "name": "active_prompt",
                    "session_object": "session://active/system_prompt"
                }
            },
            "redaction": "Large tool results and thinking blocks are represented by compact metadata in transcript objects; use returned handles and handle_read for bounded payload projections."
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_open(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let source_count = rlm_open_source_count(input);
        if source_count != 1 {
            let mut msg = String::from(
                "rlm_open: provide exactly one of `file_path` (local file), `content` (inline text), `url`, or `session_object`",
            );
            // "did you mean" for common misnamings (#2655).
            if let Some(obj) = input.as_object() {
                let seen: Vec<&str> = [
                    "prompt",
                    "resident_file",
                    "text",
                    "body",
                    "path",
                    "file",
                    "source",
                ]
                .into_iter()
                .filter(|k| obj.contains_key(*k))
                .collect();
                if !seen.is_empty() {
                    msg.push_str(&format!(
                        ". Saw {seen:?} — did you mean file_path/content/url/session_object? (to evaluate against an existing context, pass its name to rlm action='eval', or use `session_object`)"
                    ));
                }
            }
            return Err(ToolError::invalid_input(msg));
        }

        let (body, source_type, source_hint) = load_source(input, context).await?;
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

    async fn execute_eval(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(input, "name")?;
        let code = required_non_empty_str(input, "code").map_err(|_| {
            ToolError::invalid_input(
                "rlm_eval: `code` is required and runs raw Python against the RLM context (no markdown fences). \
                 Example: {\"name\": \"<ctx>\", \"code\": \"print(len(content))\"}; call FINAL(value) to return a result handle.",
            )
        })?;
        let session = get_session(context, name).await?;
        let mut session = session.lock().await;
        let config = session.config.clone();

        let Some(kernel) = session.kernel.as_mut() else {
            return Err(ToolError::invalid_input(format!(
                "rlm_eval: context `{name}` is closed"
            )));
        };

        let started = Instant::now();
        let (round, child_usage, child_route) = if let Some(client) = self.client.clone() {
            let route = client.effective_route_envelope(&self.root_model, chrono::Utc::now());
            let bridge = RlmBridge::new(
                Arc::new(client),
                self.root_model.clone(),
                config.sub_rlm_max_depth.min(HARD_SUB_RLM_DEPTH_CAP),
            );
            let usage_handle = bridge.usage_handle();
            let round_result = kernel.run(code, Some(&bridge)).await;
            let usage = usage_handle.lock().await.clone();
            let round = match round_result {
                Ok(round) => round,
                Err(error) => {
                    // A bridge request may have completed and accrued usage
                    // before the Python kernel times out or closes stdout.
                    // Return a failed ToolResult (rather than a bare ToolError)
                    // so ToolCallComplete still carries the immutable child
                    // receipt and the runtime can durably account for it.
                    session.last_used_at = Instant::now();
                    return Ok(rlm_kernel_error_result(
                        &error.to_string(),
                        started.elapsed(),
                        &route,
                        &usage,
                    ));
                }
            };
            (round, usage, Some(route))
        } else {
            let round = kernel
                .run(code, None::<&RlmBridge>)
                .await
                .map_err(|e| ToolError::execution_failed(format!("rlm_eval: {e}")))?;
            (round, Default::default(), None)
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
        // Route large stdout/stderr into a var_handle to avoid bloat in
        // the parent transcript. The model calls handle_read for bounded
        // projections; a short inline note describes availability.
        fn route_output(
            text: &str,
            feedback: &OutputFeedback,
            store: &mut crate::tools::handle::HandleStore,
            session_id: &str,
            tag: &str,
        ) -> (Option<String>, Option<crate::tools::handle::VarHandle>) {
            let threshold = STDOUT_HANDLE_THRESHOLD_CHARS;
            match (feedback, text.len()) {
                (OutputFeedback::Full, len) if len <= threshold => {
                    (Some(preview_output(text)), None)
                }
                (OutputFeedback::Full, _) if !text.trim().is_empty() => {
                    // Store full body as a handle for out-of-band retrieval
                    let name = format!("{tag}_{}", 0); // single counter is fine
                    let handle = store.insert_text(session_id, name, text);
                    (
                        Some(format!("{} chars; retrieve via handle_read", text.len())),
                        Some(handle),
                    )
                }
                _ => (None, None),
            }
        }

        let (stdout_preview, stdout_handle) = route_output(
            &round.full_stdout,
            &config.output_feedback,
            &mut *context.runtime.handle_store.lock().await,
            &session.id,
            "stdout",
        );
        let (stderr_preview, stderr_handle) = route_output(
            &round.stderr,
            &config.output_feedback,
            &mut *context.runtime.handle_store.lock().await,
            &session.id,
            "stderr",
        );

        let mut output = json!({
            "name": session.name,
            "id": session.id,
            "duration_ms": duration_ms,
            "rpc_count": rpc_count,
            "had_error": had_error,
            "new_vars": [],
            "final": final_handle,
        });
        if let Some(ref stdout_preview) = stdout_preview {
            output["stdout_preview"] = json!(stdout_preview);
        }
        if let Some(ref stderr_preview) = stderr_preview {
            output["stderr_preview"] = json!(stderr_preview);
        }
        if let (Some(h), Some(_)) = (stdout_handle, &stdout_preview) {
            output["stdout_handle"] = json!(h);
        }
        if let (Some(h), Some(_)) = (stderr_handle, &stderr_preview) {
            output["stderr_handle"] = json!(h);
        }
        if let Some(confidence) = round.final_confidence.clone() {
            output["confidence"] = confidence;
        }

        let mut metadata = json!({
            "tool": "rlm_eval",
            "duration_ms": started.elapsed().as_millis() as u64,
        });
        // RLM fans out dozens of child rounds, so an undercounted class here
        // scales; report every billable class from the shared producer (#4318).
        if let Some(route) = child_route.as_ref() {
            crate::cost_status::attach_child_usage_metadata(&mut metadata, route, &child_usage);
        }

        Ok(ToolResult::json(&output)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?
            .with_metadata(metadata))
    }

    async fn execute_configure(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(input, "name")?;
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

    async fn execute_close(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = required_non_empty_str(input, "name")?;
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

/// The exact schema the legacy per-action tool exposed, kept so hidden alias
/// registrations report an identical contract to the pre-unification tools.
fn legacy_action_schema(action: &str) -> Value {
    match action {
        "session_objects" => json!({
            "type": "object",
            "properties": {}
        }),
        "open" => json!({
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
                    "description": "HTTP/HTTPS URL to fetch (through the same path as Web action=\"fetch\") and load."
                },
                "session_object": {
                    "type": "string",
                    "description": "Stable symbolic active-session ref from rlm_session_objects, for example session://active/system_prompt or session://active/messages/0."
                }
            }
        }),
        "eval" => json!({
            "type": "object",
            "required": ["name", "code"],
            "properties": {
                "name": { "type": "string", "description": "RLM context name returned by rlm_open." },
                "code": { "type": "string", "description": "Raw Python executed against the context (no markdown fences). The loaded source is in scope as `content`; call FINAL(value)/finalize(...) to return a result handle. Example: print(len(content))." }
            }
        }),
        "configure" => json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" },
                "output_feedback": { "type": "string", "enum": ["full", "metadata"] },
                "sub_query_timeout_secs": { "type": "integer" },
                "sub_rlm_max_depth": { "type": "integer", "minimum": 0, "maximum": 3 },
                "share_session": { "type": "boolean" }
            }
        }),
        // close
        _ => json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "RLM context name from rlm_open." }
            }
        }),
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

    if let Some(object_ref) = rlm_open_source_field(input, "session_object") {
        let snapshot = context.session_objects.as_ref().ok_or_else(|| {
            ToolError::not_available("rlm_open: active session snapshot unavailable")
        })?;
        let object = snapshot.resolve(object_ref).ok_or_else(|| {
            ToolError::invalid_input(format!("rlm_open: unknown session object `{object_ref}`"))
        })?;
        return Ok((
            object.body,
            format!("session_object:{}", object.kind),
            Some(object.id),
        ));
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
    ["file_path", "content", "url", "session_object"]
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
        ToolError::invalid_input(format!(
            "unknown RLM context `{name}`; open it first with rlm action='open'"
        ))
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
    use crate::models::Role;
    use crate::models::{ContentBlock, Message, SystemPrompt};
    use crate::rlm::session::SessionObjectSnapshot;
    use crate::tools::handle::HandleReadTool;
    use crate::tools::spec::ToolContext;
    use std::path::PathBuf;

    fn ctx() -> ToolContext {
        ToolContext::new(".")
    }

    fn ctx_with_session_objects() -> ToolContext {
        ToolContext::new(".").with_session_objects(SessionObjectSnapshot::new(
            "session-1".to_string(),
            "deepseek-v4-pro".to_string(),
            PathBuf::from("."),
            Some(SystemPrompt::Text("You are CodeWhale.".to_string())),
            vec![
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "Please inspect the RLM surface.".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "I will use symbolic session objects.".to_string(),
                        cache_control: None,
                    }],
                },
            ],
        ))
    }

    #[test]
    fn schema_uses_new_tool_names() {
        assert_eq!(
            RlmTool::alias("rlm_session_objects", "session_objects", None).name(),
            "rlm_session_objects"
        );
        assert_eq!(RlmTool::alias("rlm_open", "open", None).name(), "rlm_open");
        assert_eq!(RlmTool::alias("rlm_eval", "eval", None).name(), "rlm_eval");
        assert_eq!(
            RlmTool::alias("rlm_configure", "configure", None).name(),
            "rlm_configure"
        );
        assert_eq!(
            RlmTool::alias("rlm_close", "close", None).name(),
            "rlm_close"
        );
    }

    #[test]
    fn rlm_tool_is_compatibility_only_not_model_visible() {
        let canonical = RlmTool::new("rlm", None);
        assert!(!canonical.model_visible());
        assert_eq!(canonical.name(), "rlm");
        let actions = canonical.input_schema()["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum")
            .clone();
        for action in ["session_objects", "open", "eval", "configure", "close"] {
            assert!(
                actions.iter().any(|value| value.as_str() == Some(action)),
                "canonical schema must offer action {action}"
            );
        }

        for alias in [
            RlmTool::alias("rlm_session_objects", "session_objects", None),
            RlmTool::alias("rlm_open", "open", None),
            RlmTool::alias("rlm_eval", "eval", None),
            RlmTool::alias("rlm_configure", "configure", None),
            RlmTool::alias("rlm_close", "close", None),
        ] {
            assert!(
                !alias.model_visible(),
                "compatibility alias {} must stay hidden",
                alias.name()
            );
        }
    }

    #[test]
    fn kernel_failure_result_retains_child_usage_receipt() {
        let route = crate::cost_status::EffectiveRouteEnvelope::capture(
            None,
            crate::config::ApiProvider::Deepseek,
            "deepseek-rlm",
            DEFAULT_CHILD_MODEL,
            Some(crate::config::ApiProvider::Deepseek.default_base_url()),
            chrono::Utc::now(),
        );
        let usage = crate::models::Usage {
            input_tokens: 23,
            output_tokens: 5,
            reasoning_replay_tokens: Some(7),
            ..Default::default()
        };
        let result = rlm_kernel_error_result(
            "kernel stdout closed",
            Duration::from_millis(11),
            &route,
            &usage,
        );

        assert!(!result.success);
        let metadata = result
            .metadata
            .expect("usage metadata on failed tool result");
        assert_eq!(
            crate::cost_status::child_route_envelope_from_metadata(&metadata),
            Some(route)
        );
        assert_eq!(
            crate::cost_status::child_usage_from_metadata(&metadata),
            Some(usage)
        );
    }

    #[test]
    fn rlm_eval_requires_approval() {
        let tool = RlmTool::alias("rlm_eval", "eval", None);
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Required);
        assert!(
            tool.capabilities()
                .contains(&ToolCapability::RequiresApproval)
        );

        // Approval routing on the canonical tool: only eval requires it.
        let canonical = RlmTool::new("rlm", None);
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "eval"})),
            ApprovalRequirement::Required
        );
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "open"})),
            ApprovalRequirement::Auto
        );
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "session_objects"})),
            ApprovalRequirement::Auto
        );
    }

    #[test]
    fn read_only_and_parallel_flags_match_legacy_contract() {
        // Legacy: session_objects was parallel-friendly read-only; open carried
        // ExecutesCode (not read-only) with Auto approval; eval required approval.
        let session_objects = RlmTool::alias("rlm_session_objects", "session_objects", None);
        assert!(session_objects.supports_parallel());
        assert!(session_objects.is_read_only_for(&json!({})));

        let open = RlmTool::alias("rlm_open", "open", None);
        assert!(!open.is_read_only_for(&json!({})));
        assert_eq!(open.approval_requirement(), ApprovalRequirement::Auto);

        let canonical = RlmTool::new("rlm", None);
        assert!(canonical.supports_parallel_for(&json!({"action": "session_objects"})));
        assert!(!canonical.supports_parallel_for(&json!({"action": "eval"})));
        assert!(canonical.is_read_only_for(&json!({"action": "configure"})));
        assert!(!canonical.is_read_only_for(&json!({"action": "open"})));
        assert!(!canonical.is_read_only_for(&json!({"action": "eval"})));
    }

    #[test]
    fn canonical_rejects_unknown_or_missing_action() {
        let tool = RlmTool::new("rlm", None);
        let err = tool
            .resolve_action(&json!({}))
            .expect_err("missing action must fail");
        assert!(err.to_string().contains("missing `action`"));
        let err = tool
            .resolve_action(&json!({"action": "explode"}))
            .expect_err("unknown action must fail");
        assert!(err.to_string().contains("invalid action"));
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
        assert_eq!(
            rlm_open_source_count(
                &json!({"content": "body", "session_object": "session://active/system_prompt"})
            ),
            2
        );
    }

    #[tokio::test]
    async fn rlm_session_objects_lists_active_prompt_object() {
        let ctx = ctx_with_session_objects();
        let result = RlmTool::alias("rlm_session_objects", "session_objects", None)
            .execute(json!({}), &ctx)
            .await
            .expect("list session objects");
        let body: Value = serde_json::from_str(&result.content).expect("json");
        let objects = body["objects"].as_array().expect("objects array");

        assert!(objects.iter().any(|object| {
            object["id"] == "session://active/system_prompt" && object["kind"] == "system_prompt"
        }));
        assert!(objects.iter().any(|object| {
            object["id"] == "session://active/messages/0" && object["kind"] == "message"
        }));
    }

    #[tokio::test]
    async fn rlm_open_loads_active_session_prompt_object() {
        let ctx = ctx_with_session_objects();
        let open = RlmTool::alias("rlm_open", "open", None)
            .execute(
                json!({"name": "active_prompt", "session_object": "session://active/system_prompt"}),
                &ctx,
            )
            .await
            .expect("open prompt object");
        let open_json: Value = serde_json::from_str(&open.content).expect("open json");
        assert_eq!(open_json["type"], "session_object:system_prompt");
        assert!(
            open_json["preview_500"]
                .as_str()
                .unwrap()
                .contains("CodeWhale")
        );

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "active_prompt"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_open_loads_transcript_message_object() {
        let ctx = ctx_with_session_objects();
        let open = RlmTool::alias("rlm_open", "open", None)
            .execute(
                json!({"name": "first_message", "session_object": "session://active/messages/0"}),
                &ctx,
            )
            .await
            .expect("open transcript slice");
        let open_json: Value = serde_json::from_str(&open.content).expect("open json");
        assert_eq!(open_json["type"], "session_object:message");
        assert!(
            open_json["preview_500"]
                .as_str()
                .unwrap()
                .contains("RLM surface")
        );

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "first_message"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_open_ignores_blank_source_defaults_from_schema_fillers() {
        let ctx = ctx();
        RlmTool::alias("rlm_open", "open", None)
            .execute(
                json!({"name": "blank-defaults", "file_path": "", "content": "body", "url": ""}),
                &ctx,
            )
            .await
            .expect("open with blank sibling source fields");

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "blank-defaults"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_open_misnamed_source_field_gets_did_you_mean_hint() {
        // #2655: a wrong source field name yields actionable guidance, not just
        // the canonical "provide exactly one" message.
        let ctx = ctx();
        let err = RlmTool::alias("rlm_open", "open", None)
            .execute(json!({"name": "doc", "prompt": "summarize this"}), &ctx)
            .await
            .expect_err("misnamed source field should fail");
        let msg = err.to_string();
        assert!(msg.contains("file_path"), "names the real fields: {msg}");
        assert!(
            msg.contains("`url`, or `session_object`"),
            "names session_object in the valid source field list: {msg}"
        );
        assert!(msg.contains("prompt"), "echoes the wrong field: {msg}");
    }

    #[tokio::test]
    async fn rlm_eval_missing_code_explains_raw_python() {
        // #2655: the missing-code error should teach the tool, with an example.
        let ctx = ctx();
        let err = RlmTool::alias("rlm_eval", "eval", None)
            .execute(json!({"name": "doc"}), &ctx)
            .await
            .expect_err("missing code should fail");
        let msg = err.to_string();
        assert!(msg.contains("raw Python"), "explains it runs Python: {msg}");
        assert!(
            msg.contains("print(len(content))") || msg.contains("FINAL"),
            "includes an example: {msg}"
        );
    }

    #[test]
    fn rlm_eval_schema_names_the_runtime_content_variable() {
        let schema = RlmTool::alias("rlm_eval", "eval", None).input_schema();
        let description = schema["properties"]["code"]["description"]
            .as_str()
            .expect("rlm_eval code description");

        assert!(description.contains("`content`"));
        assert!(description.contains("print(len(content))"));
        assert!(!description.contains("SOURCE"));
    }

    #[tokio::test]
    async fn rlm_session_open_eval_close_lifecycle() {
        let ctx = ctx();
        RlmTool::alias("rlm_open", "open", None)
            .execute(
                json!({"name": "sample", "content": "alpha\nbeta\ngamma"}),
                &ctx,
            )
            .await
            .expect("open");

        let eval = RlmTool::alias("rlm_eval", "eval", None)
            .execute(json!({"name": "sample", "code": "print('ok')"}), &ctx)
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        let stdout_preview = eval_json["stdout_preview"]
            .as_str()
            .expect("stdout_preview")
            .replace("\r\n", "\n");
        assert_eq!(stdout_preview, "ok\n");

        let close = RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "sample"}), &ctx)
            .await
            .expect("close");
        assert!(close.content.contains("sample"));
    }

    #[tokio::test]
    async fn rlm_canonical_action_routing_runs_full_lifecycle() {
        // The visible surface: one `rlm` tool, action-parameterized.
        let ctx = ctx();
        let tool = RlmTool::new("rlm", None);
        tool.execute(
            json!({"action": "open", "name": "canonical", "content": "body"}),
            &ctx,
        )
        .await
        .expect("open via canonical action");

        let eval = tool
            .execute(
                json!({"action": "eval", "name": "canonical", "code": "print('ok')"}),
                &ctx,
            )
            .await
            .expect("eval via canonical action");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        let stdout_preview = eval_json["stdout_preview"]
            .as_str()
            .expect("stdout_preview")
            .replace("\r\n", "\n");
        assert_eq!(stdout_preview, "ok\n");

        let close = tool
            .execute(json!({"action": "close", "name": "canonical"}), &ctx)
            .await
            .expect("close via canonical action");
        assert!(close.content.contains("canonical"));
    }

    #[tokio::test]
    async fn rlm_eval_final_returns_handle() {
        let ctx = ctx();
        RlmTool::alias("rlm_open", "open", None)
            .execute(json!({"name": "finals", "content": "body"}), &ctx)
            .await
            .expect("open");

        let eval = RlmTool::alias("rlm_eval", "eval", None)
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

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "finals"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_eval_final_preserves_json_handle() {
        let ctx = ctx();
        RlmTool::alias("rlm_open", "open", None)
            .execute(json!({"name": "json-final", "content": "body"}), &ctx)
            .await
            .expect("open");

        let eval = RlmTool::alias("rlm_eval", "eval", None)
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

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "json-final"}), &ctx)
            .await
            .expect("close");
    }

    #[tokio::test]
    async fn rlm_configure_metadata_omits_stdout() {
        let ctx = ctx();
        RlmTool::alias("rlm_open", "open", None)
            .execute(json!({"name": "quiet", "content": "body"}), &ctx)
            .await
            .expect("open");
        RlmTool::alias("rlm_configure", "configure", None)
            .execute(
                json!({"name": "quiet", "output_feedback": "metadata", "sub_rlm_max_depth": 99}),
                &ctx,
            )
            .await
            .expect("configure");

        let eval = RlmTool::alias("rlm_eval", "eval", None)
            .execute(json!({"name": "quiet", "code": "print('hidden')"}), &ctx)
            .await
            .expect("eval");
        let eval_json: Value = serde_json::from_str(&eval.content).expect("eval json");
        assert!(eval_json.get("stdout_preview").is_none());

        RlmTool::alias("rlm_close", "close", None)
            .execute(json!({"name": "quiet"}), &ctx)
            .await
            .expect("close");
    }
}
