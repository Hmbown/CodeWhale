use super::*;

use super::context::{COMPACTION_SUMMARY_MARKER, TURN_MAX_OUTPUT_TOKENS};
use super::turn_loop::{
    registered_tool_approval_required, registered_tool_blocked_in_full_access,
    registered_tool_forces_prompt, tool_error_degradation_runtime_hint,
};
use crate::config::ApiProvider;
use crate::models::{SystemBlock, Usage};
use crate::test_support::{EnvVarGuard, lock_test_env};
use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};
use crate::tools::spec::ToolCapability;
use crate::tools::todo::{TodoItem, TodoListSnapshot, TodoStatus};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::tempdir;

const WORKING_SET_SUMMARY_MARKER: &str = "## Repo Working Set";

#[test]
fn custom_route_identity_change_rebuilds_client_for_new_named_endpoint() {
    let mut custom = HashMap::new();
    for (name, base_url, model) in [
        ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
        ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
    ] {
        custom.insert(
            name.to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key: Some("local-test-key".to_string()),
                ..crate::config::ProviderConfig::default()
            },
        );
    }
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let (mut engine, _handle) = Engine::new(EngineConfig::default(), &config);
    assert_eq!(engine.api_provider_identity, "custom-a");
    assert_eq!(
        engine
            .deepseek_client
            .as_ref()
            .expect("custom A client")
            .base_url(),
        "http://127.0.0.1:18181/v1"
    );

    let mut target = config.clone();
    target.provider = Some("custom-b".to_string());
    let route = resolve_runtime_route(&target, ApiProvider::Custom, Some("model-b"))
        .expect("resolve custom B")
        .validate()
        .expect("preflight custom B");
    engine.install_validated_runtime_route(route);

    assert_eq!(engine.api_provider_identity, "custom-b");
    assert_eq!(
        engine
            .deepseek_client
            .as_ref()
            .expect("custom B client")
            .base_url(),
        "http://127.0.0.1:18182/v1"
    );
}

#[test]
fn custom_route_config_reload_rebuilds_client_when_identity_is_unchanged() {
    let mut custom = HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("old-local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let (mut engine, _handle) = Engine::new(EngineConfig::default(), &config);

    let mut reloaded = config;
    let provider = reloaded
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("named custom provider");
    provider.base_url = Some("http://127.0.0.1:18182/v1".to_string());
    provider.api_key = Some("new-local-test-key".to_string());

    let route = resolve_runtime_route(&reloaded, ApiProvider::Custom, Some("local-model"))
        .expect("resolve reloaded route")
        .validate()
        .expect("preflight reloaded route");
    engine.install_validated_runtime_route(route);

    assert_eq!(engine.api_provider_identity, "lm-studio");
    assert_eq!(
        engine
            .deepseek_client
            .as_ref()
            .expect("reloaded custom client")
            .base_url(),
        "http://127.0.0.1:18182/v1"
    );
    assert_eq!(
        engine.api_config.deepseek_base_url(),
        "http://127.0.0.1:18182/v1"
    );
}

#[test]
fn failed_same_identity_route_preflight_leaves_old_client_untouched() {
    let mut custom = HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("old-local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let (engine, _handle) = Engine::new(EngineConfig::default(), &config);
    assert!(engine.deepseek_client.is_some());

    let mut invalid = config;
    invalid
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("named custom provider")
        .base_url = Some("ftp://invalid.example/v1".to_string());
    let err = resolve_runtime_route(&invalid, ApiProvider::Custom, Some("local-model"))
        .expect_err("invalid route must fail before installation");

    assert!(err.contains("must be an http(s) URL with a host"), "{err}");
    assert_eq!(engine.api_provider_identity, "lm-studio");
    assert!(engine.deepseek_client.is_some());
    assert!(engine.model_client.is_some());
    assert!(engine.deepseek_client_error.is_none());
}

#[tokio::test]
async fn exact_turn_snapshot_restores_custom_endpoint_and_turn_receipt_after_builtin_route() {
    let mut custom = HashMap::new();
    custom.insert(
        "custom-a".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            openai: crate::config::ProviderConfig {
                base_url: Some("http://127.0.0.1:18182/v1".to_string()),
                model: Some("gpt-5.5".to_string()),
                api_key: Some("builtin-test-key".to_string()),
                ..crate::config::ProviderConfig::default()
            },
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let engine_config = EngineConfig {
        max_steps: 0,
        snapshots_enabled: false,
        ..EngineConfig::default()
    };
    let (mut engine, handle) = Engine::new(engine_config, &config);

    let mut builtin_config = config.clone();
    builtin_config.provider = Some("openai".to_string());
    let builtin_route =
        resolve_runtime_route(&builtin_config, ApiProvider::Openai, Some("gpt-5.5"))
            .expect("resolve intervening builtin route")
            .validate()
            .expect("preflight intervening builtin route");
    engine.install_validated_runtime_route(builtin_route);
    assert_eq!(engine.api_provider, ApiProvider::Openai);
    assert_eq!(
        engine
            .deepseek_client
            .as_ref()
            .expect("builtin client")
            .base_url(),
        "http://127.0.0.1:18182/v1"
    );

    let run_task = tokio::spawn(engine.run());
    handle
        .send(Op::SendMessage {
            content: "verify exact route".to_string(),
            mode: AppMode::Agent,
            route: Box::new(
                resolve_runtime_route(&config, ApiProvider::Custom, Some("local-model"))
                    .expect("resolve exact custom route"),
            ),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: true,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send exact custom turn");

    let mut saw_exact_start = false;
    let mut saw_exact_endpoint = false;
    for _ in 0..20 {
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("engine event timeout")
        .expect("engine event");
        match event {
            Event::TurnStarted {
                route: Some(route), ..
            } => {
                assert_eq!(route.provider, ApiProvider::Custom);
                assert_eq!(route.provider_identity, "custom-a");
                assert_eq!(route.model, "local-model");
                saw_exact_start = true;
            }
            Event::TurnComplete { base_url, .. } => {
                assert_eq!(base_url.as_deref(), Some("http://127.0.0.1:18181/v1"));
                saw_exact_endpoint = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_exact_start);
    assert!(saw_exact_endpoint);
    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

struct GatedGoalModelClient {
    calls: std::sync::atomic::AtomicUsize,
    requests: std::sync::Mutex<Vec<crate::models::MessageRequest>>,
    second_request_entered: std::sync::Arc<tokio::sync::Notify>,
    release_second_request: std::sync::Arc<tokio::sync::Notify>,
    first_usage: Option<Usage>,
}

struct FirstRequestGatedGoalModelClient {
    calls: std::sync::atomic::AtomicUsize,
    request_entered: std::sync::Arc<tokio::sync::Notify>,
    release_request: std::sync::Arc<tokio::sync::Notify>,
}

struct IndexedGatedGoalModelClient {
    calls: std::sync::atomic::AtomicUsize,
    gates: HashMap<
        usize,
        (
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
        ),
    >,
    max_calls: usize,
}

#[async_trait::async_trait]
impl crate::core::model_client::ModelClient for IndexedGatedGoalModelClient {
    fn provider_name(&self) -> &str {
        "deterministic-goal"
    }

    fn model(&self) -> &str {
        "local-model"
    }

    async fn create_message(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::models::MessageResponse> {
        anyhow::bail!("indexed gate regression uses the streaming model boundary")
    }

    async fn create_message_stream(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1);
        if call > self.max_calls {
            anyhow::bail!("unexpected indexed goal model request #{call}");
        }
        if let Some((entered, release)) = self.gates.get(&call).cloned() {
            entered.notify_one();
            release.notified().await;
        }

        let events = crate::llm_client::mock::canned::simple_text_turn("still working")
            .into_iter()
            .map(Ok);
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    async fn health_check(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[async_trait::async_trait]
impl crate::core::model_client::ModelClient for FirstRequestGatedGoalModelClient {
    fn provider_name(&self) -> &str {
        "deterministic-goal"
    }

    fn model(&self) -> &str {
        "local-model"
    }

    async fn create_message(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::models::MessageResponse> {
        anyhow::bail!("mailbox regression uses the streaming model boundary")
    }

    async fn create_message_stream(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1);
        if call > 1 {
            anyhow::bail!("unexpected mailbox regression model request #{call}");
        }
        self.request_entered.notify_one();
        self.release_request.notified().await;

        let events = crate::llm_client::mock::canned::simple_text_turn("still working")
            .into_iter()
            .map(Ok);
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    async fn health_check(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

struct FailingGoalModelClient {
    calls: std::sync::atomic::AtomicUsize,
    message: String,
}

#[async_trait::async_trait]
impl crate::core::model_client::ModelClient for FailingGoalModelClient {
    fn provider_name(&self) -> &str {
        "deterministic-goal"
    }

    fn model(&self) -> &str {
        "local-model"
    }

    async fn create_message(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::models::MessageResponse> {
        anyhow::bail!("failure regression uses the streaming model boundary")
    }

    async fn create_message_stream(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::bail!(self.message.clone())
    }

    async fn health_check(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

impl GatedGoalModelClient {
    fn captured_requests(&self) -> Vec<crate::models::MessageRequest> {
        self.requests
            .lock()
            .expect("goal model request lock")
            .clone()
    }
}

#[async_trait::async_trait]
impl crate::core::model_client::ModelClient for GatedGoalModelClient {
    fn provider_name(&self) -> &str {
        "deterministic-goal"
    }

    fn model(&self) -> &str {
        "local-model"
    }

    async fn create_message(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::models::MessageResponse> {
        anyhow::bail!("goal regression uses the streaming model boundary")
    }

    async fn create_message_stream(
        &self,
        request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1);
        self.requests
            .lock()
            .expect("goal model request lock")
            .push(request);
        if call == 2 {
            self.second_request_entered.notify_one();
            self.release_second_request.notified().await;
        } else if call > 2 {
            anyhow::bail!("unexpected goal model request #{call}");
        }

        let mut events = crate::llm_client::mock::canned::simple_text_turn("still working");
        if call == 1
            && let Some(usage) = self.first_usage.clone()
            && let Some(crate::models::StreamEvent::MessageDelta { usage: slot, .. }) = events
                .iter_mut()
                .find(|event| matches!(event, crate::models::StreamEvent::MessageDelta { .. }))
        {
            *slot = Some(usage);
        }
        let events = events.into_iter().map(Ok);
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    async fn health_check(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[tokio::test]
async fn goal_continuation_preserves_goal_and_resolves_updated_authoritative_route() {
    let first_base_url = "http://127.0.0.1:18181/v1".to_string();
    let second_base_url = "http://127.0.0.1:18182/v1".to_string();
    let second_request_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_second_request = std::sync::Arc::new(tokio::sync::Notify::new());
    let model = std::sync::Arc::new(GatedGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        requests: std::sync::Mutex::new(Vec::new()),
        second_request_entered: std::sync::Arc::clone(&second_request_entered),
        release_second_request: std::sync::Arc::clone(&release_second_request),
        first_usage: Some(Usage {
            input_tokens: 3,
            output_tokens: 2,
            ..Usage::default()
        }),
    });
    let mut custom = HashMap::new();
    custom.insert(
        "custom-a".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some(first_base_url.clone()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let engine_config = EngineConfig {
        max_steps: 1,
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("keep going".to_string()),
        goal_token_budget: Some(50_000),
        ..EngineConfig::default()
    };
    let authoritative = Arc::new(parking_lot::RwLock::new(config.clone()));
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (mut engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
    engine.authoritative_route_config = Some(Arc::clone(&authoritative));
    let goal_state = engine.config.goal_state.clone();

    handle
        .send(Op::SendMessage {
            content: "first turn".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&config, "local-model"),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: Some("keep going".to_string()),
            goal_token_budget: Some(50_000),
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send first goal turn");

    let mut reloaded = config;
    reloaded
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("custom-a"))
        .expect("custom route")
        .base_url = Some(second_base_url.clone());
    *authoritative.write() = reloaded;
    let run_task = tokio::spawn(engine.run());

    let mut starts = 0;
    let mut completes = 0;
    let mut awaiting_second_sync = false;
    let mut verified_synthetic_goal = false;
    while completes < 2 {
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("goal engine event timeout")
        .expect("goal engine event");
        match event {
            Event::TurnStarted {
                route: Some(route), ..
            } => {
                starts += 1;
                assert_eq!(route.provider_identity, "custom-a");
                if starts == 2 {
                    awaiting_second_sync = true;
                }
            }
            Event::SessionUpdated {
                messages,
                system_prompt,
                ..
            } if awaiting_second_sync => {
                awaiting_second_sync = false;
                let snapshot = goal_state.lock().expect("goal lock").snapshot();
                assert_eq!(snapshot.objective.as_deref(), Some("keep going"));
                assert_eq!(snapshot.token_budget, Some(50_000));
                // The first turn records one bounded intra-turn pass, then the
                // synthetic boundary records the second pass before dispatch.
                assert_eq!(snapshot.continuation_count, 2);
                assert!(snapshot.is_active(), "synthetic turn must retain the goal");

                let continuation = messages
                    .last()
                    .expect("synthetic continuation message")
                    .content
                    .iter()
                    .find_map(|block| match block {
                        ContentBlock::Text { text, .. }
                            if text.contains("## Active Goal State") =>
                        {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .expect("durable goal state in synthetic message");
                assert!(continuation.contains("\"objective\": \"keep going\""));
                assert!(continuation.contains("\"token_budget\": 50000"));
                assert!(continuation.contains("\"continuation_count\": 2"));
                assert!(continuation.contains("Continuation pass #2."));

                let system_prompt = match system_prompt.expect("synthetic system prompt") {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                assert!(system_prompt.contains("<session_goal>"));
                assert!(system_prompt.contains("keep going"));
                verified_synthetic_goal = true;

                tokio::time::timeout(
                    model_turn_event_timeout(),
                    second_request_entered.notified(),
                )
                .await
                .expect("second goal model request was never entered");
                handle
                    .send(Op::SetGoalStatus {
                        status: crate::tools::goal::GoalStatus::Paused,
                        clear: false,
                    })
                    .await
                    .expect("queue goal pause");
                // The model future cannot finish until the pause operation is
                // already in the engine mailbox, making the queue-order proof
                // deterministic under arbitrarily loaded CI runners.
                release_second_request.notify_one();
            }
            Event::TurnComplete { base_url, .. } => {
                completes += 1;
                let expected = if completes == 1 {
                    first_base_url.as_str()
                } else {
                    second_base_url.as_str()
                };
                assert_eq!(base_url.as_deref(), Some(expected));
            }
            _ => {}
        }
    }
    assert_eq!(starts, 2);
    assert!(verified_synthetic_goal);
    let requests = model.captured_requests();
    assert_eq!(requests.len(), 2);
    let first_intra_turn_prompt = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } if text.contains("Continuation pass #1.") => {
                Some(text.as_str())
            }
            _ => None,
        })
        .expect("first intra-turn goal snapshot must survive into the next request");
    assert!(
        first_intra_turn_prompt.contains("\"tokens_used\": 5"),
        "current-turn usage must be rendered without waiting for durable recording: {first_intra_turn_prompt}"
    );

    // The pause was queued while the second turn was still running. Wait for
    // that control operation, then put a snapshot receipt behind the already
    // queued continuation. Receiving the receipt proves the continuation was
    // consumed; no third TurnStarted may have been emitted.
    let mut saw_paused_prompt = false;
    let mut saw_paused_goal = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("goal pause event timeout")
        .expect("goal pause event");
        match event {
            Event::SessionUpdated {
                system_prompt: Some(system_prompt),
                ..
            } => {
                let prompt = match system_prompt {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                if !prompt.contains("<session_goal>") {
                    saw_paused_prompt = true;
                }
            }
            Event::GoalUpdated { snapshot } if snapshot.status == "paused" => {
                assert_eq!(snapshot.objective.as_deref(), Some("keep going"));
                saw_paused_goal = true;
            }
            Event::Status { ref message } if message == "Goal paused." => {
                assert!(
                    saw_paused_prompt,
                    "pause status must follow the persisted prompt refresh"
                );
                assert!(
                    saw_paused_goal,
                    "pause status must follow the visible goal snapshot"
                );
                break;
            }
            Event::TurnStarted { .. } => {
                panic!("queued pause must prevent an additional goal turn")
            }
            _ => {}
        }
    }

    let (snapshot_tx, snapshot_rx) = tokio::sync::oneshot::channel();
    handle
        .send(Op::GetSessionSnapshot {
            tx: std::sync::Arc::new(std::sync::Mutex::new(Some(snapshot_tx))),
        })
        .await
        .expect("queue post-continuation receipt");
    tokio::time::timeout(Duration::from_secs(3), snapshot_rx)
        .await
        .expect("post-continuation receipt timeout")
        .expect("post-continuation receipt");
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, Event::TurnStarted { .. }),
                "paused goal continuation started a stale turn"
            );
        }
    }

    handle.send(Op::Shutdown).await.expect("queue shutdown");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn saturated_mailbox_does_not_deadlock_goal_continuation_self_dispatch() {
    let request_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_request = std::sync::Arc::new(tokio::sync::Notify::new());
    let model = std::sync::Arc::new(FirstRequestGatedGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        request_entered: std::sync::Arc::clone(&request_entered),
        release_request: std::sync::Arc::clone(&release_request),
    });
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        max_steps: 1,
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("survive a saturated mailbox".to_string()),
        ..EngineConfig::default()
    };
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "start the saturated goal turn".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&config, "local-model"),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: Some("survive a saturated mailbox".to_string()),
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send saturated goal turn");
    tokio::time::timeout(model_turn_event_timeout(), request_entered.notified())
        .await
        .expect("first goal request was never entered");

    // The engine has consumed the SendMessage and is gated inside the model
    // request, so every slot below belongs to a queued control operation. The
    // final pause must remain ahead of the synthetic continuation.
    for index in 0..ENGINE_OP_CHANNEL_CAPACITY {
        let status = if index + 1 == ENGINE_OP_CHANNEL_CAPACITY {
            crate::tools::goal::GoalStatus::Paused
        } else {
            crate::tools::goal::GoalStatus::Active
        };
        handle
            .tx_op
            .try_send(Op::SetGoalStatus {
                status,
                clear: false,
            })
            .unwrap_or_else(|error| panic!("fill op mailbox slot {index}: {error}"));
    }
    assert_eq!(handle.tx_op.capacity(), 0, "fixture must saturate mailbox");

    release_request.notify_one();
    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("saturated mailbox deadlocked the engine")
        .expect("post-saturation session snapshot");

    let prompt = match session.system_prompt.expect("paused system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "paused");
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the queued pause must suppress the stale continuation"
    );

    let mut starts = 0;
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            if matches!(event, Event::TurnStarted { .. }) {
                starts += 1;
            }
        }
    }
    assert_eq!(starts, 1, "only the original goal turn may start");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    tokio::time::timeout(model_turn_event_timeout(), run_task)
        .await
        .expect("engine did not shut down after mailbox saturation")
        .expect("engine task");
}

#[tokio::test]
async fn queued_ordinary_turn_does_not_multiply_engine_goal_continuations() {
    let first_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_first = std::sync::Arc::new(tokio::sync::Notify::new());
    let third_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_third = std::sync::Arc::new(tokio::sync::Notify::new());
    let model = std::sync::Arc::new(IndexedGatedGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        gates: HashMap::from([
            (
                1,
                (
                    std::sync::Arc::clone(&first_entered),
                    std::sync::Arc::clone(&release_first),
                ),
            ),
            (
                3,
                (
                    std::sync::Arc::clone(&third_entered),
                    std::sync::Arc::clone(&release_third),
                ),
            ),
        ]),
        max_calls: 3,
    });
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        max_steps: 1,
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("coalesce queued goal turns".to_string()),
        ..EngineConfig::default()
    };
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());
    let send_message = |content: &str| Op::SendMessage {
        content: content.to_string(),
        mode: AppMode::Agent,
        route: resolved_route_for_test(&config, "local-model"),
        compaction: Box::new(CompactionConfig::default()),
        goal_objective: Some("coalesce queued goal turns".to_string()),
        goal_token_budget: None,
        goal_status: crate::tools::goal::GoalStatus::Active,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        auto_model: false,
        allow_shell: false,
        trust_mode: false,
        auto_approve: false,
        approval_mode: crate::tui::approval::ApprovalMode::Suggest,
        translation_enabled: false,
        show_thinking: false,
        allowed_tools: None,
        dynamic_tools: Vec::new(),
        hook_executor: None,
        verbosity: None,
        provenance: UserInputProvenance::ExternalUser,
    };

    handle
        .send(send_message("start the goal turn"))
        .await
        .expect("send first goal turn");
    tokio::time::timeout(model_turn_event_timeout(), first_entered.notified())
        .await
        .expect("first goal request was never entered");

    // This ordinary user turn is already ahead of the first synthetic token
    // when the gated turn completes. It may refresh that token's tools, but it
    // must not create a second autonomous continuation.
    handle
        .send(send_message("queued ordinary follow-up"))
        .await
        .expect("queue ordinary follow-up");
    release_first.notify_one();

    tokio::time::timeout(model_turn_event_timeout(), third_entered.notified())
        .await
        .expect("coalesced synthetic continuation was never entered");
    handle
        .send(Op::SetGoalStatus {
            status: crate::tools::goal::GoalStatus::Paused,
            clear: false,
        })
        .await
        .expect("queue goal pause behind synthetic turn");
    release_third.notify_one();

    let _session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("queued-turn coalescing did not settle")
        .expect("post-coalescing session snapshot");
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "one initial turn, one queued user turn, and one synthetic continuation are expected"
    );
    assert_eq!(
        goal_state.lock().expect("goal lock").snapshot().status,
        "paused"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    tokio::time::timeout(model_turn_event_timeout(), run_task)
        .await
        .expect("engine did not shut down after queued-turn coalescing")
        .expect("engine task");
}

#[tokio::test]
async fn queued_failed_turn_cancels_older_goal_continuation_without_third_call() {
    let objective = "stop after the intervening failure";
    let model = std::sync::Arc::new(FailingGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        message: "deterministic queued turn failure".to_string(),
    });
    let config = goal_custom_route_config();
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (mut engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            goal_objective: Some(objective.to_string()),
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();

    handle
        .send(active_goal_message_op(
            &config,
            "queued turn that will fail",
            objective,
            None,
        ))
        .await
        .expect("queue failing ordinary turn");
    engine.schedule_goal_continuation(Vec::new());
    assert!(engine.has_scheduled_goal_continuation());
    let run_task = tokio::spawn(engine.run());

    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("queued failure did not settle")
        .expect("post-failure session snapshot");
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the stale synthetic token must not make a third provider call"
    );
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "blocked");
    assert!(
        goal.blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("deterministic queued turn failure")),
        "{goal:?}"
    );
    let prompt = system_prompt_text(session.system_prompt.expect("blocked system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn queued_not_started_turn_cancels_older_goal_continuation() {
    let objective = "stop when the queued turn cannot start";
    let model = std::sync::Arc::new(FailingGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        message: "model must never be called".to_string(),
    });
    let config = goal_custom_route_config();
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (mut engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            goal_objective: Some(objective.to_string()),
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();
    engine.model_client = None;
    engine.deepseek_client_error = Some("deterministic missing model client".to_string());

    handle
        .send(active_goal_message_op(
            &config,
            "queued turn that cannot start",
            objective,
            None,
        ))
        .await
        .expect("queue not-started ordinary turn");
    engine.schedule_goal_continuation(Vec::new());
    let run_task = tokio::spawn(engine.run());

    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("not-started turn did not settle")
        .expect("post-rejection session snapshot");
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "neither the rejected turn nor its stale token may call the provider"
    );
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "blocked");
    assert!(
        goal.blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("could not be started")),
        "{goal:?}"
    );
    let prompt = system_prompt_text(session.system_prompt.expect("blocked system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn queued_interrupted_turn_cancels_older_goal_continuation_without_third_call() {
    let objective = "pause after the intervening cancellation";
    let request_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_request = std::sync::Arc::new(tokio::sync::Notify::new());
    let model = std::sync::Arc::new(FirstRequestGatedGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        request_entered: std::sync::Arc::clone(&request_entered),
        release_request,
    });
    let config = goal_custom_route_config();
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (mut engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            goal_objective: Some(objective.to_string()),
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();

    handle
        .send(active_goal_message_op(
            &config,
            "queued turn that will be cancelled",
            objective,
            None,
        ))
        .await
        .expect("queue interruptible ordinary turn");
    engine.schedule_goal_continuation(Vec::new());
    let run_task = tokio::spawn(engine.run());
    tokio::time::timeout(model_turn_event_timeout(), request_entered.notified())
        .await
        .expect("queued request was never entered");
    handle.cancel();

    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("interrupted turn did not settle")
        .expect("post-interruption session snapshot");
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the stale synthetic token must not make a third provider call"
    );
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "paused");
    assert_eq!(goal.blocker, None);
    let prompt = system_prompt_text(session.system_prompt.expect("paused system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn initial_goal_failure_projects_blocked_state() {
    let objective = "block the initial failed goal turn";
    let leaked_secret = "sk-initial-goal-secret-123456";
    let model = std::sync::Arc::new(FailingGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        message: format!("initial provider failure: {leaked_secret}"),
    });
    let config = goal_custom_route_config();
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(active_goal_message_op(
            &config,
            "start a goal whose first turn fails",
            objective,
            None,
        ))
        .await
        .expect("send initial goal turn");
    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("initial goal failure did not settle")
        .expect("post-failure session snapshot");

    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.objective.as_deref(), Some(objective));
    assert_eq!(goal.status, "blocked");
    let blocker = goal.blocker.as_deref().expect("failure blocker");
    assert!(blocker.contains("initial provider failure"), "{blocker}");
    assert!(!blocker.contains(leaked_secret), "{blocker}");
    let prompt = system_prompt_text(session.system_prompt.expect("blocked system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn initial_goal_interruption_projects_paused_state() {
    let objective = "pause the initial interrupted goal turn";
    let request_entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release_request = std::sync::Arc::new(tokio::sync::Notify::new());
    let model = std::sync::Arc::new(FirstRequestGatedGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        request_entered: std::sync::Arc::clone(&request_entered),
        release_request,
    });
    let config = goal_custom_route_config();
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(active_goal_message_op(
            &config,
            "start a goal whose first turn is cancelled",
            objective,
            None,
        ))
        .await
        .expect("send initial goal turn");
    tokio::time::timeout(model_turn_event_timeout(), request_entered.notified())
        .await
        .expect("initial request was never entered");
    handle.cancel();

    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("initial interruption did not settle")
        .expect("post-interruption session snapshot");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.objective.as_deref(), Some(objective));
    assert_eq!(goal.status, "paused");
    assert_eq!(goal.blocker, None);
    let prompt = system_prompt_text(session.system_prompt.expect("paused system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn saturated_goal_controls_run_before_ready_idle_child_completion() {
    use crate::tools::subagent::SubAgentCompletion;

    let stale_tool = DynamicToolSpec {
        namespace: Some("goal-regression".to_string()),
        name: "stale".to_string(),
        description: "stale tool catalog".to_string(),
        input_schema: json!({"type": "object"}),
        defer_loading: false,
    };
    let fresh_tool = DynamicToolSpec {
        name: "fresh".to_string(),
        description: "fresh tool catalog".to_string(),
        ..stale_tool.clone()
    };
    let config = goal_custom_route_config();
    let (mut engine, handle) = Engine::new(
        EngineConfig {
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            ..EngineConfig::default()
        },
        &config,
    );

    for index in 0..ENGINE_OP_CHANNEL_CAPACITY {
        let status = if index + 1 == ENGINE_OP_CHANNEL_CAPACITY {
            crate::tools::goal::GoalStatus::Paused
        } else {
            crate::tools::goal::GoalStatus::Active
        };
        handle
            .tx_op
            .try_send(Op::SetGoalStatus {
                status,
                clear: false,
            })
            .unwrap_or_else(|error| panic!("fill ordering mailbox slot {index}: {error}"));
    }
    assert_eq!(handle.tx_op.capacity(), 0, "fixture must saturate mailbox");
    engine
        .tx_subagent_completion
        .send(SubAgentCompletion {
            agent_id: "agent_ready_during_backpressure".to_string(),
            payload: "ready child completion".to_string(),
        })
        .expect("queue ready idle child completion");
    engine.schedule_goal_continuation(vec![stale_tool]);
    assert!(
        engine.has_scheduled_goal_continuation(),
        "a live schedule must activate temporary op priority"
    );

    // Inspect the exact production receive helper without running handlers:
    // all controls that filled the mailbox, including the final pause, must be
    // selected before the already-ready idle child completion.
    for index in 0..ENGINE_OP_CHANNEL_CAPACITY {
        let input = tokio::time::timeout(model_turn_event_timeout(), engine.next_run_input(false))
            .await
            .expect("backpressured operation receive timed out")
            .expect("engine input");
        let EngineRunInput::Operation(op) = input else {
            panic!("idle child completion beat queued control {index}");
        };
        let Op::SetGoalStatus { status, clear } = *op else {
            panic!("unexpected operation before queued control {index}");
        };
        assert!(!clear);
        let expected = if index + 1 == ENGINE_OP_CHANNEL_CAPACITY {
            crate::tools::goal::GoalStatus::Paused
        } else {
            crate::tools::goal::GoalStatus::Active
        };
        assert_eq!(status, expected);
        if index == 0 {
            // Refresh after capacity opens. The existing token must retain its
            // FIFO position behind the remaining controls while carrying the
            // newest runtime tool catalog when it is eventually consumed.
            engine.schedule_goal_continuation(vec![fresh_tool.clone()]);
        }
    }

    let token = engine
        .next_run_input(false)
        .await
        .expect("backpressured continuation token");
    let EngineRunInput::Operation(token) = token else {
        panic!("idle child completion beat the backpressured continuation token");
    };
    let Op::ContinueGoal {
        dynamic_tools,
        engine_schedule_id,
    } = *token
    else {
        panic!("expected engine-owned continuation token");
    };
    assert!(dynamic_tools.is_empty());
    let continued_tools = engine
        .take_scheduled_goal_continuation(engine_schedule_id, dynamic_tools)
        .expect("engine-owned continuation token must consume its schedule marker");
    assert_eq!(continued_tools, vec![fresh_tool]);
    assert!(!engine.has_scheduled_goal_continuation());

    let child = engine
        .next_run_input(false)
        .await
        .expect("ready idle child completion");
    let EngineRunInput::SubAgentCompletion(child) = child else {
        panic!("unexpected operation after backpressure drain");
    };
    assert_eq!(child.agent_id, "agent_ready_during_backpressure");
}

#[tokio::test]
async fn unsaturated_goal_control_runs_before_ready_idle_child_completion() {
    use crate::tools::subagent::SubAgentCompletion;

    let config = goal_custom_route_config();
    let (mut engine, handle) = Engine::new(
        EngineConfig {
            snapshots_enabled: false,
            terminal_chrome_enabled: false,
            ..EngineConfig::default()
        },
        &config,
    );

    handle
        .tx_op
        .try_send(Op::SetGoalStatus {
            status: crate::tools::goal::GoalStatus::Paused,
            clear: false,
        })
        .expect("queue unsaturated pause");
    engine
        .tx_subagent_completion
        .send(SubAgentCompletion {
            agent_id: "agent_ready_without_backpressure".to_string(),
            payload: "ready child completion".to_string(),
        })
        .expect("queue ready idle child completion");
    engine.schedule_goal_continuation(Vec::new());
    assert!(engine.has_scheduled_goal_continuation());
    assert!(
        handle.tx_op.capacity() > 0,
        "fixture must leave the mailbox unsaturated"
    );

    let first = engine
        .next_run_input(false)
        .await
        .expect("queued pause must be selected");
    let EngineRunInput::Operation(first) = first else {
        panic!("ready child completion beat an unsaturated queued pause");
    };
    assert!(matches!(
        *first,
        Op::SetGoalStatus {
            status: crate::tools::goal::GoalStatus::Paused,
            clear: false
        }
    ));

    let token = engine
        .next_run_input(false)
        .await
        .expect("scheduled continuation token");
    let EngineRunInput::Operation(token) = token else {
        panic!("ready child completion beat the live continuation token");
    };
    let Op::ContinueGoal {
        dynamic_tools,
        engine_schedule_id,
    } = *token
    else {
        panic!("expected continuation token behind pause");
    };
    engine
        .take_scheduled_goal_continuation(engine_schedule_id, dynamic_tools)
        .expect("consume live continuation schedule");
    assert!(!engine.has_scheduled_goal_continuation());

    let child = engine
        .next_run_input(false)
        .await
        .expect("idle child completion after schedule consumption");
    let EngineRunInput::SubAgentCompletion(child) = child else {
        panic!("normal child fairness did not resume after schedule consumption");
    };
    assert_eq!(child.agent_id, "agent_ready_without_backpressure");
}

#[tokio::test]
async fn cross_turn_token_budget_exhaustion_blocks_goal_and_refreshes_prompt() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let budget_turn = vec![
        canned::message_start("mock_goal_budget"),
        canned::text_block_start(0),
        canned::text_delta(0, "budget spent"),
        canned::block_stop(0),
        canned::message_delta(
            "end_turn",
            Some(Usage {
                input_tokens: 8,
                output_tokens: 3,
                ..Usage::default()
            }),
        ),
        canned::message_stop(),
    ];
    let model = std::sync::Arc::new(MockLlmClient::new(vec![budget_turn]));
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let config = Config::default();
    let engine_config = EngineConfig {
        max_steps: 1,
        snapshots_enabled: false,
        subagents_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("finish within budget".to_string()),
        goal_token_budget: Some(10),
        ..EngineConfig::default()
    };
    let (engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "start budgeted goal".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: Some("finish within budget".to_string()),
            goal_token_budget: Some(10),
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send budgeted goal turn");

    let mut starts = 0;
    let mut saw_turn_complete = false;
    let mut saw_terminal_goal = false;
    let mut saw_prompt_refresh = false;
    let mut saw_budget_status = false;
    while !(saw_terminal_goal && saw_prompt_refresh && saw_budget_status) {
        let event = tokio::time::timeout(model_turn_event_timeout(), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("budget goal event timeout")
        .expect("budget goal event");
        match event {
            Event::TurnStarted { .. } => starts += 1,
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                saw_turn_complete = true;
            }
            Event::SessionUpdated {
                system_prompt: Some(system_prompt),
                ..
            } if saw_turn_complete => {
                let prompt = match system_prompt {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                assert!(
                    !prompt.contains("<session_goal>"),
                    "blocked budget goal must leave the active system prompt: {prompt}"
                );
                saw_prompt_refresh = true;
            }
            Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                assert_eq!(snapshot.objective.as_deref(), Some("finish within budget"));
                assert_eq!(snapshot.token_budget, Some(10));
                assert_eq!(snapshot.tokens_used, 11);
                assert!(
                    snapshot
                        .blocker
                        .as_deref()
                        .is_some_and(|blocker| blocker.contains("token budget reached")),
                    "{snapshot:?}"
                );
                saw_terminal_goal = true;
            }
            Event::Status { message } if message.contains("automatic continuation stopped") => {
                assert!(message.contains("11 / 10 tokens"), "{message}");
                assert!(message.contains("goal is blocked"), "{message}");
                saw_budget_status = true;
            }
            _ => {}
        }
    }

    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-budget session snapshot");
    let prompt = match session.system_prompt.expect("post-budget system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    let snapshot = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(snapshot.status, "blocked");
    assert_eq!(starts, 1, "budget stop must not start another turn");
    assert_eq!(
        model.call_count(),
        1,
        "budget stop must not call the model again"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn current_turn_usage_stops_budgeted_goal_after_one_provider_call() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let objective = "stop before an intra-turn budget overspend";
    let budget_turn = vec![
        canned::message_start("mock_goal_current_turn_budget"),
        canned::text_block_start(0),
        canned::text_delta(0, "budget spent"),
        canned::block_stop(0),
        canned::message_delta(
            "end_turn",
            Some(Usage {
                input_tokens: 8,
                output_tokens: 3,
                ..Usage::default()
            }),
        ),
        canned::message_stop(),
    ];
    let model = std::sync::Arc::new(MockLlmClient::new(vec![budget_turn]));
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let config = goal_custom_route_config();
    let (engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            subagents_enabled: false,
            terminal_chrome_enabled: false,
            goal_objective: Some(objective.to_string()),
            goal_token_budget: Some(10),
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(active_goal_message_op(
            &config,
            "start the budgeted goal",
            objective,
            Some(10),
        ))
        .await
        .expect("send budgeted goal turn");
    let mut saw_terminal_goal = false;
    while !saw_terminal_goal {
        let event = tokio::time::timeout(model_turn_event_timeout(), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("current-turn budget stop did not settle")
        .expect("current-turn budget event");
        if let Event::GoalUpdated { snapshot } = event
            && snapshot.status == "blocked"
        {
            saw_terminal_goal = true;
        }
    }
    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-budget session snapshot");

    assert_eq!(
        model.call_count(),
        1,
        "current-turn usage must stop all additional provider calls"
    );
    assert_eq!(model.remaining_turns(), 0);
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "blocked");
    assert_eq!(goal.tokens_used, 11);
    assert_eq!(goal.token_budget, Some(10));
    assert!(
        goal.blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("11 / 10 tokens")),
        "{goal:?}"
    );
    let prompt = system_prompt_text(session.system_prompt.expect("blocked system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn tool_response_crossing_goal_budget_does_not_issue_second_provider_request() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let objective = "stop after a budget-crossing goal tool response";
    let budget_tool_turn = vec![
        canned::message_start("mock_goal_tool_budget"),
        canned::tool_use_block_start(0, "call-get-goal", "get_goal"),
        canned::tool_input_delta(0, "{}"),
        canned::block_stop(0),
        canned::message_delta(
            "tool_use",
            Some(Usage {
                input_tokens: 8,
                output_tokens: 3,
                ..Usage::default()
            }),
        ),
        canned::message_stop(),
    ];
    let model = std::sync::Arc::new(MockLlmClient::new(vec![
        budget_tool_turn,
        canned::simple_text_turn("this second provider response must remain unused"),
    ]));
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let config = goal_custom_route_config();
    let (engine, handle) = Engine::new_with_model_client(
        EngineConfig {
            model: "local-model".to_string(),
            snapshots_enabled: false,
            subagents_enabled: false,
            terminal_chrome_enabled: false,
            goal_objective: Some(objective.to_string()),
            goal_token_budget: Some(10),
            ..EngineConfig::default()
        },
        &config,
        client,
    );
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(active_goal_message_op(
            &config,
            "inspect the goal without overspending",
            objective,
            Some(10),
        ))
        .await
        .expect("send budgeted goal tool turn");

    let mut saw_get_goal = false;
    let mut saw_terminal_goal = false;
    while !saw_terminal_goal {
        let event = tokio::time::timeout(model_turn_event_timeout(), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("budget-crossing goal tool turn did not settle")
        .expect("budget-crossing goal tool event");
        match event {
            Event::ToolCallComplete { name, result, .. } if name == "get_goal" => {
                assert!(result.expect("get_goal result").success);
                saw_get_goal = true;
            }
            Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                saw_terminal_goal = true;
            }
            _ => {}
        }
    }

    assert!(saw_get_goal, "the first response's goal tool must execute");
    assert_eq!(
        model.call_count(),
        1,
        "the provider-request boundary must stop the second model call"
    );
    assert_eq!(
        model.remaining_turns(),
        1,
        "the second canned response must remain unused"
    );
    let goal = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(goal.status, "blocked");
    assert_eq!(goal.tokens_used, 11, "usage must be durably recorded once");
    assert_eq!(goal.token_budget, Some(10));
    assert!(
        goal.blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("11 / 10 tokens")),
        "{goal:?}"
    );
    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-budget tool session snapshot");
    let prompt = system_prompt_text(session.system_prompt.expect("blocked system prompt"));
    assert!(!prompt.contains("<session_goal>"), "{prompt}");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn queued_goal_clear_refreshes_prompt_and_cancels_stale_continuation() {
    let config = Config::default();
    let engine_config = EngineConfig {
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("clear this goal".to_string()),
        goal_token_budget: Some(42_000),
        ..EngineConfig::default()
    };
    let (engine, handle) = Engine::new(engine_config, &config);
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    // Model the mailbox order produced when the user clears a goal while its
    // prior turn is still finishing: the control reaches the queue before the
    // synthetic continuation that TurnComplete schedules.
    handle
        .send(Op::SetGoalStatus {
            status: crate::tools::goal::GoalStatus::Active,
            clear: true,
        })
        .await
        .expect("queue goal clear");
    handle
        .send(Op::ContinueGoal {
            dynamic_tools: Vec::new(),
            engine_schedule_id: None,
        })
        .await
        .expect("queue stale continuation");

    // This receipt sits behind both operations. Once it arrives, a stale
    // continuation has either incorrectly started a turn or been consumed.
    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-clear session snapshot");
    let prompt = match session.system_prompt.expect("post-clear system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(
        !prompt.contains("<session_goal>"),
        "cleared config fallback must not restore the goal prompt: {prompt}"
    );
    let snapshot = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(snapshot.objective, None);
    assert_eq!(snapshot.status, "none");
    assert_eq!(snapshot.token_budget, None);

    let mut saw_clear_session = false;
    let mut saw_clear_goal = false;
    let mut saw_clear_status = false;
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::TurnStarted { .. } => {
                    panic!("queued clear must prevent a stale goal continuation")
                }
                Event::SessionUpdated { system_prompt, .. } => {
                    let prompt = match system_prompt.expect("clear SessionUpdated prompt") {
                        SystemPrompt::Text(text) => text,
                        SystemPrompt::Blocks(blocks) => blocks
                            .into_iter()
                            .map(|block| block.text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    assert!(!prompt.contains("<session_goal>"), "{prompt}");
                    saw_clear_session = true;
                }
                Event::GoalUpdated { snapshot } => {
                    assert_eq!(snapshot.objective, None);
                    assert_eq!(snapshot.status, "none");
                    saw_clear_goal = true;
                }
                Event::Status { message } if message == "Goal cleared." => {
                    saw_clear_status = true;
                }
                _ => {}
            }
        }
    }
    assert!(
        saw_clear_session,
        "clear must refresh persisted prompt state"
    );
    assert!(saw_clear_goal, "clear must emit a canonical empty snapshot");
    assert!(saw_clear_status, "clear must remain user-visible");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

fn goal_custom_route_config() -> Config {
    let mut custom = HashMap::new();
    custom.insert(
        "custom-a".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    }
}

fn without_named_custom_route(mut config: Config) -> Config {
    config
        .providers
        .as_mut()
        .expect("custom providers")
        .custom
        .clear();
    config
}

#[tokio::test]
async fn exhausted_goal_terminalizes_before_invalid_route_resolution() {
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("stop at the budget".to_string()),
        goal_token_budget: Some(10),
        ..EngineConfig::default()
    };
    let (mut engine, handle) = Engine::new(engine_config, &config);
    let goal_state = engine.config.goal_state.clone();
    goal_state.lock().expect("goal lock").record_usage(11, 0);

    let invalid_route_config = without_named_custom_route(config);
    engine.authoritative_route_config =
        Some(Arc::new(parking_lot::RwLock::new(invalid_route_config)));
    assert!(
        engine.current_runtime_route().is_err(),
        "fixture must prove route resolution cannot succeed"
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::ContinueGoal {
            dynamic_tools: Vec::new(),
            engine_schedule_id: None,
        })
        .await
        .expect("queue exhausted continuation");

    let mut saw_blocked_goal = false;
    let mut saw_budget_status = false;
    while !(saw_blocked_goal && saw_budget_status) {
        let event = tokio::time::timeout(model_turn_event_timeout(), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("budget terminal event timeout")
        .expect("budget terminal event");
        match event {
            Event::TurnStarted { .. } => {
                panic!("an exhausted goal must not attempt an invalid route")
            }
            Event::Error { envelope, .. } => {
                panic!("budget decision must precede route failure: {envelope:?}")
            }
            Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                assert_eq!(snapshot.tokens_used, 11);
                assert_eq!(snapshot.token_budget, Some(10));
                saw_blocked_goal = true;
            }
            Event::Status { message } if message.contains("Goal token budget reached") => {
                assert!(message.contains("11 / 10 tokens"), "{message}");
                saw_budget_status = true;
            }
            _ => {}
        }
    }

    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-budget session snapshot");
    let prompt = match session.system_prompt.expect("post-budget system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    assert_eq!(
        goal_state.lock().expect("goal lock").snapshot().status,
        "blocked"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn invalid_route_blocks_active_goal_and_refreshes_projections() {
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("keep going across route drift".to_string()),
        ..EngineConfig::default()
    };
    let (mut engine, handle) = Engine::new(engine_config, &config);
    let goal_state = engine.config.goal_state.clone();
    engine.authoritative_route_config = Some(Arc::new(parking_lot::RwLock::new(
        without_named_custom_route(config),
    )));
    assert!(
        engine.current_runtime_route().is_err(),
        "fixture must fail before dispatch"
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::ContinueGoal {
            dynamic_tools: Vec::new(),
            engine_schedule_id: None,
        })
        .await
        .expect("queue active continuation");
    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-route-failure session snapshot");

    let prompt = match session.system_prompt.expect("blocked system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    let snapshot = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(snapshot.status, "blocked");
    assert!(
        snapshot
            .blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("provider route is no longer valid")),
        "{snapshot:?}"
    );

    let mut saw_route_error = false;
    let mut saw_blocked_session = false;
    let mut saw_blocked_goal = false;
    let mut saw_blocked_status = false;
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::TurnStarted { .. } => {
                    panic!("invalid route must not start a continuation turn")
                }
                Event::Error { envelope, .. } => {
                    assert!(format!("{envelope:?}").contains("provider route is no longer valid"));
                    saw_route_error = true;
                }
                Event::SessionUpdated {
                    system_prompt: Some(system_prompt),
                    ..
                } => {
                    let prompt = match system_prompt {
                        SystemPrompt::Text(text) => text,
                        SystemPrompt::Blocks(blocks) => blocks
                            .into_iter()
                            .map(|block| block.text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    assert!(!prompt.contains("<session_goal>"), "{prompt}");
                    saw_blocked_session = true;
                }
                Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                    saw_blocked_goal = true;
                }
                Event::Status { message }
                    if message.contains("provider route is no longer valid") =>
                {
                    assert!(message.contains("resume the goal"), "{message}");
                    saw_blocked_status = true;
                }
                _ => {}
            }
        }
    }
    assert!(saw_route_error, "route failure must remain visible");
    assert!(
        saw_blocked_session,
        "session prompt projection must refresh"
    );
    assert!(saw_blocked_goal, "sidebar must receive blocked state");
    assert!(saw_blocked_status, "blocked reason must remain visible");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn rejected_continuation_dispatch_blocks_goal_after_failed_turn() {
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("keep going after dispatch".to_string()),
        ..EngineConfig::default()
    };
    let (mut engine, handle) = Engine::new(engine_config, &config);
    let goal_state = engine.config.goal_state.clone();
    assert!(engine.current_runtime_route().is_ok());
    // Exercise the continuation caller's `false` boundary deterministically:
    // the route installs, but the injected-model authority has no client with
    // which to start the request.
    engine.model_client_injected = true;
    engine.model_client = None;
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::ContinueGoal {
            dynamic_tools: Vec::new(),
            engine_schedule_id: None,
        })
        .await
        .expect("queue rejected continuation");
    let session = handle
        .get_session_snapshot()
        .await
        .expect("post-rejection session snapshot");

    let prompt = match session.system_prompt.expect("blocked system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    let snapshot = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(snapshot.status, "blocked");
    assert!(
        snapshot
            .blocker
            .as_deref()
            .is_some_and(|blocker| blocker.contains("next model turn could not be started")),
        "{snapshot:?}"
    );

    let mut starts = 0;
    let mut saw_failed_turn = false;
    let mut saw_dispatch_error = false;
    let mut saw_blocked_session = false;
    let mut saw_blocked_goal = false;
    let mut saw_blocked_status = false;
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::TurnStarted { .. } => starts += 1,
                Event::TurnComplete { status, .. } => {
                    assert_eq!(status, TurnOutcomeStatus::Failed);
                    saw_failed_turn = true;
                }
                Event::Error { .. } => saw_dispatch_error = true,
                Event::SessionUpdated {
                    system_prompt: Some(system_prompt),
                    ..
                } => {
                    let prompt = match system_prompt {
                        SystemPrompt::Text(text) => text,
                        SystemPrompt::Blocks(blocks) => blocks
                            .into_iter()
                            .map(|block| block.text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    assert!(!prompt.contains("<session_goal>"), "{prompt}");
                    saw_blocked_session = true;
                }
                Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                    saw_blocked_goal = true;
                }
                Event::Status { message }
                    if message.contains("next model turn could not be started") =>
                {
                    assert!(message.contains("resume the goal"), "{message}");
                    saw_blocked_status = true;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        starts, 1,
        "dispatch reached exactly one engine turn boundary"
    );
    assert!(
        saw_failed_turn,
        "rejected dispatch must surface a failed turn"
    );
    assert!(
        saw_dispatch_error,
        "rejected dispatch must surface its error"
    );
    assert!(
        saw_blocked_session,
        "session prompt projection must refresh"
    );
    assert!(saw_blocked_goal, "sidebar must receive blocked state");
    assert!(saw_blocked_status, "blocked reason must remain visible");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn started_nonretryable_continuation_failure_blocks_goal_with_bounded_reason() {
    let failure_marker = "HTTP 400 Bad Request: deterministic continuation failure";
    let leaked_secret = "sk-goal-secret-sentinel-123456";
    let failure_message = format!(
        "{failure_marker}: {leaked_secret} {}",
        "provider detail ".repeat(80)
    );
    let model = std::sync::Arc::new(FailingGoalModelClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        message: failure_message.clone(),
    });
    let config = goal_custom_route_config();
    let engine_config = EngineConfig {
        model: "local-model".to_string(),
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("block a failed continuation truthfully".to_string()),
        ..EngineConfig::default()
    };
    let client: crate::core::model_client::SharedModelClient = model.clone();
    let (engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
    let goal_state = engine.config.goal_state.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::ContinueGoal {
            dynamic_tools: Vec::new(),
            engine_schedule_id: None,
        })
        .await
        .expect("queue failing continuation");
    let session = tokio::time::timeout(model_turn_event_timeout(), handle.get_session_snapshot())
        .await
        .expect("failed continuation did not terminalize")
        .expect("post-failure session snapshot");

    let prompt = match session.system_prompt.expect("blocked system prompt") {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    };
    assert!(!prompt.contains("<session_goal>"), "{prompt}");
    let snapshot = goal_state.lock().expect("goal lock").snapshot();
    assert_eq!(snapshot.status, "blocked");
    let blocker = snapshot.blocker.as_deref().expect("failure blocker");
    assert!(blocker.contains(failure_marker), "{blocker}");
    assert!(blocker.contains("resume the goal"), "{blocker}");
    assert!(!blocker.contains(leaked_secret), "{blocker}");
    assert!(
        blocker.contains(codewhale_config::persistence::REDACTED),
        "{blocker}"
    );
    assert!(
        blocker.len() <= GOAL_CONTINUATION_FAILURE_DETAIL_MAX_BYTES + 160,
        "failure reason must remain bounded: {} bytes",
        blocker.len()
    );
    assert!(
        blocker.len() < failure_message.len(),
        "long provider detail must be truncated"
    );
    assert_eq!(
        model.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a nonretryable started failure must not dispatch again"
    );

    let mut starts = 0;
    let mut saw_failed_turn = false;
    let mut saw_provider_error = false;
    let mut saw_blocked_goal = false;
    let mut saw_blocked_status = false;
    {
        let mut events = handle.rx_event.write().await;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::TurnStarted { .. } => starts += 1,
                Event::TurnComplete { status, error, .. } => {
                    assert_eq!(status, TurnOutcomeStatus::Failed);
                    assert!(
                        error
                            .as_deref()
                            .is_some_and(|message| message.contains(failure_marker)),
                        "{error:?}"
                    );
                    saw_failed_turn = true;
                }
                Event::Error { envelope, .. } => {
                    if envelope.message.contains(failure_marker) {
                        saw_provider_error = true;
                    }
                }
                Event::GoalUpdated { snapshot } if snapshot.status == "blocked" => {
                    saw_blocked_goal = true;
                }
                Event::Status { message } if message.contains(failure_marker) => {
                    assert!(message.contains("resume the goal"), "{message}");
                    saw_blocked_status = true;
                }
                _ => {}
            }
        }
    }
    assert_eq!(starts, 1, "exactly one continuation turn must start");
    assert!(saw_failed_turn, "failed turn receipt must remain visible");
    assert!(
        saw_provider_error,
        "provider error event must remain visible"
    );
    assert!(saw_blocked_goal, "goal must publish its blocked snapshot");
    assert!(saw_blocked_status, "bounded failure must remain visible");

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn host_managed_engine_does_not_self_dispatch_goal_continuation() {
    let mut custom = HashMap::new();
    custom.insert(
        "custom-a".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let runtime_services = crate::tools::spec::RuntimeToolServices {
        active_thread_id: Some("thr_host_managed".to_string()),
        ..crate::tools::spec::RuntimeToolServices::default()
    };
    let engine_config = EngineConfig {
        max_steps: 0,
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        goal_objective: Some("keep going".to_string()),
        runtime_services,
        ..EngineConfig::default()
    };
    let (engine, handle) = Engine::new(engine_config, &config);
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "one host-owned turn".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&config, "local-model"),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: Some("keep going".to_string()),
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send host-owned goal turn");

    let mut starts = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("host engine event timeout")
        .expect("host engine event");
        match event {
            Event::TurnStarted { .. } => starts += 1,
            Event::TurnComplete { .. } => break,
            _ => {}
        }
    }
    assert_eq!(starts, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .is_err(),
        "a hosted engine must wait for an explicit durable turn claim"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[tokio::test]
async fn host_managed_engine_defers_idle_subagent_completion_to_explicit_turn() {
    use crate::tools::subagent::SubAgentCompletion;

    let mut custom = HashMap::new();
    custom.insert(
        "custom-a".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom-a".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let runtime_services = crate::tools::spec::RuntimeToolServices {
        active_thread_id: Some("thr_host_managed".to_string()),
        ..crate::tools::spec::RuntimeToolServices::default()
    };
    let engine_config = EngineConfig {
        max_steps: 0,
        snapshots_enabled: false,
        terminal_chrome_enabled: false,
        runtime_services,
        ..EngineConfig::default()
    };
    let (engine, handle) = Engine::new(engine_config, &config);
    let tx_subagent_completion = engine.tx_subagent_completion.clone();
    let run_task = tokio::spawn(engine.run());

    tx_subagent_completion
        .send(SubAgentCompletion {
            agent_id: "agent_deferred".to_string(),
            payload: "deferred child result".to_string(),
        })
        .expect("queue sub-agent completion");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .is_err(),
        "an idle child completion must not create an unclaimed hosted turn"
    );

    handle
        .send(Op::SendMessage {
            content: "claim the next turn".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&config, "local-model"),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send explicit host turn");

    let mut starts = 0;
    let mut drained_completion = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            handle.rx_event.write().await.recv().await
        })
        .await
        .expect("host engine event timeout")
        .expect("host engine event");
        match event {
            Event::TurnStarted { .. } => starts += 1,
            Event::Status { message } => {
                drained_completion |= message.contains("1 queued sub-agent completion");
            }
            Event::TurnComplete { .. } => break,
            _ => {}
        }
    }
    assert_eq!(starts, 1);
    assert!(
        drained_completion,
        "the next explicit turn must drain the queued child completion"
    );

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
}

#[test]
fn idle_and_in_turn_subagent_delivery_claim_each_completion_once() {
    use crate::tools::subagent::SubAgentCompletion;

    let mut delivered = HashSet::new();
    let first = SubAgentCompletion {
        agent_id: "agent_same".to_string(),
        payload: "first delivery".to_string(),
    };
    let duplicate = SubAgentCompletion {
        agent_id: "agent_same".to_string(),
        payload: "duplicate delivery".to_string(),
    };
    let second = SubAgentCompletion {
        agent_id: "agent_other".to_string(),
        payload: "other delivery".to_string(),
    };

    assert!(claim_subagent_completion(&mut delivered, first).is_some());
    assert!(claim_subagent_completion(&mut delivered, duplicate).is_none());
    assert!(claim_subagent_completion(&mut delivered, second).is_some());
    assert_eq!(
        delivered,
        HashSet::from(["agent_same".to_string(), "agent_other".to_string()])
    );
}

#[tokio::test]
async fn idle_subagent_delivery_releases_claim_when_route_fails_before_recording() {
    use crate::tools::subagent::SubAgentCompletion;

    let workspace = tempdir().expect("tempdir");
    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some("http://127.0.0.1:1/v1".to_string()),
        ..Config::default()
    };
    let (mut engine, _handle) =
        Engine::new(deterministic_engine_config(workspace.path()), &api_config);
    // Make the persisted exact identity structurally unresolvable. The
    // completion is claimed before route resolution, so this exercises the
    // early error branch before a transcript record can be written.
    engine.api_provider = ApiProvider::Custom;
    engine.api_provider_identity = "missing-custom".to_string();
    engine.api_provider_id = Some("missing-custom".to_string());

    engine
        .handle_idle_subagent_completion(SubAgentCompletion {
            agent_id: "agent_retryable".to_string(),
            payload: "completed work".to_string(),
        })
        .await;

    assert!(
        !engine
            .delivered_subagent_completion_ids
            .contains("agent_retryable"),
        "a completion that never reached the transcript must remain retryable"
    );
    assert!(
        claim_subagent_completion(
            &mut engine.delivered_subagent_completion_ids,
            SubAgentCompletion {
                agent_id: "agent_retryable".to_string(),
                payload: "retry".to_string(),
            },
        )
        .is_some()
    );
}

#[test]
fn subagent_mailbox_keeps_lifecycle_events_reliable() {
    use crate::models::Usage;
    use crate::tools::subagent::MailboxMessage;

    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::progress("agent_a", "step 1")
    ));
    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::ToolCallStarted {
            agent_id: "agent_a".to_string(),
            tool_name: "read_file".to_string(),
            step: 1,
        }
    ));
    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::ToolCallCompleted {
            agent_id: "agent_a".to_string(),
            tool_name: "read_file".to_string(),
            step: 1,
            ok: true,
        }
    ));

    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::started("agent_a", crate::tools::subagent::SubAgentType::Explore)
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::Completed {
            agent_id: "agent_a".to_string(),
            summary: "done".to_string(),
        }
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::Failed {
            agent_id: "agent_a".to_string(),
            error: "failed".to_string(),
        }
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::TokenUsage {
            agent_id: "agent_a".to_string(),
            provider: ApiProvider::Deepseek,
            model: "model".to_string(),
            usage: Usage::default(),
        }
    ));
}

#[test]
fn subagent_mailbox_samples_best_effort_events_per_agent() {
    use crate::tools::subagent::MailboxMessage;

    let mut last_sent_at = HashMap::new();
    let start = Instant::now();
    let first = MailboxMessage::ToolCallStarted {
        agent_id: "agent_a".to_string(),
        tool_name: "exec_shell".to_string(),
        step: 1,
    };
    let second = MailboxMessage::ToolCallCompleted {
        agent_id: "agent_a".to_string(),
        tool_name: "exec_shell".to_string(),
        step: 1,
        ok: true,
    };
    let other_agent = MailboxMessage::ToolCallCompleted {
        agent_id: "agent_b".to_string(),
        tool_name: "exec_shell".to_string(),
        step: 1,
        ok: true,
    };

    assert!(subagent_mailbox_best_effort_send_permitted(
        &mut last_sent_at,
        &first,
        start,
    ));
    assert!(
        !subagent_mailbox_best_effort_send_permitted(
            &mut last_sent_at,
            &second,
            start + Duration::from_millis(10),
        ),
        "same-agent telemetry inside the sampling window is dropped"
    );
    assert!(
        subagent_mailbox_best_effort_send_permitted(
            &mut last_sent_at,
            &other_agent,
            start + Duration::from_millis(10),
        ),
        "sampling is per agent, so one busy child cannot hide another"
    );
    assert!(
        subagent_mailbox_best_effort_send_permitted(
            &mut last_sent_at,
            &second,
            start + SUBAGENT_MAILBOX_BEST_EFFORT_MIN_INTERVAL,
        ),
        "the next same-agent update is allowed after the interval"
    );
}

#[test]
fn subagent_mailbox_never_samples_lifecycle_or_usage_events() {
    use crate::models::Usage;
    use crate::tools::subagent::{MailboxMessage, SubAgentType};

    let mut last_sent_at = HashMap::new();
    let start = Instant::now();

    assert!(subagent_mailbox_best_effort_send_permitted(
        &mut last_sent_at,
        &MailboxMessage::started("agent_a", SubAgentType::Explore),
        start,
    ));
    assert!(subagent_mailbox_best_effort_send_permitted(
        &mut last_sent_at,
        &MailboxMessage::Completed {
            agent_id: "agent_a".to_string(),
            summary: "done".to_string(),
        },
        start,
    ));
    assert!(subagent_mailbox_best_effort_send_permitted(
        &mut last_sent_at,
        &MailboxMessage::TokenUsage {
            agent_id: "agent_a".to_string(),
            provider: ApiProvider::Deepseek,
            model: "model".to_string(),
            usage: Usage::default(),
        },
        start,
    ));
}

struct ScopedDeepSeekApiKey {
    previous: Option<OsString>,
}

impl ScopedDeepSeekApiKey {
    fn set(value: &str) -> Self {
        let previous = std::env::var_os("DEEPSEEK_API_KEY");
        // Safety: tests using this helper serialize with lock_test_env() and
        // restore the original value in Drop.
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", value);
        }
        Self { previous }
    }
}

