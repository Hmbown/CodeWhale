//! RLM turn loop — paper Algorithm 1 driven over a long-lived Python
//! subprocess + stdin/stdout RPC bridge (no HTTP sidecar).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::client::DeepSeekClient;
use crate::core::events::Event;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Usage};
use crate::repl::PythonRuntime;

use super::bridge::{RlmBridge, RlmLlmClient};
use super::prompt::rlm_system_prompt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of RLM iterations before the loop gives up.
const MAX_RLM_ITERATIONS: u32 = 25;
/// Max consecutive rounds where the model returns no `repl` fence before we
/// hard-fail. The paper requires `code → REPL → Final`; anything else is
/// not the RLM contract.
const MAX_CONSECUTIVE_NO_CODE: u32 = 3;
/// Max output tokens for the root LLM — it just needs to generate code.
const ROOT_MAX_TOKENS: u32 = 4096;
/// Max chars of stdout shown as metadata to the root LLM in next iteration.
const STDOUT_METADATA_PREVIEW_LEN: usize = 800;
/// Max chars of `context` shown as a preview in the metadata.
const PROMPT_PREVIEW_LEN: usize = 500;
/// Temperature for root LLM calls.
const ROOT_TEMPERATURE: f32 = 0.3;
/// Bound on conversation history we keep across iterations.
const MAX_HISTORY_MESSAGES: usize = 20;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// How an RLM turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlmTermination {
    /// `FINAL(value)` was called inside the REPL or `FINAL(...)` appeared
    /// at the top of the model's response on its own line.
    Final,
    /// The model failed to emit a `repl` block for too many rounds in a
    /// row. The accumulated last response text is surfaced as the answer
    /// rather than being thrown away.
    NoCode,
    /// Iteration cap reached without `FINAL`.
    Exhausted,
    /// Hard error — LLM call failed, REPL crashed, timeout.
    Error,
}

/// Per-round trace entry. Surfaced in the tool result so the user can see
/// exactly what the sub-agent did.
#[derive(Debug, Clone)]
pub struct RlmRoundTrace {
    pub round: u32,
    pub code_summary: String,
    pub stdout_preview: String,
    pub had_error: bool,
    pub rpc_count: u32,
    pub elapsed_ms: u64,
}

/// Result of an RLM turn.
#[derive(Debug, Clone)]
pub struct RlmTurnResult {
    pub answer: String,
    pub iterations: u32,
    pub duration: Duration,
    pub error: Option<String>,
    pub usage: Usage,
    pub termination: RlmTermination,
    /// Per-round trace. Empty when the loop never reached the REPL.
    pub trace: Vec<RlmRoundTrace>,
    /// Total sub-LLM RPCs made by the sub-agent (sum of `rpc_count` across
    /// rounds). Useful for verifying that the model engaged with `context`
    /// rather than answering directly.
    pub total_rpcs: u32,
}

/// Run a full RLM turn. `prompt` is loaded into the REPL as `context`; it
/// never enters the root LLM's window.
pub async fn run_rlm_turn(
    client: &DeepSeekClient,
    model: String,
    prompt: String,
    child_model: String,
    tx_event: mpsc::Sender<Event>,
    max_depth: u32,
) -> RlmTurnResult {
    run_rlm_turn_inner(
        Arc::new(client.clone()),
        model,
        prompt,
        None,
        child_model,
        tx_event,
        max_depth,
    )
    .await
}

/// Variant that also passes a small `root_prompt` (the user-facing task)
/// shown to the root LLM each iteration so it remembers its objective.
pub async fn run_rlm_turn_with_root(
    client: &DeepSeekClient,
    model: String,
    prompt: String,
    root_prompt: Option<String>,
    child_model: String,
    tx_event: mpsc::Sender<Event>,
    max_depth: u32,
) -> RlmTurnResult {
    run_rlm_turn_inner(
        Arc::new(client.clone()),
        model,
        prompt,
        root_prompt,
        child_model,
        tx_event,
        max_depth,
    )
    .await
}

