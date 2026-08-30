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
use uuid::Uuid;

use crate::llm_client::LlmClient;
use crate::models::Role;
use crate::models::{
    ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Usage,
    is_incomplete_stop_reason, stop_reason_detail,
};
use crate::repl::runtime::{BatchResp, RpcDispatcher, RpcRequest, RpcResponse, SingleResp};
use crate::utils::spawn_supervised;

/// One pre-dispatch reservation in the shared routed-usage ledger.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RlmUsageReservation {
    index: usize,
}

#[derive(Debug, Default)]
struct RlmUsageState {
    ledger_id: String,
    usage: Usage,
    records: Vec<Option<RlmUsageSlot>>,
    drop_records: Vec<crate::cost_status::RuntimeUsageDropRecord>,
    dropped_records: u64,
}

#[derive(Debug)]
struct RlmUsageSlot {
    record: crate::cost_status::RuntimeUsageRecord,
    completed: bool,
}

/// Shared, bounded provider-call ledger for one complete RLM tree.
///
/// Every root, child, batch member, and recursive call reserves one slot
/// before invoking a provider. A distinct call is never coalesced merely
/// because it used the same route: its dispatch instant and frozen quote are
/// independent accounting evidence. Sharing one accumulator across recursion
/// makes the bound global instead of allowing every nested bridge to reset it.
#[derive(Debug, Clone)]
pub(crate) struct RlmUsageAccumulator {
    state: Arc<Mutex<RlmUsageState>>,
}

/// Atomic snapshot returned after all RPC work for a round has settled.
#[derive(Debug, Clone, Default)]
pub(crate) struct RlmUsageSnapshot {
    pub usage: Usage,
    pub records: Vec<crate::cost_status::RuntimeUsageRecord>,
    /// Exact frozen routes for provider-success responses that did not carry
    /// authoritative usage. Keeping these separate prevents a missing payload
    /// from becoming a priced zero-usage receipt.
    pub drop_records: Vec<crate::cost_status::RuntimeUsageDropRecord>,
    /// Calls whose execution/usage became ambiguous (for example a timeout).
    /// They are never represented as authoritative zero-usage responses.
    pub dropped_records: u64,
}

impl RlmUsageAccumulator {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RlmUsageState {
                ledger_id: Uuid::new_v4().simple().to_string(),
                ..RlmUsageState::default()
            })),
        }
    }

    /// Reserve durable accounting capacity before a provider request.
    /// Definite transport failure cancels the slot; ambiguous cancellation is
    /// explicit incomplete coverage. Reaching the cap rejects before any
    /// unreceipted provider work can occur.
    pub(crate) async fn reserve(
        &self,
        route: crate::cost_status::EffectiveRouteEnvelope,
    ) -> std::result::Result<RlmUsageReservation, String> {
        let mut state = self.state.lock().await;
        if state.records.len() == crate::cost_status::MAX_CHILD_USAGE_RECORDS {
            return Err(format!(
                "RLM provider-call receipt limit reached ({}); request rejected before dispatch",
                crate::cost_status::MAX_CHILD_USAGE_RECORDS
            ));
        }
        let index = state.records.len();
        let source_id = format!("rlm:{}:request:{index}", state.ledger_id);
        state.records.push(Some(RlmUsageSlot {
            record: crate::cost_status::RuntimeUsageRecord {
                source_id,
                usage: crate::cost_status::EffectiveRouteUsage {
                    route: route.sanitized_for_persistence(),
                    usage: Usage::default(),
                },
            },
            completed: false,
        }));
        Ok(RlmUsageReservation { index })
    }

    /// Attach a provider's reported usage to its already-reserved exact route.
    pub(crate) async fn complete(&self, reservation: RlmUsageReservation, usage: &Usage) {
        let mut state = self.state.lock().await;
        let completed = if let Some(Some(slot)) = state.records.get_mut(reservation.index)
            && !slot.completed
        {
            super::add_usage_with_prompt_cache(&mut slot.record.usage.usage, usage);
            slot.completed = true;
            true
        } else {
            false
        };
        if completed {
            super::add_usage_with_prompt_cache(&mut state.usage, usage);
        }
    }

    /// Settle a decoded provider-success response without inventing usage.
    /// `MessageResponse::usage == Usage::default()` is also what adapters
    /// produce when the provider omitted the payload, so it is not proof of a
    /// genuine zero-token request.
    pub(crate) async fn settle_provider_success(
        &self,
        reservation: RlmUsageReservation,
        usage: &Usage,
    ) {
        if usage == &Usage::default() {
            self.cancel(reservation, true).await;
        } else {
            self.complete(reservation, usage).await;
        }
    }

    /// Remove a reservation that never produced provider-reported usage.
    /// Ambiguous execution increments explicit incomplete coverage instead of
    /// being persisted as a priced-zero response.
    pub(crate) async fn cancel(&self, reservation: RlmUsageReservation, coverage_unknown: bool) {
        let mut state = self.state.lock().await;
        let cancelled = state.records.get_mut(reservation.index).and_then(|slot| {
            if slot.as_ref().is_some_and(|slot| !slot.completed) {
                slot.take()
            } else {
                None
            }
        });
        if let Some(slot) = cancelled
            && coverage_unknown
        {
            state
                .drop_records
                .push(crate::cost_status::RuntimeUsageDropRecord {
                    source_id: slot.record.source_id,
                    route: slot.record.usage.route,
                });
            state.dropped_records = state.dropped_records.saturating_add(1);
        }
    }

    pub(crate) async fn snapshot(&self) -> RlmUsageSnapshot {
        let state = self.state.lock().await;
        let pending = state
            .records
            .iter()
            .flatten()
            .filter(|slot| !slot.completed)
            .map(|slot| crate::cost_status::RuntimeUsageDropRecord {
                source_id: slot.record.source_id.clone(),
                route: slot.record.usage.route.clone(),
            })
            .collect::<Vec<_>>();
        let mut drop_records = state.drop_records.clone();
        drop_records.extend(pending.iter().cloned());
        RlmUsageSnapshot {
            usage: state.usage.clone(),
            records: state
                .records
                .iter()
                .flatten()
                .filter(|slot| slot.completed)
                .map(|slot| slot.record.clone())
                .collect(),
            drop_records,
            dropped_records: state
                .dropped_records
                .saturating_add(u64::try_from(pending.len()).unwrap_or(u64::MAX)),
        }
    }
}

