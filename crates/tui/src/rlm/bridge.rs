//! RPC bridge that services `llm_query` / `rlm_query` calls coming back
//! from the long-lived Python REPL during an RLM turn.
//!
//! This is the spiritual successor to the HTTP sidecar from earlier
//! versions — except instead of binding a localhost port and routing
//! through `urllib`, requests come in through stdin/stdout and we just
//! call the LLM client directly here in Rust.
//!
//! The bridge tracks cumulative token usage and the recursion budget. For
//! `Rlm` / `RlmBatch` requests it recursively calls `run_rlm_turn_inner`
//! at depth-1; the future-type cycle (bridge → run_rlm_turn_inner →
//! bridge) is broken by `run_rlm_turn_inner` returning a boxed dyn future.

use std::sync::Arc;
use std::time::Duration;
use std::{future::Future, pin::Pin};

use anyhow::Result;
use futures_util::future::join_all;
use tokio::sync::Mutex;

use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Usage};
use crate::repl::runtime::{BatchResp, RpcDispatcher, RpcRequest, RpcResponse, SingleResp};
use crate::utils::spawn_supervised;

/// Per-child completion timeout — same as the previous sidecar default.
const CHILD_TIMEOUT_SECS: u64 = 120;
/// Default `max_tokens` for one-shot child completions.
const DEFAULT_CHILD_MAX_TOKENS: u32 = 4096;
/// Hard cap on prompts per batch RPC.
pub const MAX_BATCH: usize = 16;

/// Object-safe slice of the LLM client interface that the RLM bridge needs.
///
/// `LlmClient` itself uses native async trait methods, which are not dyn-safe.
/// The bridge only needs non-streaming completions, so this boxed-future shim
/// gives tests a clean mock seam without changing the wider provider trait.
pub(crate) trait RlmLlmClient: Send + Sync {
    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>>;
}

impl<T> RlmLlmClient for T
where
    T: LlmClient + Send + Sync,
{
    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>> {
        Box::pin(self.create_message(request))
    }
}

/// State shared with the bridge across all RPC calls in one turn.
pub struct RlmBridge {
    client: Arc<dyn RlmLlmClient>,
    child_model: String,
    /// Recursion budget remaining for `Rlm` / `RlmBatch` requests. When
    /// zero, those requests fall back to plain `Llm` completions.
    depth_remaining: u32,
    usage: Arc<Mutex<Usage>>,
}

impl RlmBridge {
    pub(crate) fn new(
        client: Arc<dyn RlmLlmClient>,
        child_model: String,
        depth_remaining: u32,
    ) -> Self {
        Self {
            client,
            child_model,
            depth_remaining,
            usage: Arc::new(Mutex::new(Usage::default())),
        }
    }

    pub fn usage_handle(&self) -> Arc<Mutex<Usage>> {
        Arc::clone(&self.usage)
    }

    async fn dispatch_llm(
        &self,
        prompt: String,
        _model: Option<String>,
        max_tokens: Option<u32>,
        system: Option<String>,
    ) -> SingleResp {
        let request = MessageRequest {
            // The Python helper accepts `model=` for older snippets, but it is
            // intentionally not authoritative. RLM child calls are pinned to
            // the tool's configured child model so model-generated Python
            // cannot silently upgrade cheap fanout work to an expensive model.
            model: self.child_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            max_tokens: max_tokens.unwrap_or(DEFAULT_CHILD_MAX_TOKENS),
            system: system.map(SystemPrompt::Text),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.4_f32),
            top_p: Some(0.9_f32),
        };