/// Inner entry point — also used by the bridge when it recurses. Returns
/// a boxed future to break the recursive opaque-future-type cycle:
/// `run_rlm_turn_inner` → `RlmBridge::dispatch` → `run_rlm_turn_inner`.
pub(crate) fn run_rlm_turn_inner(
    client: Arc<dyn RlmLlmClient>,
    model: String,
    prompt: String,
    root_prompt: Option<String>,
    child_model: String,
    tx_event: mpsc::Sender<Event>,
    max_depth: u32,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = RlmTurnResult> + Send>> {
    Box::pin(run_rlm_turn_impl(
        client,
        model,
        prompt,
        root_prompt,
        child_model,
        tx_event,
        max_depth,
    ))
}

/// RLM turns are long-running background-style work. Do not kill the whole
/// turn with the old fixed 180s wall-clock cap; per-request cancellation still
/// comes from the parent turn token and the user can cancel from the TUI.
fn turn_timeout() -> Option<Duration> {
    None
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

async fn run_rlm_turn_impl(
    client: Arc<dyn RlmLlmClient>,
    model: String,
    prompt: String,
    root_prompt: Option<String>,
    child_model: String,
    tx_event: mpsc::Sender<Event>,
    max_depth: u32,
) -> RlmTurnResult {
    let start = Instant::now();
    let mut total_usage = Usage::default();
    let mut trace: Vec<RlmRoundTrace> = Vec::new();
    let mut total_rpcs: u32 = 0;

    // 1. Stage `context` to a temp file. The REPL reads it on bootstrap so
    //    the big string never enters the process command line and doesn't
    //    show up in `ps`.
    let ctx_path = match write_context_file(&prompt) {
        Ok(p) => p,
        Err(e) => {
            return RlmTurnResult {
                answer: String::new(),
                iterations: 0,
                duration: start.elapsed(),
                error: Some(format!("rlm: failed to stage context: {e}")),
                usage: total_usage,
                termination: RlmTermination::Error,
                trace,
                total_rpcs,
            };
        }
    };

    // 2. Spawn the long-lived REPL.
    let mut repl = match PythonRuntime::spawn_with_context(&ctx_path).await {
        Ok(rt) => rt,
        Err(e) => {
            let _ = tokio::fs::remove_file(&ctx_path).await;
            return RlmTurnResult {
                answer: String::new(),
                iterations: 0,
                duration: start.elapsed(),
                error: Some(format!("rlm: failed to spawn REPL: {e}")),
                usage: total_usage,
                termination: RlmTermination::Error,
                trace,
                total_rpcs,
            };
        }
    };

    // 3. Build the bridge that services llm_query / rlm_query RPCs.
    let bridge = RlmBridge::new(Arc::clone(&client), child_model.clone(), max_depth);
    let usage_handle = bridge.usage_handle();

    let _ = tx_event
        .send(Event::status(format!(
            "RLM: spawned Python REPL (root={model}, child={child_model}, max_depth={max_depth}, ctx={} chars)",
            prompt.chars().count()
        )))
        .await;

    // 4. Build initial metadata-only history.
    let system = rlm_system_prompt();
    let mut messages: Vec<Message> = vec![build_metadata_message(
        &prompt,
        root_prompt.as_deref(),
        0,
        None,
        None,
    )];

    let mut consecutive_no_code: u32 = 0;
    let mut last_response_text = String::new();

    let result = 'turn: {
        for iteration in 0..MAX_RLM_ITERATIONS {
            if let Some(timeout) = turn_timeout()
                && start.elapsed() > timeout
            {
                break 'turn RlmTurnResult {
                    answer: String::new(),
                    iterations: iteration,
                    duration: start.elapsed(),
                    error: Some(format!("RLM turn timed out after {}s", timeout.as_secs())),
                    usage: total_usage,
                    termination: RlmTermination::Error,
                    trace: trace.clone(),
                    total_rpcs,
                };
            }

            let _ = tx_event
                .send(Event::status(format!(
                    "RLM iteration {}/{}",
                    iteration + 1,
                    MAX_RLM_ITERATIONS
                )))
                .await;

            // 4a. Root LLM generates code from metadata-only context.
            let request = build_root_request(&model, &messages, &system);

            let response = match client.create_message_boxed(request).await {
                Ok(r) => r,
                Err(e) => {
                    break 'turn RlmTurnResult {
                        answer: String::new(),
                        iterations: iteration + 1,
                        duration: start.elapsed(),
                        error: Some(format!("Root LLM call failed: {e}")),
                        usage: total_usage,
                        termination: RlmTermination::Error,
                        trace: trace.clone(),
                        total_rpcs,
                    };
                }
            };

            super::add_usage_with_prompt_cache(&mut total_usage, &response.usage);

            let response_text = extract_text_blocks(&response.content);
            last_response_text = response_text.clone();

            // 4b. Top-level FINAL(...) lets the model close out without
            //     touching the REPL — but only if it has done some work
            //     (non-zero rpc_count) on a prior round. Otherwise it's a
            //     shortcut and we reject it.
            if let Some(final_val) = parse_text_final(&response_text) {
                if total_rpcs == 0 {
                    // Discard the top-level FINAL — the model is bypassing
                    // the loop. Force it to use the REPL by appending a
                    // strict reminder.
                    consecutive_no_code = consecutive_no_code.saturating_add(1);
                    if consecutive_no_code >= MAX_CONSECUTIVE_NO_CODE {
                        break 'turn RlmTurnResult {
                            answer: final_val,
                            iterations: iteration + 1,
                            duration: start.elapsed(),
                            error: None,
                            usage: total_usage,
                            termination: RlmTermination::NoCode,
                            trace: trace.clone(),
                            total_rpcs,
                        };
                    }
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: response_text.clone(),
                            cache_control: None,
                        }],
                    });
                    messages.push(Message {
                        role: "user".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "You called FINAL(...) without ever running a ```repl block. \
                                   That defeats the recursive language model — you're guessing \
                                   from the preview alone. Emit a ```repl block now that uses \
                                   `llm_query`, `sub_query_sequence`, or an explicitly independent \
                                   `llm_query_batched(..., dependency_mode=\"independent\")` against \
                                   `context` to actually compute the answer."
                                .to_string(),
                            cache_control: None,
                        }],
                    });
                    continue;
                }
                let _ = tx_event
                    .send(Event::status(
                        "RLM: FINAL detected in response text".to_string(),
                    ))
                    .await;
                break 'turn RlmTurnResult {
                    answer: final_val,
                    iterations: iteration + 1,
                    duration: start.elapsed(),
                    error: None,
                    usage: total_usage,
                    termination: RlmTermination::Final,
                    trace: trace.clone(),
                    total_rpcs,
                };
            }

            // 4c. Extract a ```repl block.
            let code = extract_repl_code(&response_text);
            let code_to_run = match code {
                Some(c) => {
                    consecutive_no_code = 0;
                    c
                }
                None => {
                    consecutive_no_code = consecutive_no_code.saturating_add(1);
                    if consecutive_no_code >= MAX_CONSECUTIVE_NO_CODE {
                        break 'turn RlmTurnResult {
                            answer: response_text,
                            iterations: iteration + 1,
                            duration: start.elapsed(),
                            error: Some(format!(
                                "RLM: model failed to emit ```repl after {MAX_CONSECUTIVE_NO_CODE} consecutive rounds"
                            )),
                            usage: total_usage,
                            termination: RlmTermination::NoCode,
                            trace: trace.clone(),
                            total_rpcs,
                        };
                    }
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: response_text.clone(),
                            cache_control: None,
                        }],
                    });
                    messages.push(Message {
                        role: "user".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "Reminder: emit Python inside a ```repl … ``` fence. \
                                   Use `llm_query`, `sub_query_sequence`, or \
                                   `llm_query_batched(..., dependency_mode=\"independent\")` to \
                                   process `context` and call `FINAL(value)` when done."
                                .to_string(),
                            cache_control: None,
                        }],
                    });
                    continue;
                }
            };

            let _ = tx_event
                .send(Event::MessageDelta {
                    index: iteration as usize,
                    content: format!(
                        "\n[RLM round {} — code]\n```repl\n{code_to_run}\n```\n",
                        iteration + 1
                    ),
                })
                .await;

            // 4d. Execute the code in the REPL with the bridge servicing
            //     llm_query / rlm_query callbacks.
            let round = match repl.run(&code_to_run, Some(&bridge)).await {
                Ok(r) => r,
                Err(e) => {
                    break 'turn RlmTurnResult {
                        answer: String::new(),
                        iterations: iteration + 1,
                        duration: start.elapsed(),
                        error: Some(format!("REPL execution failed: {e}")),
                        usage: total_usage,
                        termination: RlmTermination::Error,
                        trace: trace.clone(),
                        total_rpcs,
                    };
                }
            };

            total_rpcs = total_rpcs.saturating_add(round.rpc_count);

            // Trace this round.
            let stdout_preview = truncate_text(round.stdout.trim(), STDOUT_METADATA_PREVIEW_LEN);
            trace.push(RlmRoundTrace {
                round: iteration + 1,
                code_summary: summarize_code(&code_to_run),
                stdout_preview: stdout_preview.clone(),
                had_error: round.has_error,
                rpc_count: round.rpc_count,
                elapsed_ms: round.elapsed.as_millis() as u64,
            });

            let _ = tx_event
                .send(Event::status(format!(
                    "RLM round {}: {} bytes stdout, {} sub-LLM call(s){}",
                    iteration + 1,
                    round.full_stdout.len(),
                    round.rpc_count,
                    if round.has_error { " (error)" } else { "" },
                )))
                .await;

            // 4e. FINAL detection.
            if let Some(final_val) = round.final_value.clone() {
                let _ = tx_event
                    .send(Event::status(
                        "RLM: FINAL detected in REPL, ending loop".to_string(),
                    ))
                    .await;
                break 'turn RlmTurnResult {
                    answer: final_val,
                    iterations: iteration + 1,
                    duration: start.elapsed(),
                    error: None,
                    usage: total_usage,
                    termination: RlmTermination::Final,
                    trace: trace.clone(),
                    total_rpcs,
                };
            }

            // 4f. Build metadata for next iteration.
            messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!("```repl\n{code_to_run}\n```"),
                    cache_control: None,
                }],
            });
            messages.push(build_metadata_message(
                &prompt,
                root_prompt.as_deref(),
                iteration + 1,
                Some(&code_to_run),
                Some(&stdout_preview),
            ));

            if messages.len() > MAX_HISTORY_MESSAGES {
                let drop_from = messages.len() - MAX_HISTORY_MESSAGES + 1;
                let mut kept = vec![messages[0].clone()];
                kept.extend(messages.drain(drop_from..));
                messages = kept;
            }
        }

        let _ = last_response_text;
        RlmTurnResult {
            answer: String::new(),
            iterations: MAX_RLM_ITERATIONS,
            duration: start.elapsed(),
            error: Some(format!(
                "RLM loop exhausted after {MAX_RLM_ITERATIONS} iterations without FINAL"
            )),
            usage: total_usage,
            termination: RlmTermination::Exhausted,
            trace: trace.clone(),
            total_rpcs,
        }
    };

    // Fold bridge usage (children + nested sub_rlm) into totals.
    let bridge_usage = usage_handle.lock().await;
    let mut final_usage = result.usage.clone();
    super::add_usage_with_prompt_cache(&mut final_usage, &bridge_usage);
    drop(bridge_usage);

    repl.shutdown().await;

    RlmTurnResult {
        usage: final_usage,
        ..result
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_context_file(prompt: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("deepseek_rlm_ctx");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "ctx_{}_{}.txt",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::write(&path, prompt)?;
    Ok(path)
}

fn build_root_request(model: &str, messages: &[Message], system: &SystemPrompt) -> MessageRequest {
    MessageRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        max_tokens: ROOT_MAX_TOKENS,
        system: Some(system.clone()),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: None,
        stream: Some(false),
        temperature: Some(ROOT_TEMPERATURE),
        top_p: Some(0.9_f32),
    }
}

