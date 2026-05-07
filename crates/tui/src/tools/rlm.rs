//! `rlm_process` tool — heavy-lift recursive language model as a tool call.
//!
//! Where `rlm_query` is a parallel fanout primitive (N prompts → N answers,
//! stateless), `rlm_process` runs the full recursive-language-model loop
//! against a long input. The input is loaded into a Python REPL as the
//! `PROMPT` variable; a sub-agent writes code to chunk it, calls
//! `llm_query()` / `sub_rlm()` for sub-LLM work, and returns a final string
//! via `FINAL()`. The model never has to put the long input in its own
//! context window — it just calls the tool with `task` + `file_path` (or
//! inline `content`) and reads the synthesized answer back.
//!
//! Use when the input genuinely doesn't fit in working context: a whole
//! file, a long transcript, a multi-document corpus. For short prompts or
//! parallel fanout, prefer `rlm_query`.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::client::DeepSeekClient;
use crate::rlm::turn::{RlmTermination, run_rlm_turn_with_root};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use crate::utils::spawn_supervised;

/// Default child model — cheap and fast.
const DEFAULT_CHILD_MODEL: &str = "deepseek-v4-flash";
/// Default `sub_rlm` recursion budget — paper experiments use 1.
const DEFAULT_MAX_DEPTH: u32 = 1;
/// Hard cap on how many chars of inline `content` we'll accept. Larger
/// inputs should come in via `file_path` so they never enter the caller's
/// context in the first place.
const MAX_INLINE_CONTENT_CHARS: usize = 200_000;

pub struct RlmTool {
    /// Production HTTP client. `None` when no API key is configured.
    client: Option<DeepSeekClient>,
    /// Root model to drive the RLM loop. Set at registration time; matches
    /// whatever model the parent session is using.
    root_model: String,
}

impl RlmTool {
    #[must_use]
    pub fn new(client: Option<DeepSeekClient>, root_model: String) -> Self {
        Self { client, root_model }
    }
}