        let fut = self.client.create_message_boxed(request);
        let response =
            match tokio::time::timeout(Duration::from_secs(CHILD_TIMEOUT_SECS), fut).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query failed: {e}")),
                    };
                }
                Err(_) => {
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query timed out after {CHILD_TIMEOUT_SECS}s")),
                    };
                }
            };

        let text = response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        {
            let mut u = self.usage.lock().await;
            super::add_usage_with_prompt_cache(&mut u, &response.usage);
        }

        SingleResp { text, error: None }
    }

    async fn dispatch_llm_batch(
        &self,
        prompts: Vec<String>,
        _model: Option<String>,
        dependency_mode: Option<String>,
    ) -> BatchResp {
        if let Some(resp) = batch_guard(prompts.len(), dependency_mode.as_deref()) {
            return resp;
        }

        let model = Arc::new(self.child_model.clone());

        let futures = prompts.into_iter().map(|prompt| {
            let model = Arc::clone(&model);
            async move {
                self.dispatch_llm((*prompt).to_string(), Some((*model).clone()), None, None)
                    .await
            }
        });

        BatchResp {
            results: join_all(futures).await,
        }
    }

    async fn dispatch_rlm(&self, prompt: String, _model: Option<String>) -> SingleResp {
        if self.depth_remaining == 0 {
            // Budget exhausted — fall back to a one-shot child completion
            // rather than returning an error. Matches the paper's behaviour
            // ("sub_RLM gracefully degrades to llm_query at depth=0").
            return self.dispatch_llm(prompt, None, None, None).await;
        }

        // Build a drain channel to absorb status events from the nested
        // turn (we don't surface them; this dispatch is invisible to the
        // outer agent stream).
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = spawn_supervised(
            "rlm-bridge-drain",
            std::panic::Location::caller(),
            async move { while rx.recv().await.is_some() {} },
        );

        let child_model = self.child_model.clone();

        // Recursive call. The dyn-erasure on `run_rlm_turn_inner` breaks
        // the `bridge → turn → bridge` opaque-future cycle.
        let result = super::turn::run_rlm_turn_inner(
            Arc::clone(&self.client),
            child_model.clone(),
            prompt,
            None,
            child_model,
            tx,
            self.depth_remaining.saturating_sub(1),
        )
        .await;

        drain.abort();

        {
            let mut u = self.usage.lock().await;
            super::add_usage_with_prompt_cache(&mut u, &result.usage);
        }

        SingleResp {
            text: result.answer,
            error: result.error,
        }
    }

    async fn dispatch_rlm_batch(
        &self,
        prompts: Vec<String>,
        _model: Option<String>,
        dependency_mode: Option<String>,
    ) -> BatchResp {
        if let Some(resp) = batch_guard(prompts.len(), dependency_mode.as_deref()) {
            return resp;
        }

        let futures = prompts
            .into_iter()
            .map(|p| async move { self.dispatch_rlm(p, None).await });
        BatchResp {
            results: join_all(futures).await,
        }
    }
}

fn batch_guard(prompt_count: usize, dependency_mode: Option<&str>) -> Option<BatchResp> {
    if prompt_count == 0 {
        return Some(BatchResp { results: vec![] });
    }
    if prompt_count > MAX_BATCH {
        return Some(BatchResp {
            results: (0..prompt_count)
                .map(|_| SingleResp {
                    text: String::new(),
                    error: Some(format!("batch too large: {prompt_count} > {MAX_BATCH}")),
                })
                .collect(),
        });
    }
    let mode = dependency_mode
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    if !matches!(
        mode.as_str(),
        "independent" | "parallel_safe" | "map_reduce"
    ) {
        return Some(BatchResp {
            results: (0..prompt_count)
                .map(|_| SingleResp {
                    text: String::new(),
                    error: Some(
                        "batch requires dependency_mode='independent'; use sub_query_sequence or sequential sub_query calls for dependent work"
                            .to_string(),
                    ),
                })
                .collect(),
        });
    }
    None
}