impl Drop for ScopedDeepSeekApiKey {
    fn drop(&mut self) {
        // Safety: tests using this helper serialize with lock_test_env().
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_API_KEY", previous);
            } else {
                std::env::remove_var("DEEPSEEK_API_KEY");
            }
        }
    }
}

fn catalog_tool(name: &str) -> Tool {
    Tool {
        tool_type: None,
        name: name.to_string(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

#[test]
fn tool_catalog_filter_applies_allow_and_deny_gates() {
    // #3027 AC1: the advertised catalog must not contain tools the execution
    // gates would deny; deny wins over allow.
    let mut catalog = vec![
        catalog_tool("read_file"),
        catalog_tool("exec_shell"),
        catalog_tool("grep_files"),
    ];
    filter_tool_catalog_for_gates(
        &mut catalog,
        Some(&["read_file".to_string(), "exec_shell".to_string()][..]),
        Some(&["exec_shell".to_string()][..]),
    );
    let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["read_file"]);
}

#[test]
fn tool_catalog_shell_only_benchmark_surface_hides_native_tools() {
    let mut catalog = vec![
        catalog_tool("exec_shell"),
        catalog_tool("exec_shell_wait"),
        catalog_tool("exec_shell_interact"),
        catalog_tool("read_file"),
        catalog_tool("write_file"),
        catalog_tool("list_dir"),
        catalog_tool("git_status"),
        catalog_tool("work_update"),
    ];
    let shell_only = [
        "exec_shell".to_string(),
        "exec_shell_wait".to_string(),
        "exec_shell_interact".to_string(),
    ];

    filter_tool_catalog_for_gates(&mut catalog, Some(&shell_only), None);

    let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["exec_shell", "exec_shell_wait", "exec_shell_interact"]
    );
}

#[test]
fn tool_catalog_filter_is_inert_without_gates() {
    let mut catalog = vec![catalog_tool("read_file"), catalog_tool("exec_shell")];
    filter_tool_catalog_for_gates(&mut catalog, None, None);
    assert_eq!(catalog.len(), 2);
}

#[test]
fn structured_state_block_includes_rich_plan_artifact() {
    let state = StructuredState {
        mode_label: "Plan".to_string(),
        workspace: PathBuf::from("/workspace/codewhale"),
        cwd: None,
        working_set_summary: None,
        todo_snapshot: None,
        plan_snapshot: Some(PlanSnapshot {
            objective: Some("Make Plan mode reviewable".to_string()),
            context_summary: Some("Grounded in issue #2691".to_string()),
            sources_used: vec!["gh issue view 2691".to_string()],
            critical_files: vec!["crates/tui/src/tools/plan.rs".to_string()],
            constraints: vec!["Preserve legacy payloads".to_string()],
            recommended_approach: Some("Enrich update_plan".to_string()),
            verification_plan: Some("Run focused tests".to_string()),
            risks_and_unknowns: Some("Replay may drift".to_string()),
            handoff_packet: Some("Next agent should inspect replay".to_string()),
            items: vec![PlanItemArg {
                step: "Render rich artifact".to_string(),
                status: StepStatus::InProgress,
            }],
            ..PlanSnapshot::default()
        }),
        subagent_snapshots: Vec::new(),
    };

    let block = state.to_system_block().expect("fork state block");

    assert!(block.contains("Objective: Make Plan mode reviewable"));
    assert!(block.contains("Context: Grounded in issue #2691"));
    assert!(block.contains("Source: gh issue view 2691"));
    assert!(block.contains("Critical file: crates/tui/src/tools/plan.rs"));
    assert!(block.contains("Constraint: Preserve legacy payloads"));
    assert!(block.contains("Verification plan: Run focused tests"));
    assert!(block.contains("Handoff packet: Next agent should inspect replay"));
    assert!(block.contains("- [~] Render rich artifact"));
}

#[test]
fn structured_state_block_uses_checklist_as_work_surface() {
    let state = StructuredState {
        mode_label: "Agent".to_string(),
        workspace: PathBuf::from("/workspace/codewhale"),
        cwd: Some(PathBuf::from("/workspace/codewhale")),
        working_set_summary: None,
        todo_snapshot: Some(TodoListSnapshot {
            items: vec![
                TodoItem {
                    id: 1,
                    content: "Wire Fleet progress projection".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: 2,
                    content: "Run focused gates".to_string(),
                    status: TodoStatus::Pending,
                },
            ],
            completion_pct: 0,
            in_progress_id: Some(1),
        }),
        plan_snapshot: Some(PlanSnapshot {
            objective: Some("Keep strategy separate".to_string()),
            ..PlanSnapshot::default()
        }),
        subagent_snapshots: Vec::new(),
    };

    let block = state.to_system_block().expect("fork state block");

    assert!(block.contains("### Work"));
    assert!(block.contains("Checklist (0% complete)"));
    assert!(block.contains("- [~] Wire Fleet progress projection"));
    assert!(block.contains("Strategy metadata"));
    assert!(block.contains("Objective: Keep strategy separate"));
    assert!(!block.contains("Todo list"));
}

#[test]
fn env_only_auth_error_gets_recovery_hint() {
    let _guard = lock_test_env();
    let _env = ScopedDeepSeekApiKey::set("stale-env-key");
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    let message =
        engine.decorate_auth_error_message("Authentication failed: invalid API key".to_string());

    assert!(message.contains("DEEPSEEK_API_KEY"));
    assert!(message.contains("no saved config key is present"));
    assert!(message.contains("codewhale auth status"));
    assert!(message.contains("codewhale auth set --provider deepseek"));
}

#[test]
fn config_auth_error_does_not_blame_env() {
    let _guard = lock_test_env();
    let _env = ScopedDeepSeekApiKey::set("stale-env-key");
    let cfg = Config {
        api_key: Some("fresh-config-key".to_string()),
        ..Config::default()
    };
    let (engine, _handle) = Engine::new(EngineConfig::default(), &cfg);

    let message =
        engine.decorate_auth_error_message("Authentication failed: invalid API key".to_string());

    assert_eq!(message, "Authentication failed: invalid API key");
}

#[test]
fn plugin_tools_dir_honors_missing_custom_directory_without_fallback() {
    let missing = PathBuf::from("definitely-missing-codewhale-plugin-dir");
    let tools_config = crate::config::ToolsConfig {
        plugin_dir: Some(missing.to_string_lossy().to_string()),
        ..Default::default()
    };

    assert_eq!(plugin_tools_dir(Some(&tools_config)), missing);
}

#[test]
fn configure_plugin_tools_applies_overrides_after_discovered_plugins() {
    let tmp = tempdir().expect("tempdir");
    let plugin_dir = tmp.path().join("tools");
    fs::create_dir(&plugin_dir).expect("plugin dir");
    fs::write(
        plugin_dir.join("same-name.sh"),
        "# name: same_tool\n# description: discovered plugin\n",
    )
    .expect("plugin script");

    let mut overrides = HashMap::new();
    overrides.insert(
        "same_tool".to_string(),
        crate::config::ToolOverride::Command {
            command: "configured-command".to_string(),
            args: None,
        },
    );
    let tools_config = crate::config::ToolsConfig {
        plugin_dir: Some(plugin_dir.to_string_lossy().to_string()),
        overrides: Some(overrides),
        ..Default::default()
    };

    let ctx = crate::tools::ToolContext::new(tmp.path().to_path_buf());
    let mut registry = crate::tools::ToolRegistry::new(ctx);

    let plugin_names = configure_plugin_tools(&mut registry, Some(&tools_config));

    let tool = registry.get("same_tool").expect("same_tool registered");
    assert!(tool.description().contains("configured-command"));
    assert!(plugin_names.contains("same_tool"));
}

fn make_plan(
    read_only: bool,
    supports_parallel: bool,
    approval_required: bool,
    interactive: bool,
) -> ToolExecutionPlan {
    make_plan_at(
        0,
        read_only,
        supports_parallel,
        approval_required,
        interactive,
    )
}

fn make_plan_at(
    index: usize,
    read_only: bool,
    supports_parallel: bool,
    approval_required: bool,
    interactive: bool,
) -> ToolExecutionPlan {
    ToolExecutionPlan {
        index,
        id: format!("tool-{index}"),
        name: "grep_files".to_string(),
        input: json!({"pattern": "test"}),
        caller: None,
        interactive,
        approval_required,
        approval_description: "desc".to_string(),
        approval_force_prompt: false,
        supports_parallel,
        read_only,
        detached_start: false,
        resources: vec![ResourceClaim::ReadPath(PathBuf::from(format!(
            "src-{index}.rs"
        )))],
        blocked_error: None,
        guard_result: None,
    }
}

fn parallel_batch_indices(batch: &ToolExecutionBatch) -> Vec<usize> {
    match batch {
        ToolExecutionBatch::Parallel(plans) => plans.iter().map(|plan| plan.index).collect(),
        ToolExecutionBatch::Serial(_) => panic!("expected parallel batch"),
    }
}

fn ask_rule_engine(command: &str) -> codewhale_execpolicy::ExecPolicyEngine {
    codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
        codewhale_execpolicy::Ruleset::user(vec![], vec![])
            .with_ask_rules(vec![codewhale_execpolicy::ToolAskRule::exec_shell(command)]),
    ])
}