/// Build `Metadata(state)` from the paper. Surfaces:
/// - the small `root_prompt` (if any) — repeated each iteration
/// - `context` length + preview
/// - the REPL helpers
/// - the previous round's code summary + stdout preview
fn build_metadata_message(
    prompt: &str,
    root_prompt: Option<&str>,
    iteration: u32,
    previous_code: Option<&str>,
    previous_stdout: Option<&str>,
) -> Message {
    let prompt_len = prompt.chars().count();
    let prompt_preview = truncate_text(prompt, PROMPT_PREVIEW_LEN);

    let mut parts = Vec::new();
    parts.push(format!("## REPL state (round {iteration})"));
    parts.push(String::new());
    if let Some(rp) = root_prompt
        && !rp.trim().is_empty()
    {
        parts.push("**Original task** (re-shown every round)".to_string());
        parts.push(format!("> {}", truncate_text(rp.trim(), 600)));
        parts.push(String::new());
    }
    parts.push("**`context`** — the long input lives in the REPL only".to_string());
    parts.push(format!("- Length: {prompt_len} chars"));
    parts.push(format!("- Preview: \"{prompt_preview}\""));
    parts.push(String::new());

    parts.push("**REPL helpers** (use inside ```repl blocks)".to_string());
    parts.push("- `context` / `ctx`                       — the full input string".to_string());
    parts.push("- `len(context)` / `context[a:b]` / `context.splitlines()` — slice it".to_string());
    parts.push(
        "- `chunk_context(max_chars=20000, overlap=0)` — full-coverage chunks with index/start/end/text"
            .to_string(),
    );
    parts.push(
        "- `chunk_coverage(chunks)`              — coverage report for chunk_context output"
            .to_string(),
    );
    parts.push(
        "- `llm_query(prompt, model=None)`        — one-shot child LLM; `model` is ignored and child calls stay pinned to Flash"
            .to_string(),
    );
    parts.push(
        "- `llm_query_batched([p1, p2, ...], dependency_mode=\"independent\")` — concurrent fan-out for independent prompts only; `model` is ignored"
            .to_string(),
    );
    parts.push(
        "- `rlm_query(prompt, model=None)`        — recursive sub-RLM; `model` is ignored"
            .to_string(),
    );
    parts.push(
        "- `rlm_query_batched([p1, p2, ...], dependency_mode=\"independent\")` — concurrent recursive sub-RLMs for independent prompts only; `model` is ignored"
            .to_string(),
    );
    parts.push(
        "- `sub_query_sequence(prompt, slices)`   — sequential child calls for A->B dependencies and rollback-sensitive work"
            .to_string(),
    );
    parts.push(
        "- Batch safety: never batch dependent steps, global-state refactors, schema migrations, or rollback-sensitive tasks"
            .to_string(),
    );
    parts.push("- `SHOW_VARS()`                          — list user variables".to_string());
    parts.push("- `repl_set(name, value)` / `repl_get(name)` — explicit store".to_string());
    parts.push(
        "- `FINAL(value)`                         — end the loop with this answer".to_string(),
    );
    parts.push(
        "- `FINAL_VAR(name)`                      — end the loop with a variable's value"
            .to_string(),
    );
    parts.push(String::new());

    if iteration > 0 {
        parts.push("**Previous round**".to_string());
        if let Some(code) = previous_code {
            parts.push(format!("- Code: {}", summarize_code(code)));
        }
        if let Some(stdout) = previous_stdout {
            let stdout_clean = stdout.trim();
            if !stdout_clean.is_empty() {
                parts.push(format!("- Stdout preview: \"{stdout_clean}\""));
            } else {
                parts.push("- Stdout: (empty)".to_string());
            }
        }
    }

    let text = parts.join("\n");

    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

fn summarize_code(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    if lines.len() <= 8 {
        return code.to_string();
    }
    let head = lines[..4].join("\n");
    let tail = lines[lines.len() - 4..].join("\n");
    format!("{} lines:\n{head}\n…\n{tail}", lines.len())
}

fn extract_text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract the first ` ```repl ` block from `text`. Falls back to
/// ` ```python `/`` ```py `` for compatibility with prompts that learned
/// the older fence style.
fn extract_repl_code(text: &str) -> Option<String> {
    let start_markers = [
        "```repl\n",
        "```repl\r\n",
        "```python\n",
        "```py\n",
        "```python\r\n",
        "```py\r\n",
    ];
    let mut best_start: Option<(usize, &str)> = None;

    for marker in &start_markers {
        if let Some(idx) = text.find(marker) {
            let end_pos = idx + marker.len();
            match best_start {
                Some((best_idx, _)) if idx < best_idx => {
                    best_start = Some((idx, &text[end_pos..]));
                }
                None => {
                    best_start = Some((idx, &text[end_pos..]));
                }
                _ => {}
            }
        }
    }

    let after_fence = best_start.map(|(_, rest)| rest)?;

    let end_idx = after_fence
        .find("\n```")
        .or_else(|| after_fence.find("```"))?;

    let code = after_fence[..end_idx].trim().to_string();
    if code.is_empty() {
        return None;
    }
    Some(code)
}