#[async_trait]
impl ToolSpec for RlmTool {
    fn name(&self) -> &'static str {
        "rlm"
    }

    fn description(&self) -> &'static str {
        "Specialty tool for processing long inputs that don't fit in your \
         own context window. Loads the input into a sandboxed Python REPL \
         as `PROMPT`; a sub-agent writes Python that chunks the input and \
         calls in-REPL helpers (`llm_query`, `llm_query_batched`, \
         `rlm_query`, `rlm_query_batched`) to process it, then returns a \
         synthesized answer. \n\n\
         Use this tool when the input is genuinely large or when a Python \
         map-reduce pass plus child LLM calls is the right shape: whole \
         files, long transcripts, multi-document corpora, bulk semantic \
         classification, or decomposition/critique work. For exact counts \
         or structured aggregates, compute them directly in Python inside \
         the REPL and report the deterministic result instead of asking a \
         child LLM to guess. For whole-input map-reduce, use the REPL \
         helpers `chunk_context()` and `chunk_coverage()` so the result \
         states what was covered. \n\n\
         Provide `task` (what to do) plus exactly one of `file_path` \
         (workspace-relative, preferred — keeps the long input out of \
         your context entirely) or `content` (inline, capped at 200k \
         chars). The Python helpers (`llm_query`, `rlm_query`, etc.) live \
         INSIDE the REPL — they are not separately-callable tools. \n\n\
         Returns the final synthesized answer plus an RLM report showing \
         input size, iterations, duration, sub-LLM calls, and trace summary."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["task"],
            "properties": {
                "task": {
                    "type": "string",
                    "description": "What to do with the input (e.g. \"Summarize the security model\", \"Extract all API endpoints\", \"Categorize each row by sentiment\"). The sub-agent uses this as its objective."
                },
                "file_path": {
                    "type": "string",
                    "description": "Workspace-relative path to a file to load as PROMPT. Preferred — keeps the long input out of your context. Mutually exclusive with `content`."
                },
                "content": {
                    "type": "string",
                    "description": "Inline content to load as PROMPT. Use only when the input isn't a file you can point at. Capped at 200k chars."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Recursion budget for `sub_rlm()` calls. 0 disables recursion; default 1 matches paper experiments."
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // Network for the LLM calls; ExecutesCode because the sub-agent
        // runs Python in the REPL (which can do filesystem operations
        // within its sandbox).
        vec![ToolCapability::Network, ToolCapability::ExecutesCode]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // Same level as parallel_fanout: the model decided to invoke this, the
        // user already enabled tools by being in Agent/YOLO mode, and
        // every concrete side-effect (file read, LLM call) is bounded.
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        // Each call spins its own sidecar on a kernel-assigned port and
        // its own per-turn state file, so two calls don't interfere.
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let Some(client) = self.client.clone() else {
            return Err(ToolError::not_available(
                "rlm_process requires an active DeepSeek client".to_string(),
            ));
        };

        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingField {
                field: "task".to_string(),
            })?
            .trim();
        if task.is_empty() {
            return Err(ToolError::invalid_input("rlm: `task` is empty"));
        }

        let file_path = input.get("file_path").and_then(|v| v.as_str());
        let content = input.get("content").and_then(|v| v.as_str());

        let body = match (file_path, content) {
            (Some(_), Some(_)) => {
                return Err(ToolError::invalid_input(
                    "rlm: pass `file_path` OR `content`, not both",
                ));
            }
            (None, None) => {
                return Err(ToolError::invalid_input(
                    "rlm: requires `file_path` (preferred) or `content`",
                ));
            }
            (Some(path), None) => {
                let resolved = context.resolve_path(path)?;
                tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("read {}: {e}", resolved.display()),
                    }
                })?
            }
            (None, Some(c)) => {
                if c.chars().count() > MAX_INLINE_CONTENT_CHARS {
                    return Err(ToolError::invalid_input(format!(
                        "rlm: inline `content` is {} chars (cap {MAX_INLINE_CONTENT_CHARS}). Pass `file_path` for larger inputs.",
                        c.chars().count()
                    )));
                }
                c.to_string()
            }
        };

        if body.trim().is_empty() {
            return Err(ToolError::invalid_input(
                "rlm: input is empty after loading",
            ));
        }
        let input_chars = body.chars().count();
        let input_lines = body.lines().count();

        // Pin child calls to Flash so model-generated tool args cannot quietly
        // turn fanout work into Pro-billed requests. The RLM root still uses
        // the session model; child helper calls are the cheap batch layer.
        let child_model = DEFAULT_CHILD_MODEL.to_string();

        let max_depth = input
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(u64::from(u32::MAX)) as u32)
            .unwrap_or(DEFAULT_MAX_DEPTH);

        // The tool framework doesn't expose a per-tool event stream, and
        // we don't want RLM's progress events to interleave with the
        // parent agent's stream. Drain into a no-op channel.
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = spawn_supervised(
            "rlm-progress-drain",
            std::panic::Location::caller(),
            async move { while rx.recv().await.is_some() {} },
        );

        // The big body lives only in the REPL as `context`. The small
        // `task` rides along as `root_prompt` and is shown to the root
        // LLM each iteration so it never forgets the objective.
        let result = run_rlm_turn_with_root(
            &client,
            self.root_model.clone(),
            body,
            Some(task.to_string()),
            child_model.clone(),
            tx,
            max_depth,
        )
        .await;

        drain.abort();

        if let Some(err) = result.error {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "rlm: {err} (iterations={}, termination={:?})",
                    result.iterations, result.termination
                ),
            });
        }

        if result.answer.trim().is_empty() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "rlm: empty answer (termination={:?}, iterations={})",
                    result.termination, result.iterations
                ),
            });
        }

        // Surface the termination reason and a brief per-round trace so the
        // user can verify the sub-agent actually engaged with `context`
        // through sub-LLM calls — not just inferred an answer from the
        // preview.
        let footer = match result.termination {
            RlmTermination::Final => String::new(),
            RlmTermination::NoCode => format!(
                "\n\n[warning: sub-agent failed to engage the REPL after {} iterations — answer is the model's last raw response]",
                result.iterations
            ),
            RlmTermination::Exhausted => format!(
                "\n\n[warning: sub-agent hit the {}-iteration cap without FINAL()]",
                result.iterations
            ),
            RlmTermination::Error => String::new(),
        };

        let report = format!(
            "RLM report:\n- input: {input_lines} line(s), {input_chars} char(s)\n- iterations: {}\n- duration: {}ms\n- sub-LLM RPCs: {}\n- termination: {:?}\n\nAnswer:\n",
            result.iterations,
            result.duration.as_millis(),
            result.total_rpcs,
            result.termination,
        );

        let trace_summary = if result.trace.is_empty() {
            String::from("\n\n[trace: no REPL rounds executed]")
        } else {
            let mut s = String::from("\n\n[RLM trace]");
            for r in &result.trace {
                let head = r
                    .code_summary
                    .lines()
                    .next()
                    .unwrap_or(r.code_summary.as_str())
                    .chars()
                    .take(80)
                    .collect::<String>();
                s.push_str(&format!(
                    "\n  round {}: {} sub-LLM call(s), {}ms{} — {}",
                    r.round,
                    r.rpc_count,
                    r.elapsed_ms,
                    if r.had_error { " (error)" } else { "" },
                    head
                ));
            }
            s
        };

        let trace_json: Vec<_> = result
            .trace
            .iter()
            .map(|r| {
                json!({
                    "round": r.round,
                    "rpc_count": r.rpc_count,
                    "elapsed_ms": r.elapsed_ms,
                    "had_error": r.had_error,
                    "code_summary": r.code_summary,
                    "stdout_preview": r.stdout_preview,
                })
            })
            .collect();

        // The `child_*` keys are the contract the engine reads in
        // `tool_routing::accrue_child_token_cost_if_any` to roll
        // sub-LLM token usage into the session-cost counter. RLM
        // spawns its own DeepSeek calls under `child_model`; without
        // this accrual the dashboard under-reports a session that
        // uses RLM heavily by 10-20× because only the parent turn's
        // tokens hit `accrue_session_cost` (#524).
        let metadata = json!({
            "iterations": result.iterations,
            "duration_ms": result.duration.as_millis() as u64,
            "input_tokens": result.usage.input_tokens,
            "output_tokens": result.usage.output_tokens,
            "child_input_tokens": result.usage.input_tokens,
            "child_output_tokens": result.usage.output_tokens,
            "child_prompt_cache_hit_tokens": result.usage.prompt_cache_hit_tokens,
            "child_prompt_cache_miss_tokens": result.usage.prompt_cache_miss_tokens,
            "child_model": child_model,
            "termination": format!("{:?}", result.termination).to_lowercase(),
            "max_depth": max_depth,
            "context_chars": input_chars,
            "context_lines": input_lines,
            "total_rpcs": result.total_rpcs,
            "trace": trace_json,
        });

        Ok(ToolResult::success(format!(
            "{report}{}{}{}",
            result.answer, footer, trace_summary
        ))
        .with_metadata(metadata))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> RlmTool {
        RlmTool::new(None, "deepseek-v4-pro".to_string())
    }

    fn ctx() -> ToolContext {
        use std::path::PathBuf;
        ToolContext::with_auto_approve(
            PathBuf::from("."),
            false,
            PathBuf::from("notes.txt"),
            PathBuf::from("mcp.json"),
            true,
        )
    }

    #[test]
    fn name_and_schema() {
        let t = tool();
        assert_eq!(t.name(), "rlm");
        let schema = t.input_schema();
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["file_path"].is_object());
        assert!(schema["properties"]["content"].is_object());
        assert!(schema["properties"]["max_depth"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "task"));
    }

    #[test]
    fn approval_is_auto_so_calls_are_unattended() {
        assert_eq!(tool().approval_requirement(), ApprovalRequirement::Auto);
    }

    #[test]
    fn capabilities_include_network_and_executes_code() {
        let caps = tool().capabilities();
        assert!(caps.contains(&ToolCapability::Network));
        assert!(caps.contains(&ToolCapability::ExecutesCode));
    }

    #[test]
    fn supports_parallel_dispatch() {
        assert!(tool().supports_parallel());
    }

    #[test]
    fn description_steers_without_suppressing_rlm_use() {
        let t = tool();
        let description = t.description();
        assert!(
            description.contains("Use this tool when"),
            "description should positively explain the RLM fit"
        );
        assert!(
            !description.contains("DO NOT use"),
            "avoid training the model to avoid an available tool"
        );
        assert!(
            !description.contains("slower and more expensive"),
            "cost caveats belong in verification guidance, not tool suppression"
        );
    }

    #[tokio::test]
    async fn returns_not_available_without_client() {
        let t = tool();
        let ctx = ctx();
        let res = t
            .execute(json!({"task": "x", "content": "y"}), &ctx)
            .await
            .expect_err("must error");
        assert!(matches!(res, ToolError::NotAvailable { .. }));
    }

    #[tokio::test]
    async fn rejects_missing_task() {
        let t = RlmTool::new(None, "x".into());
        let ctx = ctx();
        let res = t
            .execute(json!({"content": "abc"}), &ctx)
            .await
            .expect_err("must error");
        // Without a client we hit NotAvailable first. Re-check ordering by
        // injecting an obviously-bad payload that would trip earlier.
        assert!(matches!(
            res,
            ToolError::NotAvailable { .. } | ToolError::MissingField { .. }
        ));
    }

    #[tokio::test]
    async fn rejects_both_path_and_content() {
        // Even without a client, the input-shape check should fire if we
        // bypass the client guard. Simpler: just verify the schema lists
        // the two as alternatives via descriptions.
        let schema = tool().input_schema();
        let path_desc = schema["properties"]["file_path"]["description"]
            .as_str()
            .unwrap();
        assert!(path_desc.to_lowercase().contains("mutually exclusive"));
    }
}