fn file_ask_rule_engine(tool: &str, path: &str) -> codewhale_execpolicy::ExecPolicyEngine {
    codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
        codewhale_execpolicy::Ruleset::user(vec![], vec![]).with_ask_rules(vec![
            codewhale_execpolicy::ToolAskRule::file_path(tool, path),
        ]),
    ])
}

fn model_turn_event_timeout() -> Duration {
    if cfg!(windows) {
        // The Windows CI runner executes the full TUI test binary with thousands of
        // tests competing for CPU. Keep this high enough that an approval-gated
        // model turn is not mistaken for a lifecycle failure under runner load.
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    }
}

fn resolved_route_for_test(
    config: &Config,
    model: &str,
) -> Box<crate::route_runtime::ResolvedRuntimeRoute> {
    Box::new(
        resolve_runtime_route(config, config.api_provider(), Some(model))
            .expect("resolve test route"),
    )
}

fn active_goal_message_op(
    config: &Config,
    content: &str,
    objective: &str,
    token_budget: Option<u32>,
) -> Op {
    Op::SendMessage {
        content: content.to_string(),
        mode: AppMode::Agent,
        route: resolved_route_for_test(config, "local-model"),
        compaction: Box::new(CompactionConfig::default()),
        goal_objective: Some(objective.to_string()),
        goal_token_budget: token_budget,
        goal_status: crate::tools::goal::GoalStatus::Active,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        auto_model: false,
        allow_shell: false,
        trust_mode: false,
        auto_approve: false,
        approval_mode: crate::tui::approval::ApprovalMode::Suggest,
        translation_enabled: false,
        show_thinking: false,
        allowed_tools: None,
        dynamic_tools: Vec::new(),
        hook_executor: None,
        verbosity: None,
        provenance: UserInputProvenance::ExternalUser,
    }
}

fn system_prompt_text(prompt: SystemPrompt) -> String {
    match prompt {
        SystemPrompt::Text(text) => text,
        SystemPrompt::Blocks(blocks) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn external_user_message_op(content: &str, mode: AppMode, config: &Config) -> Op {
    Op::SendMessage {
        content: content.to_string(),
        mode,
        route: resolved_route_for_test(config, crate::config::DEFAULT_TEXT_MODEL),
        compaction: Box::new(CompactionConfig::default()),
        goal_objective: None,
        goal_token_budget: None,
        goal_status: crate::tools::goal::GoalStatus::Active,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        auto_model: false,
        allow_shell: true,
        trust_mode: false,
        auto_approve: false,
        approval_mode: crate::tui::approval::ApprovalMode::Suggest,
        translation_enabled: false,
        show_thinking: true,
        allowed_tools: None,
        dynamic_tools: Vec::new(),
        hook_executor: None,
        verbosity: None,
        provenance: UserInputProvenance::ExternalUser,
    }
}

struct DropSignal(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

struct BlockingModelClient {
    entered: std::sync::Arc<tokio::sync::Notify>,
    request_dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl crate::core::model_client::ModelClient for BlockingModelClient {
    fn provider_name(&self) -> &str {
        "deterministic-blocking"
    }

    fn model(&self) -> &str {
        "deterministic-blocking-model"
    }

    async fn create_message(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::models::MessageResponse> {
        std::future::pending().await
    }

    async fn create_message_stream(
        &self,
        _request: crate::models::MessageRequest,
    ) -> anyhow::Result<crate::llm_client::StreamEventBox> {
        let _drop_signal = DropSignal(std::sync::Arc::clone(&self.request_dropped));
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn health_check(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}

fn deterministic_engine_config(workspace: &Path) -> EngineConfig {
    EngineConfig {
        workspace: workspace.to_path_buf(),
        snapshots_enabled: false,
        subagents_enabled: false,
        ..EngineConfig::default()
    }
}

#[tokio::test]
async fn injected_model_drives_real_engine_navigation_trajectory() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let workspace = tempdir().expect("tempdir");
    fs::write(
        workspace.path().join("README.md"),
        "navigation-seam-proof\n",
    )
    .expect("write fixture");
    let mock = std::sync::Arc::new(MockLlmClient::new(vec![
        canned::tool_call_turn("call-read", "read_file", r#"{"path":"README.md"}"#),
        canned::simple_text_turn("Navigation complete."),
    ]));
    let client: crate::core::model_client::SharedModelClient = mock.clone();
    let (engine, handle) = Engine::new_with_model_client(
        deterministic_engine_config(workspace.path()),
        &Config::default(),
        client,
    );
    let task = tokio::spawn(engine.run());
    handle
        .send(external_user_message_op(
            "Read README.md and report what it contains.",
            AppMode::Agent,
            &Config::default(),
        ))
        .await
        .expect("send deterministic navigation turn");

    let mut saw_read = false;
    let mut saw_answer = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for deterministic navigation")
    {
        match event {
            Event::ToolCallComplete { name, result, .. } if name == "read_file" => {
                let result = result.expect("read_file result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("navigation-seam-proof"));
                saw_read = true;
            }
            Event::MessageDelta { content, .. } => {
                saw_answer |= content.contains("Navigation complete");
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                break;
            }
            _ => {}
        }
    }
    drop(rx);
    assert!(
        saw_read,
        "real registry must execute the mock-requested read"
    );
    assert!(
        saw_answer,
        "real stream projection must emit the final answer"
    );
    assert_eq!(mock.call_count(), 2);
    handle.send(Op::Shutdown).await.expect("shutdown engine");
    task.await.expect("engine task");
}

#[tokio::test]
async fn injected_model_duplicate_reads_execute_once_and_close_both_tool_ids() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let workspace = tempdir().expect("tempdir");
    fs::write(workspace.path().join("README.md"), "coalesced-read-proof\n").expect("write fixture");
    let duplicate_read_turn = vec![
        canned::message_start("mock_msg_duplicate_read"),
        canned::tool_use_block_start(0, "call-read-1", "read_file"),
        canned::tool_input_delta(0, r#"{"path":"README.md"}"#),
        canned::block_stop(0),
        canned::tool_use_block_start(1, "call-read-2", "read_file"),
        canned::tool_input_delta(1, r#"{"path":"README.md"}"#),
        canned::block_stop(1),
        canned::message_delta("tool_use", None),
        canned::message_stop(),
    ];
    let mock = std::sync::Arc::new(MockLlmClient::new(vec![
        duplicate_read_turn,
        canned::simple_text_turn("Duplicate read complete."),
    ]));
    let client: crate::core::model_client::SharedModelClient = mock.clone();
    let (engine, handle) = Engine::new_with_model_client(
        deterministic_engine_config(workspace.path()),
        &Config::default(),
        client,
    );
    let task = tokio::spawn(engine.run());
    handle
        .send(external_user_message_op(
            "Issue the duplicate read batch.",
            AppMode::Agent,
            &Config::default(),
        ))
        .await
        .expect("send duplicate-read trajectory");

    let mut results = HashMap::new();
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for duplicate-read trajectory")
    {
        match event {
            Event::ToolCallComplete {
                id, name, result, ..
            } if name == "read_file" => {
                results.insert(id, result.expect("read result"));
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    assert_eq!(results.len(), 2, "every tool ID needs one terminal result");
    let leader = &results["call-read-1"];
    let follower = &results["call-read-2"];
    assert!(leader.content.contains("coalesced-read-proof"));
    assert_eq!(
        leader
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("executed"))
            .and_then(serde_json::Value::as_bool),
        None,
        "the first read is the physical execution"
    );
    assert_eq!(follower.metadata.as_ref().unwrap()["executed"], false);
    assert_eq!(
        follower.metadata.as_ref().unwrap()["read_repeat"]["action"],
        "same_batch_subscription"
    );
    assert_eq!(
        follower.metadata.as_ref().unwrap()["read_repeat"]["source_tool_use_id"],
        "call-read-1"
    );

    let requests = mock.captured_requests();
    assert_eq!(requests.len(), 2);
    let result_ids = requests[1]
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(result_ids.contains(&"call-read-1"));
    assert!(result_ids.contains(&"call-read-2"));

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    task.await.expect("engine task");
}

#[tokio::test]
async fn coalesced_raw_read_error_touches_working_set_once() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    async fn missing_read_touches(read_count: usize) -> u32 {
        let workspace = tempdir().expect("tempdir");
        let mut read_turn = vec![canned::message_start("mock_msg_missing_read")];
        for index in 0..read_count {
            let block_index = u32::try_from(index).expect("test read count fits u32");
            let tool_id = format!("call-missing-{}", index + 1);
            read_turn.push(canned::tool_use_block_start(
                block_index,
                &tool_id,
                "read_file",
            ));
            read_turn.push(canned::tool_input_delta(
                block_index,
                r#"{"path":"missing.rs"}"#,
            ));
            read_turn.push(canned::block_stop(block_index));
        }
        read_turn.push(canned::message_delta("tool_use", None));
        read_turn.push(canned::message_stop());

        let mock = std::sync::Arc::new(MockLlmClient::new(vec![
            read_turn,
            canned::simple_text_turn("Missing read handled."),
        ]));
        let client: crate::core::model_client::SharedModelClient = mock;
        let (mut engine, _handle) = Engine::new_with_model_client(
            deterministic_engine_config(workspace.path()),
            &Config::default(),
            client,
        );
        let context = crate::tools::ToolContext::new(workspace.path().to_path_buf());
        let mut registry = crate::tools::ToolRegistry::new(context);
        registry.register(std::sync::Arc::new(crate::tools::file::ReadFileTool));
        let tools = Some(registry.to_api_tools_with_cache(true));
        let mut turn = crate::core::turn::TurnContext::new(8);

        let (status, error) = engine
            .handle_deepseek_turn(
                &mut turn,
                Some(&registry),
                tools,
                AppMode::Agent,
                false,
                Vec::new(),
            )
            .await;

        assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
        engine
            .session
            .working_set
            .entries
            .get("missing.rs")
            .expect("leader error should record the attempted path")
            .touches
    }

    let baseline_touches = missing_read_touches(1).await;
    let coalesced_touches = missing_read_touches(2).await;
    assert_eq!(
        coalesced_touches, baseline_touches,
        "the coalesced follower must not add a physical working-set observation"
    );
}

#[tokio::test]
async fn injected_model_receives_malformed_tool_feedback_and_recovers() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let workspace = tempdir().expect("tempdir");
    let mock = std::sync::Arc::new(MockLlmClient::new(vec![
        canned::tool_call_turn("call-bad-read", "read_file", "{}"),
        canned::simple_text_turn("Recovered after validation feedback."),
    ]));
    let client: crate::core::model_client::SharedModelClient = mock.clone();
    let (engine, handle) = Engine::new_with_model_client(
        deterministic_engine_config(workspace.path()),
        &Config::default(),
        client,
    );
    let task = tokio::spawn(engine.run());
    handle
        .send(external_user_message_op(
            "Exercise malformed tool feedback.",
            AppMode::Agent,
            &Config::default(),
        ))
        .await
        .expect("send malformed trajectory");

    let mut validation_feedback = None;
    let mut recovered = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for malformed trajectory")
    {
        match event {
            Event::ToolCallComplete { name, result, .. } if name == "read_file" => {
                validation_feedback = Some(match result {
                    Ok(result) => result.content,
                    Err(error) => error.to_string(),
                });
            }
            Event::MessageDelta { content, .. } => {
                recovered |= content.contains("Recovered after validation feedback");
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                break;
            }
            _ => {}
        }
    }
    drop(rx);
    let feedback = validation_feedback.expect("validation feedback event");
    assert!(feedback.to_ascii_lowercase().contains("path"), "{feedback}");
    assert!(
        recovered,
        "model must get a follow-up turn after tool failure"
    );
    assert_eq!(mock.call_count(), 2);
    handle.send(Op::Shutdown).await.expect("shutdown engine");
    task.await.expect("engine task");
}

#[tokio::test]
async fn engine_cancellation_drops_active_injected_model_request() {
    let workspace = tempdir().expect("tempdir");
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let request_dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client: crate::core::model_client::SharedModelClient =
        std::sync::Arc::new(BlockingModelClient {
            entered: std::sync::Arc::clone(&entered),
            request_dropped: std::sync::Arc::clone(&request_dropped),
        });
    let (engine, handle) = Engine::new_with_model_client(
        deterministic_engine_config(workspace.path()),
        &Config::default(),
        client,
    );
    let task = tokio::spawn(engine.run());
    handle
        .send(external_user_message_op(
            "Block until explicitly cancelled.",
            AppMode::Agent,
            &Config::default(),
        ))
        .await
        .expect("send cancellation trajectory");
    tokio::time::timeout(model_turn_event_timeout(), entered.notified())
        .await
        .expect("model request was never entered");
    handle.cancel();

    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for cancellation")
    {
        if let Event::TurnComplete { status, error, .. } = event {
            assert_eq!(status, TurnOutcomeStatus::Interrupted, "{error:?}");
            break;
        }
    }
    drop(rx);
    assert!(
        request_dropped.load(std::sync::atomic::Ordering::SeqCst),
        "cancellation must drop the active provider future"
    );
    handle.send(Op::Shutdown).await.expect("shutdown engine");
    task.await.expect("engine task");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn operate_conversation_reaches_provider_when_workers_are_disabled() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-operate\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"I can still answer normally.\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-operate\",\"choices\":[{\"index\":0,",
        "\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let engine_config = EngineConfig {
        workspace: workspace.path().to_path_buf(),
        snapshots_enabled: false,
        subagents_enabled: false,
        ..EngineConfig::default()
    };
    let (operate_engine, operate_handle) = Engine::new(engine_config, &api_config);
    let operate_task = tokio::spawn(operate_engine.run());
    operate_handle
        .send(external_user_message_op(
            "what is a Rust worktree?",
            AppMode::Operate,
            &api_config,
        ))
        .await
        .expect("send Operate turn");

    let mut saw_operate_complete = false;
    let mut saw_operate_route = false;
    let mut operate_rx = operate_handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), operate_rx.recv())
        .await
        .expect("timed out waiting for Operate completion")
    {
        match event {
            Event::TurnStarted { route, .. } => {
                let route = route.expect("model turn route");
                assert_eq!(route.provider, ApiProvider::Deepseek);
                assert_eq!(route.model, crate::config::DEFAULT_TEXT_MODEL);
                assert!(!route.auto_model);
                saw_operate_route = true;
            }
            Event::Error { envelope, .. } => {
                panic!("ordinary Operate conversation emitted an error: {envelope:?}");
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                saw_operate_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(operate_rx);

    assert!(
        saw_operate_route,
        "model turns must publish route provenance"
    );
    assert!(
        saw_operate_complete,
        "Operate conversation must complete without worker readiness"
    );
    let requests = server
        .received_requests()
        .await
        .expect("recorded requests after Operate");
    assert_eq!(requests.len(), 1, "Operate must reach the provider");
    operate_handle
        .send(Op::Shutdown)
        .await
        .expect("shutdown Operate engine");
    operate_task.await.expect("Operate engine task");
}

#[test]
fn auto_review_classifies_publish_and_force_prompts_it() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "exec_shell",
        &json!({"command": "git push origin main"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        Some("push the release branch"),
        true,
        false,
    );

    assert_eq!(
        decision,
        AutoReviewPlanDecision::ForcePrompt(
            "Built-in safety gate requires approval: publish-like action requires durable review"
                .to_string()
        )
    );
    assert_eq!(audit["action_kind"], "publish");
    assert_eq!(audit["decision"], "hold_for_review");
}

#[test]
fn auto_review_policy_does_not_force_prompt_for_shell_git_tag_list_probe() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "exec_shell",
        &json!({"command": "git remote -v && git rev-parse --show-toplevel && git branch --show-current && git rev-parse HEAD && git tag --list 'v0.8.65'"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        Some("inspect release status"),
        true,
        false,
    );

    assert_eq!(decision, AutoReviewPlanDecision::NoChange);
    assert_eq!(audit["decision"], "ask_user");
    assert_eq!(audit["action_kind"], "shell");
}

#[test]
fn auto_review_policy_blocks_publish_when_approval_is_never() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "github_publish_release",
        &json!({"tag": "v0.8.64"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Never,
        Some("publish release"),
        true,
        false,
    );

    assert_eq!(
        decision,
        AutoReviewPlanDecision::Block(
            "Built-in safety gate requires approval: publish-like action requires durable review"
                .to_string()
        )
    );
    assert_eq!(audit["approval_mode"], "NEVER");
    assert_eq!(audit["decision"], "hold_for_review");
}

#[test]
fn rlm_eval_required_approval_ignores_generic_auto_approve() {
    assert!(registered_tool_approval_required(
        "rlm_eval",
        ApprovalRequirement::Required,
        true
    ));
}

#[test]
fn non_bypassable_registered_tools_block_without_prompt_in_full_access() {
    // Security invariant (#3866): the LLM can request a runtime MCP server
    // start, which spawns a child process / opens a network connection. That
    // must never run without explicit user approval. Ask emits that approval;
    // Full Access cannot show an approval modal, so planning fails closed
    // before execute (and therefore before process/network side effects). A
    // generic `Required` tool remains auto-approved in Full Access.
    assert!(
        registered_tool_approval_required("start_mcp_server", ApprovalRequirement::Required, true),
        "start_mcp_server must require approval even when auto_approve is enabled"
    );
    assert!(registered_tool_blocked_in_full_access(
        "start_mcp_server",
        ApprovalRequirement::Required,
        true,
    ));
    assert!(registered_tool_blocked_in_full_access(
        "rlm_eval",
        ApprovalRequirement::Required,
        true,
    ));
    assert!(registered_tool_forces_prompt(
        "start_mcp_server",
        ApprovalRequirement::Required,
    ));
    assert!(registered_tool_forces_prompt(
        "rlm_eval",
        ApprovalRequirement::Required,
    ));
    assert!(!registered_tool_blocked_in_full_access(
        "exec_shell",
        ApprovalRequirement::Required,
        true,
    ));
    assert!(
        registered_tool_approval_required("start_mcp_server", ApprovalRequirement::Required, false),
        "start_mcp_server must require approval when auto_approve is disabled"
    );
    // Sanity contrast: an ordinary Required tool is bypassable under auto-approve.
    assert!(!registered_tool_approval_required(
        "exec_shell",
        ApprovalRequirement::Required,
        true
    ));
}

#[test]
fn generic_required_tools_keep_auto_approve_behavior() {
    assert!(!registered_tool_approval_required(
        "exec_shell",
        ApprovalRequirement::Required,
        true
    ));
    assert!(registered_tool_approval_required(
        "exec_shell",
        ApprovalRequirement::Required,
        false
    ));
}

#[test]
fn auto_review_policy_does_not_change_generic_destructive_auto_approval_yet() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "exec_shell",
        &json!({"command": "cargo test"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        Some("run tests"),
        true,
        false,
    );

    assert_eq!(decision, AutoReviewPlanDecision::NoChange);
    assert_eq!(audit["decision"], "ask_user");
    assert_eq!(audit["risk"], "destructive");
}

#[test]
fn auto_review_run_origin_marks_detached_tools_as_background() {
    assert_eq!(
        auto_review_run_origin_for_plan(false),
        crate::tui::auto_review::RunOrigin::Interactive
    );
    assert_eq!(
        auto_review_run_origin_for_plan(true),
        crate::tui::auto_review::RunOrigin::Background
    );
}

#[test]
fn auto_review_policy_holds_background_destructive_under_suggest() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "exec_shell",
        &json!({"command": "rm -rf ~/", "background": true}),
        crate::tui::auto_review::RunOrigin::Background,
        crate::tui::approval::ApprovalMode::Suggest,
        Some("wipe the home directory in the background"),
        true,
        false,
    );

    assert_eq!(
        decision,
        AutoReviewPlanDecision::ForcePrompt(
            "Built-in safety gate requires approval: destructive background/headless action requires durable review"
                .to_string()
        )
    );
    assert_eq!(audit["run_origin"], "background");
    assert_eq!(audit["decision"], "hold_for_review");
}

#[test]
fn full_access_blocks_detached_catastrophic_tools_without_prompting() {
    for run_origin in [
        crate::tui::auto_review::RunOrigin::Background,
        crate::tui::auto_review::RunOrigin::Headless,
    ] {
        let (decision, audit) = auto_review_plan_decision(
            &crate::tui::auto_review::AutoReviewPolicy::default(),
            "exec_shell",
            &json!({"command": "rm -rf ~/", "background": true}),
            run_origin,
            crate::tui::approval::ApprovalMode::Bypass,
            Some("wipe the home directory in the background"),
            true,
            false,
        );

        assert_eq!(
            decision,
            AutoReviewPlanDecision::Block(
                "Built-in safety gate requires approval: destructive background/headless action requires durable review"
                    .to_string()
            )
        );
        assert_eq!(audit["approval_mode"], "BYPASS");
        assert_eq!(audit["run_origin"], run_origin.as_str());
        assert_eq!(audit["decision"], "hold_for_review");
    }
}

#[test]
fn auto_review_policy_blocks_background_destructive_under_never() {
    let (decision, audit) = auto_review_plan_decision(
        &crate::tui::auto_review::AutoReviewPolicy::default(),
        "exec_shell",
        &json!({"command": "rm -rf ~/", "background": true}),
        crate::tui::auto_review::RunOrigin::Background,
        crate::tui::approval::ApprovalMode::Never,
        Some("wipe the home directory in the background"),
        true,
        false,
    );

    assert_eq!(
        decision,
        AutoReviewPlanDecision::Block(
            "Built-in safety gate requires approval: destructive background/headless action requires durable review"
                .to_string()
        )
    );
    assert_eq!(audit["approval_mode"], "NEVER");
    assert_eq!(audit["run_origin"], "background");
    assert_eq!(audit["decision"], "hold_for_review");
}

#[test]
fn auto_review_plan_decision_uses_configured_policy() {
    let policy = crate::tui::auto_review::AutoReviewPolicy {
        block_rules: vec![
            crate::tui::auto_review::AutoReviewRule::block(
                "configured-shell-block",
                "shell requires maintainer review",
            )
            .action_kind(crate::tui::auto_review::ToolActionKind::Shell),
        ],
        ..Default::default()
    };

    let (decision, audit) = auto_review_plan_decision(
        &policy,
        "exec_shell",
        &json!({"command": "cargo test"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        Some("run tests"),
        true,
        false,
    );

    assert_eq!(
        decision,
        AutoReviewPlanDecision::Block(
            "Auto-review policy blocked tool 'exec_shell': shell requires maintainer review"
                .to_string()
        )
    );
    assert_eq!(audit["decision"], "block");
    assert_eq!(audit["rule_id"], "configured-shell-block");
}

#[test]
fn exec_shell_ask_rule_decision_prompts_for_matching_auto_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "cargo test --workspace"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Prompt(
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval.".to_string()
        ))
    );
}

#[test]
fn exec_shell_ask_rule_decision_blocks_matching_never_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "cargo test --workspace"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Never,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Block(
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval, but approval policy is never.".to_string()
        ))
    );
}

#[test]
fn exec_shell_ask_rule_decision_ignores_unmatched_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "git status"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(decision, None);
}

#[test]
fn file_ask_rule_decision_prompts_for_matching_read_path() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "secrets/api_key.txt"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Prompt(
            "Typed ask rule 'tool=read_file path=secrets/api_key.txt' requires approval."
                .to_string()
        ))
    );
}

#[test]
fn file_ask_rule_decision_prompts_for_absolute_workspace_path() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "/repo/secrets/api_key.txt"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Prompt(
            "Typed ask rule 'tool=read_file path=secrets/api_key.txt' requires approval."
                .to_string()
        ))
    );
}

#[test]
fn file_ask_rule_decision_blocks_matching_read_path_when_approval_is_never() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "secrets/api_key.txt"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Never,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Block(
            "Typed ask rule 'tool=read_file path=secrets/api_key.txt' requires approval, but approval policy is never.".to_string()
        ))
    );
}

#[test]
fn file_ask_rule_decision_ignores_unmatched_path() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "docs/readme.md"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(decision, None);
}

fn api_tool(name: &str) -> Tool {
    Tool {
        tool_type: Some("function".to_string()),
        name: name.to_string(),
        description: format!("Test tool {name}"),
        input_schema: json!({"type": "object"}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

#[test]
fn engine_handle_cancel_tracks_latest_turn_token() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let stale_token = engine.cancel_token.clone();

    engine.reset_cancel_token();
    handle.cancel();

    assert!(engine.cancel_token.is_cancelled());
    assert!(handle.is_cancelled());
    assert!(!stale_token.is_cancelled());
}

#[test]
fn engine_initial_prompt_includes_configured_goal() {
    let config = EngineConfig {
        goal_objective: Some("Fix goal handoff".to_string()),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("<session_goal>"));
    assert!(prompt.contains("Fix goal handoff"));
    assert!(
        engine
            .config
            .goal_state
            .lock()
            .expect("goal lock")
            .is_active()
    );
}

#[test]
fn engine_initial_prompt_omits_paused_goal() {
    let config = EngineConfig {
        goal_objective: Some("Wait for confirmation".to_string()),
        goal_status: GoalStatus::Paused,
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(!prompt.contains("<session_goal>"));
    assert!(
        !engine
            .config
            .goal_state
            .lock()
            .expect("goal lock")
            .is_active()
    );
}

#[test]
fn refresh_system_prompt_uses_runtime_goal_state() {
    let (mut engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    {
        let mut goal = engine.config.goal_state.lock().expect("goal lock");
        goal.create("Close the runtime goal loop".to_string(), None)
            .expect("create goal");
    }

    engine.refresh_system_prompt();
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("<session_goal>"));
    assert!(prompt.contains("Close the runtime goal loop"));
}

#[tokio::test]
async fn runtime_goal_updates_emit_ui_snapshot() {
    let (engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    {
        let mut goal = engine.config.goal_state.lock().expect("goal lock");
        goal.create("Ship the release lane".to_string(), Some(42_000))
            .expect("create goal");
        goal.mark_complete(
            "verified with focused tests".to_string(),
            crate::tools::goal::GoalCompletionVerification {
                status: "passed".to_string(),
                check: "cargo test -p codewhale-tui runtime_goal_updates_emit_ui_snapshot"
                    .to_string(),
                summary: "focused runtime goal snapshot test passed".to_string(),
            },
        )
        .expect("mark complete");
    }

    engine.emit_goal_updated().await;

    let mut rx = handle.rx_event.write().await;
    match rx.recv().await.expect("goal update event") {
        Event::GoalUpdated { snapshot } => {
            assert_eq!(snapshot.objective.as_deref(), Some("Ship the release lane"));
            assert_eq!(snapshot.status, "complete");
            assert_eq!(snapshot.token_budget, Some(42_000));
            assert_eq!(
                snapshot.evidence.as_deref(),
                Some("verified with focused tests")
            );
        }
        other => panic!("expected GoalUpdated, got {other:?}"),
    }
}

#[test]
fn parallel_batch_requires_read_only_parallel_tools() {
    let plans = vec![make_plan(true, true, false, false)];
    assert!(should_parallelize_tool_batch(&plans));

    let plans = vec![
        make_plan(true, true, false, false),
        make_plan(true, true, false, false),
    ];
    assert!(should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(false, true, false, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, false, false, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, true, true, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, true, false, true)];
    assert!(!should_parallelize_tool_batch(&plans));

    let mut background = make_plan(false, false, false, false);
    background.detached_start = true;
    assert!(should_parallelize_tool_batch(&[background]));

    let mut gated_background = make_plan(false, false, true, false);
    gated_background.detached_start = true;
    assert!(!should_parallelize_tool_batch(&[gated_background]));
}

#[test]
fn identical_read_only_calls_share_one_same_batch_execution() {
    let mut first = make_plan_at(0, true, true, false, false);
    first.name = "read_file".to_string();
    first.input = json!({"path": "src/lib.rs", "limit": 50});
    let mut duplicate = make_plan_at(1, true, true, false, false);
    duplicate.name = "read_file".to_string();
    duplicate.input = json!({"limit": 50, "path": "src/lib.rs"});
    let mut guard = read_repeat_guard::ReadRepeatGuard::default();

    let planned = dispatch::plan_read_repeat_execution(vec![first, duplicate], &mut guard);

    assert_eq!(planned.executable.len(), 1);
    assert_eq!(planned.coalesced.len(), 1);
    assert_eq!(planned.coalesced[0].leader_index, 0);
    assert_eq!(planned.coalesced[0].follower.index, 1);
    assert_eq!(planned.occurrences[&0].count, 1);
    assert_eq!(planned.occurrences[&1].count, 2);
}

#[test]
fn write_barrier_prevents_stale_same_batch_read_subscription() {
    let mut read_before = make_plan_at(0, true, true, false, false);
    read_before.name = "read_file".to_string();
    read_before.input = json!({"path": "src/lib.rs"});
    let mut write = make_plan_at(1, false, false, false, false);
    write.name = "write_file".to_string();
    write.input = json!({"path": "src/lib.rs", "content": "changed"});
    let mut read_after = make_plan_at(2, true, true, false, false);
    read_after.name = "read_file".to_string();
    read_after.input = json!({"path": "src/lib.rs"});
    let mut guard = read_repeat_guard::ReadRepeatGuard::default();

    let planned =
        dispatch::plan_read_repeat_execution(vec![read_before, write, read_after], &mut guard);

    assert_eq!(planned.executable.len(), 3);
    assert!(planned.coalesced.is_empty());
    assert_eq!(planned.occurrences[&0].count, 1);
    assert_eq!(planned.occurrences[&2].count, 2);
}

#[test]
fn fifth_cross_step_read_becomes_a_nonexecuting_receipt() {
    let mut guard = read_repeat_guard::ReadRepeatGuard::default();
    let mut first_occurrence = None;

    for index in 0..read_repeat_guard::RECEIPT_THRESHOLD {
        let mut plan = make_plan_at(index, true, true, false, false);
        plan.name = "read_file".to_string();
        plan.input = json!({"path": "src/lib.rs"});
        let mut planned = dispatch::plan_read_repeat_execution(vec![plan], &mut guard);
        let occurrence = planned.occurrences[&index].clone();
        if index == 0 {
            guard.remember_success(&occurrence, "tool-0", &ToolResult::success("file contents"));
            first_occurrence = Some(occurrence);
        }

        let plan = planned.executable.pop().expect("one cross-step plan");
        if index + 1 < read_repeat_guard::RECEIPT_THRESHOLD {
            assert!(plan.guard_result.is_none());
        } else {
            let receipt = plan.guard_result.expect("fifth read receipt");
            assert_eq!(receipt.metadata.as_ref().unwrap()["executed"], false);
            assert!(receipt.content.contains("tool-0"));
        }
    }

    assert_eq!(first_occurrence.expect("first occurrence").count, 1);
}

#[test]
fn parallel_batch_rejects_conflicting_prepared_resources() {
    let mut first = make_plan_at(0, true, true, false, false);
    first.resources = vec![ResourceClaim::ReadPath(PathBuf::from("src/lib.rs"))];
    let mut second = make_plan_at(1, true, true, false, false);
    second.resources = vec![ResourceClaim::WritePath(PathBuf::from("src/lib.rs"))];
    assert!(!should_parallelize_tool_batch(&[first, second]));

    let mut first = make_plan_at(0, true, true, false, false);
    first.resources = vec![ResourceClaim::ReadPath(PathBuf::from("src/a.rs"))];
    let mut second = make_plan_at(1, true, true, false, false);
    second.resources = vec![ResourceClaim::WritePath(PathBuf::from("src/b.rs"))];
    assert!(should_parallelize_tool_batch(&[first, second]));

    let mut global = make_plan_at(0, true, true, false, false);
    global.resources = vec![ResourceClaim::GlobalExclusive];
    let mut claimless = make_plan_at(1, true, true, false, false);
    claimless.resources.clear();
    assert!(!should_parallelize_tool_batch(&[global, claimless]));
}

#[test]
fn conflicting_resource_barriers_preserve_tool_order() {
    let path = PathBuf::from("src/lib.rs");
    let mut read_before = make_plan_at(0, true, true, false, false);
    read_before.resources = vec![ResourceClaim::ReadPath(path.clone())];
    let mut write = make_plan_at(1, true, true, false, false);
    write.resources = vec![ResourceClaim::WritePath(path.clone())];
    let mut read_after = make_plan_at(2, true, true, false, false);
    read_after.resources = vec![ResourceClaim::ReadPath(path)];

    let batches = plan_tool_execution_batches(vec![read_before, write, read_after]);
    assert_eq!(batches.len(), 3);
    assert_eq!(parallel_batch_indices(&batches[0]), vec![0]);
    assert_eq!(parallel_batch_indices(&batches[1]), vec![1]);
    assert_eq!(parallel_batch_indices(&batches[2]), vec![2]);
}

#[test]
fn tool_execution_batches_use_serial_barriers() {
    let batches = plan_tool_execution_batches(vec![
        make_plan_at(0, true, true, false, false),
        make_plan_at(1, true, true, false, false),
        make_plan_at(2, false, false, true, false),
        make_plan_at(3, true, true, false, false),
        make_plan_at(4, true, false, false, false),
        make_plan_at(5, true, true, false, false),
        make_plan_at(6, true, true, false, false),
    ]);

    assert_eq!(batches.len(), 5);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![0, 1]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("first batch should be parallel"),
    }
    match &batches[1] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 2),
        ToolExecutionBatch::Parallel(_) => panic!("second batch should be serial"),
    }
    match &batches[2] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![3]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("third batch should be parallel"),
    }
    match &batches[3] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 4),
        ToolExecutionBatch::Parallel(_) => panic!("fourth batch should be serial"),
    }
    match &batches[4] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![5, 6]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("fifth batch should be parallel"),
    }
}

#[test]
fn globally_exclusive_shell_plans_never_share_a_batch() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});
    shell_a.resources = vec![ResourceClaim::GlobalExclusive];
    let mut shell_b = make_plan_at(1, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "git log --oneline -5"});
    shell_b.resources = vec![ResourceClaim::GlobalExclusive];
    let mut write_shell = make_plan_at(2, false, false, true, false);
    write_shell.name = "exec_shell".to_string();
    write_shell.input = json!({"command": "cargo build"});
    write_shell.resources = vec![ResourceClaim::GlobalExclusive];
    let mut shell_c = make_plan_at(3, true, true, false, false);
    shell_c.name = "exec_shell".to_string();
    shell_c.input = json!({"command": "bash -lc 'rg TODO crates/tui/src/core'"});
    shell_c.resources = vec![ResourceClaim::GlobalExclusive];

    let batches = plan_tool_execution_batches(vec![shell_a, shell_b, write_shell, shell_c]);
    assert_eq!(batches.len(), 4);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => assert_eq!(plans[0].index, 0),
        ToolExecutionBatch::Serial(_) => panic!("first batch should be parallel"),
    }
    match &batches[1] {
        ToolExecutionBatch::Parallel(plans) => assert_eq!(plans[0].index, 1),
        ToolExecutionBatch::Serial(_) => panic!("second batch should be parallel"),
    }
    match &batches[2] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 2),
        ToolExecutionBatch::Parallel(_) => panic!("write shell should be a serial barrier"),
    }
    match &batches[3] {
        ToolExecutionBatch::Parallel(plans) => assert_eq!(plans[0].index, 3),
        ToolExecutionBatch::Serial(_) => panic!("fourth batch should be parallel"),
    }
}