/// Parse a top-level `FINAL(...)` directive from the model's raw text.
/// Mirrors the reference RLM's `find_final_answer`: directive must appear
/// at the start of a line, *outside* any code fence.
fn parse_text_final(text: &str) -> Option<String> {
    let outside_fence = strip_code_fences(text);

    for line in outside_fence.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("FINAL_VAR(") {
            // FINAL_VAR can't be resolved from text alone — defer to REPL.
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("FINAL(") {
            let inner = rest.trim_end();
            if let Some(end) = inner.rfind(')') {
                let value = inner[..end].trim();
                if !value.is_empty() {
                    return Some(strip_quotes(value));
                }
            }
        }
    }
    None
}

fn strip_code_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn strip_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let take = max_chars.saturating_sub(3);
    let mut result: String = text.chars().take(take).collect();
    result.push_str("...");
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_repl_code_finds_simple_block() {
        let text = "Here:\n```repl\nprint('hi')\n```\nEnd.";
        let code = extract_repl_code(text).unwrap();
        assert_eq!(code, "print('hi')");
    }

    #[test]
    fn extract_repl_code_falls_back_to_python_marker() {
        let text = "Code:\n```python\nx = 1 + 2\n```";
        let code = extract_repl_code(text).unwrap();
        assert_eq!(code, "x = 1 + 2");
    }

    #[test]
    fn extract_repl_code_returns_none_when_missing() {
        assert!(extract_repl_code("Just text.").is_none());
    }

    #[test]
    fn extract_repl_code_returns_none_on_empty_block() {
        assert!(extract_repl_code("```repl\n\n```").is_none());
    }

    #[test]
    fn extract_repl_code_handles_multiple_blocks() {
        let text = "```repl\na=1\n```\n```repl\nb=2\n```";
        let code = extract_repl_code(text).unwrap();
        assert_eq!(code, "a=1");
    }

    #[test]
    fn extract_repl_code_ignores_other_fences() {
        let text = "```\nfoo\n```\n```repl\nreal_code()\n```";
        let code = extract_repl_code(text).unwrap();
        assert_eq!(code, "real_code()");
    }

    #[test]
    fn parse_text_final_extracts_simple_value() {
        let text = "OK.\nFINAL(42)\nThanks.";
        assert_eq!(parse_text_final(text).as_deref(), Some("42"));
    }

    #[test]
    fn parse_text_final_strips_quotes() {
        let text = "FINAL(\"the answer is yes\")";
        assert_eq!(parse_text_final(text).as_deref(), Some("the answer is yes"));
    }

    #[test]
    fn parse_text_final_ignores_inside_code_fence() {
        let text =
            "Some prose.\n```repl\n# Note: when ready, call FINAL(value)\nx = 1\n```\nMore prose.";
        assert!(parse_text_final(text).is_none());
    }

    #[test]
    fn parse_text_final_returns_none_when_absent() {
        assert!(parse_text_final("just talking, no final.").is_none());
    }

    #[test]
    fn build_metadata_contains_key_information() {
        let msg = build_metadata_message("Hello, world!", None, 0, None, None);
        let text = extract_text_blocks(&msg.content);
        assert!(text.contains("context"));
        assert!(text.contains("Hello, world!"));
        assert!(text.contains("round 0"));
        assert!(text.contains("llm_query"));
        assert!(text.contains("rlm_query"));
        assert!(text.contains("FINAL"));
    }

    #[test]
    fn build_metadata_truncates_long_context_without_leaking_tail() {
        let secret_tail = "DO_NOT_LEAK_CONTEXT_TAIL";
        let prompt = format!("{}{}", "a".repeat(PROMPT_PREVIEW_LEN + 100), secret_tail);
        let msg = build_metadata_message(&prompt, None, 0, None, None);
        let text = extract_text_blocks(&msg.content);

        assert!(text.contains(&format!("- Length: {} chars", prompt.chars().count())));
        assert!(text.contains("- Preview: \""));
        assert!(text.contains("..."));
        assert!(
            !text.contains(secret_tail),
            "metadata leaked the non-preview tail of context"
        );
    }

    #[test]
    fn build_root_request_keeps_context_tail_out_of_root_payload() {
        let secret_tail = "DO_NOT_LEAK_ROOT_REQUEST";
        let prompt = format!("{}{}", "a".repeat(PROMPT_PREVIEW_LEN + 100), secret_tail);
        let messages = vec![build_metadata_message(
            &prompt,
            Some("answer from the long context"),
            0,
            None,
            None,
        )];

        let request = build_root_request("root-model", &messages, &rlm_system_prompt());
        let payload = serde_json::to_string(&request).expect("request should serialize");

        assert!(payload.contains(&format!("- Length: {} chars", prompt.chars().count())));
        assert!(
            !payload.contains(secret_tail),
            "root LLM request leaked the non-preview tail of context"
        );
    }

    #[test]
    fn build_metadata_with_iteration_shows_previous_code() {
        let msg = build_metadata_message("Test prompt", None, 3, Some("print('hi')"), Some("hi"));
        let text = extract_text_blocks(&msg.content);
        assert!(text.contains("round 3"));
        assert!(text.contains("print('hi')"));
        assert!(text.contains("hi"));
    }

    #[test]
    fn build_metadata_includes_root_prompt() {
        let msg = build_metadata_message(
            "long context",
            Some("Summarize the security model"),
            1,
            Some("# noop"),
            Some("ok"),
        );
        let text = extract_text_blocks(&msg.content);
        assert!(text.contains("Original task"));
        assert!(text.contains("Summarize the security model"));
    }

    #[test]
    fn truncate_text_leaves_short_alone() {
        assert_eq!(truncate_text("hello", 100), "hello");
    }

    #[test]
    fn truncate_text_shortens_long_text() {
        let long = "a".repeat(1000);
        let truncated = truncate_text(&long, 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn truncate_text_is_unicode_safe() {
        let s = "日本語テスト";
        let out = truncate_text(s, 4);
        assert_eq!(out.chars().count(), 4);
        assert!(out.ends_with("..."));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn extract_text_blocks_joins_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "first".to_string(),
                cache_control: None,
            },
            ContentBlock::Thinking {
                thinking: "skip".to_string(),
            },
            ContentBlock::Text {
                text: "second".to_string(),
                cache_control: None,
            },
        ];
        assert_eq!(extract_text_blocks(&blocks), "first\nsecond");
    }

    #[test]
    fn metadata_msg_role_is_user() {
        let msg = build_metadata_message("test", None, 0, None, None);
        assert_eq!(msg.role, "user");
    }

    #[test]
    fn summarize_code_keeps_short() {
        assert_eq!(summarize_code("a\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn summarize_code_compresses_long() {
        let lines: Vec<String> = (0..20).map(|i| format!("line{i}")).collect();
        let code = lines.join("\n");
        let s = summarize_code(&code);
        assert!(s.starts_with("20 lines:"));
        assert!(s.contains("line0"));
        assert!(s.contains("line19"));
        assert!(s.contains("…"));
    }

    #[test]
    fn rlm_turn_has_no_fixed_wall_clock_timeout() {
        assert!(
            turn_timeout().is_none(),
            "RLM turns should not be killed by the old fixed 180s wall-clock cap"
        );
    }
}