impl RpcDispatcher for RlmBridge {
    fn dispatch<'a>(
        &'a self,
        req: RpcRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RpcResponse> + Send + 'a>> {
        Box::pin(async move {
            match req {
                RpcRequest::Llm {
                    prompt,
                    model,
                    max_tokens,
                    system,
                } => {
                    RpcResponse::Single(self.dispatch_llm(prompt, model, max_tokens, system).await)
                }
                RpcRequest::LlmBatch {
                    prompts,
                    model,
                    dependency_mode,
                    safety_note: _,
                } => RpcResponse::Batch(
                    self.dispatch_llm_batch(prompts, model, dependency_mode)
                        .await,
                ),
                RpcRequest::Rlm { prompt, model } => {
                    RpcResponse::Single(self.dispatch_rlm(prompt, model).await)
                }
                RpcRequest::RlmBatch {
                    prompts,
                    model,
                    dependency_mode,
                    safety_note: _,
                } => RpcResponse::Batch(
                    self.dispatch_rlm_batch(prompts, model, dependency_mode)
                        .await,
                ),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::mock::MockLlmClient;

    fn mock_response_with_usage(text: &str, usage: Usage) -> MessageResponse {
        MessageResponse {
            id: "mock_msg".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            model: "mock-model".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            container: None,
            usage,
        }
    }

    fn mock_response(text: &str, input_tokens: u32, output_tokens: u32) -> MessageResponse {
        mock_response_with_usage(
            text,
            Usage {
                input_tokens,
                output_tokens,
                ..Usage::default()
            },
        )
    }

    fn bridge_for(mock: Arc<MockLlmClient>, depth_remaining: u32) -> RlmBridge {
        let client: Arc<dyn RlmLlmClient> = mock;
        RlmBridge::new(client, "child-model".to_string(), depth_remaining)
    }

    #[test]
    fn batch_guard_allows_non_empty_batches_at_the_cap() {
        assert!(batch_guard(MAX_BATCH, Some("independent")).is_none());
    }

    #[test]
    fn batch_guard_returns_empty_response_for_empty_batches() {
        let response = batch_guard(0, None).expect("empty batch should be handled");
        assert!(response.results.is_empty());
    }

    #[test]
    fn batch_guard_returns_one_error_per_oversized_prompt() {
        let response = batch_guard(MAX_BATCH + 2, Some("independent"))
            .expect("oversized batch should be handled");
        assert_eq!(response.results.len(), MAX_BATCH + 2);
        assert!(response.results.iter().all(|result| {
            result.text.is_empty()
                && result
                    .error
                    .as_deref()
                    .is_some_and(|err| err.contains("batch too large"))
        }));
    }

    #[test]
    fn batch_guard_requires_explicit_independence_for_parallel_work() {
        let response = batch_guard(2, None).expect("missing dependency mode should be handled");
        assert_eq!(response.results.len(), 2);
        assert!(response.results.iter().all(|result| {
            result.text.is_empty()
                && result
                    .error
                    .as_deref()
                    .is_some_and(|err| err.contains("dependency_mode='independent'"))
        }));

        let response = batch_guard(2, Some("sequential"))
            .expect("dependent dependency mode should be handled");
        assert!(response.results.iter().all(|result| {
            result
                .error
                .as_deref()
                .is_some_and(|err| err.contains("sub_query_sequence"))
        }));
    }

    #[tokio::test]
    async fn llm_dispatch_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("child answer", 7, 11));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::Llm {
                prompt: "child prompt".to_string(),
                model: Some("override-model".to_string()),
                max_tokens: Some(123),
                system: Some("child system".to_string()),
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "child answer");
                assert!(single.error.is_none());
            }
            other => panic!("expected single response, got {other:?}"),
        }

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "child-model");
        assert_eq!(captured[0].max_tokens, 123);
        assert_eq!(
            captured[0].system,
            Some(SystemPrompt::Text("child system".to_string()))
        );

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 11);
    }

    #[tokio::test]
    async fn llm_dispatch_preserves_prompt_cache_usage() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response_with_usage(
            "cached child answer",
            Usage {
                input_tokens: 1000,
                output_tokens: 100,
                prompt_cache_hit_tokens: Some(800),
                prompt_cache_miss_tokens: Some(200),
                ..Usage::default()
            },
        ));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::Llm {
                prompt: "child prompt".to_string(),
                model: None,
                max_tokens: None,
                system: None,
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "cached child answer");
                assert!(single.error.is_none());
            }
            other => panic!("expected single response, got {other:?}"),
        }

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(800));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(200));
    }

    #[tokio::test]
    async fn llm_batch_dispatch_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("one", 1, 2));
        mock.push_message_response(mock_response("two", 3, 4));
        mock.push_message_response(mock_response("three", 5, 6));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::LlmBatch {
                prompts: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                model: Some("batch-model".to_string()),
                dependency_mode: Some("independent".to_string()),
                safety_note: Some("test prompts are independent".to_string()),
            })
            .await;

        match response {
            RpcResponse::Batch(batch) => {
                let texts: Vec<_> = batch
                    .results
                    .iter()
                    .map(|result| result.text.as_str())
                    .collect();
                assert_eq!(texts, ["one", "two", "three"]);
                assert!(batch.results.iter().all(|result| result.error.is_none()));
            }
            other => panic!("expected batch response, got {other:?}"),
        }

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 3);
        assert!(
            captured
                .iter()
                .all(|request| request.model == "child-model")
        );

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 9);
        assert_eq!(usage.output_tokens, 12);
    }

    #[tokio::test]
    async fn rlm_dispatch_at_depth_zero_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("fallback answer", 3, 5));
        let bridge = bridge_for(Arc::clone(&mock), 0);

        let response = bridge
            .dispatch(RpcRequest::Rlm {
                prompt: "nested prompt".to_string(),
                model: Some("override-model".to_string()),
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "fallback answer");
                assert!(single.error.is_none());
            }
            other => panic!("expected single response, got {other:?}"),
        }

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "child-model");
    }
}