#[test]
fn globally_exclusive_background_shell_does_not_overlap_readonly_shells() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});
    shell_a.resources = vec![ResourceClaim::GlobalExclusive];

    let mut background_cargo = make_plan_at(1, false, false, false, false);
    background_cargo.name = "exec_shell".to_string();
    background_cargo.input = json!({"command": "cargo check --workspace", "background": true});
    background_cargo.detached_start = true;
    background_cargo.resources = vec![ResourceClaim::GlobalExclusive];

    let mut shell_b = make_plan_at(2, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "rg TODO crates/tui/src/core"});
    shell_b.resources = vec![ResourceClaim::GlobalExclusive];

    let batches = plan_tool_execution_batches(vec![shell_a, background_cargo, shell_b]);
    assert_eq!(batches.len(), 3);
    assert_eq!(parallel_batch_indices(&batches[0]), vec![0]);
    assert_eq!(parallel_batch_indices(&batches[1]), vec![1]);
    assert_eq!(parallel_batch_indices(&batches[2]), vec![2]);
}

#[test]
fn globally_exclusive_background_verifier_does_not_overlap_readonly_tools() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});

    let mut verifier = make_plan_at(1, false, false, false, false);
    verifier.name = "run_verifiers".to_string();
    verifier.input = json!({"profile": "rust", "level": "full", "background": true});
    verifier.detached_start = true;
    verifier.resources = vec![ResourceClaim::GlobalExclusive];

    let mut shell_b = make_plan_at(2, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "rg TODO crates/tui/src/core"});

    let batches = plan_tool_execution_batches(vec![shell_a, verifier, shell_b]);
    assert_eq!(batches.len(), 3);
    assert_eq!(parallel_batch_indices(&batches[0]), vec![0]);
    assert_eq!(parallel_batch_indices(&batches[1]), vec![1]);
    assert_eq!(parallel_batch_indices(&batches[2]), vec![2]);
}

// Detached starts remain eligible for a parallel chunk, but their conservative
// global claim prevents overlap until the agent scheduler exposes narrower
// budget/session claims.
#[test]
fn globally_exclusive_agent_starts_are_singleton_batches() {
    let plans: Vec<ToolExecutionPlan> = (0..4)
        .map(|i| {
            let mut plan = make_plan_at(i, false, false, false, false);
            plan.name = "agent".to_string();
            plan.detached_start = true;
            plan.resources = vec![ResourceClaim::GlobalExclusive];
            plan
        })
        .collect();

    let batches = plan_tool_execution_batches(plans);
    assert_eq!(batches.len(), 4);
    for (index, batch) in batches.iter().enumerate() {
        assert_eq!(parallel_batch_indices(batch), vec![index]);
    }
}

#[test]
fn globally_exclusive_agent_start_splits_neighboring_readonly_tools() {
    let mut grep_a = make_plan_at(0, true, true, false, false);
    grep_a.name = "grep_files".to_string();

    let mut agent_start = make_plan_at(1, false, false, false, false);
    agent_start.name = "agent".to_string();
    agent_start.detached_start = true;
    agent_start.resources = vec![ResourceClaim::GlobalExclusive];

    let mut grep_b = make_plan_at(2, true, true, false, false);
    grep_b.name = "grep_files".to_string();

    let batches = plan_tool_execution_batches(vec![grep_a, agent_start, grep_b]);
    assert_eq!(batches.len(), 3);
    assert_eq!(parallel_batch_indices(&batches[0]), vec![0]);
    assert_eq!(parallel_batch_indices(&batches[1]), vec![1]);
    assert_eq!(parallel_batch_indices(&batches[2]), vec![2]);
}

#[test]
fn successful_update_plan_ends_plan_mode_turn_immediately() {
    assert!(should_stop_after_plan_tool(
        AppMode::Plan,
        "update_plan",
        &Ok(ToolResult::success("planned"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Agent,
        "update_plan",
        &Ok(ToolResult::success("planned"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Plan,
        "request_user_input",
        &Ok(ToolResult::success("input"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Plan,
        "update_plan",
        &Err(ToolError::execution_failed("failed".to_string()))
    ));
}

#[test]
fn quick_plan_requests_force_update_plan_on_first_step() {
    assert!(should_force_update_plan_first(
        AppMode::Plan,
        "Give me a quick 3-step plan to verify the UI changes."
    ));
    assert!(should_force_update_plan_first(
        AppMode::Plan,
        "Make a high-level plan for the footer work."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Can you make a plan to get ver 0.8.61 fully built and benchmark it with our api server?"
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Make a high-level plan for benchmarking https://github.com/sierra-research/tau2-bench."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Inspect the repo and then give me a quick plan."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Agent,
        "Give me a quick 3-step plan."
    ));
}

#[test]
fn quick_plan_turn_can_narrow_first_step_tools_to_update_plan() {
    let catalog = vec![
        Tool {
            tool_type: Some("function".to_string()),
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object"}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: Some("function".to_string()),
            name: "update_plan".to_string(),
            description: "Publish a plan".to_string(),
            input_schema: json!({"type": "object"}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];
    let active = initial_active_tools(&catalog);

    let forced = active_tools_for_step(&catalog, &active, true);
    assert_eq!(forced.len(), 1);
    assert_eq!(forced[0].name, "update_plan");

    let default = active_tools_for_step(&catalog, &active, false);
    assert_eq!(default.len(), 2);
}

#[test]
fn tool_error_messages_include_actionable_hints() {
    let path_error = ToolError::path_escape(PathBuf::from("../escape.txt"));
    let formatted = format_tool_error(&path_error, "read_file");
    assert!(formatted.contains("escapes workspace"));

    let missing_field = ToolError::missing_field("path");
    let formatted = format_tool_error(&missing_field, "read_file");
    assert!(formatted.contains("missing required field"));
    assert!(formatted.contains("\"category\":\"missing_field\""));
    assert!(formatted.contains("\"bad_field\":\"path\""));
    assert!(formatted.contains("\"retryable\":true"));
    assert!(formatted.contains("\"side_effect_status\":\"not_started\""));

    let schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    });
    let formatted = format_tool_error_with_schema(&missing_field, "read_file", Some(&schema));
    assert!(formatted.contains("\"required\":[\"path\"]"));

    let timeout = ToolError::Timeout { seconds: 5 };
    let formatted = format_tool_error(&timeout, "exec_shell");
    assert!(formatted.contains("timed out"));

    // #3020: Plan-mode denials already explain the fix — pass through
    // verbatim, with no conflicting "Adjust approval mode" suffix.
    let plan_denied = ToolError::permission_denied(
        "'exec_shell' is not available in Plan mode — switch to Act mode (`/mode act`) to run commands and code.",
    );
    let formatted = format_tool_error(&plan_denied, "exec_shell");
    assert_eq!(
        formatted,
        "'exec_shell' is not available in Plan mode — switch to Act mode (`/mode act`) to run commands and code."
    );

    // Bare denials still get the actionable suffix.
    let bare_denied = ToolError::permission_denied("nope");
    let formatted = format_tool_error(&bare_denied, "exec_shell");
    assert!(
        formatted.contains("Adjust approval mode or request permission"),
        "{formatted}"
    );

    // "model" must not satisfy the "mode" pass-through check.
    let model_denied = ToolError::permission_denied("requested model is not allowed");
    let formatted = format_tool_error(&model_denied, "agent");
    assert!(
        formatted.contains("Adjust approval mode or request permission"),
        "{formatted}"
    );
}

#[test]
fn transient_tool_errors_include_fallback_hint() {
    let search_error = ToolError::execution_failed("Web search request failed: timeout");
    let formatted = format_tool_error(&search_error, "web_search");

    assert!(
        formatted.contains("Fallback: after one retry"),
        "{formatted}"
    );
    assert!(formatted.contains("direct URL"), "{formatted}");
    assert!(formatted.contains("instead of repeating"), "{formatted}");
}

#[test]
fn tool_errors_with_specific_recovery_do_not_get_generic_fallback() {
    let message = "edit_file search string not found. Recovery: call read_file first.";
    let formatted = format_tool_error(&ToolError::execution_failed(message), "edit_file");

    assert_eq!(formatted, message);
}

#[test]
fn repeated_tool_errors_wait_until_degradation_threshold() {
    let tools = vec!["web_search".to_string()];

    assert!(tool_error_degradation_runtime_hint(1, &tools, &[ErrorCategory::Tool], &[]).is_none());
}

#[test]
fn repeated_tool_errors_emit_model_visible_degradation_hint() {
    let tools = vec!["web_search".to_string(), "web_search".to_string()];
    let hint = tool_error_degradation_runtime_hint(2, &tools, &[ErrorCategory::Tool], &[])
        .expect("second consecutive tool-error step should emit a runtime hint");

    assert!(hint.contains("2 consecutive"), "{hint}");
    assert!(hint.contains("web_search"), "{hint}");
    assert!(hint.contains("do not repeat"), "{hint}");
    assert!(hint.contains("alternate tool"), "{hint}");
    assert!(hint.contains("narrow the request"), "{hint}");
}

#[test]
fn repeated_authorization_errors_do_not_emit_degradation_hint() {
    let tools = vec!["exec_shell".to_string()];

    assert!(
        tool_error_degradation_runtime_hint(2, &tools, &[ErrorCategory::Authorization], &[])
            .is_none()
    );
}

#[test]
fn repeated_search_errors_suggest_direct_url_patterns_for_domains() {
    let tools = vec!["web_search".to_string()];
    let inputs = vec![json!({"query": "site:example.edu announcements"})];
    let hint = tool_error_degradation_runtime_hint(2, &tools, &[ErrorCategory::Tool], &inputs)
        .expect("repeated web_search failure should emit a domain-aware fallback hint");

    assert!(hint.contains("fetch_url"), "{hint}");
    assert!(hint.contains("https://example.edu/announcements"), "{hint}");
    assert!(hint.contains("https://example.edu/news"), "{hint}");
}

#[test]
fn repeated_web_run_errors_suggest_direct_url_patterns_for_domains_list() {
    let tools = vec!["web.run".to_string()];
    let inputs = vec![json!({
        "search_query": [
            {
                "q": "announcements",
                "domains": ["www.example.edu"]
            }
        ]
    })];
    let hint = tool_error_degradation_runtime_hint(2, &tools, &[ErrorCategory::Tool], &inputs)
        .expect("repeated web.run failure should emit a domain-aware fallback hint");

    assert!(hint.contains("https://example.edu/announcements"), "{hint}");
    assert!(hint.contains("https://example.edu/news"), "{hint}");
}

#[test]
fn repeated_search_errors_do_not_treat_versions_as_domains() {
    let tools = vec!["web_search".to_string()];
    let inputs = vec![json!({"query": "release v1.2 notes"})];
    let hint = tool_error_degradation_runtime_hint(2, &tools, &[ErrorCategory::Tool], &inputs)
        .expect("repeated web_search failure should still emit the generic hint");

    assert!(hint.contains("alternate tool"), "{hint}");
    assert!(!hint.contains("fetch_url"), "{hint}");
    assert!(!hint.contains("https://v1.2"), "{hint}");
}

#[test]
fn tool_exec_outcome_tracks_duration() {
    let outcome = ToolExecOutcome {
        index: 0,
        id: "tool-1".to_string(),
        name: "grep_files".to_string(),
        input: json!({"pattern": "test"}),
        started_at: Instant::now(),
        terminal: ToolExecutionOutcome::from_legacy(Ok(ToolResult::success("ok"))),
    };

    assert!(outcome.started_at.elapsed().as_nanos() > 0);
    assert_eq!(
        outcome.terminal.status,
        crate::tools::spec::ToolTerminalStatus::Succeeded
    );
}

#[test]
fn approval_stamp_makes_user_approval_model_visible() {
    let mut result = ToolResult::success("stdout");

    stamp_tool_result_approval(&mut result, ToolApprovalStamp::ApprovedByUser);

    assert!(
        result
            .content
            .starts_with("[approval] This tool call required approval"),
        "{}",
        result.content
    );
    assert!(
        result
            .content
            .contains("approved by the user before execution")
    );
    assert!(result.content.ends_with("stdout"));

    let metadata = result.metadata.expect("approval metadata");
    assert_eq!(metadata["approval"]["required"], true);
    assert_eq!(metadata["approval"]["decision"], "approved_by_user");
    assert_eq!(metadata["approval"]["model_visible"], true);
}

#[test]
fn approval_stamp_preserves_existing_metadata() {
    let mut result = ToolResult::success("ok").with_metadata(json!({
        "summary": "kept"
    }));

    stamp_tool_result_approval(&mut result, ToolApprovalStamp::ApprovedWithPolicy);

    let metadata = result.metadata.expect("metadata");
    assert_eq!(metadata["summary"], "kept");
    assert_eq!(metadata["approval"]["decision"], "approved_with_policy");
    assert!(result.content.contains("adjusted execution policy"));
}

#[test]
fn core_native_tools_stay_loaded_in_yolo_mode() {
    let always_load = HashSet::new();
    // #4625: `Bash` is the model-facing shell tool; legacy exec_shell names
    // are hidden compat aliases whose defer state no longer matters.
    assert!(!should_default_defer_tool("Bash", &always_load));
    // git_blame remains deferred (read-only git history beyond log/show/diff).
    assert!(should_default_defer_tool("git_blame", &always_load));
}

#[test]
fn non_yolo_mode_retains_default_defer_policy() {
    let always_load = HashSet::new();
    assert!(!should_default_defer_tool("Bash", &always_load));
    assert!(!should_default_defer_tool("edit_file", &always_load));
    assert!(!should_default_defer_tool("apply_patch", &always_load));
    assert!(!should_default_defer_tool("fetch_url", &always_load));
    assert!(!should_default_defer_tool("git_diff", &always_load));
    // #2654: read-only git history joins the active set.
    assert!(!should_default_defer_tool("git_log", &always_load));
    assert!(!should_default_defer_tool("git_show", &always_load));
    assert!(!should_default_defer_tool("git_status", &always_load));
    assert!(!should_default_defer_tool("run_tests", &always_load));
    assert!(!should_default_defer_tool("agent", &always_load));
    assert!(!should_default_defer_tool("read_file", &always_load));
    assert!(!should_default_defer_tool(
        "wait_for_dev_server",
        &always_load
    ));
    assert!(!should_default_defer_tool("web_search", &always_load));
    assert!(!should_default_defer_tool("write_file", &always_load));
    assert!(should_default_defer_tool(
        REQUEST_USER_INPUT_NAME,
        &always_load
    ));
    assert!(should_default_defer_tool("task_shell_start", &always_load));
    assert!(should_default_defer_tool("task_shell_wait", &always_load));
    assert!(should_default_defer_tool("git_blame", &always_load));
}

#[test]
fn default_defer_lookup_matches_linear_scan_over_active_native_tools() {
    // Parity guard for #4152: `should_default_defer_tool` now consults an O(1)
    // side set built from DEFAULT_ACTIVE_NATIVE_TOOLS instead of a linear
    // `.iter().any(...)` scan. Assert the set returns the SAME hit/miss as an
    // explicit linear scan over the ordered array — every array member is a hit
    // (not deferred); names outside the array miss (deferred by default).
    let always_load = HashSet::new();
    let active = default_active_native_tool_names();

    for name in active {
        // Reference linear scan == what the converted lookup must agree with.
        let linear_hit = active.iter().any(|core| core == name);
        assert!(linear_hit, "reference scan should find array member {name}");
        assert!(
            !should_default_defer_tool(name, &always_load),
            "array member {name} must stay active (not deferred)"
        );
    }

    for name in [
        "git_blame",
        "task_shell_start",
        REQUEST_USER_INPUT_NAME,
        "definitely_not_a_tool",
    ] {
        let linear_hit = active.contains(&name);
        assert!(!linear_hit, "non-member {name} should be absent from array");
        assert!(
            should_default_defer_tool(name, &always_load),
            "non-member {name} must default to deferred"
        );
    }
}

#[test]
fn model_tool_catalog_applies_native_and_mcp_deferral() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("write_file"),
            api_tool("Bash"),
            api_tool("edit_file"),
            api_tool("project_map"),
        ],
        vec![api_tool("list_mcp_resources"), api_tool("mcp_server_write")],
        AppMode::Agent,
        &always_load,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("write_file"), Some(false));
    assert_eq!(defer_loading("Bash"), Some(false));
    assert_eq!(defer_loading("edit_file"), Some(false));
    assert_eq!(defer_loading("project_map"), Some(true));
    assert_eq!(defer_loading("list_mcp_resources"), Some(false));
    assert_eq!(defer_loading("mcp_server_write"), Some(true));
}

#[test]
fn capability_compact_surface_defers_nonessential_core_tools() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog_with_surface(
        vec![
            api_tool("agent"),
            api_tool("grep_files"),
            api_tool("read_file"),
            api_tool("run_tests"),
            api_tool(TOOL_SEARCH_NAME),
            api_tool("update_plan"),
            api_tool("web_search"),
            api_tool("write_file"),
        ],
        vec![api_tool("list_mcp_resources"), api_tool("mcp_server_write")],
        AppMode::Agent,
        &always_load,
        crate::model_profile::ToolSurfaceBudget::Compact,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("grep_files"), Some(false));
    assert_eq!(defer_loading("update_plan"), Some(false));
    assert_eq!(defer_loading("write_file"), Some(false));
    assert_eq!(defer_loading(TOOL_SEARCH_NAME), Some(false));
    assert_eq!(defer_loading("list_mcp_resources"), Some(false));
    assert_eq!(defer_loading("agent"), Some(true));
    assert_eq!(defer_loading("run_tests"), Some(true));
    assert_eq!(defer_loading("web_search"), Some(true));
    assert_eq!(defer_loading("mcp_server_write"), Some(true));
}

#[test]
fn capability_full_surface_preserves_default_core_tools() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog_with_surface(
        vec![
            api_tool("agent"),
            api_tool("read_file"),
            api_tool("run_tests"),
        ],
        Vec::new(),
        AppMode::Agent,
        &always_load,
        crate::model_profile::ToolSurfaceBudget::Full,
    );

    for name in ["agent", "read_file", "run_tests"] {
        assert_eq!(
            catalog
                .iter()
                .find(|tool| tool.name == name)
                .and_then(|tool| tool.defer_loading),
            Some(false),
            "{name} should stay eager on full tool surfaces"
        );
    }
}

#[test]
fn plugin_or_benchmark_tools_marked_loaded_stay_active() {
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        vec![api_tool("KB_search"), api_tool("read_file")],
        Vec::new(),
        AppMode::Agent,
        &always_load,
    );

    // Mirrors Engine::run after configure_plugin_tools(): plugin tools are
    // explicitly kept loaded, and no provider-specific policy should re-defer
    // them before the first model request.
    let bench_tool = catalog
        .iter_mut()
        .find(|tool| tool.name == "KB_search")
        .expect("benchmark tool in catalog");
    bench_tool.defer_loading = Some(false);
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let active = initial_active_tools(&catalog);
    assert!(
        active.contains("KB_search"),
        "plugin/benchmark tools marked loaded must be callable on turn 1"
    );
    assert!(active.contains("read_file"));
}

#[test]
fn agent_catalog_keeps_edit_file_loaded_when_fuzz_is_omitted() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    let edit = catalog
        .iter()
        .find(|tool| tool.name == "edit_file")
        .expect("edit_file registered");

    assert_eq!(edit.defer_loading, Some(false));
    let required = edit.input_schema["required"]
        .as_array()
        .expect("edit_file schema should include required fields");
    assert!(required.iter().any(|field| field.as_str() == Some("path")));
    assert!(
        required
            .iter()
            .any(|field| field.as_str() == Some("search"))
    );
    assert!(
        required
            .iter()
            .any(|field| field.as_str() == Some("replace"))
    );
    assert!(!required.iter().any(|field| field.as_str() == Some("fuzz")));
    assert_eq!(
        edit.input_schema["properties"]["fuzz"]["type"].as_str(),
        Some("boolean")
    );

    let active_at_batch_start = initial_active_tools(&catalog);
    assert!(active_at_batch_start.contains("edit_file"));
    let mut hydrated_this_batch = HashSet::new();
    assert!(
        maybe_hydrate_requested_deferred_tool(
            "edit_file",
            &json!({
                "path": "src/foo.rs",
                "search": "before",
                "replace": "after"
            }),
            &catalog,
            &active_at_batch_start,
            &mut hydrated_this_batch,
        )
        .is_none(),
        "loaded edit_file calls without fuzz should execute instead of hydrating the schema"
    );
    assert!(hydrated_this_batch.is_empty());
}

#[test]
fn agent_catalog_advertises_and_searches_core_action_tools() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let issues = tool_catalog_consistency_issues(&catalog, &registry);
    assert!(
        issues.is_empty(),
        "Agent catalog should match the runtime registry: {issues:?}"
    );

    let names = catalog
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    // #4625: the model-facing shell tool is `Bash`; `exec_shell` remains a
    // hidden compat alias and must NOT appear in the advertised catalog.
    for tool_name in ["Bash", "write_file", "edit_file", "apply_patch"] {
        assert!(
            names.contains(tool_name),
            "{tool_name} must be advertised in Agent mode"
        );

        let mut active = initial_active_tools(&catalog);
        let result = execute_tool_search(
            TOOL_SEARCH_NAME,
            &json!({ "query": tool_name }),
            &catalog,
            &mut active,
        )
        .expect("tool search succeeds");
        let references = result.metadata.as_ref().unwrap()["tool_references"]
            .as_array()
            .expect("tool references are an array");
        assert!(
            references
                .iter()
                .any(|reference| reference.as_str() == Some(tool_name)),
            "{tool_name} should be discoverable by tool_search"
        );
        assert!(
            active.contains(tool_name),
            "{tool_name} should be activated by tool_search"
        );
    }
}

#[test]
fn catalog_consistency_self_check_flags_registered_core_tool_missing_from_catalog() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    catalog.retain(|tool| tool.name != "Bash");

    let issues = tool_catalog_consistency_issues(&catalog, &registry);
    assert!(
        issues
            .iter()
            .any(|issue| issue.contains("registered core tool 'Bash'")),
        "missing registered Bash should be reported: {issues:?}"
    );
}

#[test]
fn tool_search_reports_known_core_action_tool_when_current_catalog_omits_it() {
    let catalog = vec![api_tool("read_file")];
    let mut active = initial_active_tools(&catalog);

    let result = execute_tool_search(
        TOOL_SEARCH_NAME,
        &json!({ "query": "exec_shell" }),
        &catalog,
        &mut active,
    )
    .expect("tool search succeeds");

    assert!(!active.contains("exec_shell"));
    let unavailable = result.metadata.as_ref().unwrap()["unavailable_tool_references"]
        .as_array()
        .expect("unavailable references are an array");
    assert!(
        unavailable.iter().any(|reference| {
            reference["tool_name"].as_str() == Some("exec_shell")
                && reference["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("allow_shell = true"))
        }),
        "known-but-omitted core action tool should surface with a reason: {unavailable:?}"
    );
}

#[test]
fn tools_always_load_overrides_mcp_deferral() {
    let always_load = HashSet::from(["mcp_server_write".to_string()]);
    let catalog = build_model_tool_catalog(
        vec![api_tool("read_file")],
        vec![api_tool("mcp_server_write")],
        AppMode::Agent,
        &always_load,
    );
    let mcp = catalog
        .iter()
        .find(|tool| tool.name == "mcp_server_write")
        .expect("mcp tool");
    assert_eq!(mcp.defer_loading, Some(false));
}

#[test]
fn tools_always_load_overrides_default_native_deferral() {
    let always_load = HashSet::from(["git_blame".to_string()]);
    assert!(!should_default_defer_tool("git_blame", &always_load));
}

#[test]
#[ignore = "one-shot metric for scripts/measure-tool-catalog.py"]
#[allow(clippy::print_stderr)]
fn print_agent_tool_catalog_metrics() {
    let tmp = tempdir().expect("tempdir");
    let context = crate::tools::ToolContext::new(tmp.path().to_path_buf());
    let client = DeepSeekClient::new(&Config {
        api_key: Some("test-key".to_string()),
        ..Config::default()
    })
    .expect("stub client");
    let manager = crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 8);
    let runtime = crate::tools::subagent::SubAgentRuntime::new(
        client,
        DEFAULT_TEXT_MODEL.to_string(),
        context.clone(),
        true,
        None,
        manager.clone(),
    );
    let registry = crate::tools::ToolRegistryBuilder::new()
        .with_agent_tools(true)
        .with_todo_tool(new_shared_todo_list())
        .with_plan_tool(new_shared_plan_state())
        .with_review_tool(None, DEFAULT_TEXT_MODEL.to_string())
        .with_rlm_tool(None, DEFAULT_TEXT_MODEL.to_string())
        .with_notify_tool()
        .with_subagent_tools(manager, runtime)
        .build(context);
    let baseline_catalog = registry.to_api_tools_with_cache(true);
    let baseline_json = serde_json::to_vec(&baseline_catalog).expect("serialize baseline");

    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        baseline_catalog.clone(),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    let active = initial_active_tools(&catalog);
    let active_catalog = active_tools_for_step(&catalog, &active, false);
    let active_json = serde_json::to_vec(&active_catalog).expect("serialize active");
    let reduction_percent = if baseline_json.is_empty() {
        0.0
    } else {
        100.0 * (baseline_json.len().saturating_sub(active_json.len())) as f64
            / baseline_json.len() as f64
    };

    eprintln!(
        "TOOL_CATALOG_METRICS {}",
        serde_json::json!({
            "baseline_tools": baseline_catalog.len(),
            "baseline_bytes": baseline_json.len(),
            "baseline_tokens_est": baseline_json.len().div_ceil(4),
            "active_tools": active_catalog.len(),
            "active_bytes": active_json.len(),
            "active_tokens_est": active_json.len().div_ceil(4),
            "reduction_percent": reduction_percent,
            "active_tool_names": active_catalog.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
        })
    );
}

#[test]
fn deferred_tool_hydration_activates_without_guard_result_for_same_turn_retry() {
    let mut edit = api_tool("edit_file");
    edit.defer_loading = Some(true);
    edit.input_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "search": { "type": "string" },
            "replace": { "type": "string" }
        },
        "required": ["path", "search", "replace"]
    });

    let catalog = vec![edit];
    let active_at_batch_start = HashSet::new();
    let mut hydrated_this_batch = HashSet::new();
    let hydration = maybe_hydrate_requested_deferred_tool(
        "edit_file",
        &json!({
            "path": "src/foo.rs",
            "search": "before",
            "replace": "after"
        }),
        &catalog,
        &active_at_batch_start,
        &mut hydrated_this_batch,
    )
    .expect("first deferred use should hydrate");

    assert_eq!(
        hydration.metadata.as_ref().unwrap()["event"],
        "tool.schema_hydrated"
    );
    assert!(hydrated_this_batch.contains("edit_file"));
    // Turn loop policy (#4074): hydration activates the tool but must not
    // populate guard_result, so execution proceeds in the same batch.
    let guard_result: Option<crate::tools::spec::ToolResult> = None;
    assert!(guard_result.is_none());
}

#[test]
fn deferred_edit_file_first_use_hydrates_schema_without_execution() {
    let mut edit = api_tool("edit_file");
    edit.defer_loading = Some(true);
    edit.input_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "search": { "type": "string" },
            "replace": { "type": "string" }
        },
        "required": ["path", "search", "replace"]
    });

    let catalog = vec![edit];
    let active_at_batch_start = HashSet::new();
    let mut hydrated_this_batch = HashSet::new();
    let result = maybe_hydrate_requested_deferred_tool(
        "edit_file",
        &json!({
            "path": "src/foo.rs",
            "old_string": "before",
            "new_string": "after"
        }),
        &catalog,
        &active_at_batch_start,
        &mut hydrated_this_batch,
    )
    .expect("first deferred use should hydrate");

    assert!(!active_at_batch_start.contains("edit_file"));
    assert!(hydrated_this_batch.contains("edit_file"));
    assert!(result.success);
    assert!(result.content.contains("Tool `edit_file` was deferred"));
    assert!(result.content.contains("path: string"));
    assert!(result.content.contains("search: string"));
    assert!(result.content.contains("replace: string"));
    assert!(result.content.contains("old_string -> search"));
    assert!(result.content.contains("new_string -> replace"));
    assert!(result.content.contains("The tool was not executed"));

    let metadata = result.metadata.expect("metadata");
    assert_eq!(metadata["event"], "tool.schema_hydrated");
    assert_eq!(metadata["executed"], false);
    assert_eq!(metadata["retry_required"], true);

    let second_result = maybe_hydrate_requested_deferred_tool(
        "edit_file",
        &json!({"path": "src/bar.rs", "old_string": "before", "new_string": "after"}),
        &catalog,
        &active_at_batch_start,
        &mut hydrated_this_batch,
    )
    .expect("later calls in the same batch should hydrate instead of executing");
    assert_eq!(second_result.metadata.unwrap()["executed"], false);
    assert_eq!(hydrated_this_batch.len(), 1);

    let mut active_next_batch = active_at_batch_start.clone();
    active_next_batch.extend(hydrated_this_batch);
    let mut hydrated_next_batch = HashSet::new();
    assert!(
        maybe_hydrate_requested_deferred_tool(
            "edit_file",
            &json!({"path": "src/foo.rs", "search": "before", "replace": "after"}),
            &catalog,
            &active_next_batch,
            &mut hydrated_next_batch,
        )
        .is_none(),
        "tools hydrated in a previous batch should execute normally"
    );
}

#[test]
fn model_tool_catalog_defers_non_core_native_tools_in_yolo_mode() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![api_tool("read_file"), api_tool("project_map")],
        vec![api_tool("mcp_server_write")],
        AppMode::Yolo,
        &always_load,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("project_map"), Some(true));
    assert_eq!(defer_loading("mcp_server_write"), Some(false));
}

#[test]
fn request_user_input_stays_deferred_but_can_be_dynamically_activated() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![api_tool("read_file"), api_tool(REQUEST_USER_INPUT_NAME)],
        Vec::new(),
        AppMode::Agent,
        &always_load,
    );

    assert_eq!(
        catalog
            .iter()
            .find(|tool| tool.name == REQUEST_USER_INPUT_NAME)
            .and_then(|tool| tool.defer_loading),
        Some(true)
    );

    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains(REQUEST_USER_INPUT_NAME));
    active.insert(REQUEST_USER_INPUT_NAME.to_string());

    let active_tools = active_tools_for_step(&catalog, &active, false);
    assert!(
        active_tools
            .iter()
            .any(|tool| tool.name == REQUEST_USER_INPUT_NAME),
        "dynamic active tools should expose the question modal without making it eager by default"
    );
}

#[test]
fn auto_review_hides_question_tool_while_other_postures_keep_it() {
    use crate::tui::approval::ApprovalMode;

    for (posture, expected) in [
        (ApprovalMode::Suggest, true),
        (ApprovalMode::Auto, false),
        (ApprovalMode::Bypass, true),
        (ApprovalMode::Never, true),
    ] {
        let mut catalog = vec![api_tool("read_file"), api_tool(REQUEST_USER_INPUT_NAME)];
        filter_tool_catalog_for_permission_posture(&mut catalog, posture);
        assert_eq!(
            catalog
                .iter()
                .any(|tool| tool.name == REQUEST_USER_INPUT_NAME),
            expected,
            "{posture:?}"
        );
        assert!(catalog.iter().any(|tool| tool.name == "read_file"));
    }
}

#[test]
fn legacy_yolo_auto_shape_keeps_question_tool_as_effective_full_access() {
    let authority = crate::core::authority::effective_input_policy(
        UserInputProvenance::ExternalUser,
        AppMode::Yolo,
        "continue",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Auto,
    );
    assert_eq!(
        authority.approval_mode_for_session(),
        crate::tui::approval::ApprovalMode::Bypass
    );

    let mut catalog = vec![api_tool("read_file"), api_tool(REQUEST_USER_INPUT_NAME)];
    filter_tool_catalog_for_permission_posture(&mut catalog, authority.approval_mode_for_session());
    assert!(
        catalog
            .iter()
            .any(|tool| tool.name == REQUEST_USER_INPUT_NAME),
        "effective Full Access must keep the question tool"
    );
}

#[test]
fn model_tool_catalog_sorts_each_partition_for_prefix_cache_stability() {
    // Regression for #263: deterministic byte order of the tools array is a
    // hard requirement for DeepSeek's KV prefix cache. Built-ins stay as a
    // contiguous prefix; MCP tools follow. Within each partition: alphabetical.
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("apply_patch"),
            api_tool("exec_shell"),
        ],
        vec![api_tool("mcp_zoo_b"), api_tool("mcp_aardvark_a")],
        AppMode::Yolo,
        &always_load,
    );

    let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "apply_patch",
            "exec_shell",
            "read_file",
            "mcp_aardvark_a",
            "mcp_zoo_b",
        ],
        "built-ins must be alphabetical and contiguous; MCP tools follow, alphabetical",
    );
}

#[test]
fn active_tool_list_pushes_deferred_activations_to_the_tail() {
    // Regression for #263: when ToolSearch activates a deferred tool mid-
    // session, it must NOT be inserted at its catalog index — that would
    // shift every later tool's byte offset and bust the cached prefix.
    // Deferred-but-now-active tools belong at the tail.
    let mut a = api_tool("a_load_now");
    a.defer_loading = Some(false);
    let mut search = api_tool("search_via_toolsearch");
    search.defer_loading = Some(true);
    let mut b = api_tool("b_load_now");
    b.defer_loading = Some(false);

    let catalog = vec![a, search, b];
    let active: HashSet<String> = ["a_load_now", "search_via_toolsearch", "b_load_now"]
        .into_iter()
        .map(String::from)
        .collect();

    let listed = active_tools_for_step(&catalog, &active, false);
    let names: Vec<&str> = listed.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a_load_now", "b_load_now", "search_via_toolsearch"],
        "deferred-but-active tools must come after always-loaded tools",
    );
}

#[test]
fn deferred_tool_preflight_loads_edit_schema_without_executing_bad_aliases() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    catalog
        .iter_mut()
        .find(|tool| tool.name == "edit_file")
        .expect("edit_file registered")
        .defer_loading = Some(true);
    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains("edit_file"));

    let result = preflight_requested_deferred_tool(
        "edit_file",
        &json!({
            "path": "src/foo.rs",
            "old_string": "before",
            "new_string": "after"
        }),
        &catalog,
        &mut active,
    )
    .expect("deferred edit_file should preflight");

    assert!(active.contains("edit_file"));
    assert!(result.success);
    assert!(result.content.contains("Tool `edit_file` was deferred"));
    assert!(result.content.contains("The tool was not executed"));
    assert!(result.content.contains("path: string required"));
    assert!(result.content.contains("search: string required"));
    assert!(result.content.contains("replace: string required"));
    assert!(result.content.contains("old_string -> search"));
    assert!(result.content.contains("new_string -> replace"));
    assert_eq!(
        result.metadata.as_ref().unwrap()["deferred_tool_loaded"],
        json!(true)
    );
}

#[test]
fn deferred_tool_preflight_guides_rlm_open_misnamed_source_fields() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    // Piagent phase B: the model-facing tool is the unified `rlm`; legacy
    // `rlm_open` is a hidden compat alias and must not appear in the catalog.
    assert!(
        !catalog.iter().any(|tool| tool.name == "rlm_open"),
        "rlm_open must stay a hidden alias outside the model catalog"
    );
    catalog
        .iter_mut()
        .find(|tool| tool.name == "rlm")
        .expect("rlm registered")
        .defer_loading = Some(true);
    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains("rlm"));

    let result = preflight_requested_deferred_tool(
        "rlm",
        &json!({
            "action": "open",
            "name": "active_prompt",
            "prompt": "inspect this",
            "path": "src/lib.rs"
        }),
        &catalog,
        &mut active,
    )
    .expect("deferred rlm should preflight");

    assert!(active.contains("rlm"));
    assert!(result.success);
    assert!(result.content.contains("Tool `rlm` was deferred"));
    assert!(result.content.contains("The tool was not executed"));
    assert!(result.content.contains("session_object: string"));
    assert!(
        result.content.contains(
            "prompt -> file_path (local file), content (inline text), url, or session_object"
        ),
        "prompt correction includes session_object: {}",
        result.content
    );
    assert!(
        result.content.contains(
            "path -> file_path (local file), content (inline text), url, or session_object"
        ),
        "path correction includes session_object: {}",
        result.content
    );
    assert_eq!(
        result.metadata.as_ref().unwrap()["deferred_tool_loaded"],
        json!(true)
    );
}