/// Object-safe runtime-model adapter for a working kernel.
///
/// The normal turn loop owns a `SharedModelClient`, while the original RLM
/// bridge predates that boundary and accepts the concrete [`LlmClient`] trait.
/// Keeping this small adapter here means a persistent kernel follows exactly
/// the selected model route (including custom providers) without teaching the
/// kernel about provider transports or falling back to a side channel.
pub(crate) struct ModelClientRlmAdapter {
    client: crate::core::model_client::SharedModelClient,
}

impl ModelClientRlmAdapter {
    pub(crate) fn new(client: crate::core::model_client::SharedModelClient) -> Self {
        Self { client }
    }
}

/// Per-child completion timeout — same as the previous sidecar default.
const CHILD_TIMEOUT_SECS: u64 = 120;
/// Hard cap on prompts per batch RPC.
pub const MAX_BATCH: usize = 16;

/// Object-safe slice of the LLM client interface that the RLM bridge needs.
///
/// `LlmClient` itself uses native async trait methods, which are not dyn-safe.
/// The bridge only needs non-streaming completions, so this boxed-future shim
/// gives tests a clean mock seam without changing the wider provider trait.
pub(crate) trait RlmLlmClient: Send + Sync {
    fn effective_route_envelope(
        &self,
        requested_model: &str,
        dispatched_at: chrono::DateTime<chrono::Utc>,
    ) -> crate::cost_status::EffectiveRouteEnvelope;

    fn effective_max_output_tokens(&self, requested_model: &str) -> u32;

    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>>;
}

impl RlmLlmClient for ModelClientRlmAdapter {
    fn effective_route_envelope(
        &self,
        requested_model: &str,
        dispatched_at: chrono::DateTime<chrono::Utc>,
    ) -> crate::cost_status::EffectiveRouteEnvelope {
        self.client
            .effective_route_envelope(requested_model, dispatched_at)
    }

    fn effective_max_output_tokens(&self, requested_model: &str) -> u32 {
        self.client.effective_max_output_tokens(requested_model)
    }

    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>> {
        let client = Arc::clone(&self.client);
        Box::pin(async move { client.create_message(request).await })
    }
}

impl<T> RlmLlmClient for T
where
    T: LlmClient + Send + Sync,
{
    fn effective_route_envelope(
        &self,
        requested_model: &str,
        dispatched_at: chrono::DateTime<chrono::Utc>,
    ) -> crate::cost_status::EffectiveRouteEnvelope {
        LlmClient::effective_route_envelope(self, requested_model, dispatched_at)
    }

    fn effective_max_output_tokens(&self, requested_model: &str) -> u32 {
        LlmClient::effective_max_output_tokens(self, requested_model)
    }

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
    usage: RlmUsageAccumulator,
}