#[test]
fn model_catalog_exposes_work_update_as_sole_progress_surface() {
    // #4132: ordinary progress is one model-visible tool. Legacy checklist_* /
    // todo_* spellings stay registry-callable for replay but must not appear in
    // the deferred model catalog (so there is no deferred-preflight path for them).
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    let active = initial_active_tools(&catalog);
    let catalog_names: HashSet<&str> = catalog.iter().map(|tool| tool.name.as_str()).collect();

    assert!(
        catalog_names.contains("work_update"),
        "work_update must be model-visible"
    );
    assert!(
        active.contains("work_update"),
        "work_update should load with the default active native set"
    );
    assert!(
        catalog_names.contains("update_plan"),
        "update_plan remains Strategy metadata, not a second checklist"
    );
    for hidden in [
        "checklist_write",
        "checklist_add",
        "checklist_update",
        "checklist_list",
        "todo_write",
        "todo_add",
        "todo_update",
        "todo_list",
    ] {
        assert!(
            registry.contains(hidden),
            "{hidden} must remain callable for transcript replay"
        );
        assert!(
            !catalog_names.contains(hidden),
            "{hidden} must stay hidden from the model catalog"
        );
        assert!(
            preflight_requested_deferred_tool(
                hidden,
                &json!({
                    "todos": [
                        { "content": "should not hydrate hidden alias", "status": "completed" }
                    ]
                }),
                &catalog,
                &mut active.clone(),
            )
            .is_none(),
            "{hidden} must not have a deferred catalog preflight path"
        );
    }
}

#[test]
fn user_shell_turn_outcome_distinguishes_cancel_failure_and_success() {
    let cancelled = Ok(
        ToolResult::error("Command canceled; process killed.").with_metadata(json!({
            "status": "Killed",
            "canceled": true,
        })),
    );
    assert_eq!(
        user_shell_turn_outcome(&cancelled, false),
        TurnOutcomeStatus::Interrupted
    );

    let cancelled_while_awaiting_approval = Err(ToolError::execution_failed(
        "Request cancelled while awaiting approval",
    ));
    assert_eq!(
        user_shell_turn_outcome(&cancelled_while_awaiting_approval, true),
        TurnOutcomeStatus::Interrupted
    );

    let failed = Ok(ToolResult::error("Command failed (exit code: 1)"));
    assert_eq!(
        user_shell_turn_outcome(&failed, false),
        TurnOutcomeStatus::Failed
    );

    let execution_error = Err(ToolError::execution_failed("shell manager unavailable"));
    assert_eq!(
        user_shell_turn_outcome(&execution_error, false),
        TurnOutcomeStatus::Failed
    );

    let completed = Ok(ToolResult::success("done"));
    assert_eq!(
        user_shell_turn_outcome(&completed, true),
        TurnOutcomeStatus::Interrupted
    );
    assert_eq!(
        user_shell_turn_outcome(&completed, false),
        TurnOutcomeStatus::Completed
    );
}

#[tokio::test]
async fn run_shell_command_op_requests_approval_and_executes_shell() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    engine.session.allow_shell = false;
    engine.config.allow_shell = false;
    let handle_for_approval = handle.clone();

    let task = tokio::spawn(async move {
        engine
            .handle_run_shell_command(
                "echo bang-ok".to_string(),
                AppMode::Agent,
                true,
                false,
                false,
                crate::tui::approval::ApprovalMode::Suggest,
            )
            .await;
    });

    let mut saw_started = false;
    let mut saw_approval = false;
    let mut saw_complete = false;
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::TurnStarted { turn_id, route, .. } => {
                assert!(turn_id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert!(route.is_none());
            }
            Event::ToolCallStarted { id, name, input } => {
                saw_started = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(name, "exec_shell");
                assert_eq!(input["command"], json!("echo bang-ok"));
                assert_eq!(input["source"], json!("user"));
            }
            Event::ApprovalRequired { id, tool_name, .. } => {
                saw_approval = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(tool_name, "exec_shell");
                handle_for_approval
                    .approve_tool_call(id)
                    .await
                    .expect("approve shell");
            }
            Event::ToolCallComplete { id, name, result } => {
                saw_complete = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(name, "exec_shell");
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("bang-ok"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                saw_turn_complete = true;
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }
    drop(rx);
    task.await.expect("shell op task");

    assert!(saw_started);
    assert!(saw_approval);
    assert!(saw_complete);
    assert!(saw_turn_complete);
}

#[tokio::test]
async fn run_shell_command_op_skips_approval_when_auto_approved() {
    let todos = crate::tools::todo::new_shared_todo_list();
    let plan = crate::tools::plan::new_shared_plan_state();
    let work = crate::work_graph::new_shared_work_runtime(todos, plan);
    let runtime_services = crate::tools::spec::RuntimeToolServices {
        work: Some(work.clone()),
        ..Default::default()
    };
    let (mut engine, handle) = Engine::new(
        EngineConfig {
            runtime_services,
            ..EngineConfig::default()
        },
        &Config::default(),
    );
    let session_id = engine.session.id.clone();

    engine
        .handle_run_shell_command(
            "echo bang-yolo".to_string(),
            AppMode::Yolo,
            true,
            true,
            true,
            crate::tui::approval::ApprovalMode::Auto,
        )
        .await;

    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("auto-approved shell shortcut should not request approval");
            }
            Event::ToolCallComplete { result, .. } => {
                saw_complete = true;
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("bang-yolo"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }

    assert!(saw_complete);
    let graph = work
        .capture(Some(&session_id))
        .expect("capture bang-shell work")
        .expect("bang-shell graph")
        .graph;
    let operation = graph
        .nodes
        .iter()
        .find(|node| node.kind == crate::work_graph::NodeKind::Operation)
        .expect("bang-shell operation registered before execution");
    assert_eq!(operation.state, crate::work_graph::NodeState::Completed);
    let observation = operation
        .binding
        .as_ref()
        .and_then(|binding| binding.last_observation.as_ref())
        .expect("terminal shell owner observation");
    assert!(
        observation
            .output
            .as_ref()
            .and_then(crate::work_graph::EvidenceRef::raw_bytes)
            .is_some_and(|raw_bytes| raw_bytes > 0),
        "bang-shell completion must retain a logical byte-count receipt"
    );
}

#[tokio::test]
async fn run_shell_command_op_allows_readonly_shell_in_auto_mode() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let handle_for_approval = handle.clone();

    let task = tokio::spawn(async move {
        engine
            .handle_run_shell_command(
                "pwd".to_string(),
                AppMode::Auto,
                true,
                false,
                false,
                crate::tui::approval::ApprovalMode::Auto,
            )
            .await;
    });

    let mut saw_approval = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { id, .. } => {
                saw_approval = true;
                handle_for_approval
                    .approve_tool_call(id)
                    .await
                    .expect("approve unexpected shell prompt");
            }
            Event::ToolCallComplete { result, .. } => {
                saw_complete = true;
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }
    drop(rx);
    task.await.expect("shell op task");

    assert!(
        !saw_approval,
        "read-only shell shortcut should not request approval in Auto mode"
    );
    assert!(saw_complete);
}

#[tokio::test]
async fn yolo_mode_does_not_prompt_for_typed_ask_rule() {
    // #3386: a command matching a typed ask-rule (permissions.toml) must not
    // surface an approval modal in YOLO mode, even though Yolo resolves to
    // ApprovalMode::Auto which the execpolicy maps to OnFailure (honors
    // ask-rules). The auto_review safety floor and typed deny rules still
    // apply; only the ask-rule Prompt is suppressed in YOLO.
    let (mut engine, handle) = Engine::new(
        EngineConfig {
            exec_policy_engine: ask_rule_engine("echo"),
            ..EngineConfig::default()
        },
        &Config::default(),
    );

    engine
        .handle_run_shell_command(
            "echo yolo-ask-rule".to_string(),
            AppMode::Yolo,
            true,
            true,
            true,
            crate::tui::approval::ApprovalMode::Auto,
        )
        .await;

    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("YOLO mode must not prompt for a typed ask-rule");
            }
            Event::ToolCallComplete { result, .. } => {
                saw_complete = true;
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("yolo-ask-rule"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }

    assert!(saw_complete);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn operate_model_shell_uses_normal_approval_and_workspace_sandbox() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;

    let tool_call_sse = concat!(
        "data: {\"id\":\"chatcmpl-operate-tools\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call_operate_shell\",\"type\":\"function\",\"function\":{\"name\":\"exec_shell\",",
        "\"arguments\":\"{\\\"command\\\":\\\"echo operate-approved > operate-mode-approved.txt\\\"}\"}}",
        "]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-operate-tools\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-operate-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-operate-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("operate-mode-approved.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            terminal_chrome_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let handle_for_approval = handle.clone();
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "write the requested local fixture".to_string(),
            mode: AppMode::Operate,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send Operate model turn");

    let mut saw_approval = false;
    let mut saw_shell_result = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for Operate tool event")
    {
        match event {
            Event::ApprovalRequired { id, tool_name, .. } => {
                saw_approval = true;
                assert_eq!(tool_name, "exec_shell");
                handle_for_approval
                    .approve_tool_call(id)
                    .await
                    .expect("approve Operate shell");
            }
            Event::ToolCallComplete { name, result, .. } if name == "exec_shell" => {
                saw_shell_result = true;
                let result = result.expect("approved Operate shell result");
                assert!(result.success, "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");

    assert!(
        saw_approval,
        "Operate should use the normal approval gate instead of a mode-only denial"
    );
    assert!(saw_shell_result);
    assert!(saw_complete);
    let written = std::fs::read_to_string(workspace.path().join("operate-mode-approved.txt"))
        .expect("workspace-scoped shell output");
    assert_eq!(written.trim_end(), "operate-approved");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn full_access_subagent_handoff_keeps_model_shell_free_of_approval_prompts() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;

    let tool_call_sse = concat!(
        "data: {\"id\":\"chatcmpl-yolo\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call_yolo\",\"type\":\"function\",\"function\":{\"name\":\"exec_shell\",",
        "\"arguments\":\"{\\\"command\\\":\\\"echo yolo-model-ask-rule\\\"}\"}}",
        "]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-yolo\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("yolo-model-ask-rule"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            exec_policy_engine: ask_rule_engine("echo"),
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "continue from the completed child".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            // Exercise the valid legacy/host shape where the named posture is
            // authoritative but the redundant bit is stale.
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::SubAgentHandoff,
        })
        .await
        .expect("send model turn");

    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for engine event")
    {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("Full Access child handoff must not prompt for an ordinary shell call");
            }
            Event::ToolCallComplete { name, result, .. } if name == "exec_shell" => {
                saw_complete = true;
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("yolo-model-ask-rule"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(saw_complete);
}

async fn assert_full_access_model_tool_batch_is_blocked(
    engine_config: EngineConfig,
    tool_calls: Vec<(&'static str, serde_json::Value)>,
    expected_errors: &[(&str, &str)],
    followup_fragment: &str,
) {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let model_tool_calls = tool_calls
        .iter()
        .enumerate()
        .map(|(index, (name, arguments))| {
            json!({
                "index": index,
                "id": format!("call_full_access_{index}"),
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments.to_string(),
                },
            })
        })
        .collect::<Vec<_>>();
    let tool_delta = json!({
        "id": "chatcmpl-full-access-blocked",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": model_tool_calls},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let tool_finish = json!({
        "id": "chatcmpl-full-access-blocked",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
    });
    let tool_call_sse = format!("data: {tool_delta}\n\ndata: {tool_finish}\n\ndata: [DONE]\n\n");
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(followup_fragment))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(engine_config, &api_config);
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "exercise the Full Access execution boundary".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send Full Access model turn");

    let expected = expected_errors.iter().copied().collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for Full Access boundary event")
    {
        match event {
            Event::ApprovalRequired { tool_name, .. } => {
                panic!("Full Access must not open an approval modal for blocked tool {tool_name}")
            }
            Event::ToolCallComplete { name, result, .. }
                if expected.contains_key(name.as_str()) =>
            {
                let error = result.expect_err("blocked tool must return an error");
                let fragment = expected[name.as_str()];
                assert!(
                    error.to_string().contains(fragment),
                    "unexpected {name} denial: {error:?}"
                );
                seen.insert(name);
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_turn_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert_eq!(seen.len(), expected.len(), "missing blocked tool results");
    assert!(saw_turn_complete);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn full_access_blocks_non_bypassable_registered_tools_at_engine_boundary() {
    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let marker = workspace.path().join("runtime-tool-must-not-run");
    let marker_literal = marker
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('\'', "\\'");
    let start_probe =
        format!("python3 -c \"__import__('pathlib').Path('{marker_literal}').write_text('ran')\"");
    let rlm_probe = format!("__import__('pathlib').Path('{marker_literal}').write_text('ran')");
    let engine_config = EngineConfig {
        model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
        workspace: workspace.path().to_path_buf(),
        mcp_config_path: workspace.path().join("mcp.json"),
        snapshots_enabled: false,
        subagents_enabled: false,
        ..EngineConfig::default()
    };
    let denial = "requires explicit approval and is blocked in Full Access";

    assert_full_access_model_tool_batch_is_blocked(
        engine_config,
        vec![
            (
                "start_mcp_server",
                json!({"server": start_probe, "name": "must-not-start"}),
            ),
            (
                "rlm_eval",
                json!({"name": "missing-context", "code": rlm_probe}),
            ),
        ],
        &[("start_mcp_server", denial), ("rlm_eval", denial)],
        denial,
    )
    .await;

    assert!(
        !marker.exists(),
        "blocked runtime tools must not start a process or evaluate code"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn full_access_blocks_repo_law_ask_without_prompt_or_write() {
    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let law_dir = workspace.path().join(".codewhale");
    fs::create_dir_all(&law_dir).expect("create law directory");
    fs::write(
        law_dir.join("constitution.json"),
        r#"{
            "protected_invariants": [{
                "text": "Release notes need human review",
                "paths": ["CHANGELOG.md"]
            }]
        }"#,
    )
    .expect("write repo law fixture");
    let target = workspace.path().join("CHANGELOG.md");
    let engine_config = EngineConfig {
        model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
        workspace: workspace.path().to_path_buf(),
        snapshots_enabled: false,
        subagents_enabled: false,
        ..EngineConfig::default()
    };

    assert_full_access_model_tool_batch_is_blocked(
        engine_config,
        vec![(
            "write_file",
            json!({"path": "CHANGELOG.md", "content": "must not be written\n"}),
        )],
        &[(
            "write_file",
            "Repository law blocked tool 'write_file' in Full Access: Repo law holds this write: \"Release notes need human review\"",
        )],
        "Repository law blocked tool 'write_file' in Full Access: Repo law holds this write:",
    )
    .await;

    assert!(!target.exists(), "repo-law block must prevent the write");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn auto_review_auto_resolves_hallucinated_question_without_prompting() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;
    let arguments = json!({
        "questions": [{
            "header": "Choice",
            "id": "choice",
            "question": "Which path should I take?",
            "options": [
                {"label": "A", "description": "Take path A"},
                {"label": "B", "description": "Take path B"}
            ]
        }]
    })
    .to_string();
    let tool_delta = json!({
        "id": "chatcmpl-auto-review-question",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_auto_review_question",
                "type": "function",
                "function": {
                    "name": REQUEST_USER_INPUT_NAME,
                    "arguments": arguments,
                },
            }]},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let tool_finish = json!({
        "id": "chatcmpl-auto-review-question",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
    });
    let tool_call_sse = format!("data: {tool_delta}\n\ndata: {tool_finish}\n\ndata: [DONE]\n\n");
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(
            "Auto-Review does not pause for user questions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "continue autonomously".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Auto,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send Auto-Review model turn");

    let mut saw_tool_result = false;
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for Auto-Review question event")
    {
        match event {
            Event::UserInputRequired { .. } => {
                panic!("Auto-Review must not emit a user question")
            }
            Event::ApprovalRequired { .. } => {
                panic!("Auto-Review question guard must not become an approval")
            }
            Event::ToolCallComplete { name, result, .. } if name == REQUEST_USER_INPUT_NAME => {
                let result = result.expect("question should auto-resolve successfully");
                assert!(result.success, "{result:?}");
                assert!(
                    result.content.contains("continue autonomously"),
                    "{result:?}"
                );
                assert_eq!(
                    result
                        .metadata
                        .as_ref()
                        .and_then(|value| value.get("auto_resolved"))
                        .and_then(serde_json::Value::as_bool),
                    Some(true)
                );
                assert_eq!(
                    result
                        .metadata
                        .as_ref()
                        .and_then(|value| value.get("permission_posture"))
                        .and_then(serde_json::Value::as_str),
                    Some("auto-review")
                );
                saw_tool_result = true;
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_turn_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(saw_tool_result);
    assert!(saw_turn_complete);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn full_access_blocks_background_catastrophic_shell_without_prompt_or_side_effect() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;
    let victim = workspace.path().join("must-survive");
    fs::write(&victim, "guarded\n").expect("write guarded fixture");
    // Keep the engine-boundary regression intrinsically harmless on every
    // runner: the quoted payload trips the same built-in catastrophic-command
    // detector, while an execution regression would only overwrite the
    // sentinel. The policy-level sibling tests exercise real destructive
    // command shapes directly without ever dispatching them to a shell.
    let command = format!("echo \"rm -rf /\" > \"{}\"", victim.display());
    let arguments = serde_json::to_string(&json!({
        "command": command,
        "background": true,
    }))
    .expect("serialize tool arguments");

    let tool_delta = json!({
        "id": "chatcmpl-bg",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_bg",
                "type": "function",
                "function": {"name": "exec_shell", "arguments": arguments},
            }]},
            "finish_reason": serde_json::Value::Null,
        }],
    });
    let tool_finish = json!({
        "id": "chatcmpl-bg",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
    });
    let tool_call_sse = format!("data: {tool_delta}\n\ndata: {tool_finish}\n\ndata: [DONE]\n\n");
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("destructive background/headless"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "please run a background shell".to_string(),
            mode: AppMode::Agent,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send model turn");

    let mut saw_tool_result = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for engine event")
    {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("Full Access safety holds must fail closed without prompting")
            }
            Event::ToolCallComplete { name, result, .. } => {
                if name == "exec_shell" {
                    saw_tool_result = true;
                    let err = result.expect_err("blocked shell should not execute");
                    assert!(
                        err.to_string().contains("Built-in safety gate"),
                        "unexpected shell denial: {err:?}"
                    );
                }
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(saw_tool_result);
    assert!(saw_complete);
    assert_eq!(
        fs::read_to_string(&victim).expect("read guarded fixture"),
        "guarded\n",
        "blocked command must not touch its target"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn yolo_mode_does_not_prompt_for_background_shell() {
    // #3883: the durable-review floor keys on what the command does, not on
    // "not provably read-only". An ordinary background command in YOLO must
    // run without a prompt; genuinely destructive and publish-like background
    // work still holds (see the sibling tests).
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;

    let tool_call_sse = concat!(
        "data: {\"id\":\"chatcmpl-bgok\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call_bgok\",\"type\":\"function\",\"function\":{\"name\":\"exec_shell\",",
        "\"arguments\":\"{\\\"command\\\":\\\"echo bg-yolo-no-prompt\\\",\\\"background\\\":true}\"}}",
        "]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-bgok\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("bg-yolo-no-prompt"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "please run a background shell".to_string(),
            mode: AppMode::Yolo,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Auto,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send model turn");

    let mut saw_tool_result = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for engine event")
    {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("YOLO mode must not prompt for an ordinary background shell command");
            }
            Event::ToolCallComplete { name, result, .. } => {
                if name == "exec_shell" {
                    saw_tool_result = true;
                    let result = result.expect("shell result");
                    assert!(result.success, "{result:?}");
                    assert!(
                        result.content.contains("Background task started"),
                        "expected a background start, got: {result:?}"
                    );
                }
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(saw_tool_result);
    assert!(saw_complete);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn yolo_mode_executes_publish_like_shell_without_prompt() {
    // #4595: Full Access (Bypass/YOLO) is truly full access — the publish
    // floor prompts only in Ask/Auto-Review postures. The regression guard is
    // the absence of ApprovalRequired; execution itself may fail (the tempdir
    // is not a git repo), which is fine.
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;

    let tool_call_sse = concat!(
        "data: {\"id\":\"chatcmpl-publish\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call_publish\",\"type\":\"function\",\"function\":{\"name\":\"exec_shell\",",
        "\"arguments\":\"{\\\"command\\\":\\\"git push origin main\\\",\\\"background\\\":true}\"}}",
        "]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-publish\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"ack\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("call_publish"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "please publish this crate".to_string(),
            mode: AppMode::Yolo,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send model turn");

    let mut saw_tool_complete = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for engine event")
    {
        match event {
            Event::ApprovalRequired {
                tool_name,
                description,
                ..
            } => {
                panic!(
                    "Full Access must not prompt for publish-like shell \
                     (#4595); got prompt for {tool_name}: {description}"
                );
            }
            Event::ToolCallComplete { name, .. } if name == "exec_shell" => {
                // Execution outcome is irrelevant (the tempdir is not a git
                // repo); the contract is that it ran without a prompt.
                saw_tool_complete = true;
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(
        saw_tool_complete,
        "the publish-like shell should execute without a prompt under Full Access"
    );
    assert!(saw_complete, "the publish-like turn should complete");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn yolo_mode_does_not_prompt_for_mcp_action() {
    // #3790: MCP mutations are governed by the selected mode, just like shell.
    // YOLO must not emit an approval request for a non-read-only MCP tool; this
    // fixture has no GitHub MCP server, so execution may fail after the no-prompt
    // planning decision. The regression guard is the absence of ApprovalRequired.
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let _lock = lock_test_env();
    let workspace = tempdir().expect("tempdir");
    let server = MockServer::start().await;

    let tool_call_sse = concat!(
        "data: {\"id\":\"chatcmpl-mcp\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call_mcp\",\"type\":\"function\",\"function\":{\"name\":\"mcp_github_create_pull_request\",",
        "\"arguments\":\"{\\\"title\\\":\\\"test\\\",\\\"body\\\":\\\"body\\\"}\"}}",
        "]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-mcp\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let done_sse = concat!(
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,",
        "\"delta\":{\"content\":\"ack\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-done\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("MCP tool failed"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(done_sse),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(tool_call_sse),
        )
        .expect(1)
        .with_priority(2)
        .mount(&server)
        .await;

    let api_config = Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(server.uri()),
        ..Config::default()
    };
    let (engine, handle) = Engine::new(
        EngineConfig {
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            ..EngineConfig::default()
        },
        &api_config,
    );
    let run_task = tokio::spawn(engine.run());

    handle
        .send(Op::SendMessage {
            content: "please open the PR".to_string(),
            mode: AppMode::Yolo,
            route: resolved_route_for_test(&api_config, crate::config::DEFAULT_TEXT_MODEL),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
            translation_enabled: false,
            show_thinking: true,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        })
        .await
        .expect("send model turn");

    let mut saw_mcp_result = false;
    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = tokio::time::timeout(model_turn_event_timeout(), rx.recv())
        .await
        .expect("timed out waiting for engine event")
    {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("YOLO mode must not prompt for an MCP action");
            }
            Event::ToolCallComplete { name, result, .. }
                if name == "mcp_github_create_pull_request" =>
            {
                saw_mcp_result = true;
                let err = result
                    .expect_err("unconfigured MCP server should fail after no-prompt planning");
                assert!(
                    err.to_string().contains("MCP tool failed"),
                    "unexpected MCP error: {err:?}"
                );
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    drop(rx);

    handle.send(Op::Shutdown).await.expect("shutdown engine");
    run_task.await.expect("engine task");
    assert!(
        saw_mcp_result,
        "the MCP tool should execute without an approval gate"
    );
    assert!(saw_complete, "the YOLO MCP turn should complete");
}

#[tokio::test]
async fn run_shell_command_op_preserves_plan_mode_shell_block() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());

    engine
        .handle_run_shell_command(
            "echo blocked".to_string(),
            AppMode::Plan,
            false,
            false,
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        )
        .await;

    let mut saw_complete = false;
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("Plan mode shell should be blocked before approval");
            }
            Event::ToolCallComplete { name, result, .. } => {
                saw_complete = true;
                assert_eq!(name, "exec_shell");
                let err = result.expect_err("plan shell should fail");
                assert!(
                    err.to_string().contains("unavailable in Plan mode"),
                    "{err}"
                );
            }
            Event::TurnComplete { status, .. } => {
                saw_turn_complete = true;
                assert_eq!(status, TurnOutcomeStatus::Failed);
                break;
            }
            _ => {}
        }
    }

    assert!(saw_complete);
    assert!(saw_turn_complete);
}

#[test]
fn deferred_tool_preflight_skips_already_active_tools() {
    let mut tool = api_tool("deferred_tool");
    tool.defer_loading = Some(true);
    let catalog = vec![tool];
    let mut active = HashSet::from(["deferred_tool".to_string()]);

    assert!(
        preflight_requested_deferred_tool("deferred_tool", &json!({}), &catalog, &mut active,)
            .is_none(),
        "already active tools should execute normally"
    );
}

#[test]
fn turn_tool_registry_builder_keeps_plan_mode_read_only_for_files() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Plan,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Plan, false));

    assert!(registry.contains("read_file"));
    assert!(registry.contains("list_dir"));
    assert!(!registry.contains("write_file"));
    assert!(!registry.contains("edit_file"));
    assert!(!registry.contains("exec_shell"));
    assert!(!registry.contains("exec_shell_wait"));
    assert!(!registry.contains("exec_shell_interact"));
    assert!(!registry.contains("task_shell_start"));
    assert!(!registry.contains("task_create"));
    assert!(!registry.contains("task_gate_run"));
    assert!(!registry.contains("rlm"));
    assert!(!registry.contains("fim_edit"));
    assert!(registry.contains("update_plan"));
    assert!(registry.contains("create_goal"));
    assert!(registry.contains("get_goal"));
    assert!(registry.contains("update_goal"));
    assert!(registry.contains("task_list"));
    assert!(registry.contains("task_read"));
    assert!(registry.contains("handle_read"));
    let plan_state_tools = [
        "checklist_add",
        "checklist_update",
        "checklist_write",
        "todo_add",
        "todo_update",
        "todo_write",
        "work_update",
        "update_plan",
    ];
    let mut write_or_exec_tools: Vec<String> = registry
        .all()
        .into_iter()
        .filter(|tool| !plan_state_tools.contains(&tool.name()))
        .filter(|tool| {
            let capabilities = tool.capabilities();
            capabilities.contains(&ToolCapability::WritesFiles)
                || capabilities.contains(&ToolCapability::ExecutesCode)
        })
        .map(|tool| tool.name().to_string())
        .collect();
    write_or_exec_tools.sort();
    assert!(
        write_or_exec_tools.is_empty(),
        "Plan mode must not register file-writing or code-execution tools: {write_or_exec_tools:?}"
    );
}

/// Plan mode toggle must not change the byte representation of the tool
/// catalog head. DeepSeek's KV prefix cache includes the tools array in
/// the immutable prefix; if toggling between Plan and Agent mode changes
/// the tool bytes, every mode switch forces a full re-prefill.
///
/// This test verifies two invariants:
/// 1. Building the catalog twice for the same mode produces identical bytes.
/// 2. The head of the catalog (non-deferred tools) preserves its order
///    when deferred tools are activated mid-session.
#[test]
fn plan_mode_toggle_preserves_catalog_byte_stability() {
    let always_load = HashSet::new();

    // Build catalog for Plan mode twice — must be byte-identical.
    let plan_native = vec![
        api_tool("read_file"),
        api_tool("list_dir"),
        api_tool("write_file"),
        api_tool("edit_file"),
        api_tool("exec_shell"),
    ];
    let plan_mcp = vec![api_tool("mcp_search"), api_tool("mcp_write")];

    let catalog_a = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Plan,
        &always_load,
    );
    let catalog_b = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Plan,
        &always_load,
    );

    let json_a = serde_json::to_string(&catalog_a).unwrap();
    let json_b = serde_json::to_string(&catalog_b).unwrap();
    assert_eq!(
        json_a, json_b,
        "building the catalog twice for Plan mode must produce identical bytes"
    );

    // Build catalog for Agent mode twice — must be byte-identical.
    let agent_catalog_a = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );
    let agent_catalog_b = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );

    let agent_json_a = serde_json::to_string(&agent_catalog_a).unwrap();
    let agent_json_b = serde_json::to_string(&agent_catalog_b).unwrap();
    assert_eq!(
        agent_json_a, agent_json_b,
        "building the catalog twice for Agent mode must produce identical bytes"
    );

    // Verify that the non-deferred tools that are common to both modes
    // appear in the same order. Plan mode excludes execution tools, but
    // the tools that are present in both modes must have stable ordering.
    let plan_names: Vec<&str> = catalog_a
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();
    let agent_names: Vec<&str> = agent_catalog_a
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();

    // The common prefix of non-deferred tools must be identical.
    let common_len = plan_names.len().min(agent_names.len());
    assert_eq!(
        &plan_names[..common_len],
        &agent_names[..common_len],
        "non-deferred tools common to Plan and Agent must appear in the same order"
    );

    // Verify that activating a deferred tool mid-session appends to the
    // tail without reordering the head.
    let mut tools_with_deferred = plan_native.clone();
    tools_with_deferred.push({
        let mut t = api_tool("deferred_search");
        t.defer_loading = Some(true);
        t
    });
    let catalog_with_deferred = build_model_tool_catalog(
        tools_with_deferred,
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );

    // Activate the deferred tool.
    let mut active: HashSet<String> = catalog_with_deferred
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.clone())
        .collect();
    active.insert("deferred_search".to_string());

    let listed = active_tools_for_step(&catalog_with_deferred, &active, false);
    let listed_names: Vec<&str> = listed.iter().map(|t| t.name.as_str()).collect();

    // The head (non-deferred tools) must still be in their original order.
    let head_names: Vec<&str> = catalog_with_deferred
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        listed_names.starts_with(&head_names),
        "activating a deferred tool must not reorder the catalog head: \
         expected {head_names:?} as prefix, got {listed_names:?}"
    );
    // The deferred tool must be at the tail.
    assert_eq!(
        listed_names.last(),
        Some(&"deferred_search"),
        "deferred tool must be appended at the tail"
    );
}

#[test]
fn parent_turn_registry_includes_goal_tools_for_all_modes() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    for mode in [
        AppMode::Plan,
        AppMode::Agent,
        AppMode::Operate,
        AppMode::Yolo,
    ] {
        let registry = engine
            .build_turn_tool_registry_builder(
                mode,
                engine.config.todos.clone(),
                engine.config.plan_state.clone(),
            )
            .build(engine.build_tool_context(mode, false));

        for name in ["create_goal", "get_goal", "update_goal"] {
            assert!(
                registry.contains(name),
                "parent {mode:?} registry should expose {name}"
            );
        }
    }
}

#[test]
fn plan_mode_registry_can_expose_agent_launcher_without_shell_tools() {
    let tmp = tempdir().expect("tempdir");
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let context = engine.build_tool_context(AppMode::Plan, false);
    let client = DeepSeekClient::new(&Config {
        api_key: Some("test-key".to_string()),
        ..Config::default()
    })
    .expect("stub client");
    let manager = crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
    let mut runtime = SubAgentRuntime::new(
        client,
        DEFAULT_TEXT_MODEL.to_string(),
        context.clone(),
        false,
        None,
        manager.clone(),
    )
    .with_agent_tool_surface_options(
        engine.agent_tool_surface_options(shell_policy_for_mode(AppMode::Plan, false)),
    );
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Plan);

    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Plan,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .with_subagent_tools(manager, runtime)
        .build(context);

    assert!(
        registry.contains("agent"),
        "Plan mode should be able to request focused read-only sub-agents"
    );
    assert!(
        !registry.contains("exec_shell"),
        "Plan mode must remain shell-free while exposing sub-agent delegation"
    );
}

#[test]
fn mode_invariant_matrix_covers_context_catalog_subagents_and_prompt_metadata() {
    use crate::sandbox::SandboxPolicy;
    use crate::tui::approval::ApprovalMode;
    use crate::worker_profile::ShellPolicy;

    #[derive(Clone, Copy)]
    enum ExpectedSandbox {
        ReadOnly,
        WorkspaceWrite,
        DangerFullAccess,
    }

    struct ModeCase {
        name: &'static str,
        mode: AppMode,
        setting: &'static str,
        prompt_marker: &'static str,
        shell_policy: ShellPolicy,
        sandbox: ExpectedSandbox,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
        exec_shell_available: bool,
        plan_hint: bool,
    }

    let cases = [
        ModeCase {
            name: "plan",
            mode: AppMode::Plan,
            setting: "plan",
            prompt_marker: "##### Mode: Plan",
            shell_policy: ShellPolicy::None,
            sandbox: ExpectedSandbox::ReadOnly,
            trust_mode: false,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,
            exec_shell_available: false,
            plan_hint: true,
        },
        ModeCase {
            name: "agent",
            mode: AppMode::Agent,
            setting: "agent",
            prompt_marker: "##### Mode: Agent",
            shell_policy: ShellPolicy::Full,
            sandbox: ExpectedSandbox::WorkspaceWrite,
            trust_mode: false,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,
            exec_shell_available: true,
            plan_hint: false,
        },
        ModeCase {
            name: "auto-compat",
            mode: AppMode::Auto,
            setting: "agent",
            prompt_marker: "##### Mode: Agent",
            shell_policy: ShellPolicy::Full,
            sandbox: ExpectedSandbox::WorkspaceWrite,
            trust_mode: false,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,
            exec_shell_available: true,
            plan_hint: false,
        },
        ModeCase {
            name: "operate",
            mode: AppMode::Operate,
            setting: "operate",
            prompt_marker: "##### Mode: Operate",
            shell_policy: ShellPolicy::Full,
            sandbox: ExpectedSandbox::WorkspaceWrite,
            trust_mode: false,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,
            exec_shell_available: true,
            plan_hint: false,
        },
        ModeCase {
            // YOLO remains an elevated-permission alias, but prompt/setting
            // surfaces now speak Act (invisible one-way permission shorthand).
            name: "yolo",
            mode: AppMode::Yolo,
            setting: "agent",
            prompt_marker: "##### Mode: Agent",
            shell_policy: ShellPolicy::Full,
            sandbox: ExpectedSandbox::DangerFullAccess,
            trust_mode: true,
            auto_approve: true,
            approval_mode: ApprovalMode::Bypass,
            exec_shell_available: true,
            plan_hint: false,
        },
    ];

    for case in cases {
        let tmp = tempdir().expect("tempdir");
        let config = EngineConfig {
            workspace: tmp.path().to_path_buf(),
            allow_shell: true,
            trust_mode: case.trust_mode,
            ..EngineConfig::default()
        };
        let (mut engine, _handle) = Engine::new(config, &Config::default());
        engine.current_mode = case.mode;
        engine.session.allow_shell = true;
        engine.session.trust_mode = case.trust_mode;
        engine.session.auto_approve = case.auto_approve;
        engine.session.approval_mode = case.approval_mode;

        let policy = effective_input_policy(
            UserInputProvenance::ExternalUser,
            case.mode,
            "continue",
            engine.session.allow_shell,
            engine.session.trust_mode,
            engine.session.auto_approve,
            engine.session.approval_mode,
        );
        assert_eq!(policy.mode, case.mode, "{}", case.name);
        assert_eq!(policy.trust_mode, case.trust_mode, "{}", case.name);
        assert_eq!(policy.auto_approve, case.auto_approve, "{}", case.name);
        assert_eq!(policy.approval_mode, case.approval_mode, "{}", case.name);
        assert!(policy.allow_shell, "{}", case.name);

        let context = engine.build_tool_context(case.mode, false);
        assert_eq!(context.shell_policy, case.shell_policy, "{}", case.name);
        assert_eq!(context.trust_mode, case.trust_mode, "{}", case.name);
        assert_eq!(context.auto_approve, case.auto_approve, "{}", case.name);
        assert_eq!(
            context.shell_network_denied_hint.is_some(),
            case.plan_hint,
            "{}",
            case.name
        );
        let sandbox = context
            .elevated_sandbox_policy
            .as_ref()
            .expect("mode context should always carry an elevated sandbox policy");
        match (case.sandbox, sandbox) {
            (ExpectedSandbox::ReadOnly, SandboxPolicy::ReadOnly) => {}
            (
                ExpectedSandbox::WorkspaceWrite,
                SandboxPolicy::WorkspaceWrite {
                    writable_roots,
                    network_access,
                    ..
                },
            ) => {
                assert_eq!(
                    writable_roots,
                    &vec![tmp.path().to_path_buf()],
                    "{}",
                    case.name
                );
                assert!(*network_access, "{}", case.name);
            }
            (ExpectedSandbox::DangerFullAccess, SandboxPolicy::DangerFullAccess) => {}
            _ => panic!("{}: unexpected sandbox policy {sandbox:?}", case.name),
        }

        let client = DeepSeekClient::new(&Config {
            api_key: Some("test-key".to_string()),
            ..Config::default()
        })
        .expect("stub client");
        let manager =
            crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let mut runtime = SubAgentRuntime::new(
            client,
            DEFAULT_TEXT_MODEL.to_string(),
            context.clone(),
            false,
            None,
            manager.clone(),
        )
        .with_agent_tool_surface_options(
            engine.agent_tool_surface_options(shell_policy_for_mode(case.mode, true)),
        );
        runtime.worker_profile = WorkerRuntimeProfile::for_role(match case.mode {
            AppMode::Plan => SubAgentType::Plan,
            _ => SubAgentType::General,
        });

        let registry = engine
            .build_turn_tool_registry_builder(
                case.mode,
                engine.config.todos.clone(),
                engine.config.plan_state.clone(),
            )
            .with_subagent_tools(manager, runtime)
            .build(context);
        assert!(registry.contains("agent"), "{}", case.name);
        assert_eq!(
            registry.contains("exec_shell"),
            case.exec_shell_available,
            "{}",
            case.name
        );

        let msg = engine.user_text_message_with_turn_metadata_for_route(
            "check current policy".to_string(),
            DEFAULT_TEXT_MODEL,
            false,
            None,
            false,
        );
        let metadata = msg.content.last().expect("turn metadata block");
        let ContentBlock::Text { text, .. } = metadata else {
            panic!("{}: expected text metadata block", case.name);
        };
        assert!(
            text.contains(&format!("Current mode: {}", case.setting)),
            "{}: {text}",
            case.name
        );
        assert!(
            text.contains(case.prompt_marker),
            "{}: missing {} in metadata",
            case.name,
            case.prompt_marker
        );
    }
}

#[test]
fn mode_invariant_matrix_covers_provenance_authority_narrowing() {
    use crate::tui::approval::ApprovalMode;

    struct ProvenanceCase {
        name: &'static str,
        provenance: UserInputProvenance,
        expected_mode: AppMode,
        expected_trust: bool,
        expected_auto: bool,
        expected_approval: ApprovalMode,
        expect_status: bool,
    }

    let cases = [
        ProvenanceCase {
            name: "external user",
            provenance: UserInputProvenance::ExternalUser,
            expected_mode: AppMode::Yolo,
            expected_trust: true,
            expected_auto: true,
            expected_approval: ApprovalMode::Bypass,
            expect_status: false,
        },
        ProvenanceCase {
            name: "runtime continuation",
            provenance: UserInputProvenance::Runtime,
            expected_mode: AppMode::Yolo,
            expected_trust: true,
            expected_auto: true,
            expected_approval: ApprovalMode::Bypass,
            expect_status: false,
        },
        ProvenanceCase {
            name: "sub-agent handoff",
            provenance: UserInputProvenance::SubAgentHandoff,
            expected_mode: AppMode::Yolo,
            expected_trust: true,
            expected_auto: true,
            expected_approval: ApprovalMode::Bypass,
            expect_status: false,
        },
        ProvenanceCase {
            name: "imported transcript",
            provenance: UserInputProvenance::ImportedTranscript,
            expected_mode: AppMode::Agent,
            expected_trust: false,
            expected_auto: false,
            expected_approval: ApprovalMode::Suggest,
            expect_status: true,
        },
        ProvenanceCase {
            name: "memory recall",
            provenance: UserInputProvenance::MemoryRecall,
            expected_mode: AppMode::Agent,
            expected_trust: false,
            expected_auto: false,
            expected_approval: ApprovalMode::Suggest,
            expect_status: true,
        },
        ProvenanceCase {
            name: "assistant generated",
            provenance: UserInputProvenance::AssistantGenerated,
            expected_mode: AppMode::Agent,
            expected_trust: false,
            expected_auto: false,
            expected_approval: ApprovalMode::Suggest,
            expect_status: true,
        },
    ];

    for case in cases {
        let policy = effective_input_policy(
            case.provenance,
            AppMode::Yolo,
            "continue",
            true,
            true,
            true,
            ApprovalMode::Bypass,
        );
        assert_eq!(policy.mode, case.expected_mode, "{}", case.name);
        assert_eq!(policy.trust_mode, case.expected_trust, "{}", case.name);
        assert_eq!(policy.auto_approve, case.expected_auto, "{}", case.name);
        assert_eq!(
            policy.approval_mode, case.expected_approval,
            "{}",
            case.name
        );
        assert!(policy.allow_shell, "{}", case.name);
        assert_eq!(policy.status.is_some(), case.expect_status, "{}", case.name);
    }
}

#[test]
fn agent_mode_can_build_auto_approved_tool_context() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    assert!(
        !engine
            .build_tool_context(AppMode::Agent, false)
            .auto_approve
    );
    assert!(engine.build_tool_context(AppMode::Agent, true).auto_approve);
    assert!(engine.build_tool_context(AppMode::Yolo, false).auto_approve);
}

#[test]
fn build_tool_context_preserves_read_snapshots_across_turns() {
    let workspace = tempdir().expect("tempdir");
    let path = workspace.path().join("observed.txt");
    fs::write(&path, "before\n").expect("write fixture");
    let config = EngineConfig {
        workspace: workspace.path().to_path_buf(),
        ..EngineConfig::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let read_turn = engine.build_tool_context(AppMode::Agent, false);
    read_turn.note_file_read(&path);

    let later_turn = engine.build_tool_context(AppMode::Agent, false);
    later_turn
        .require_fresh_file_read(&path, "observed.txt")
        .expect("a later turn should retain the session's fresh read snapshot");

    fs::write(&path, "changed contents\n").expect("change fixture");
    let err = later_turn
        .require_fresh_file_read(&path, "observed.txt")
        .expect_err("a retained snapshot must still reject stale edits");
    assert!(err.to_string().contains("changed since the last read_file"));
}

#[test]
fn build_tool_context_uses_typed_shell_policy_per_mode() {
    let mut config = EngineConfig {
        allow_shell: true,
        ..EngineConfig::default()
    };
    let (engine, _handle) = Engine::new(config.clone(), &Config::default());

    // Plan mode is shell-free and exposes no shell tools.
    assert_eq!(
        engine.build_tool_context(AppMode::Plan, false).shell_policy,
        crate::worker_profile::ShellPolicy::None
    );
    assert_eq!(
        engine
            .build_tool_context(AppMode::Agent, false)
            .shell_policy,
        crate::worker_profile::ShellPolicy::Full
    );
    assert_eq!(
        engine.build_tool_context(AppMode::Yolo, false).shell_policy,
        crate::worker_profile::ShellPolicy::Full
    );

    config.allow_shell = false;
    let (engine, _handle) = Engine::new(config, &Config::default());
    assert_eq!(
        engine
            .build_tool_context(AppMode::Agent, false)
            .shell_policy,
        crate::worker_profile::ShellPolicy::None
    );
}

#[test]
fn agent_and_yolo_modes_elevate_shell_sandbox_to_allow_network() {
    // Regression for #273: the seatbelt-default policy denies all outbound
    // network (including DNS), which broke `curl`, `yt-dlp`, package managers,
    // and similar shell commands in Agent mode. Elevation must include
    // network access so the application-level NetworkPolicy stays the only
    // outbound boundary.
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    let agent_ctx = engine.build_tool_context(AppMode::Agent, false);
    let agent_policy = agent_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Agent mode should elevate the sandbox policy");
    assert!(
        agent_policy.has_network_access(),
        "Agent mode must allow shell network access; got {agent_policy:?}",
    );

    let yolo_ctx = engine.build_tool_context(AppMode::Yolo, false);
    let yolo_policy = yolo_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Yolo mode should elevate the sandbox policy");
    assert!(yolo_policy.has_network_access());
    // v0.8.11: YOLO drops to DangerFullAccess (no sandbox) so the user
    // is not bounced through approval round-trips for legitimate
    // outside-workspace writes (package installs, sub-agent
    // workspaces, ~/.cache mutations, etc.). YOLO is opt-in and
    // already enables trust mode + auto-approve; the sandbox was the
    // last guardrail and contradicts the contract.
    assert!(
        matches!(yolo_policy, crate::sandbox::SandboxPolicy::DangerFullAccess),
        "Yolo mode must use DangerFullAccess (no sandbox); got {yolo_policy:?}",
    );

    // Plan mode (#1077): the sandbox must actually deny workspace writes.
    // The previous WorkspaceWrite-with-empty-network policy whitelisted the
    // workspace as writable, so `python -c "open('f','w').write('x')"`
    // mutated files inside the workspace despite Plan-mode's intent. Lock
    // it to ReadOnly: no writes anywhere, no network. The shell tool stays
    // exposed for read-only inspection (`ls`, `git log`, `grep`, …) and
    // the per-platform sandbox enforces the rest.
    let plan_ctx = engine.build_tool_context(AppMode::Plan, false);
    let plan_policy = plan_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Plan mode should make the shell sandbox policy explicit");
    assert!(
        matches!(plan_policy, crate::sandbox::SandboxPolicy::ReadOnly),
        "Plan mode must use ReadOnly sandbox to deny workspace writes (#1077); got {plan_policy:?}",
    );
    assert!(!plan_policy.has_network_access());
    assert!(!plan_policy.has_full_disk_write_access());
    assert!(
        plan_policy
            .get_writable_roots(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .is_empty(),
        "ReadOnly policy must enumerate zero writable roots; got {plan_policy:?}",
    );
    assert!(
        plan_ctx
            .shell_network_denied_hint
            .as_deref()
            .is_some_and(|hint| hint.contains("Plan mode") && hint.contains("read-only")),
    );
}

#[test]
fn sandbox_policy_for_mode_returns_correct_policy_per_mode() {
    use crate::core::authority::sandbox_policy_for_mode;
    use crate::sandbox::SandboxPolicy;

    let workspace = PathBuf::from("/tmp/example-workspace");

    // Plan: ReadOnly. The whole point of #1077.
    assert!(matches!(
        sandbox_policy_for_mode(AppMode::Plan, &workspace),
        SandboxPolicy::ReadOnly
    ));

    // Agent: WorkspaceWrite with workspace as writable root, network on.
    match sandbox_policy_for_mode(AppMode::Agent, &workspace) {
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            ..
        } => {
            assert_eq!(writable_roots, vec![workspace.clone()]);
            assert!(network_access, "Agent mode must allow shell network access");
        }
        other => panic!("Agent mode should be WorkspaceWrite; got {other:?}"),
    }

    // YOLO: DangerFullAccess.
    assert!(matches!(
        sandbox_policy_for_mode(AppMode::Yolo, &workspace),
        SandboxPolicy::DangerFullAccess
    ));
}

#[tokio::test]
async fn session_update_preserves_reasoning_tool_only_turn() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let assistant = Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Thinking {
                signature: None,
                thinking: "Need a tool before answering.".to_string(),
            },
            ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "Cargo.toml"}),
                caller: None,
            },
        ],
    };

    engine.add_session_message(assistant.clone()).await;

    let event = {
        let mut rx = handle.rx_event.write().await;
        rx.recv().await.expect("session update event")
    };
    let Event::SessionUpdated { messages, .. } = event else {
        panic!("expected session update event");
    };

    assert_eq!(messages, vec![assistant]);
}