impl RlmBridge {
    pub(crate) fn new(
        client: Arc<dyn RlmLlmClient>,
        child_model: String,
        depth_remaining: u32,
    ) -> Self {
        Self::with_usage_accumulator(
            client,
            child_model,
            depth_remaining,
            RlmUsageAccumulator::new(),
        )
    }

    pub(crate) fn with_usage_accumulator(
        client: Arc<dyn RlmLlmClient>,
        child_model: String,
        depth_remaining: u32,
        usage: RlmUsageAccumulator,
    ) -> Self {
        Self {
            client,
            child_model,
            depth_remaining,
            usage,
        }
    }

    pub(crate) async fn usage_snapshot(&self) -> RlmUsageSnapshot {
        self.usage.snapshot().await
    }

    async fn dispatch_llm(
        &self,
        prompt: String,
        _model: Option<String>,
        max_tokens: Option<u32>,
        system: Option<String>,
    ) -> SingleResp {
        let request_route = self
            .client
            .effective_route_envelope(&self.child_model, chrono::Utc::now());
        let reservation = match self.usage.reserve(request_route.clone()).await {
            Ok(reservation) => reservation,
            Err(error) => {
                return SingleResp {
                    text: String::new(),
                    error: Some(error),
                };
            }
        };
        let route_max_tokens = self
            .client
            .effective_max_output_tokens(&request_route.model);
        let request = MessageRequest {
            // The Python helper accepts `model=` for older snippets, but it is
            // intentionally not authoritative. RLM child calls are pinned to
            // the tool's configured child model so model-generated Python
            // cannot silently upgrade cheap fanout work to an expensive model.
            model: self.child_model.clone(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            // An explicit RLM helper bound remains authoritative, but the
            // default is the selected route's ordinary allowance rather than
            // a hidden 4K ceiling.
            max_tokens: max_tokens.map_or(route_max_tokens, |limit| limit.min(route_max_tokens)),
            system: system.map(SystemPrompt::Text),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: None,
            top_p: None,
        };

        let fut = self.client.create_message_boxed(request);
        let response =
            match tokio::time::timeout(Duration::from_secs(CHILD_TIMEOUT_SECS), fut).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    self.usage.cancel(reservation, false).await;
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query failed: {e}")),
                    };
                }
                Err(_) => {
                    self.usage.cancel(reservation, true).await;
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query timed out after {CHILD_TIMEOUT_SECS}s")),
                    };
                }
            };

        // Incomplete output is rejected below, but it is still a successful
        // provider response and therefore billed. Complete the reserved route
        // before inspecting the stop reason.
        self.usage
            .settle_provider_success(reservation, &response.usage)
            .await;

        if is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            return SingleResp {
                text: String::new(),
                error: Some(format!(
                    "llm_query response incomplete: provider stop reason `{}`; partial output was not accepted.",
                    stop_reason_detail(response.stop_reason.as_deref())
                )),
            };
        }

        let text = response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

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
        let result = super::turn::run_rlm_turn_inner_with_usage(
            Arc::clone(&self.client),
            child_model.clone(),
            prompt,
            None,
            child_model,
            tx,
            self.depth_remaining.saturating_sub(1),
            self.usage.clone(),
        )
        .await;

        drain.abort();

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

        let snapshot = bridge.usage_snapshot().await;
        assert_eq!(snapshot.usage.input_tokens, 7);
        assert_eq!(snapshot.usage.output_tokens, 11);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].usage.usage, snapshot.usage);
        assert!(snapshot.drop_records.is_empty());
        assert_eq!(snapshot.dropped_records, 0);
    }

    #[tokio::test]
    async fn llm_dispatch_keeps_semantic_success_but_marks_missing_usage_once() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response_with_usage(
            "usable child answer",
            Usage::default(),
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

        let RpcResponse::Single(response) = response else {
            panic!("expected single response");
        };
        assert_eq!(response.text, "usable child answer");
        assert!(response.error.is_none());

        let first = bridge.usage_snapshot().await;
        let replay = bridge.usage_snapshot().await;
        assert_eq!(first.usage, Usage::default());
        assert!(first.records.is_empty());
        assert_eq!(first.drop_records.len(), 1);
        assert_eq!(first.dropped_records, 1);
        assert_eq!(replay.drop_records, first.drop_records);
        assert_eq!(replay.dropped_records, 1);
        assert_eq!(first.drop_records[0].route.model, "child-model");
        assert!(first.drop_records[0].source_id.starts_with("rlm:"));
    }

    #[tokio::test]
    async fn repeated_reservation_settlement_cannot_duplicate_usage_or_missing_coverage() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        let bridge = bridge_for(Arc::clone(&mock), 1);
        let route = RlmLlmClient::effective_route_envelope(
            mock.as_ref(),
            "child-model",
            chrono::Utc::now(),
        );

        let usage_reservation = bridge
            .usage
            .reserve(route.clone())
            .await
            .expect("usage reservation");
        let reported = Usage {
            input_tokens: 3,
            output_tokens: 5,
            ..Usage::default()
        };
        bridge.usage.complete(usage_reservation, &reported).await;
        bridge.usage.complete(usage_reservation, &reported).await;

        let missing_reservation = bridge
            .usage
            .reserve(route)
            .await
            .expect("missing reservation");
        bridge.usage.cancel(missing_reservation, true).await;
        bridge.usage.cancel(missing_reservation, true).await;

        let snapshot = bridge.usage_snapshot().await;
        assert_eq!(snapshot.usage, reported);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.drop_records.len(), 1);
        assert_eq!(snapshot.dropped_records, 1);
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

        let usage = bridge.usage_snapshot().await.usage;
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(800));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(200));
    }

    #[tokio::test]
    async fn llm_dispatch_rejects_max_tokens_partial_output_after_charging_usage() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        let usage = Usage {
            input_tokens: 23,
            output_tokens: 4096,
            reasoning_tokens: Some(4000),
            ..Usage::default()
        };
        let mut response = mock_response_with_usage(
            "FINAL('partial answer')\n```repl\nFINAL('also partial')\n```",
            usage.clone(),
        );
        response.stop_reason = Some("max_tokens".to_string());
        mock.push_message_response(response);
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
                assert!(
                    single.text.is_empty(),
                    "partial output must not be accepted"
                );
                let error = single.error.expect("truncation must surface as an error");
                assert!(error.contains("incomplete"), "{error}");
                assert!(error.contains("max_tokens"), "{error}");
            }
            other => panic!("expected single response, got {other:?}"),
        }

        let snapshot = bridge.usage_snapshot().await;
        assert_eq!(snapshot.usage, usage);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].usage.usage, usage);
        assert_eq!(mock.call_count(), 1, "truncation must not retry");
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

        let snapshot = bridge.usage_snapshot().await;
        assert_eq!(snapshot.usage.input_tokens, 9);
        assert_eq!(snapshot.usage.output_tokens, 12);
        assert_eq!(snapshot.records.len(), 3);
        assert_ne!(
            snapshot.records[0].source_id, snapshot.records[1].source_id,
            "distinct provider calls must keep distinct stable identities"
        );
    }

    #[tokio::test]
    async fn shared_accumulator_rejects_the_first_unreceipted_request_before_provider_work() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        let client: Arc<dyn RlmLlmClient> = mock.clone();
        let usage = RlmUsageAccumulator::new();
        let bridge = RlmBridge::with_usage_accumulator(
            Arc::clone(&client),
            "child-model".to_string(),
            1,
            usage.clone(),
        );
        let nested_bridge =
            RlmBridge::with_usage_accumulator(client, "child-model".to_string(), 1, usage);
        let route = RlmLlmClient::effective_route_envelope(
            mock.as_ref(),
            "child-model",
            chrono::Utc::now(),
        );
        for _ in 0..crate::cost_status::MAX_CHILD_USAGE_RECORDS {
            let reservation = bridge
                .usage
                .reserve(route.clone())
                .await
                .expect("receipt slot below cap");
            bridge
                .usage
                .complete(
                    reservation,
                    &Usage {
                        input_tokens: 1,
                        ..Usage::default()
                    },
                )
                .await;
        }

        let response = nested_bridge
            .dispatch(RpcRequest::Llm {
                prompt: "must not reach provider".to_string(),
                model: None,
                max_tokens: None,
                system: None,
            })
            .await;
        let RpcResponse::Single(response) = response else {
            panic!("expected single response");
        };
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("rejected before dispatch"))
        );
        assert_eq!(mock.call_count(), 0);
        let snapshot = bridge.usage_snapshot().await;
        assert_eq!(
            snapshot.records.len(),
            crate::cost_status::MAX_CHILD_USAGE_RECORDS
        );
        assert_eq!(snapshot.dropped_records, 0);
        assert!(snapshot.drop_records.is_empty());
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

        let usage = bridge.usage_snapshot().await.usage;
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "child-model");
    }
}