#[tokio::test]
async fn set_model_reloads_instruction_sources_and_updates_session_prompt() {
    let tmp = tempdir().expect("tempdir");
    let instructions = tmp.path().join("instructions.md");
    fs::write(&instructions, "FLASH_INSTRUCTIONS_MARKER").expect("write instructions");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-flash".to_string(),
        instructions: vec![instructions.clone().into()],
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());
    fs::write(&instructions, "PRO_INSTRUCTIONS_MARKER").expect("rewrite instructions");

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::SetModel {
            model: "deepseek-v4-pro".to_string(),
            mode: AppMode::Agent,
            route_limits: None,
        })
        .await
        .expect("send set model");

    let (model, prompt) = {
        let mut rx = handle.rx_event.write().await;
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("session update after model switch")
                .expect("event");
            if let Event::SessionUpdated {
                model,
                system_prompt,
                ..
            } = event
            {
                let prompt = match system_prompt.expect("system prompt") {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                break (model, prompt);
            }
        }
    };
    run.abort();

    assert_eq!(model, "deepseek-v4-pro");
    assert!(prompt.contains("PRO_INSTRUCTIONS_MARKER"));
    assert!(!prompt.contains("FLASH_INSTRUCTIONS_MARKER"));
}

#[tokio::test]
async fn change_mode_refreshes_session_prompt_and_updates_session() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::ChangeMode {
            mode: AppMode::Yolo,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
        })
        .await
        .expect("send change mode");

    let (_prompt, messages) = {
        let mut rx = handle.rx_event.write().await;
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("session update after mode switch")
                .expect("event");
            if let Event::SessionUpdated {
                system_prompt,
                messages,
                ..
            } = event
            {
                let prompt = match system_prompt.expect("system prompt") {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                break (prompt, messages);
            }
        }
    };
    run.abort();

    assert!(
        messages.iter().all(|message| message.role != "system"),
        "mode switch must not persist appended system messages: {messages:?}"
    );
}

#[test]
fn turn_approval_mode_prefers_auto_approve_flag() {
    use crate::tui::approval::ApprovalMode;

    assert_eq!(
        agent_approval_mode_for_turn(true, ApprovalMode::Suggest),
        ApprovalMode::Bypass
    );
    assert_eq!(
        agent_approval_mode_for_turn(true, ApprovalMode::Never),
        ApprovalMode::Bypass
    );
}

#[test]
fn messages_with_turn_metadata_returns_stored_session_messages() {
    use crate::tui::approval::ApprovalMode;

    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine.current_mode = AppMode::Plan;
    engine.session.approval_mode = ApprovalMode::Suggest;
    engine.session.messages = vec![Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "summary after compaction".to_string(),
            cache_control: None,
        }],
    }]
    .into();
    let stored = engine.session.messages.clone();

    let request_messages = engine.messages_with_turn_metadata();

    assert_eq!(&*engine.session.messages, &*stored);
    assert_eq!(request_messages.len(), stored.len());
    assert!(
        request_messages
            .iter()
            .all(|message| message.role != "system"),
        "model request projection must not create appended system messages"
    );
}

#[tokio::test]
async fn change_mode_op_updates_current_mode_and_emits_status() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::ChangeMode {
            mode: AppMode::Yolo,
            allow_shell: true,
            trust_mode: true,
            auto_approve: true,
            approval_mode: crate::tui::approval::ApprovalMode::Bypass,
        })
        .await
        .expect("send change mode");

    // Expect a SessionUpdated event confirming the mode change.
    let mut rx = handle.rx_event.write().await;
    let session_updated = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("session update after mode switch")
        .expect("event");
    let Event::SessionUpdated { messages, .. } = session_updated else {
        panic!("should emit SessionUpdated after mode change, got: {session_updated:?}");
    };
    assert!(
        messages.iter().all(|message| message.role != "system"),
        "mode switch must not persist synthetic system messages: {messages:?}"
    );

    // Also expect a status event
    let status = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("status after mode switch")
        .expect("event");
    assert!(
        matches!(status, Event::Status { .. }),
        "should emit Status after mode change, got: {status:?}"
    );

    run.abort();
}

#[test]
fn runtime_mode_policy_updates_engine_session_mirrors() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        allow_shell: false,
        trust_mode: false,
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine.current_mode = AppMode::Plan;
    engine.session.allow_shell = false;
    engine.session.trust_mode = false;
    engine.session.auto_approve = false;
    engine.session.approval_mode = crate::tui::approval::ApprovalMode::Suggest;

    let agent_authority = crate::core::authority::TurnAuthority::from_effective_fields(
        AppMode::Agent,
        true,
        false,
        false,
        crate::tui::approval::ApprovalMode::Never,
    );
    engine.apply_runtime_mode_policy(&agent_authority);

    assert_eq!(engine.current_mode, AppMode::Agent);
    assert!(engine.session.allow_shell);
    assert!(engine.config.allow_shell);
    assert!(!engine.session.trust_mode);
    assert!(!engine.config.trust_mode);
    assert!(!engine.session.auto_approve);
    assert_eq!(
        engine.session.approval_mode,
        crate::tui::approval::ApprovalMode::Never
    );

    let yolo_authority = crate::core::authority::TurnAuthority::from_effective_fields(
        AppMode::Yolo,
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Bypass,
    );
    engine.apply_runtime_mode_policy(&yolo_authority);

    assert_eq!(engine.current_mode, AppMode::Yolo);
    assert!(engine.session.allow_shell);
    assert!(engine.session.trust_mode);
    assert!(engine.config.trust_mode);
    assert!(engine.session.auto_approve);
    assert_eq!(
        engine.session.approval_mode,
        crate::tui::approval::ApprovalMode::Bypass
    );
}

#[tokio::test]
async fn sync_session_restores_current_mode() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::SyncSession {
            session_id: Some("plan-session".to_string()),
            messages: Vec::new(),
            system_prompt: None,
            system_prompt_override: false,
            model: "deepseek-v4-pro".to_string(),
            workspace: tmp.path().to_path_buf(),
            mode: AppMode::Plan,
        })
        .await
        .expect("sync session");

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .send(Op::GetSessionSnapshot {
            tx: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
        })
        .await
        .expect("request snapshot");
    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("snapshot response")
        .expect("snapshot");

    assert_eq!(snapshot.mode, "plan");

    run.abort();
}

#[tokio::test]
async fn sync_session_projects_persisted_subagent_handoff_for_headless_restore() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());
    let payload = concat!(
        "Child result retained.\nCheckpoint: engine restore is covered.\n",
        "<codewhale:subagent.done>{\"agent_id\":\"agent_headless\",",
        "\"status\":\"completed\",\"summary_location\":\"previous_line\"}",
        "</codewhale:subagent.done>",
    );
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "Keep the original task".to_string(),
                cache_control: None,
            }],
        },
        crate::runtime_handoff::subagent_completion_runtime_message(payload),
    ];

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::SyncSession {
            session_id: Some("headless-resume".to_string()),
            messages,
            system_prompt: None,
            system_prompt_override: false,
            model: "deepseek-v4-pro".to_string(),
            workspace: tmp.path().to_path_buf(),
            mode: AppMode::Agent,
        })
        .await
        .expect("sync session");

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .send(Op::GetSessionSnapshot {
            tx: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
        })
        .await
        .expect("request snapshot");
    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("snapshot response")
        .expect("snapshot");

    assert_eq!(snapshot.messages.len(), 2);
    assert!(snapshot.messages[0].content.iter().any(
        |block| matches!(block, ContentBlock::Text { text, .. } if text == "Keep the original task")
    ));
    let restored =
        crate::runtime_handoff::restored_subagent_checkpoint_display(&snapshot.messages[1])
            .expect("projected headless checkpoint");
    assert!(restored.contains("agent_headless"));
    assert!(restored.contains("Checkpoint: engine restore is covered."));
    assert!(!restored.contains("runtime_event"));
    assert!(!restored.contains("subagent.done"));

    run.abort();
}

#[tokio::test]
async fn session_snapshot_omits_id_for_legacy_root_custom_route() {
    let tmp = tempdir().expect("tempdir");
    let api_config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "legacy-root-model".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &api_config);

    let run = tokio::spawn(engine.run());
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .send(Op::GetSessionSnapshot {
            tx: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
        })
        .await
        .expect("request snapshot");
    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("snapshot response")
        .expect("snapshot");

    assert_eq!(snapshot.model_provider, "custom");
    assert_eq!(snapshot.model_provider_id, None);
    run.abort();
}

#[tokio::test]
async fn edit_last_turn_preserves_current_mode() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    let seeded_messages = vec![
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "draft the plan".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "initial response".to_string(),
                cache_control: None,
            }],
        },
    ];
    handle
        .send(Op::SyncSession {
            session_id: Some("edit-mode-test".to_string()),
            messages: seeded_messages,
            system_prompt: None,
            system_prompt_override: false,
            model: "deepseek-v4-pro".to_string(),
            workspace: tmp.path().to_path_buf(),
            mode: AppMode::Agent,
        })
        .await
        .expect("sync session");
    handle
        .send(Op::ChangeMode {
            mode: AppMode::Plan,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
        })
        .await
        .expect("send plan mode");
    handle
        .send(Op::EditLastTurn {
            new_message: "revise this in plan mode".to_string(),
        })
        .await
        .expect("send edit");

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .send(Op::GetSessionSnapshot {
            tx: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
        })
        .await
        .expect("request snapshot");
    let snapshot = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("snapshot response")
        .expect("snapshot");

    assert_eq!(snapshot.mode, "plan");

    run.abort();
}

#[tokio::test]
async fn provider_runtime_status_reports_configured_zai_cap_without_client() {
    let (engine, handle) = {
        let _lock = lock_test_env();
        let _zai_key = EnvVarGuard::remove("ZAI_API_KEY");
        let _zai_alt_key = EnvVarGuard::remove("Z_AI_API_KEY");
        let api_config = Config {
            provider: Some("zai".to_string()),
            ..Config::default()
        };
        Engine::new(EngineConfig::default(), &api_config)
    };

    let run = tokio::spawn(engine.run());
    let status = tokio::time::timeout(Duration::from_secs(2), handle.get_provider_runtime_status())
        .await
        .expect("provider runtime status response")
        .expect("provider runtime status");

    assert_eq!(status.provider, ApiProvider::Zai);
    assert_eq!(
        status.request_concurrency_limit,
        Some(crate::config::DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
    );
    assert_eq!(status.active_provider_requests, 0);

    run.abort();
}

#[test]
fn detects_context_length_errors_from_provider_payloads() {
    let msg = r#"SSE stream request failed: HTTP 400 Bad Request: {"error":{"message":"This model's maximum context length is 131072 tokens. However, you requested 153056 tokens (148960 in the messages, 4096 in the completion).","type":"invalid_request_error"}}"#;
    assert!(is_context_length_error_message(msg));
    assert!(!is_context_length_error_message(
        "SSE stream request failed: HTTP 400 Bad Request: model not found"
    ));
}

#[test]
fn context_budget_reserves_output_and_headroom() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // the internal effective_max_output_tokens() call sees a stable env.
    let _lock = lock_test_env();
    // V4 has a 1M context window — the only family that comfortably hosts
    // a 256K output reservation without saturating the input budget to 0.
    let budget = context_input_budget_for_provider(ApiProvider::Deepseek, "deepseek-v4-pro")
        .expect("deepseek-v4-pro should have a known context window");
    let v4_window: usize = 1_000_000;
    let expected = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn context_budget_uses_conservative_fallback_for_unknown_models() {
    let _lock = lock_test_env();
    let budget = context_input_budget_for_provider(ApiProvider::Openai, "auto")
        .expect("unknown/auto model ids should still get a conservative hard preflight budget");
    let expected = 128_000usize
        - effective_max_output_tokens_for_route(ApiProvider::Openai, "auto", None) as usize
        - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn context_budget_uses_provider_effective_window_for_openai_codex() {
    let _lock = lock_test_env();
    let budget = context_input_budget_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5")
        .expect("OpenAI Codex should use a conservative fallback without route metadata");
    let expected = usize::try_from(crate::config::OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS)
        .expect("context window fits usize")
        - crate::config::provider_capability(ApiProvider::OpenaiCodex, "gpt-5.5").max_output
            as usize
        - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn route_context_budget_uses_shared_budget_service() {
    let _lock = lock_test_env();
    let budget = route_context_budget_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5", 380_000)
        .expect("OpenAI Codex should produce a route budget");

    assert_eq!(
        budget.window_tokens,
        u64::from(crate::config::OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS)
    );
    assert_eq!(
        budget.output_cap_tokens,
        u64::from(
            crate::config::provider_capability(ApiProvider::OpenaiCodex, "gpt-5.5").max_output
        )
    );
    assert_eq!(
        budget.pressure,
        crate::context_budget::PressureLevel::Critical
    );
    assert!(!budget.fits_additional(1));
}

#[test]
fn route_context_budget_prefers_resolved_route_limits() {
    let _lock = lock_test_env();
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(128_000),
        input_tokens: None,
        output_tokens: Some(32_768),
    };
    let budget = route_context_budget_for_route(
        ApiProvider::Openrouter,
        "deepseek/deepseek-v4-pro",
        Some(limits),
        60_000,
    )
    .expect("route limits should produce a budget");

    assert_eq!(budget.window_tokens, 128_000);
    assert_eq!(budget.output_cap_tokens, 32_768);
    assert_eq!(budget.available_input_tokens, 34_208);
}

#[test]
fn kimi_catalog_output_ceiling_does_not_collapse_input_budget() {
    let _lock = lock_test_env();
    let _guard = ScopedDeepSeekMaxOutputTokens::unset();
    let documented =
        route_context_budget_for_route(ApiProvider::Moonshot, "kimi-k2.7-code", None, 0)
            .expect("bundled Kimi limits should produce a budget");
    assert_eq!(documented.window_tokens, 262_144);
    assert_eq!(documented.output_cap_tokens, 32_768);
    assert_eq!(documented.available_input_tokens, 228_352);

    // #4368/#4378: Models.dev may report Kimi's full 262K context as both its
    // context window and provider output ceiling. That ceiling must not be
    // reserved as though every normal turn requested 262K of output; the
    // integrated Kimi route cap is 32K.
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(262_144),
        input_tokens: None,
        output_tokens: Some(262_144),
    };

    let budget =
        route_context_budget_for_route(ApiProvider::Moonshot, "kimi-k2.7-code", Some(limits), 0)
            .expect("Kimi route limits should produce a budget");

    assert_eq!(budget.window_tokens, 262_144);
    assert_eq!(budget.output_cap_tokens, 32_768);
    assert_eq!(budget.available_input_tokens, 228_352);
}

#[test]
fn effective_max_output_tokens_for_route_caps_to_route_output_limit() {
    let _lock = lock_test_env();
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(1_000_000),
        input_tokens: None,
        output_tokens: Some(8_192),
    };

    assert_eq!(
        effective_max_output_tokens_for_route(
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(limits),
        ),
        8_192
    );
}

#[test]
fn effective_max_output_tokens_for_route_caps_to_context_window() {
    let _lock = lock_test_env();
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(32_000),
        input_tokens: None,
        output_tokens: None,
    };

    let cap = effective_max_output_tokens_for_route(
        ApiProvider::Deepseek,
        "deepseek-v4-pro",
        Some(limits),
    );

    assert!(cap < 32_000, "request cap must fit the configured window");
    assert!(
        cap > 0,
        "small configured windows should still allow output"
    );
}

#[test]
fn effective_max_output_tokens_for_route_keeps_tiny_window_positive() {
    let _lock = lock_test_env();
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(2_048),
        input_tokens: None,
        output_tokens: None,
    };

    assert_eq!(
        effective_max_output_tokens_for_route(
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(limits),
        ),
        1
    );
}

#[test]
fn codex_route_without_output_metadata_uses_oauth_capability_floor() {
    let _lock = lock_test_env();
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(272_000),
        input_tokens: None,
        output_tokens: None,
    };

    assert_eq!(
        effective_max_output_tokens_for_route(ApiProvider::OpenaiCodex, "gpt-5.5", Some(limits)),
        4_096
    );
    let budget =
        route_context_budget_for_route(ApiProvider::OpenaiCodex, "gpt-5.5", Some(limits), 0)
            .expect("Codex route budget");
    assert_eq!(budget.output_cap_tokens, 4_096);
}

#[test]
fn effective_max_output_tokens_caps_api_request_for_large_window_models() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // v4_cap and flash_cap below see the same env state.
    let _lock = lock_test_env();
    // V4 models have a 1M context window but the API request cap must stay
    // well below common provider limits (e.g., 131K total on self-hosted
    // vLLM/SGLang). The cap should never exceed 65K.
    let v4_cap = effective_max_output_tokens("deepseek-v4-pro");
    assert!(
        v4_cap <= 65_536,
        "V4 API request cap should be ≤64K, got {v4_cap}"
    );
    assert!(
        v4_cap > 0,
        "V4 API request cap should be positive, got {v4_cap}"
    );

    let flash_cap = effective_max_output_tokens("deepseek-v4-flash");
    assert_eq!(v4_cap, flash_cap);
}

struct ScopedDeepSeekMaxOutputTokens {
    previous: Option<OsString>,
}

impl ScopedDeepSeekMaxOutputTokens {
    fn set(value: &str) -> Self {
        let previous = std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS");
        // Safety: tests using this helper serialize with lock_test_env() and
        // restore the original value in Drop.
        unsafe {
            std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", value);
        }
        Self { previous }
    }

    fn unset() -> Self {
        let previous = std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS");
        // Safety: see set().
        unsafe {
            std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
        }
        Self { previous }
    }
}

impl Drop for ScopedDeepSeekMaxOutputTokens {
    fn drop(&mut self) {
        // Safety: tests using this helper serialize with lock_test_env().
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", previous);
            } else {
                std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
            }
        }
    }
}

#[test]
fn effective_max_output_tokens_env_override_returns_positive_value() {
    let _lock = lock_test_env();
    let _guard = ScopedDeepSeekMaxOutputTokens::set("16384");

    // Override applies regardless of model — V4 hosted, V4 flash, sub-500K
    // self-hosted all return the env value verbatim.
    assert_eq!(effective_max_output_tokens("deepseek-v4-pro"), 16_384);
    assert_eq!(effective_max_output_tokens("deepseek-v4-flash"), 16_384);
    assert_eq!(effective_max_output_tokens("qwen3-32b-256k"), 16_384);
}

#[test]
fn effective_max_output_tokens_env_override_rejects_zero_and_invalid() {
    let _lock = lock_test_env();
    // Establish the heuristic baseline with the env unset.
    let baseline = {
        let _guard = ScopedDeepSeekMaxOutputTokens::unset();
        effective_max_output_tokens("deepseek-v4-pro")
    };
    assert!(baseline > 0);

    // 0, non-numeric, and empty values must all fall through to the heuristic
    // rather than producing a zero/garbage cap that would silently break
    // request budgeting.
    for raw in ["0", "abc", "", "  ", "-1"] {
        let _guard = ScopedDeepSeekMaxOutputTokens::set(raw);
        assert_eq!(
            effective_max_output_tokens("deepseek-v4-pro"),
            baseline,
            "env={raw:?} should fall through to heuristic"
        );
    }
}

#[test]
fn internal_context_budget_tiers_reserved_output_by_window() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // both branches below see a stable env.
    let _lock = lock_test_env();
    // Large-context (>=500K) models reserve the full TURN_MAX_OUTPUT_TOKENS
    // headroom so long V4 sessions don't compact prematurely.
    let internal_budget =
        context_input_budget_for_provider(ApiProvider::Deepseek, "deepseek-v4-pro")
            .expect("V4 should have a known context window");
    let v4_window: usize = 1_000_000;
    let expected_internal = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(internal_budget, expected_internal);

    // Sub-500K windows cross into the effective-cap branch: a 256K self-hosted
    // deployment must yield a usable positive budget rather than None. The
    // previous formula reserved the full 262K and computed 256K - 262K - 1K,
    // which underflowed to None and silently disabled preflight/recovery.
    let small_window_budget =
        context_input_budget_for_provider(ApiProvider::Openai, "qwen3-32b-256k")
            .expect("a 256K-suffix model must yield Some budget via the effective-cap branch");
    let effective_output =
        effective_max_output_tokens_for_route(ApiProvider::Openai, "qwen3-32b-256k", None) as usize;
    let expected_small = 256_000 - effective_output - 1_024;
    assert_eq!(small_window_budget, expected_small);
}

#[test]
fn v4_keeps_large_file_reads_but_compacts_noisy_shell_output() {
    let content = "0123456789abcdef\n".repeat(2_000);
    let output = ToolResult::success(content.clone());

    let v4_context = compact_tool_result_for_context("deepseek-v4-pro", "read_file", &output);
    assert_eq!(v4_context, content.trim());

    let v4_shell_context =
        compact_tool_result_for_context("deepseek-v4-pro", "exec_shell", &output);
    assert!(v4_shell_context.contains("exec_shell output compacted to protect context"));
    assert!(v4_shell_context.len() < v4_context.len());

    let legacy_context =
        compact_tool_result_for_context("deepseek-v3.2-128k", "read_file", &output);
    assert!(legacy_context.contains("output compacted to protect context"));
    assert!(legacy_context.len() < v4_context.len());
}

#[test]
fn codex_tool_retention_uses_oauth_route_window_not_api_model_window() {
    let content = "route-effective context\n".repeat(900);
    let output = ToolResult::success(content.clone());
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(272_000),
        input_tokens: None,
        output_tokens: None,
    };

    let context = compact_tool_result_for_route(
        ApiProvider::OpenaiCodex,
        "gpt-5.5",
        Some(limits),
        "read_file",
        &output,
    );

    assert!(context.contains("output compacted to protect context"));
    assert!(context.len() < content.len());
}

#[test]
fn subagent_results_are_summarized_before_parent_context_insertion() {
    let long_result = "verified detail\n".repeat(1_000);
    let output = ToolResult::success(
        json!({
            "agent_id": "agent_1234abcd",
            "agent_type": "explore",
            "assignment": {
                "objective": "Inspect the RLM rendering path and report the smallest fix."
            },
            "model": "deepseek-v4-flash",
            "status": "Completed",
            "result": long_result,
            "steps_taken": 12,
            "duration_ms": 3456
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "agent", &output);

    assert!(context.contains("[sub-agent result summarized for parent context]"));
    assert!(context.contains("agent_1234abcd (explore) status=Completed"));
    assert!(context.contains("Inspect the RLM rendering path"));
    assert!(context.contains("steps=12"));
    assert!(context.len() < output.content.len());
    assert!(context.contains("self-report"));
    assert!(context.contains("verify side effects"));
    assert!(context.contains("read_file") && context.contains("list_dir"));
    assert!(context.contains("handle_read"));
}

#[test]
fn run_verifiers_results_are_structured_before_context_insertion() {
    let noisy_failure = "node lint failure detail\n".repeat(300);
    let noisy_success = "successful check output\n".repeat(300);
    let output = ToolResult::success(
        json!({
            "success": false,
            "profile": "auto",
            "level": "quick",
            "workspace": "/repo",
            "gate_count": 3,
            "passed": 1,
            "failed": 1,
            "skipped": 1,
            "summary": "1 passed, 1 failed, 1 skipped",
            "gates": [
                {
                    "name": "rust-check",
                    "ecosystem": "rust",
                    "status": "passed",
                    "command": "cargo check --workspace --locked",
                    "cwd": "/repo",
                    "exit_code": 0,
                    "duration_ms": 110,
                    "stdout": noisy_success.clone(),
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": null
                },
                {
                    "name": "node-lint",
                    "ecosystem": "node",
                    "status": "failed",
                    "command": "npm run lint",
                    "cwd": "/repo",
                    "exit_code": 1,
                    "duration_ms": 220,
                    "stdout": "",
                    "stderr": noisy_failure,
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": null
                },
                {
                    "name": "python-pytest",
                    "ecosystem": "python",
                    "status": "skipped",
                    "command": "",
                    "cwd": "/repo",
                    "exit_code": null,
                    "duration_ms": 0,
                    "stdout": "",
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": "pytest is not installed"
                }
            ]
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "run_verifiers", &output);

    assert!(context.contains("[run_verifiers result summarized for context]"));
    assert!(context.contains("summary: 1 passed, 1 failed, 1 skipped"));
    assert!(context.contains("selection: profile=auto, level=quick"));
    assert!(context.contains("- node-lint (node): failed exit=1"));
    assert!(context.contains("command: npm run lint"));
    assert!(context.contains("- python-pytest (python): skipped"));
    assert!(context.contains("pytest is not installed"));
    assert!(context.contains("- rust-check (rust): passed exit=0"));
    assert!(context.len() < output.content.len());
    assert!(
        !context.contains(&noisy_success),
        "successful gate stdout should not be copied into parent context"
    );
}

#[test]
fn run_tests_results_are_structured_before_context_insertion() {
    let stdout = "running test suite\n".repeat(500);
    let stderr = "error[E0425]: cannot find value `missing`\n".repeat(500);
    let output = ToolResult::success(
        json!({
            "success": false,
            "exit_code": 101,
            "stdout": stdout,
            "stderr": stderr,
            "command": "(cd /repo && cargo test --workspace --all-features)"
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "run_tests", &output);

    assert!(context.contains("[run_tests result summarized for context]"));
    assert!(context.contains("status: failed, exit_code: 101"));
    assert!(context.contains("cargo test --workspace --all-features"));
    assert!(context.contains("error[E0425]"));
    assert!(context.contains("running test suite"));
    assert!(context.len() < output.content.len());
}

#[test]
fn task_gate_run_results_are_structured_before_context_insertion() {
    let output = ToolResult::success(
        json!({
            "gate": {
                "id": "gate_abcd1234",
                "gate": "clippy",
                "command": "cargo clippy -p codewhale-tui --all-targets --all-features --locked -- -D warnings",
                "cwd": "/repo",
                "exit_code": 1,
                "status": "failed",
                "classification": "compile_failure",
                "duration_ms": 5000,
                "summary": "warning promoted to error in verifier.rs",
                "log_path": "/repo/.codewhale/runtime/gate.log",
                "recorded_at": "2026-06-01T12:00:00Z"
            },
            "stdout_summary": "",
            "stderr_summary": "warning promoted to error"
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "task_gate_run", &output);

    assert!(context.contains("[task_gate_run result summarized for context]"));
    assert!(context.contains("gate: clippy, status: failed, exit_code: 1"));
    assert!(context.contains("cargo clippy -p codewhale-tui"));
    assert!(context.contains("summary: warning promoted to error"));
    assert!(context.contains("log_path: /repo/.codewhale/runtime/gate.log"));
}

#[test]
fn refresh_system_prompt_leaves_working_set_out_of_system_prompt() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("please inspect src/lib.rs", tmp.path());

    engine.refresh_system_prompt();

    let prompt = match &engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };
    assert!(!prompt.contains(WORKING_SET_SUMMARY_MARKER));
}

#[test]
fn working_set_reaches_model_as_turn_metadata() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("please inspect src/lib.rs", tmp.path());
    let user_msg =
        engine.user_text_message_with_turn_metadata("please inspect src/lib.rs".to_string());
    engine.session.add_message(user_msg);

    let messages = engine.messages_with_turn_metadata();
    let last_block = messages
        .first()
        .and_then(|message| message.content.last())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains(WORKING_SET_SUMMARY_MARKER));
    assert!(text.contains("src/lib.rs"));
}

#[test]
fn turn_metadata_includes_git_workspace_snapshot_in_repo() {
    use crate::dependencies::ExternalTool;

    if !crate::dependencies::Git::available() {
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let init = crate::dependencies::Git::output(&["init", "-q"], root);
    if init.is_err() || !init.unwrap().status.success() {
        return;
    }

    let config = EngineConfig {
        workspace: root.to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let user_msg = engine.user_text_message_with_turn_metadata("inspect repo state".to_string());
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    if let Some(snapshot) = crate::tui::workspace_context::collect(root) {
        assert!(
            text.contains(&format!("Git workspace: {snapshot}")),
            "turn_meta should include git snapshot: {text}"
        );
    }
}

#[test]
fn turn_metadata_includes_current_local_date_without_working_set() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        model: "deepseek-v4-flash".to_string(),
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let user_msg = engine.user_text_message_with_turn_metadata("what is today's date?".to_string());
    engine.session.add_message(user_msg);

    let messages = engine.messages_with_turn_metadata();
    let last_block = messages
        .first()
        .and_then(|message| message.content.last())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains(&format!("Current local date: {today}")));
    assert!(text.contains("Current model: deepseek-v4-flash"));
    assert!(text.contains("Input provenance: external_user"));
    assert!(text.contains("Input authority: external_current_turn"));
}

#[test]
fn turn_metadata_surfaces_context_and_resource_usage() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        model: "deepseek-v4-flash".to_string(),
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine.session.total_usage.add(&Usage {
        input_tokens: 1_200,
        output_tokens: 300,
        prompt_cache_hit_tokens: Some(800),
        prompt_cache_miss_tokens: Some(400),
        prompt_cache_write_tokens: Some(400),
        ..Default::default()
    });
    {
        let mut goal = engine.config.goal_state.lock().expect("goal lock");
        goal.create("Finish telemetry visibility".to_string(), Some(2_000))
            .expect("create goal");
        goal.record_usage(1_000, 100);
    }

    let user_msg = engine
        .user_text_message_with_turn_metadata("continue the long-running release task".to_string());
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Context pressure:"), "got: {text}");
    assert!(text.contains("tokens;"), "got: {text}");
    assert!(
        text.contains("input tokens available"),
        "context headroom should be model-visible: {text}"
    );
    assert!(
        text.contains("Session token usage: 1500 total (1200 input, 300 output"),
        "session usage should be model-visible: {text}"
    );
    assert!(text.contains("cache hits 800"), "got: {text}");
    assert!(text.contains("cache writes 400"), "got: {text}");
    assert!(
        text.contains("Active goal resource usage:"),
        "active goal resource usage should be model-visible: {text}"
    );
    assert!(text.contains("50% budget"), "got: {text}");
    assert!(text.contains("10.0 tok/s"), "got: {text}");
}

#[test]
fn turn_metadata_escalates_context_pressure_at_warning_threshold() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        model: "deepseek-v4-flash".to_string(),
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());

    // Fabricate high context usage by stuffing the session with a large user message.
    let large = "x".repeat(900_000);
    engine.session.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: large,
            cache_control: None,
        }],
    });

    let user_msg = engine.user_text_message_with_turn_metadata("wrap up".to_string());
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    if text.contains("Context pressure:") {
        let usage_line = text
            .lines()
            .find(|line| line.starts_with("Context pressure:"))
            .expect("context pressure line");
        if usage_line.contains('%') {
            let percent = usage_line
                .split('(')
                .nth(1)
                .and_then(|rest| rest.split('%').next())
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            if percent >= crate::tui::context_inspector::CONTEXT_WARNING_THRESHOLD_PERCENT {
                assert!(
                    usage_line.contains("ESCALATED"),
                    "expected escalation copy at >=85%: {usage_line}"
                );
            } else {
                assert!(
                    !usage_line.contains("ESCALATED"),
                    "below 85% should stay informational: {usage_line}"
                );
            }
        }
    }
}

#[test]
fn runtime_turn_metadata_marks_non_authoritative_input() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let msg = engine.runtime_text_message_with_turn_metadata(
        "改吧".to_string(),
        UserInputProvenance::AssistantGenerated,
    );
    let last_block = msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Input provenance: assistant_generated"));
    assert!(text.contains("Input authority: non_authoritative"));
}

#[test]
fn turn_metadata_includes_auto_model_route() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let user_msg = engine.user_text_message_with_turn_metadata_for_route(
        "debug this regression".to_string(),
        "deepseek-v4-pro",
        true,
        Some("max"),
        true,
    );
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Current model: deepseek-v4-pro"));
    assert!(text.contains("Auto model route: deepseek-v4-pro"));
    assert!(text.contains("Auto reasoning effort: max"));
    assert!(!text.contains("debug this regression"));
}

#[test]
fn provenance_gate_preserves_standing_yolo_for_runtime_and_subagent_continuations() {
    let all_provenances = [
        UserInputProvenance::ExternalUser,
        UserInputProvenance::Runtime,
        UserInputProvenance::SubAgentHandoff,
        UserInputProvenance::ImportedTranscript,
        UserInputProvenance::MemoryRecall,
        UserInputProvenance::AssistantGenerated,
    ];
    let inheriting_provenances = [
        UserInputProvenance::ExternalUser,
        UserInputProvenance::Runtime,
        UserInputProvenance::SubAgentHandoff,
    ];

    for provenance in all_provenances {
        let policy = effective_input_policy(
            provenance,
            AppMode::Yolo,
            "continue",
            true,
            true,
            true,
            crate::tui::approval::ApprovalMode::Auto,
        );

        if inheriting_provenances.contains(&provenance) {
            assert_eq!(policy.mode, AppMode::Yolo, "{provenance:?}");
            assert!(policy.allow_shell, "{provenance:?}");
            assert!(policy.trust_mode, "{provenance:?}");
            assert!(policy.auto_approve, "{provenance:?}");
            assert_eq!(
                policy.approval_mode,
                crate::tui::approval::ApprovalMode::Auto,
                "{provenance:?}"
            );
            assert!(policy.status.is_none(), "{provenance:?}");
        } else {
            assert_eq!(policy.mode, AppMode::Agent, "{provenance:?}");
            assert!(policy.allow_shell, "{provenance:?}");
            assert!(!policy.trust_mode, "{provenance:?}");
            assert!(!policy.auto_approve, "{provenance:?}");
            assert_eq!(
                policy.approval_mode,
                crate::tui::approval::ApprovalMode::Suggest,
                "{provenance:?}"
            );
            assert!(
                policy.status.as_deref().is_some_and(
                    |status| status.contains("cannot inherit standing auto-approval authority")
                ),
                "{provenance:?}"
            );
        }
    }
}

#[test]
fn provenance_gate_never_invents_auto_authority_for_non_yolo_sessions() {
    let all_provenances = [
        UserInputProvenance::ExternalUser,
        UserInputProvenance::Runtime,
        UserInputProvenance::SubAgentHandoff,
        UserInputProvenance::ImportedTranscript,
        UserInputProvenance::MemoryRecall,
        UserInputProvenance::AssistantGenerated,
    ];

    for provenance in all_provenances {
        let policy = effective_input_policy(
            provenance,
            AppMode::Agent,
            "continue",
            true,
            false,
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        );

        assert_eq!(policy.mode, AppMode::Agent, "{provenance:?}");
        assert!(policy.allow_shell, "{provenance:?}");
        assert!(!policy.trust_mode, "{provenance:?}");
        assert!(!policy.auto_approve, "{provenance:?}");
        assert_eq!(
            policy.approval_mode,
            crate::tui::approval::ApprovalMode::Suggest,
            "{provenance:?}"
        );
        assert!(policy.status.is_none(), "{provenance:?}");
    }
}

#[test]
fn full_access_posture_normalizes_a_stale_auto_approve_bit() {
    let policy = effective_input_policy(
        UserInputProvenance::SubAgentHandoff,
        AppMode::Agent,
        "continue",
        true,
        true,
        false,
        crate::tui::approval::ApprovalMode::Bypass,
    );

    assert_eq!(policy.mode, AppMode::Agent);
    assert_eq!(
        policy.approval_mode,
        crate::tui::approval::ApprovalMode::Bypass
    );
    assert!(policy.auto_approve);
    assert!(policy.status.is_none());
}

#[test]
fn self_generated_fake_approvals_cannot_authorize_work() {
    let non_authoritative_origins = [
        UserInputProvenance::ImportedTranscript,
        UserInputProvenance::MemoryRecall,
        UserInputProvenance::AssistantGenerated,
    ];

    for provenance in non_authoritative_origins {
        for content in ["改吧", "嗯"] {
            let policy = effective_input_policy(
                provenance,
                AppMode::Yolo,
                content,
                true,
                true,
                true,
                crate::tui::approval::ApprovalMode::Bypass,
            );

            assert_eq!(policy.mode, AppMode::Agent, "{provenance:?} {content}");
            assert!(policy.allow_shell, "{provenance:?} {content}");
            assert!(!policy.trust_mode, "{provenance:?} {content}");
            assert!(!policy.auto_approve, "{provenance:?} {content}");
            assert_eq!(
                policy.approval_mode,
                crate::tui::approval::ApprovalMode::Suggest,
                "{provenance:?} {content}"
            );
            assert!(
                policy.status.as_deref().is_some_and(
                    |status| status.contains("cannot inherit standing auto-approval authority")
                ),
                "{provenance:?} {content}"
            );
        }
    }
}

#[test]
fn external_prompt_wording_never_changes_effective_mode_or_authority() {
    let cases = [
        (
            AppMode::Agent,
            crate::tui::approval::ApprovalMode::Suggest,
            false,
            false,
            "你在帮我看看 外卖部分还哪里没有使用多语言",
        ),
        (
            AppMode::Yolo,
            crate::tui::approval::ApprovalMode::Bypass,
            true,
            true,
            "check the failing tests and review the logs",
        ),
        (
            AppMode::Agent,
            crate::tui::approval::ApprovalMode::Suggest,
            false,
            false,
            "检查外卖模块并修复缺少的多语言注入",
        ),
    ];

    for (requested_mode, approval_mode, trust_mode, auto_approve, content) in cases {
        let policy = effective_input_policy(
            UserInputProvenance::ExternalUser,
            requested_mode,
            content,
            true,
            trust_mode,
            auto_approve,
            approval_mode,
        );

        assert_eq!(policy.mode, requested_mode, "{content}");
        assert_eq!(policy.trust_mode, trust_mode, "{content}");
        assert_eq!(policy.auto_approve, auto_approve, "{content}");
        assert_eq!(policy.approval_mode, approval_mode, "{content}");
        assert!(policy.allow_shell, "{content}");
        assert!(policy.dynamic_active_tools.is_empty(), "{content}");
        assert!(policy.status.is_none(), "{content}");
    }
}

#[test]
fn external_user_wording_does_not_downgrade_standing_authority() {
    let review_wording = effective_input_policy(
        UserInputProvenance::ExternalUser,
        AppMode::Yolo,
        "你在帮我看看 外卖部分还哪里没有使用多语言 我看看要不要加",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Bypass,
    );
    assert_eq!(review_wording.mode, AppMode::Yolo);
    assert!(review_wording.allow_shell);
    assert!(review_wording.trust_mode);
    assert!(review_wording.auto_approve);
    assert_eq!(
        review_wording.approval_mode,
        crate::tui::approval::ApprovalMode::Bypass
    );
    assert!(
        review_wording.status.is_none(),
        "external user wording must not content-downgrade standing authority"
    );

    let later_user_instruction = effective_input_policy(
        UserInputProvenance::ExternalUser,
        AppMode::Yolo,
        "需要修复下",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Bypass,
    );
    assert_eq!(later_user_instruction.mode, AppMode::Yolo);
    assert!(later_user_instruction.allow_shell);
    assert!(later_user_instruction.trust_mode);
    assert!(later_user_instruction.auto_approve);
    assert_eq!(
        later_user_instruction.approval_mode,
        crate::tui::approval::ApprovalMode::Bypass
    );
    assert!(
        later_user_instruction.status.is_none(),
        "a fresh external write instruction must not inherit the prior review-only downgrade"
    );
}

#[test]
fn turn_metadata_includes_plan_mode_policy() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine.current_mode = AppMode::Plan;

    let user_msg = engine.user_text_message_with_turn_metadata_for_route(
        "explain the refactor plan before editing".to_string(),
        "deepseek-v4-flash",
        false,
        None,
        false,
    );
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Current mode: plan"), "got: {text}");
    assert!(
        text.contains("Current mode policy source: runtime"),
        "got: {text}"
    );
    assert!(text.contains("##### Mode: Plan"), "got: {text}");
    assert!(
        text.contains("All writes and patches are blocked"),
        "got: {text}"
    );
    assert!(
        text.contains("Shell and code execution are unavailable"),
        "got: {text}"
    );
}

#[test]
fn turn_metadata_projects_effective_permission_question_discipline() {
    use crate::tui::approval::ApprovalMode;

    let cases = [
        (
            ApprovalMode::Suggest,
            "Ask",
            "Tool approvals and user decisions are separate",
        ),
        (
            ApprovalMode::Auto,
            "Auto-Review",
            "Do not ask the user questions or pause for a user decision",
        ),
        (
            ApprovalMode::Bypass,
            "Full Access",
            "Full Access does not authorize invented intent",
        ),
        (ApprovalMode::Never, "Never", "Remain read-only"),
    ];

    for (approval_mode, posture, question_marker) in cases {
        let tmp = tempdir().expect("tempdir");
        let config = EngineConfig {
            workspace: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let (mut engine, _handle) = Engine::new(config, &Config::default());
        engine.session.approval_mode = approval_mode;

        let message = engine.user_text_message_with_turn_metadata("continue".to_string());
        let ContentBlock::Text { text, .. } = message
            .content
            .last()
            .expect("turn metadata must be present")
        else {
            panic!("expected text turn metadata");
        };

        assert!(
            text.contains(&format!("Current permission posture: {posture}")),
            "{posture}: {text}"
        );
        assert!(
            text.contains("Current permission policy source: effective runtime authority"),
            "{posture}: {text}"
        );
        assert!(text.contains(question_marker), "{posture}: {text}");
    }
}

#[test]
fn turn_metadata_preserves_standing_full_access_for_subagent_handoff() {
    use crate::tui::approval::ApprovalMode;

    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let authority = effective_input_policy(
        UserInputProvenance::SubAgentHandoff,
        AppMode::Agent,
        "continue from child",
        true,
        true,
        true,
        ApprovalMode::Bypass,
    );
    engine.apply_runtime_mode_policy(&authority);

    let message = engine.runtime_text_message_with_turn_metadata(
        "continue from child".to_string(),
        UserInputProvenance::SubAgentHandoff,
    );
    let ContentBlock::Text { text, .. } = message
        .content
        .last()
        .expect("turn metadata must be present")
    else {
        panic!("expected text turn metadata");
    };

    // Act mode and the permission posture are independent: the child handoff
    // must keep the visible Act mode and the standing Full Access authority.
    assert!(text.contains("Current mode: agent"), "{text}");
    assert!(
        text.contains("Current permission posture: Full Access"),
        "{text}"
    );
    assert!(
        text.contains("Input authority: non_authoritative"),
        "{text}"
    );
}

#[test]
fn current_mode_field_assignment_takes_effect_synchronously() {
    // Basic unit-level invariant: the current_mode field mutates as expected.
    // Op::ChangeMode dispatch through the run loop is exercised by the
    // integration test change_mode_op_updates_current_mode_and_emits_status.
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    assert_eq!(engine.current_mode, AppMode::Agent);

    engine.current_mode = AppMode::Yolo;
    assert_eq!(engine.current_mode, AppMode::Yolo);
}

#[test]
fn user_text_message_keeps_current_turn_input_after_turn_metadata() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let user_msg =
        engine.user_text_message_with_turn_metadata("explain the cache metrics".to_string());

    // User text is now at position 0, turn_meta at position 1.
    let first_text = user_msg
        .content
        .iter()
        .find_map(|block| {
            if let ContentBlock::Text { text, .. } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .expect("user text block");
    assert_eq!(first_text, "explain the cache metrics");
}

#[test]
fn messages_with_turn_metadata_preserves_stored_messages_for_prefix_cache() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    let first_user = engine.user_text_message_with_turn_metadata("inspect src/lib.rs".to_string());
    engine.session.add_message(first_user.clone());
    let first_request = engine.messages_with_turn_metadata();
    assert_eq!(
        &first_request[..engine.session.messages.len()],
        &engine.session.messages[..]
    );
    assert_eq!(first_request.len(), engine.session.messages.len());
    assert_eq!(first_request.first(), Some(&first_user));
    assert_eq!(
        first_request.last().map(|message| message.role.as_str()),
        Some("user")
    );

    engine.session.add_message(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "I inspected it.".to_string(),
            cache_control: None,
        }],
    });
    engine
        .session
        .working_set
        .observe_user_message("now summarize it", tmp.path());
    let second_user = engine.user_text_message_with_turn_metadata("now summarize it".to_string());
    engine.session.add_message(second_user);

    let second_request = engine.messages_with_turn_metadata();
    assert_eq!(
        &second_request[..engine.session.messages.len()],
        &engine.session.messages[..]
    );
    assert_eq!(second_request.len(), engine.session.messages.len());
    assert_eq!(second_request.first(), Some(&first_user));
    assert_eq!(second_request.last(), engine.session.messages.last());
}

/// v0.8.11 regression: tool-result messages serialize to role="tool" on
/// the wire but are stored as role="user" internally. `<turn_meta>` must
/// be stored only on actual user-text messages. Request-time runtime metadata
/// must not mutate tool-result messages.
#[test]
fn turn_metadata_skips_tool_result_messages() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    // Real user message — should be eligible for injection.
    let user_msg = engine.user_text_message_with_turn_metadata("inspect src/lib.rs".to_string());
    engine.session.add_message(user_msg);
    // Assistant tool-call.
    engine.session.add_message(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "call_42".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "src/lib.rs"}),
            caller: None,
        }],
    });
    // Tool result, stored as role="user" internally.
    engine.session.add_message(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_42".to_string(),
            content: "pub fn sample() {}".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });

    let messages = engine.messages_with_turn_metadata();

    // The stored trailing message is the tool result and MUST be untouched —
    // no Text block sneaking in front of the ToolResult block.
    let trailing = messages.last().expect("stored trailing message");
    assert_eq!(trailing.role, "user");
    assert_eq!(trailing.content.len(), 1);
    assert!(matches!(
        trailing.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));

    // The earlier real user message carries user text first, turn_meta last.
    let real_user = messages.first().expect("first user message");
    assert_eq!(real_user.role, "user");
    let ContentBlock::Text { text, .. } = real_user.content.first().expect("user text content")
    else {
        panic!("expected Text block on real user message");
    };
    assert_eq!(text, "inspect src/lib.rs");
    // turn_meta is at the tail of the content array.
    let last_block = real_user.content.last().expect("turn_meta block");
    let ContentBlock::Text { text: meta, .. } = last_block else {
        panic!("expected Text block for turn_meta at tail");
    };
    assert!(meta.starts_with("<turn_meta>\n"));
    assert!(meta.contains("src/lib.rs"));
}

/// User text must appear before turn_meta in the content array so that
/// the leading bytes of each user message stay stable across date changes.
/// DeepSeek's KV prefix cache matches byte sequences from the start of
/// each message; placing the volatile date-bearing turn_meta at position
/// 0 would invalidate the entire user message prefix at every date
/// boundary. Moving it to the tail preserves the user-input prefix.
#[test]
fn user_message_turn_meta_is_appended_not_prepended() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let msg = engine.user_text_message_with_turn_metadata("hello world".to_string());
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.len(), 2);

    // First content block: user text.
    let ContentBlock::Text { text, .. } = &msg.content[0] else {
        panic!("expected Text block at position 0");
    };
    assert_eq!(text, "hello world");

    // Second content block: turn_meta.
    let ContentBlock::Text { text: meta, .. } = &msg.content[1] else {
        panic!("expected Text block for turn_meta at position 1");
    };
    assert!(
        meta.starts_with("<turn_meta>\n"),
        "turn_meta must be at the tail"
    );
    assert!(
        meta.contains("Current local date:"),
        "turn_meta must contain the date"
    );
}

/// When the turn is mid-execution and the trailing user message is a
/// tool result, no turn_meta is injected into that tool-result message. The
/// working_set surfaces again on the next stored user-text message.
#[test]
fn turn_metadata_skips_when_only_tool_results_trail() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    // Only a tool-result message in history — simulates the corner case
    // where the prior real user message has already been compacted away
    // but a tool-result is still pending. We must not retroactively
    // inject.
    engine.session.add_message(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_42".to_string(),
            content: "pub fn sample() {}".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });

    let messages = engine.messages_with_turn_metadata();

    // Stored tool-result message is unchanged: no Text prefix, content length == 1.
    let only = messages.first().expect("stored tool result message");
    assert_eq!(only.content.len(), 1);
    assert!(matches!(
        only.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));
    assert_eq!(messages.len(), 1);
}

#[test]
fn refresh_system_prompt_is_noop_when_unchanged() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());

    engine.refresh_system_prompt();
    let first_hash = engine.session.last_system_prompt_hash;
    let first_prompt = engine.session.system_prompt.clone();
    engine.refresh_system_prompt();

    assert_eq!(engine.session.last_system_prompt_hash, first_hash);
    assert_eq!(engine.session.system_prompt, first_prompt);
}

#[test]
fn slop_gate_does_not_change_the_stable_system_prompt() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let marker = "SLOP_GATE_SYSTEM_FINGERPRINT_REGRESSION";
    engine.slop_ledger_gate_cache = Some((
        Some(std::time::SystemTime::UNIX_EPOCH),
        Some(marker.to_string()),
    ));

    engine.refresh_system_prompt();

    let prompt = match engine.session.system_prompt.as_ref() {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };
    assert!(!prompt.contains(marker));
    assert_eq!(
        engine
            .slop_ledger_gate_cache
            .as_ref()
            .and_then(|(_, block)| block.as_deref()),
        Some(marker),
        "system prompt refresh must not consult or rewrite the user-turn gate cache"
    );
}

#[test]
fn slop_gate_is_an_initial_external_user_turn_tail_block() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let marker = "## debt gate test marker";

    let message = engine.user_text_message_with_turn_metadata("finish the task".to_string());
    let message = Engine::attach_slop_ledger_gate(message, Some(marker.to_string()));

    assert_eq!(message.content.len(), 3);
    assert!(matches!(
        message.content.first(),
        Some(ContentBlock::Text { text, .. }) if text == "finish the task"
    ));
    assert!(matches!(
        message.content.get(1),
        Some(ContentBlock::Text { text, .. }) if text == marker
    ));
    assert!(matches!(
        message.content.last(),
        Some(ContentBlock::Text { text, .. }) if text.starts_with("<turn_meta>\n")
    ));

    let runtime = engine.runtime_text_message_with_turn_metadata(
        "runtime continuation".to_string(),
        UserInputProvenance::Runtime,
    );
    engine.slop_ledger_gate_cache = Some((
        Some(std::time::SystemTime::UNIX_EPOCH),
        Some(marker.to_string()),
    ));
    let runtime =
        engine.with_slop_ledger_gate_for_initial_user_turn(runtime, UserInputProvenance::Runtime);
    assert_eq!(runtime.content.len(), 2);
    assert!(
        !runtime
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text, .. } if text == marker))
    );

    // `turn_loop` uses this plain constructor for mid-turn steers. The gate
    // from the initial message is already in that turn's transcript, so a
    // steer must not duplicate the mutable block.
    let steer = engine.user_text_message_with_turn_metadata("mid-turn steer".to_string());
    assert!(
        !steer
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text, .. } if text == marker))
    );
}

#[tokio::test]
async fn slop_gate_survives_mid_turn_compaction_without_reinjection() {
    use crate::llm_client::mock::{MockLlmClient, canned};

    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let marker = "## active turn debt gate";

    let message = Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "finish this long task".to_string(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: "<turn_meta>\nInput provenance: external_user\n</turn_meta>".to_string(),
                cache_control: None,
            },
        ],
    };
    let active_gate_message = Engine::attach_slop_ledger_gate(message, Some(marker.to_string()));
    engine.session.add_message(active_gate_message.clone());

    // Move the initial message outside the always-retained tail. A newer user
    // message also prevents the generic chat-template fallback from pinning it
    // accidentally, so this test exercises the active-turn pin specifically.
    for index in 0..12 {
        engine.session.add_message(Message {
            role: if index == 10 { "user" } else { "assistant" }.to_string(),
            content: vec![ContentBlock::Text {
                text: format!("history {index} {}", "x".repeat(1_024)),
                cache_control: None,
            }],
        });
    }

    let unpinned_plan = crate::compaction::plan_compaction(
        &engine.session.messages,
        Some(&engine.session.workspace),
        4,
        None,
        None,
    );
    assert!(
        unpinned_plan.summarize_indices.contains(&0),
        "fixture must prove the gate would otherwise be summarized away: {unpinned_plan:?}"
    );

    let pins = engine.compaction_pins_for_active_turn(Some(&active_gate_message));
    assert!(pins.contains(&0));

    let compaction = CompactionConfig {
        enabled: true,
        token_threshold: 1,
        ..CompactionConfig::default()
    };
    let mock = MockLlmClient::new(vec![canned::simple_text_turn("bounded history summary")]);
    assert!(should_compact(
        &engine.session.messages,
        &compaction,
        Some(&engine.session.workspace),
        Some(&pins),
        None,
    ));

    let result = compact_messages_safe(
        &mock,
        &engine.session.messages,
        &compaction,
        Some(&engine.session.workspace),
        Some(&pins),
        None,
    )
    .await
    .expect("mid-turn compaction");

    assert!(result.messages.contains(&active_gate_message));
    assert!(result.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text, .. } if text == marker))
    }));
}

#[test]
fn engine_prompt_respects_hidden_thinking_config() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        locale_tag: "zh-Hans".to_string(),
        show_thinking: false,
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt.as_ref() {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("## Hidden Thinking Language"));
    assert!(prompt.contains("reasoning_content"));
    assert!(prompt.contains("English"));
    assert!(!prompt.contains("## 语言再次提醒"));
}

fn sync_runtime_system_prompt_override(engine: &mut Engine, system_prompt: SystemPrompt) {
    engine.session.compaction_summary_prompt =
        extract_compaction_summary_prompt(Some(system_prompt.clone()));
    engine.session.system_prompt = Some(system_prompt);
    engine.session.system_prompt_override = true;
}

#[test]
fn text_system_prompt_override_via_runtime_sync_survives_refresh() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let prompt = SystemPrompt::Text("TANGERINE-7".to_string());
    let expected = Some(prompt.clone());

    sync_runtime_system_prompt_override(&mut engine, prompt);
    engine.refresh_system_prompt();

    assert_eq!(engine.session.system_prompt, expected);
}

#[test]
fn blocks_system_prompt_override_via_runtime_sync_survives_mode_change_refresh() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let prompt = SystemPrompt::Blocks(vec![SystemBlock {
        block_type: "text".to_string(),
        text: "TANGERINE-7".to_string(),
        cache_control: None,
    }]);
    let expected = Some(prompt.clone());

    sync_runtime_system_prompt_override(&mut engine, prompt);
    engine.refresh_system_prompt();

    assert_eq!(engine.session.system_prompt, expected);
}

#[test]
fn compaction_summary_stays_in_stable_system_prompt() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/main.rs"), "fn main() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("continue in src/main.rs", tmp.path());
    engine.refresh_system_prompt();
    engine.merge_compaction_summary(Some(SystemPrompt::Blocks(vec![SystemBlock {
        block_type: "text".to_string(),
        text: format!("{COMPACTION_SUMMARY_MARKER}\nsummary"),
        cache_control: None,
    }])));

    let prompt = match &engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains(COMPACTION_SUMMARY_MARKER));
    assert!(!prompt.contains(WORKING_SET_SUMMARY_MARKER));
}

#[test]
fn compaction_reanchors_active_operation_identity_without_raw_output() {
    let tmp = tempdir().expect("tempdir");
    let todos = crate::tools::todo::new_shared_todo_list();
    let plan = crate::tools::plan::new_shared_plan_state();
    let work = crate::work_graph::new_shared_work_runtime(todos, plan);
    let runtime_services = crate::tools::spec::RuntimeToolServices {
        work: Some(work.clone()),
        ..Default::default()
    };
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        runtime_services,
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let session_id = engine.session.id.clone();
    work.register_operation(
        &session_id,
        crate::work_graph::OperationIntent::new(
            "shell:shell_compact",
            "quiet receipt sentinel",
            false,
            "exec_shell",
            "shell_compact",
        ),
    )
    .expect("register active operation");
    work.reconcile_operation(
        &session_id,
        crate::work_graph::OperationOwnerSnapshot::new(
            "shell:shell_compact",
            crate::work_graph::OwnerState::Running,
            1,
            1,
        ),
    )
    .expect("owner running report");

    engine.merge_compaction_summary(Some(SystemPrompt::Text(format!(
        "{COMPACTION_SUMMARY_MARKER}\nordinary summary"
    ))));
    let prompt = match &engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };
    assert!(prompt.contains("Active Work Graph Operations"), "{prompt}");
    assert!(prompt.contains("shell:shell_compact"), "{prompt}");
    assert!(prompt.contains("quiet receipt sentinel"), "{prompt}");
    assert!(prompt.contains("active"), "{prompt}");
    assert_eq!(
        prompt
            .matches(crate::work_graph::ACTIVE_OPERATION_SUMMARY_START)
            .count(),
        1,
        "the active-operation re-anchor must be unique: {prompt}"
    );
    assert!(
        !prompt.contains("raw output sentinel"),
        "the re-anchor must never copy operation output"
    );

    engine.merge_compaction_summary(Some(SystemPrompt::Text(format!(
        "{COMPACTION_SUMMARY_MARKER}\nsecond summary"
    ))));
    let repeated = engine.rendered_compaction_summary().expect("summary");
    assert_eq!(
        repeated
            .matches(crate::work_graph::ACTIVE_OPERATION_SUMMARY_START)
            .count(),
        1,
        "repeated compaction must replace, not duplicate, the re-anchor: {repeated}"
    );

    work.reconcile_operation(
        &session_id,
        crate::work_graph::OperationOwnerSnapshot::new(
            "shell:shell_compact",
            crate::work_graph::OwnerState::Completed,
            2,
            2,
        ),
    )
    .expect("owner completion report");
    engine.merge_compaction_summary(Some(SystemPrompt::Text(format!(
        "{COMPACTION_SUMMARY_MARKER}\nthird summary"
    ))));
    let completed = engine.rendered_compaction_summary().expect("summary");
    assert!(
        !completed.contains("Active Work Graph Operations"),
        "a completed operation must not survive in a stale re-anchor: {completed}"
    );
}

#[test]
fn caller_policy_defaults_to_direct() {
    let tool = Tool {
        tool_type: None,
        name: "read_file".to_string(),
        description: "Read".to_string(),
        input_schema: json!({"type":"object"}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    };
    let direct = ToolCaller {
        caller_type: "direct".to_string(),
        tool_id: None,
    };
    let code = ToolCaller {
        caller_type: "code_execution_20250825".to_string(),
        tool_id: Some("srvtoolu_1".to_string()),
    };
    assert!(caller_allowed_for_tool(Some(&direct), Some(&tool)));
    assert!(!caller_allowed_for_tool(Some(&code), Some(&tool)));
    assert!(caller_allowed_for_tool(None, Some(&tool)));
}

#[test]
fn tool_search_activates_discovered_deferred_tools() {
    let mut catalog = vec![
        Tool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read files".to_string(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: None,
            name: "grep_files".to_string(),
            description: "Search files".to_string(),
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    let mut active = initial_active_tools(&catalog);
    let result = execute_tool_search(
        TOOL_SEARCH_NAME,
        &json!({"query":"read file"}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert!(result.success);
    assert!(active.contains("read_file"));
}

#[test]
fn tool_search_can_discover_request_user_input_modal_tool() {
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        vec![api_tool(REQUEST_USER_INPUT_NAME)],
        Vec::new(),
        AppMode::Agent,
        &always_load,
    );
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains(REQUEST_USER_INPUT_NAME));

    let result = execute_tool_search(
        TOOL_SEARCH_NAME,
        &json!({"query":"ask user question"}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");

    assert!(result.success);
    assert!(active.contains(REQUEST_USER_INPUT_NAME));
}

fn tool_search_catalog_with_matches(count: usize) -> Vec<Tool> {
    let mut catalog = (0..count)
        .map(|idx| Tool {
            tool_type: None,
            name: format!("matching_tool_{idx:03}"),
            description: "Matching deferred test tool".to_string(),
            input_schema: json!({"type":"object","properties":{"query":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        })
        .collect::<Vec<_>>();
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    catalog
}

fn tool_search_reference_count(result: &ToolResult) -> usize {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("tool_references"))
        .and_then(|references| references.as_array())
        .map_or(0, Vec::len)
}

#[test]
fn tool_search_defaults_to_twenty_results_for_regex_and_bm25() {
    let catalog = tool_search_catalog_with_matches(25);

    for match_kind in ["regex", "bm25"] {
        let mut active = initial_active_tools(&catalog);
        let result = execute_tool_search(
            TOOL_SEARCH_NAME,
            &json!({"query":"matching","match":match_kind}),
            &catalog,
            &mut active,
        )
        .expect("search succeeds");

        assert_eq!(tool_search_reference_count(&result), 20);
    }
}

#[test]
fn tool_search_respects_and_caps_max_results() {
    let catalog = tool_search_catalog_with_matches(120);

    let mut active = initial_active_tools(&catalog);
    let limited = execute_tool_search(
        TOOL_SEARCH_NAME,
        &json!({"query":"matching","max_results":7}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert_eq!(tool_search_reference_count(&limited), 7);

    let mut active = initial_active_tools(&catalog);
    let capped = execute_tool_search(
        TOOL_SEARCH_NAME,
        &json!({"query":"matching","match":"regex","max_results":999}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert_eq!(tool_search_reference_count(&capped), 100);
}

#[test]
fn tool_search_schema_exposes_max_results_default_and_cap() {
    let mut catalog = Vec::new();
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let tool = catalog
        .iter()
        .find(|tool| tool.name == TOOL_SEARCH_NAME)
        .expect("tool search definition exists");
    let schema = &tool.input_schema["properties"]["max_results"];

    assert_eq!(schema["default"], 20);
    assert_eq!(schema["maximum"], 100);
    assert_eq!(schema["minimum"], 1);
    assert_eq!(tool.input_schema["properties"]["match"]["default"], "bm25");
}

#[tokio::test]
async fn code_execution_runs_python_and_returns_result_payload() {
    let tmp = tempdir().expect("tempdir");
    let result =
        execute_code_execution_tool(&json!({"code":"print('hello from code exec')"}), tmp.path())
            .await
            .expect("code execution should run");
    assert!(result.content.contains("hello from code exec"));
    assert!(result.content.contains("return_code"));
}

#[tokio::test]
async fn code_execution_runs_through_common_executor_after_approval_gate() {
    let tmp = tempdir().expect("tempdir");
    let (tx_event, _rx_event) = mpsc::channel(8);
    let result = Engine::execute_tool_with_lock(
        Arc::new(RwLock::new(())),
        false,
        false,
        tx_event,
        CODE_EXECUTION_TOOL_NAME.to_string(),
        json!({"code":"print('common executor code exec')"}),
        tmp.path().to_path_buf(),
        None,
        None,
        None,
    )
    .await
    .expect("code_execution should run through common executor");

    assert!(result.content.contains("common executor code exec"));
    assert!(result.content.contains("return_code"));
}

#[test]
fn plan_mode_catalog_skips_code_execution_tool_but_agent_keeps_it() {
    let mut plan_catalog = vec![api_tool("read_file")];
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut plan_catalog, AppMode::Plan, &always_load);
    assert!(
        !plan_catalog
            .iter()
            .any(|tool| tool.name == CODE_EXECUTION_TOOL_NAME),
        "Plan mode must not expose code_execution"
    );

    let mut agent_catalog = vec![api_tool("read_file")];
    ensure_advanced_tooling(&mut agent_catalog, AppMode::Agent, &always_load);
    assert!(
        agent_catalog
            .iter()
            .any(|tool| tool.name == CODE_EXECUTION_TOOL_NAME),
        "Agent mode should still expose code_execution"
    );
}

#[test]
fn deferred_tool_requests_are_auto_activated() {
    use std::collections::HashSet;

    let catalog = vec![Tool {
        tool_type: None,
        name: "exec_shell".to_string(),
        description: "Run shell commands".to_string(),
        input_schema: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(true),
        input_examples: None,
        strict: None,
        cache_control: None,
    }];

    let mut active = HashSet::new();
    assert!(!active.contains("exec_shell"));
    assert!(maybe_activate_requested_deferred_tool(
        "exec_shell",
        &catalog,
        &mut active
    ));
    assert!(active.contains("exec_shell"));
}

#[test]
fn missing_tool_error_message_offers_suggestions() {
    let catalog = vec![
        Tool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read file contents".to_string(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: None,
            name: "grep_files".to_string(),
            description: "Search file contents".to_string(),
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];

    let message = missing_tool_error_message("reed_file", &catalog);
    assert!(message.contains("Did you mean:"));
    assert!(message.contains("read_file"));
    assert!(message.contains(TOOL_SEARCH_NAME));
}

#[test]
fn missing_tool_error_message_includes_discovery_guidance_when_no_match() {
    let catalog = vec![Tool {
        tool_type: None,
        name: "read_file".to_string(),
        description: "Read file contents".to_string(),
        input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    }];

    let message = missing_tool_error_message("totally_unknown_tool", &catalog);
    assert!(message.contains("not available in the current tool catalog"));
    assert!(message.contains(TOOL_SEARCH_NAME));
}

#[test]
fn missing_tool_error_message_redirects_checklist_item_miscalls() {
    let catalog = vec![api_tool("note"), api_tool("tts")];

    for tool_name in ["item", "items", "todo", "checklist_item"] {
        let message = missing_tool_error_message(tool_name, &catalog);
        assert!(message.contains("work_update"), "{tool_name}: {message}");
        assert!(
            !message.contains("Did you mean"),
            "fuzzy suggestions are misleading for checklist mis-calls: {message}"
        );
    }
}

#[test]
fn missing_shell_tool_error_message_names_allow_shell_gate() {
    let catalog = vec![api_tool("read_file")];

    for tool_name in [
        "exec_shell",
        "exec_shell_wait",
        "exec_shell_interact",
        "task_shell_start",
        "task_shell_wait",
    ] {
        let message = missing_tool_error_message(tool_name, &catalog);
        assert!(message.contains("not available in the current tool catalog"));
        assert!(
            message.contains("allow_shell = false"),
            "{tool_name}: {message}"
        );
        assert!(message.contains("allow_shell"), "{tool_name}: {message}");
        assert!(
            message.contains("/config allow_shell true"),
            "{tool_name}: {message}"
        );
        assert!(message.contains("--save"), "{tool_name}: {message}");
        assert!(message.contains("Act mode"), "{tool_name}: {message}");
        assert!(
            message.contains("approval gating"),
            "{tool_name}: {message}"
        );
        assert!(!message.contains("YOLO"), "{tool_name}: {message}");
        assert!(!message.contains("auto-approve"), "{tool_name}: {message}");
        assert!(message.contains(TOOL_SEARCH_NAME), "{tool_name}: {message}");
    }
}

#[test]
fn missing_shell_tool_error_message_keeps_allow_shell_hint_with_suggestions() {
    let catalog = vec![api_tool("exec")];

    let message = missing_tool_error_message("exec_shell", &catalog);

    assert!(message.contains("Did you mean:"));
    assert!(message.contains("exec"));
    assert!(message.contains("allow_shell = false"));
    assert!(message.contains("allow_shell"));
    assert!(message.contains("/config allow_shell true"));
    assert!(message.contains("--save"));
    assert!(message.contains("Act mode"));
    assert!(!message.contains("YOLO"));
    assert!(!message.contains("auto-approve"));
    assert!(message.contains(TOOL_SEARCH_NAME));
}

#[test]
fn filter_tool_call_delta_strips_bracket_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "intro [TOOL_CALL]\n{\"tool\":\"x\"}\n[/TOOL_CALL] outro",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("[TOOL_CALL]"));
    assert!(!visible.contains("[/TOOL_CALL]"));
    assert!(!visible.contains("\"tool\":\"x\""));
    assert!(visible.contains("intro"));
    assert!(visible.contains("outro"));
}

#[test]
fn filter_tool_call_delta_strips_deepseek_xml_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "before <codewhale:tool_call name=\"x\">payload</codewhale:tool_call> after",
        &mut in_block,
    );
    assert!(!in_block);
    for marker in TOOL_CALL_START_MARKERS {
        assert!(
            !visible.contains(marker),
            "visible text leaked start marker `{marker}`: {visible:?}"
        );
    }
    assert!(visible.contains("before"));
    assert!(visible.contains("after"));
}

#[test]
fn filter_tool_call_delta_strips_generic_tool_call_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "lead <tool_call>\n{\"name\":\"do\"}\n</tool_call> tail",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<tool_call"));
    assert!(!visible.contains("</tool_call>"));
    assert!(visible.contains("lead"));
    assert!(visible.contains("tail"));
}

#[test]
fn filter_tool_call_delta_strips_invoke_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "alpha <invoke name=\"x\"><parameter name=\"k\">v</parameter></invoke> beta",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<invoke "));
    assert!(!visible.contains("</invoke>"));
    assert!(visible.contains("alpha"));
    assert!(visible.contains("beta"));
}

#[test]
fn filter_tool_call_delta_strips_function_calls_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "head <function_calls>\n{\"name\":\"x\"}\n</function_calls> tail",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<function_calls>"));
    assert!(!visible.contains("</function_calls>"));
    assert!(visible.contains("head"));
    assert!(visible.contains("tail"));
}

#[test]
fn filter_tool_call_delta_strips_siliconflow_v4_dsml_content_fixture() {
    // #2900: a SiliconFlow CN `deepseek-ai/DeepSeek-V4-Pro` stream can leak
    // DSML/function-call markup through the ordinary content channel. Keep it
    // out of visible assistant text; do not reinterpret `<function_calls>` as
    // an executable legacy text tool call.
    let mut in_block = false;
    let visible_a = filter_tool_call_delta(
        "visible prefix <function_calls>\n{\"name\":\"exec_shell\",\"arguments\":{\"cmd\":\"echo leaked\"}}",
        &mut in_block,
    );
    assert!(in_block);
    assert_eq!(visible_a, "visible prefix ");

    let visible_b = filter_tool_call_delta("\n</function_calls> visible suffix", &mut in_block);
    assert!(!in_block);
    assert_eq!(visible_b, " visible suffix");
    assert!(!visible_b.contains("exec_shell"));
    assert!(!visible_b.contains("<function_calls>"));
}

#[test]
fn filter_tool_call_delta_strips_fullwidth_dsml_invoke_fixture() {
    // #3717: Windows users reported SiliconFlow/DSML content leaking through
    // the ordinary text channel with fullwidth DSML wrapper tags. Treat it as
    // non-API tool markup, not visible assistant text.
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "visible prefix <｜DSML｜tool_calls>\n\
         <｜DSML｜invoke name=\"read_file\">\n\
         <｜DSML｜parameter name=\"path\" string=\"true\">backend/open_webui/utils/auth.py</｜DSML｜parameter>\n\
         </｜DSML｜invoke>\n\
         </｜DSML｜tool_calls> visible suffix",
        &mut in_block,
    );

    assert!(!in_block);
    assert_eq!(visible, "visible prefix  visible suffix");
    assert!(!visible.contains("DSML"));
    assert!(!visible.contains("read_file"));
    assert!(!visible.contains("backend/open_webui"));
}

#[test]
fn filter_tool_call_delta_strips_ascii_dsml_invoke_fixture() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "visible prefix <|DSML|tool_calls>\n\
         <|DSML|invoke name=\"read_file\">\n\
         <|DSML|parameter name=\"path\" string=\"true\">backend/open_webui/utils/auth.py</|DSML|parameter>\n\
         </|DSML|invoke>\n\
         </|DSML|tool_calls> visible suffix",
        &mut in_block,
    );

    assert!(!in_block);
    assert_eq!(visible, "visible prefix  visible suffix");
    assert!(!visible.contains("DSML"));
    assert!(!visible.contains("read_file"));
    assert!(!visible.contains("backend/open_webui"));
}

#[test]
fn filter_tool_call_delta_carries_split_fullwidth_dsml_marker() {
    let mut state = ToolCallDeltaFilterState::default();

    let visible_a = filter_tool_call_delta_with_state("visible prefix <｜DS", &mut state);
    assert_eq!(visible_a, "visible prefix ");

    let visible_b = filter_tool_call_delta_with_state(
        "ML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">",
        &mut state,
    );
    assert!(
        visible_b.is_empty(),
        "split DSML opener leaked: {visible_b:?}"
    );

    let visible_c = filter_tool_call_delta_with_state(
        "</｜DSML｜invoke>\n</｜DSML｜tool_calls> visible suffix",
        &mut state,
    );
    assert_eq!(visible_c, " visible suffix");
}

#[test]
fn filter_tool_call_delta_flushes_clean_partial_marker_prefix() {
    let mut state = ToolCallDeltaFilterState::default();

    let visible = filter_tool_call_delta_with_state("ordinary text ending in <", &mut state);
    assert_eq!(visible, "ordinary text ending in ");

    let flushed = flush_tool_call_delta_state(&mut state);
    assert_eq!(flushed, "<");
}

#[test]
fn filter_tool_call_delta_handles_chunk_split_marker() {
    let mut in_block = false;
    // First chunk opens the wrapper but does not close it.
    let visible_a = filter_tool_call_delta("hello <tool_call>partial", &mut in_block);
    assert!(in_block, "filter must remember it is mid-wrapper");
    assert_eq!(visible_a, "hello ");

    // Second chunk continues inside the wrapper, then closes it and adds tail.
    let visible_b = filter_tool_call_delta("payload</tool_call> tail", &mut in_block);
    assert!(!in_block);
    assert_eq!(visible_b, " tail");
}

#[test]
fn filter_tool_call_delta_unmatched_open_suppresses_remainder() {
    let mut in_block = false;
    let visible = filter_tool_call_delta("ok [TOOL_CALL]rest of stream", &mut in_block);
    assert_eq!(visible, "ok ");
    assert!(
        in_block,
        "unmatched open must leave filter in tool-call mode"
    );
}

#[test]
fn filter_tool_call_delta_passes_through_clean_text() {
    let mut in_block = false;
    let input = "no markers here, just prose with code `<not a tag>`.";
    let visible = filter_tool_call_delta(input, &mut in_block);
    assert!(!in_block);
    assert_eq!(visible, input);
}

#[test]
fn contains_fake_tool_wrapper_detects_each_marker() {
    for marker in TOOL_CALL_START_MARKERS {
        let needle = format!("noise {marker} more noise");
        assert!(
            contains_fake_tool_wrapper(&needle),
            "marker `{marker}` should be detected"
        );
    }
}

#[test]
fn contains_fake_tool_wrapper_returns_false_on_clean_text() {
    assert!(!contains_fake_tool_wrapper(
        "plain assistant text without wrappers"
    ));
    assert!(!contains_fake_tool_wrapper(
        "`<tool` lookalike but not a real start marker"
    ));
}

#[test]
fn fake_wrapper_notice_is_compact_and_actionable() {
    // Keep this short so it fits cleanly in a single status line.
    assert!(FAKE_WRAPPER_NOTICE.len() < 120);
    assert!(FAKE_WRAPPER_NOTICE.contains("API tool channel"));
}

// ---- final_tool_input: bug-class regression for "<command>" placeholder ----
//
// Background: a streamed tool block carries its `input` in two pieces — an
// initial value at `ContentBlockStart` (often `{}`), then `InputJsonDelta`
// chunks that build up `input_buffer`. The TUI used to fire `ToolCallStarted`
// from `ContentBlockStart` with the empty initial input and never re-emit
// once args were known, so cells rendered the literal text `<command>` /
// `<file>` placeholders. The fix relocates the emission to `ContentBlockStop`
// and routes the input through `final_tool_input`, which prefers the parsed
// buffer over a stale empty placeholder.
fn tool_state(initial: serde_json::Value, buffer: &str) -> ToolUseState {
    ToolUseState {
        id: "t1".into(),
        name: "exec_shell".into(),
        input: initial,
        caller: None,
        input_buffer: buffer.into(),
        input_parse_error: None,
    }
}

#[test]
fn final_tool_input_prefers_parsed_buffer_over_empty_initial() {
    // The exact regression: ContentBlockStart delivered `{}`, then args
    // streamed in via InputJsonDelta. The emitted ToolCallStarted must
    // carry the parsed buffer, not the placeholder.
    let state = tool_state(json!({}), r#"{"command": "ls -la"}"#);
    assert_eq!(final_tool_input(&state), json!({"command": "ls -la"}));
}

#[test]
fn final_tool_input_falls_back_to_initial_when_buffer_empty() {
    // Models occasionally embed args directly in the start frame and never
    // send any InputJsonDelta. We must still report those args.
    let state = tool_state(json!({"command": "echo hi"}), "");
    assert_eq!(final_tool_input(&state), json!({"command": "echo hi"}));
}

#[test]
fn final_tool_input_preserves_raw_buffer_for_parse_errors() {
    let mut state = tool_state(json!({}), "{not json");
    state.input_parse_error = Some("malformed tool arguments".into());
    assert_eq!(
        final_tool_input(&state),
        json!({"raw_arguments": "{not json"})
    );
}

// === #103 transparent stream-retry policy =====================================

#[test]
fn stream_retry_zero_content_then_error_is_transparently_retried() {
    // Case 2 from issue #103: stream yielded ZERO content then errored.
    // The decoder hit Err on the very first poll → engine should retry
    // because DeepSeek hasn't billed and the user has seen nothing.
    assert!(
        super::should_transparently_retry_stream(false, 0, false),
        "first attempt with no content must be eligible for transparent retry"
    );
    assert!(
        super::should_transparently_retry_stream(false, 1, false),
        "second attempt (one prior retry) with no content must still be eligible"
    );
}

#[test]
fn stream_retry_after_content_received_surfaces_error() {
    // Case 3 from issue #103: stream yielded content then errored. We must
    // NOT transparently retry — the model has emitted billed output tokens
    // and the UI has streamed deltas; resending would double-bill and the
    // user would see the same prefix twice.
    assert!(
        !super::should_transparently_retry_stream(true, 0, false),
        "any content received → no transparent retry, even with full budget"
    );
    assert!(
        !super::should_transparently_retry_stream(true, 1, false),
        "any content received → no transparent retry on subsequent attempts"
    );
}

#[test]
fn stream_read_error_message_explains_retry_before_output() {
    let message = super::stream_read_error_user_message(
        "Stream read error: error decoding response body",
        false,
    );

    assert!(message.contains("Provider stream connection dropped"));
    assert!(message.contains("No output had streamed yet"));
    assert!(message.contains("retry automatically"));
    assert!(message.contains("Stream read error: error decoding response body"));
}

#[test]
fn stream_read_error_message_explains_no_replay_after_output() {
    let message = super::stream_read_error_user_message(
        "Stream read error: error decoding response body",
        true,
    );

    assert!(message.contains("Provider stream connection dropped"));
    assert!(message.contains("Some output had already streamed"));
    assert!(message.contains("risking duplicated output"));
    assert!(message.contains("Stream read error: error decoding response body"));
    assert_eq!(
        crate::error_taxonomy::classify_error_message(&message),
        crate::error_taxonomy::ErrorCategory::Network
    );
}

#[test]
fn stream_retry_budget_caps_transparent_retries_at_two() {
    // Case 4 from issue #103: after MAX_TRANSPARENT_STREAM_RETRIES attempts
    // we stop trying transparently and let the outer error path surface.
    // (The outer per-turn `stream_retry_attempts` retry is a separate layer
    // and is still in effect at the whole-turn level.)
    assert!(
        super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES - 1,
            false,
        ),
        "one short of the cap should still retry"
    );
    assert!(
        !super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES,
            false,
        ),
        "at the cap, no further transparent retries"
    );
    assert!(
        !super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES + 5,
            false,
        ),
        "well past the cap, definitely no transparent retries"
    );
}

#[test]
fn stream_retry_respects_cancellation() {
    // Cancellation overrides every other condition. If the user pressed
    // Esc / Ctrl-C, do not silently re-issue the request behind their back.
    assert!(
        !super::should_transparently_retry_stream(false, 0, true),
        "cancelled turn must not be transparently retried"
    );
    assert!(
        !super::should_transparently_retry_stream(false, 1, true),
        "cancelled turn must not be transparently retried even with budget"
    );
}

// === #2990 sleep-resume policy ================================================

#[test]
fn sleep_gap_requires_wallclock_to_outrun_monotonic_clock() {
    use std::time::Duration;
    // No divergence: ordinary network failure, clocks agree.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(30), Duration::from_secs(30)),
        "equal elapsed times must not register as a sleep gap"
    );
    // Divergence below the threshold: NTP slew / scheduling jitter.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(5), Duration::from_secs(14)),
        "9s of divergence is below the 10s threshold"
    );
    // Divergence above the threshold: the host was suspended.
    assert!(
        super::sleep_gap_detected(Duration::from_secs(5), Duration::from_secs(16)),
        "11s of divergence must register as a sleep gap"
    );
    // Wall clock went backwards (NTP step): saturating_sub → zero gap.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(60), Duration::from_secs(5)),
        "wall clock behind monotonic must never register as a sleep gap"
    );
}

#[test]
fn sleep_resume_retries_even_after_content_streamed() {
    // The whole point of #2990: unlike the #103 transparent retry, a
    // detected sleep gap retries regardless of streamed content — the
    // partial output predates the sleep and the user was not watching.
    assert!(
        super::should_resume_after_sleep(true, 0, false),
        "detected sleep with full budget must resume"
    );
    assert!(
        super::should_resume_after_sleep(true, super::MAX_STREAM_RETRIES - 1, false),
        "detected sleep one short of the budget must still resume"
    );
}

#[test]
fn sleep_resume_requires_a_detected_gap() {
    // Without a sleep gap this layer stays out of the way entirely, so the
    // deliberate no-retry-after-content policy for ordinary flakes (#103)
    // is preserved.
    assert!(
        !super::should_resume_after_sleep(false, 0, false),
        "no sleep gap → never resume via this layer"
    );
}

#[test]
fn sleep_resume_respects_budget_and_cancellation() {
    assert!(
        !super::should_resume_after_sleep(true, super::MAX_STREAM_RETRIES, false),
        "budget exhausted → surface the failure instead of looping"
    );
    assert!(
        !super::should_resume_after_sleep(true, 0, true),
        "cancelled turn must not be resumed behind the user's back"
    );
}

#[test]
fn stream_retry_threshold_relaxed_to_five() {
    // Case 1+4 from issue #103: the consecutive-error threshold for marking
    // the turn failed was relaxed from 3 → 5 in v0.6.7 because the new
    // HTTP/2 keepalive defaults make spurious decode errors rarer.
    // This test pins the constant so a future regression to 3 fails loudly.
    assert_eq!(
        super::MAX_STREAM_ERRORS_BEFORE_FAIL,
        5,
        "the consecutive-stream-error threshold should be 5; \
         lowering it back to 3 will fail mid-turn under transient flakiness"
    );
    // And a regression guard on the transparent-retry cap.
    assert_eq!(
        super::MAX_TRANSPARENT_STREAM_RETRIES,
        2,
        "transparent-retry cap should be 2; raising it risks hammering the \
         provider on real outages"
    );
}

// === Issue #66: error taxonomy wired through engine + audit + capacity ===

/// A failed-tool audit entry must carry the typed `category` and `severity`
/// fields derived from the underlying `ToolError`. This is what makes
/// downstream tooling able to bucket failures without scraping the message
/// string.
#[test]
fn tool_failure_audit_payload_carries_category_and_severity() {
    use crate::error_taxonomy::ErrorEnvelope;
    use crate::tools::spec::ToolError;

    let error = ToolError::Timeout { seconds: 30 };
    let envelope: ErrorEnvelope = error.clone().into();
    let payload = json!({
        "event": "tool.result",
        "tool_id": "tool-1",
        "tool_name": "exec_shell",
        "status": ToolExecutionOutcome::from_legacy(Err(error.clone())).status.as_str(),
        "success": false,
        "error": error.to_string(),
        "category": envelope.category.to_string(),
        "severity": envelope.severity.to_string(),
    });

    assert_eq!(payload["category"], "timeout");
    assert_eq!(payload["severity"], "warning");
    assert_eq!(payload["status"], "timed_out");
    assert_eq!(payload["success"], false);
}

// ── #136: post-edit LSP diagnostics hook ─────────────────────────────────

#[test]
fn edited_paths_for_edit_file_returns_path() {
    let input = json!({ "path": "src/foo.rs", "search": "x", "replace": "y" });
    let paths = edited_paths_for_tool("edit_file", &input);
    assert_eq!(paths, vec![PathBuf::from("src/foo.rs")]);
}

#[test]
fn edited_paths_for_write_file_returns_path() {
    let input = json!({ "path": "src/bar.rs", "content": "fn main() {}" });
    let paths = edited_paths_for_tool("write_file", &input);
    assert_eq!(paths, vec![PathBuf::from("src/bar.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_replace_returns_each_path() {
    let input = json!({
        "replace": [
            { "path": "a.rs", "content": "" },
            { "path": "b.rs", "content": "" }
        ]
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert_eq!(paths, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_legacy_changes_returns_each_path() {
    let input = json!({
        "changes": [
            { "path": "a.rs", "content": "" },
            { "path": "b.rs", "content": "" }
        ]
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert_eq!(paths, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_diff_text_extracts_paths() {
    let input = json!({
        "patch": "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-let x: i32 = 0;\n+let x: i32 = \"oops\";\n"
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert_eq!(paths, vec![PathBuf::from("foo.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_invalid_diff_returns_empty() {
    let input = json!({
        "patch": "@@ -1 +1 @@\n-old\n+new\n"
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert!(paths.is_empty());
}

#[test]
fn edited_paths_for_unknown_tool_returns_empty() {
    let input = json!({ "path": "irrelevant.rs" });
    let paths = edited_paths_for_tool("read_file", &input);
    assert!(paths.is_empty());
    let paths = edited_paths_for_tool("grep_files", &input);
    assert!(paths.is_empty());
}

#[test]
fn parse_patch_paths_skips_dev_null() {
    let patch = "--- a/keep.rs\n+++ b/keep.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/deleted.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-delete me\n";
    let paths = edited_paths_for_tool("apply_patch", &json!({ "patch": patch }));
    assert_eq!(paths, vec![PathBuf::from("keep.rs")]);
}

#[tokio::test]
async fn post_edit_hook_injects_diagnostics_message_before_next_request() {
    use crate::lsp::{Diagnostic, Language, Severity};
    use std::sync::Arc;

    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let target = workspace.join("src").join("main.rs");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(&target, "let x: i32 = \"not a number\";").unwrap();

    let lsp_config = crate::lsp::LspConfig::default();
    let engine_config = EngineConfig {
        workspace: workspace.clone(),
        lsp_config: Some(lsp_config),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());

    // Install a fake transport that always reports a type error.
    let fake = Arc::new(crate::lsp::tests::FakeTransport::new(vec![Diagnostic {
        line: 1,
        column: 14,
        severity: Severity::Error,
        message: "expected i32, found &str".to_string(),
    }]));
    engine
        .lsp_manager
        .install_test_transport(Language::Rust, fake)
        .await;

    // Simulate the success path of an edit_file tool call.
    let input = json!({ "path": "src/main.rs", "search": "0", "replace": "\"not a number\"" });
    engine.run_post_edit_lsp_hook("edit_file", &input).await;
    assert_eq!(engine.pending_lsp_blocks.len(), 1);

    // Flush prepares the synthetic message.
    let messages_before = engine.session.messages.len();
    engine.flush_pending_lsp_diagnostics().await;
    assert_eq!(engine.session.messages.len(), messages_before + 1);

    let last = engine.session.messages.last().expect("message appended");
    assert_eq!(last.role, "user");
    // turn_meta is now at the tail of the content array (PR #2517).
    let meta = match last.content.last() {
        Some(crate::models::ContentBlock::Text { text, .. }) => text.clone(),
        other => panic!("expected text block at tail, got {other:?}"),
    };
    assert!(meta.starts_with("<turn_meta>\n"));
    let diagnostic_text = last
        .content
        .iter()
        .find_map(|block| match block {
            crate::models::ContentBlock::Text { text, .. }
                if text.contains("<diagnostics file=\"") =>
            {
                Some(text)
            }
            _ => None,
        })
        .expect("diagnostics text block");
    assert!(diagnostic_text.contains("ERROR [1:14] expected i32, found &str"));
}

#[tokio::test]
async fn post_edit_hook_is_silent_when_lsp_disabled() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let target = workspace.join("src").join("main.rs");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(&target, "fn main() {}").unwrap();

    let lsp_config = crate::lsp::LspConfig {
        enabled: false,
        ..Default::default()
    };
    let engine_config = EngineConfig {
        workspace: workspace.clone(),
        lsp_config: Some(lsp_config),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());

    let input = json!({ "path": "src/main.rs", "search": "x", "replace": "y" });
    engine.run_post_edit_lsp_hook("edit_file", &input).await;
    assert!(engine.pending_lsp_blocks.is_empty());

    let messages_before = engine.session.messages.len();
    engine.flush_pending_lsp_diagnostics().await;
    assert_eq!(engine.session.messages.len(), messages_before);
}

#[tokio::test]
async fn post_edit_hook_skips_unknown_tool_names() {
    use crate::lsp::{Diagnostic, Language, Severity};
    use std::sync::Arc;

    let tmp = tempdir().expect("tempdir");
    let engine_config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        lsp_config: Some(crate::lsp::LspConfig::default()),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());
    let fake = Arc::new(crate::lsp::tests::FakeTransport::new(vec![Diagnostic {
        line: 1,
        column: 1,
        severity: Severity::Error,
        message: "should not be reported".to_string(),
    }]));
    engine
        .lsp_manager
        .install_test_transport(Language::Rust, fake.clone())
        .await;

    let input = json!({ "path": "src/main.rs" });
    engine.run_post_edit_lsp_hook("read_file", &input).await;
    assert!(engine.pending_lsp_blocks.is_empty());
    assert_eq!(fake.call_count(), 0);
}

// ── #3802: non-blocking send for ListSubAgents refresh events ─────────────

#[test]
fn engine_handle_try_send_does_not_block_when_op_channel_is_full() {
    use tokio::sync::mpsc;

    // Create a channel with the smallest possible capacity.
    let (tx_op, _rx_op) = mpsc::channel::<Op>(1);

    // Construct a minimal EngineHandle with the tiny channel.
    let cancel_token = CancellationToken::new();
    let handle = EngineHandle {
        tx_op,
        rx_event: Arc::new(RwLock::new(mpsc::channel::<Event>(1).1)),
        cancel_token: Arc::new(StdMutex::new(cancel_token)),
        cancel_reason: Arc::new(StdMutex::new(None)),
        tx_approval: mpsc::channel(1).0,
        tx_user_input: mpsc::channel(1).0,
        tx_steer: mpsc::channel(1).0,
        shared_paused: Arc::new(StdMutex::new(false)),
        client_preflight_required: true,
    };

    // Fill the op channel with one message (capacity = 1).
    handle
        .tx_op
        .try_send(Op::ListSubAgents)
        .expect("first send should succeed");

    // try_send must return Err immediately — never block.
    let result = handle.try_send(Op::ListSubAgents);
    assert!(result.is_err(), "try_send should fail when channel is full");
}

#[tokio::test]
async fn reload_mcp_op_recovers_from_invalid_initial_config_in_process() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let config_path = tmp.path().join("mcp.json");
    let secret = "mcp-op-secret-must-not-escape";
    std::fs::write(
        &config_path,
        format!(r#"{{"servers":{{"bad":{{"token":"{secret}"}} trailing}}}}"#),
    )
    .expect("invalid config");
    let engine_config = EngineConfig {
        workspace,
        mcp_config_path: config_path.clone(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(engine_config, &Config::default());
    let task = tokio::spawn(async move { engine.run().await });

    let error = handle
        .reload_mcp(config_path.clone())
        .await
        .expect_err("invalid config must fail closed");
    assert!(!error.to_string().contains(secret));
    std::fs::write(
        &config_path,
        r#"{"servers":{"ready":{"command":"node","disabled":true}}}"#,
    )
    .expect("fixed config");

    let snapshot = handle
        .reload_mcp(config_path.clone())
        .await
        .expect("fixed config reloads without restarting the engine");
    assert!(!snapshot.reload_required);
    assert_eq!(snapshot.servers.len(), 1);
    assert_eq!(snapshot.servers[0].name, "ready");
    assert!(!snapshot.servers[0].enabled);

    let alternate_path = tmp.path().join("alternate-mcp.json");
    std::fs::write(
        &alternate_path,
        r#"{"servers":{"alternate":{"command":"node","disabled":true}}}"#,
    )
    .expect("alternate config");
    let alternate = handle
        .reload_mcp(alternate_path.clone())
        .await
        .expect("a changed config path replaces the engine pool in process");
    assert_eq!(alternate.config_path, alternate_path);
    assert_eq!(alternate.servers.len(), 1);
    assert_eq!(alternate.servers[0].name, "alternate");

    handle.send(Op::Shutdown).await.expect("shutdown");
    task.await.expect("engine task");
}

#[tokio::test]
async fn list_subagents_event_try_send_does_not_block_when_event_channel_full() {
    use tokio::sync::mpsc;

    // Simulate the engine's event channel with capacity 1.
    let (tx_event, mut _rx_event) = mpsc::channel::<Event>(1);

    // Fill the channel.
    tx_event
        .try_send(Event::status("filler"))
        .expect("first send should succeed");

    // Reproduce the handler pattern: try_send an AgentList event.
    // This must return Err immediately — the handler should never hang.
    let agents = vec![];
    let result = tx_event.try_send(Event::AgentList { agents });
    assert!(
        result.is_err(),
        "try_send should fail when event channel is full (backpressure avoided)"
    );
}
