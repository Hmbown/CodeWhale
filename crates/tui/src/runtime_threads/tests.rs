use super::*;
use crate::core::engine::{MockApprovalEvent, mock_engine_handle};
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use crate::test_support::{EnvVarGuard, lock_test_env};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;
use uuid::Uuid;

fn test_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!("deepseek-runtime-threads-{}", Uuid::new_v4()))
}

fn test_manager_config(data_dir: PathBuf) -> RuntimeThreadManagerConfig {
    RuntimeThreadManagerConfig {
        task_data_dir: data_dir.clone(),
        data_dir,
        max_active_threads: 4,
    }
}

fn test_manager(data_dir: PathBuf) -> Result<RuntimeThreadManager> {
    RuntimeThreadManager::open(
        Config::default(),
        PathBuf::from("."),
        test_manager_config(data_dir),
    )
}

struct ApprovalTimeoutGuard {
    previous_ms: u64,
}

impl Drop for ApprovalTimeoutGuard {
    fn drop(&mut self) {
        set_test_approval_decision_timeout_ms(self.previous_ms);
    }
}

fn test_approval_timeout_ms(ms: u64) -> ApprovalTimeoutGuard {
    ApprovalTimeoutGuard {
        previous_ms: set_test_approval_decision_timeout_ms(ms),
    }
}

struct DynamicToolTimeoutGuard {
    previous_ms: u64,
}

impl Drop for DynamicToolTimeoutGuard {
    fn drop(&mut self) {
        set_test_dynamic_tool_result_timeout_ms(self.previous_ms);
    }
}

fn test_dynamic_tool_timeout_ms(ms: u64) -> DynamicToolTimeoutGuard {
    DynamicToolTimeoutGuard {
        previous_ms: set_test_dynamic_tool_result_timeout_ms(ms),
    }
}

struct EventAppendFaultGuard {
    restore: Option<EventAppendTestFaultRestore>,
}

impl EventAppendFaultGuard {
    fn arm(thread_id: &str, fault: EventAppendTestFault) -> Self {
        Self::arm_repeated(thread_id, fault, 1)
    }

    fn arm_repeated(thread_id: &str, fault: EventAppendTestFault, count: usize) -> Self {
        Self {
            restore: Some(set_test_event_append_fault(thread_id, fault, count)),
        }
    }
}

impl Drop for EventAppendFaultGuard {
    fn drop(&mut self) {
        if let Some(restore) = self.restore.take() {
            restore_test_event_append_fault(restore);
        }
    }
}

fn sample_thread(thread_id: &str) -> ThreadRecord {
    let now = Utc::now();
    ThreadRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: thread_id.to_string(),
        created_at: now,
        updated_at: now,
        model: DEFAULT_TEXT_MODEL.to_string(),
        model_provider: None,
        model_provider_id: None,
        workspace: PathBuf::from("."),
        mode: AppMode::Agent.as_setting().to_string(),
        allow_shell: false,
        trust_mode: false,
        auto_approve: false,
        latest_turn_id: None,
        latest_response_bookmark: None,
        archived: false,
        system_prompt: None,
        task_id: None,
        title: None,
        session_id: None,
    }
}

fn sample_turn(thread_id: &str, turn_id: &str, status: RuntimeTurnStatus) -> TurnRecord {
    let now = Utc::now();
    TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: turn_id.to_string(),
        thread_id: thread_id.to_string(),
        status,
        input_summary: "sample".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: None,
        duration_ms: None,
        usage: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_model: None,
        error: None,
        item_ids: Vec::new(),
        steer_count: 0,
    }
}

#[test]
fn runtime_compaction_uses_provider_route_context() {
    let limits = codewhale_config::route::RouteLimits {
        context_tokens: Some(272_000),
        input_tokens: None,
        output_tokens: None,
    };
    let config = runtime_compaction_config(
        ApiProvider::OpenaiCodex,
        "gpt-5.5",
        Some(limits),
        false,
        false,
        80.0,
    );

    assert!(config.enabled);
    // The threshold is 80% of the route's spendable input budget after
    // output reservation and headroom, not 80% of the raw context window.
    assert_eq!(config.token_threshold, 213_504);
    assert_eq!(config.effective_context_window, Some(272_000));
}

#[test]
fn legacy_turn_record_has_no_invented_route_provenance() {
    let turn = sample_turn("thr_legacy", "turn_legacy", RuntimeTurnStatus::Completed);
    let mut value = serde_json::to_value(turn).expect("serialize turn");
    let object = value.as_object_mut().expect("turn object");
    object.remove("effective_provider");
    object.remove("effective_provider_id");
    object.remove("effective_billing_surface");
    object.remove("effective_model");

    let restored: TurnRecord = serde_json::from_value(value).expect("deserialize legacy turn");
    assert_eq!(restored.effective_provider, None);
    assert_eq!(restored.effective_billing_surface, None);
    assert_eq!(restored.effective_model, None);
}

#[tokio::test]
async fn named_custom_thread_identity_round_trips_and_fails_closed_when_removed() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            model: Some("local-default".to_string()),
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
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("local-code-model".to_string()),
            model_provider: Some("lm-studio".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let persisted = manager.get_thread(&thread.id).await?;
    assert_eq!(persisted.model_provider.as_deref(), Some("custom"));
    assert_eq!(persisted.model_provider_id.as_deref(), Some("lm-studio"));
    let serialized = serde_json::to_string(&persisted)?;
    assert!(serialized.contains("\"model_provider\":\"custom\""));
    assert!(serialized.contains("\"model_provider_id\":\"lm-studio\""));
    assert!(!serialized.contains("127.0.0.1:1234"));

    let route = manager.resolved_route_for_thread(&config, &persisted)?;
    assert_eq!(route.identity.provider, ApiProvider::Custom);
    assert_eq!(route.identity.key, "lm-studio");
    assert_eq!(route.model, "local-code-model");
    assert_eq!(route.config.deepseek_base_url(), "http://127.0.0.1:1234/v1");

    let err = manager
        .resolved_route_for_thread(&Config::default(), &persisted)
        .expect_err("removed provider must fail closed");
    let message = err.to_string();
    assert!(message.contains("[providers.lm-studio]"), "{message}");
    assert!(message.contains("will not fall back"), "{message}");

    let mut legacy_value = serde_json::to_value(&persisted)?;
    legacy_value
        .as_object_mut()
        .expect("thread object")
        .remove("model_provider");
    legacy_value
        .as_object_mut()
        .expect("thread object")
        .remove("model_provider_id");
    let legacy: ThreadRecord = serde_json::from_value(legacy_value)?;
    assert_eq!(legacy.model_provider, None);
    Ok(())
}

#[test]
fn legacy_literal_custom_thread_resume_requires_and_keeps_root_route() -> Result<()> {
    let config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-default-model".to_string()),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let mut persisted = sample_thread("thr_legacy_custom");
    persisted.model = "legacy-saved-model".to_string();
    persisted.model_provider = Some("custom".to_string());
    let restored: ThreadRecord = serde_json::from_str(&serde_json::to_string(&persisted)?)?;

    let route = manager.resolved_route_for_thread(&config, &restored)?;
    assert_eq!(route.identity.provider, ApiProvider::Custom);
    assert_eq!(route.identity.key, "custom");
    assert_eq!(route.model, "legacy-saved-model");
    assert_eq!(
        route.config.deepseek_base_url(),
        "http://127.0.0.1:18180/v1"
    );
    assert!(
        route
            .config
            .providers
            .as_ref()
            .is_none_or(|providers| !providers.custom.contains_key("custom")),
        "route resolution must not synthesize an ambiguous [providers.custom] table"
    );
    assert_eq!(
        route
            .config
            .resolve_provider_identity("custom")
            .map_err(anyhow::Error::msg)?,
        crate::config::ProviderIdentity {
            provider: ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: None,
        }
    );
    let repeated = manager.resolved_route_for_thread(&route.config, &restored)?;
    assert_eq!(repeated.identity.key, "custom");
    assert_eq!(repeated.model, "legacy-saved-model");
    assert_eq!(
        repeated.config.deepseek_base_url(),
        "http://127.0.0.1:18180/v1"
    );

    let named_config = {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "lm-studio".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                model: Some("named-model".to_string()),
                ..crate::config::ProviderConfig::default()
            },
        );
        Config {
            provider: Some("lm-studio".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..crate::config::ProvidersConfig::default()
            }),
            ..Config::default()
        }
    };
    let error = manager
        .resolved_route_for_thread(&named_config, &restored)
        .expect_err("id-less root record must not migrate to a named table")
        .to_string();
    assert!(error.contains("root-level"), "{error}");
    assert!(error.contains("will not guess or fall back"), "{error}");

    Ok(())
}

#[tokio::test]
async fn root_custom_thread_and_turn_writers_omit_exact_id() -> Result<()> {
    let config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config,
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("legacy-root-model".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    assert_eq!(thread.model_provider.as_deref(), Some("custom"));
    assert_eq!(thread.model_provider_id, None);
    assert!(!serde_json::to_string(&thread)?.contains("model_provider_id"));

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "keep the root route".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert_eq!(turn.effective_provider.as_deref(), Some("custom"));
    assert_eq!(turn.effective_provider_id, None);
    assert!(!serde_json::to_string(&turn)?.contains("effective_provider_id"));
    match harness.rx_op.recv().await {
        Some(Op::SendMessage { route, .. }) => {
            assert_eq!(route.identity.key, "custom");
            assert_eq!(route.identity.exact_id, None);
            assert_eq!(
                route.config.deepseek_base_url(),
                "http://127.0.0.1:18180/v1"
            );
        }
        other => panic!("expected root custom send, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn real_turn_client_preflight_failure_writes_no_in_progress_record() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "preflight-failure".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://preflight.invalid/v1".to_string()),
            model: Some("preflight-model".to_string()),
            api_key: Some("test-key".to_string()),
            // Client construction rejects this independently of ambient auth,
            // keeping the async regression hermetic without a global env lock.
            insecure_skip_tls_verify: Some(true),
            ..crate::config::ProviderConfig::default()
        },
    );
    let manager = RuntimeThreadManager::open(
        Config {
            provider: Some("preflight-failure".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..crate::config::ProvidersConfig::default()
            }),
            ..Config::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;

    let error = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "must not become a zombie turn".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await
        .expect_err("missing credentials must fail before turn persistence")
        .to_string();

    assert!(
        error.contains("TLS certificate verification cannot be disabled"),
        "{error}"
    );
    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    assert_eq!(manager.get_thread(&thread.id).await?.latest_turn_id, None);
    Ok(())
}

#[tokio::test]
async fn closed_turn_mailbox_rolls_back_durable_records_and_active_claim() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let before_active = {
        let active = manager.active.lock().await;
        let state = active.engines.get(&thread.id).expect("installed engine");
        (
            state.active_turn.as_ref().map(|turn| turn.turn_id.clone()),
            state.route_identity.clone(),
            state.route_model.clone(),
            active.lru.clone(),
        )
    };
    let before_thread = serde_json::to_value(manager.get_thread(&thread.id).await?)?;
    let before_events = serde_json::to_value(manager.events_since(&thread.id, None)?)?;
    drop(harness.rx_op);

    let error = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "mailbox is already closed".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await
        .expect_err("closed mailbox must reject the turn")
        .to_string();
    assert!(error.contains("Failed to start turn"), "{error}");

    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    assert_eq!(
        serde_json::to_value(manager.get_thread(&thread.id).await?)?,
        before_thread
    );
    assert_eq!(
        serde_json::to_value(manager.events_since(&thread.id, None)?)?,
        before_events
    );
    assert_eq!(
        std::fs::read_dir(&manager.store.items_dir)?.count(),
        0,
        "failed send must remove the optimistic user item"
    );
    let after_active = {
        let active = manager.active.lock().await;
        let state = active.engines.get(&thread.id).expect("installed engine");
        (
            state.active_turn.as_ref().map(|turn| turn.turn_id.clone()),
            state.route_identity.clone(),
            state.route_model.clone(),
            active.lru.clone(),
        )
    };
    assert_eq!(after_active, before_active);
    Ok(())
}

#[tokio::test]
async fn cancellation_while_waiting_for_mailbox_capacity_claims_nothing() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;

    for _ in 0..32 {
        harness.handle.try_send(Op::ListSubAgents)?;
    }
    let start_sender_count = harness.handle.tx_op.strong_count();
    let start_manager = manager.clone();
    let start_thread_id = thread.id.clone();
    let start_task = tokio::spawn(async move {
        start_manager
            .start_turn(
                &start_thread_id,
                StartTurnRequest {
                    prompt: "cancel before mailbox capacity".to_string(),
                    ..StartTurnRequest::default()
                },
            )
            .await
    });
    wait_for_sender_strong_count(&harness.handle.tx_op, start_sender_count + 2).await?;
    assert!(
        !start_task.is_finished(),
        "start should be waiting for capacity"
    );
    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    assert_eq!(manager.get_thread(&thread.id).await?.latest_turn_id, None);
    assert_eq!(manager.active_turn_flags(&thread.id, "missing").await, None);
    start_task.abort();
    let _ = start_task.await;
    for _ in 0..32 {
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::ListSubAgents)
        ));
    }

    for _ in 0..32 {
        harness.handle.try_send(Op::ListSubAgents)?;
    }
    let compact_sender_count = harness.handle.tx_op.strong_count();
    let compact_manager = manager.clone();
    let compact_thread_id = thread.id.clone();
    let compact_task = tokio::spawn(async move {
        compact_manager
            .compact_thread(&compact_thread_id, CompactThreadRequest::default())
            .await
    });
    wait_for_sender_strong_count(&harness.handle.tx_op, compact_sender_count + 2).await?;
    assert!(
        !compact_task.is_finished(),
        "compaction should be waiting for capacity"
    );
    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    assert_eq!(manager.get_thread(&thread.id).await?.latest_turn_id, None);
    compact_task.abort();
    let _ = compact_task.await;
    for _ in 0..32 {
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::ListSubAgents)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn caller_cancellation_after_engine_acceptance_keeps_owned_turn_lifecycle() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;

    // Block the first start-event append so the public future remains
    // cancellable after the operation has entered the engine mailbox.
    let event_state_guard = manager.store.state.lock().await;
    let start_manager = manager.clone();
    let thread_id = thread.id.clone();
    let start_task = tokio::spawn(async move {
        start_manager
            .start_turn(
                &thread_id,
                StartTurnRequest {
                    prompt: "the lifecycle outlives its caller".to_string(),
                    ..StartTurnRequest::default()
                },
            )
            .await
    });
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), harness.rx_op.recv()).await?,
        Some(Op::SendMessage { .. })
    ));
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 1);
    let turn_id = turns[0].id.clone();
    assert_eq!(turns[0].status, RuntimeTurnStatus::InProgress);
    assert_eq!(turns[0].item_ids.len(), 1);
    assert_eq!(
        manager.store.load_item(&turns[0].item_ids[0])?.turn_id,
        turn_id
    );
    assert!(
        manager
            .active_turn_flags(&thread.id, &turn_id)
            .await
            .is_some()
    );

    start_task.abort();
    let _ = start_task.await;
    drop(event_state_guard);

    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "owned monitor is live".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn_id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    assert_eq!(manager.active_turn_flags(&thread.id, &turn_id).await, None);

    let lifecycle: Vec<String> = manager
        .events_since(&thread.id, None)?
        .iter()
        .filter(|event| event.turn_id.as_deref() == Some(turn_id.as_str()))
        .map(|event| event.event.clone())
        .collect();
    assert_eq!(
        &lifecycle[..3],
        &["turn.started", "item.started", "item.completed"]
    );
    assert_eq!(lifecycle.last().map(String::as_str), Some("turn.completed"));
    Ok(())
}

#[tokio::test]
async fn thread_updates_while_start_waits_for_capacity_survive_latest_turn_write() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    for _ in 0..32 {
        harness.handle.try_send(Op::ListSubAgents)?;
    }
    let sender_count = harness.handle.tx_op.strong_count();

    let start_manager = manager.clone();
    let thread_id = thread.id.clone();
    let start_task = tokio::spawn(async move {
        start_manager
            .start_turn(
                &thread_id,
                StartTurnRequest {
                    prompt: "preserve concurrent metadata".to_string(),
                    ..StartTurnRequest::default()
                },
            )
            .await
    });
    wait_for_sender_strong_count(&harness.handle.tx_op, sender_count + 2).await?;
    assert!(!start_task.is_finished());

    manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                title: Some("new title while queued".to_string()),
                ..UpdateThreadRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::ListSubAgents)
    ));
    let turn = tokio::time::timeout(Duration::from_secs(2), start_task).await???;
    let mut saw_send = false;
    for _ in 0..32 {
        if matches!(harness.rx_op.recv().await, Some(Op::SendMessage { .. })) {
            saw_send = true;
            break;
        }
    }
    assert!(
        saw_send,
        "accepted send must remain behind refresh operations"
    );

    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "metadata retained".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    assert_eq!(turn.item_ids.len(), 1);
    assert!(
        terminal.item_ids.contains(&turn.item_ids[0]),
        "the accepted user item must survive later assistant-item writes"
    );
    assert_eq!(
        manager.store.load_turn(&turn.id)?.item_ids,
        terminal.item_ids
    );
    let updated = manager.get_thread(&thread.id).await?;
    assert_eq!(updated.title.as_deref(), Some("new title while queued"));
    assert_eq!(updated.latest_turn_id.as_deref(), Some(turn.id.as_str()));
    Ok(())
}

#[tokio::test]
async fn execution_update_while_start_waits_rejects_stale_operation() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    for _ in 0..32 {
        harness.handle.try_send(Op::ListSubAgents)?;
    }
    let sender_count = harness.handle.tx_op.strong_count();
    let start_manager = manager.clone();
    let thread_id = thread.id.clone();
    let start_task = tokio::spawn(async move {
        start_manager
            .start_turn(
                &thread_id,
                StartTurnRequest {
                    prompt: "must not use stale mode".to_string(),
                    ..StartTurnRequest::default()
                },
            )
            .await
    });
    wait_for_sender_strong_count(&harness.handle.tx_op, sender_count + 2).await?;

    manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                mode: Some(AppMode::Plan.as_setting().to_string()),
                ..UpdateThreadRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::ListSubAgents)
    ));
    let error = tokio::time::timeout(Duration::from_secs(2), start_task)
        .await??
        .expect_err("stale operation must fail")
        .to_string();
    assert!(error.contains("execution settings changed"), "{error}");
    for _ in 0..31 {
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::ListSubAgents)
        ));
    }
    assert!(harness.rx_op.try_recv().is_err());
    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    let updated = manager.get_thread(&thread.id).await?;
    assert_eq!(updated.mode, AppMode::Plan.as_setting());
    assert_eq!(updated.latest_turn_id, None);
    assert_eq!(manager.active_turn_flags(&thread.id, "missing").await, None);
    Ok(())
}

#[tokio::test]
async fn compact_lifecycle_outlives_caller_and_preserves_concurrent_thread_updates() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    for _ in 0..32 {
        harness.handle.try_send(Op::ListSubAgents)?;
    }
    let sender_count = harness.handle.tx_op.strong_count();

    let compact_manager = manager.clone();
    let thread_id = thread.id.clone();
    let compact_task = tokio::spawn(async move {
        compact_manager
            .compact_thread(&thread_id, CompactThreadRequest::default())
            .await
    });
    wait_for_sender_strong_count(&harness.handle.tx_op, sender_count + 2).await?;
    assert!(!compact_task.is_finished());
    manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                title: Some("title before compact claim".to_string()),
                ..UpdateThreadRequest::default()
            },
        )
        .await?;
    // Once capacity is released, block the acknowledgement events so the
    // API future can be dropped after the engine accepted the operation.
    let event_state_guard = manager.store.state.lock().await;
    let mut saw_compact = false;
    for _ in 0..33 {
        if matches!(
            tokio::time::timeout(Duration::from_secs(2), harness.rx_op.recv()).await?,
            Some(Op::CompactContext { .. })
        ) {
            saw_compact = true;
            break;
        }
    }
    assert!(
        saw_compact,
        "manual compaction must enter the engine mailbox"
    );
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 1);
    let turn_id = turns[0].id.clone();
    compact_task.abort();
    let _ = compact_task.await;
    drop(event_state_guard);

    harness
        .tx_event
        .send(EngineEvent::CompactionStarted {
            id: "manual_owned".to_string(),
            auto: false,
            message: "compaction started".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionCompleted {
            id: "manual_owned".to_string(),
            auto: false,
            message: "compaction completed".to_string(),
            messages_before: Some(4),
            messages_after: Some(2),
            summary_prompt: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn_id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    assert_eq!(manager.active_turn_flags(&thread.id, &turn_id).await, None);
    let updated = manager.get_thread(&thread.id).await?;
    assert_eq!(updated.title.as_deref(), Some("title before compact claim"));
    assert_eq!(updated.latest_turn_id.as_deref(), Some(turn_id.as_str()));
    Ok(())
}

#[tokio::test]
async fn concurrent_turn_starts_leave_one_claim_and_one_consistent_durable_turn() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let first = manager.start_turn(
        &thread.id,
        StartTurnRequest {
            prompt: "first concurrent turn".to_string(),
            ..StartTurnRequest::default()
        },
    );
    let second = manager.start_turn(
        &thread.id,
        StartTurnRequest {
            prompt: "second concurrent turn".to_string(),
            ..StartTurnRequest::default()
        },
    );

    let (first, second) = tokio::join!(first, second);
    let (turn, rejection) = match (first, second) {
        (Ok(turn), Err(error)) | (Err(error), Ok(turn)) => (turn, error),
        (first, second) => {
            panic!("expected one accepted turn and one rejection: {first:?} {second:?}")
        }
    };
    assert!(
        rejection.to_string().contains("already has an active turn"),
        "{rejection}"
    );
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].id, turn.id);
    assert_eq!(
        manager.get_thread(&thread.id).await?.latest_turn_id,
        Some(turn.id.clone())
    );
    assert_eq!(
        manager.active_turn_flags(&thread.id, &turn.id).await,
        Some((false, false))
    );
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    assert!(harness.rx_op.try_recv().is_err());
    Ok(())
}

#[test]
fn legacy_custom_thread_stays_on_root_when_literal_table_coexists() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "custom".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18182/v1".to_string()),
            model: Some("table-model".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18181/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let mut legacy = sample_thread("thr_ambiguous_legacy_custom");
    legacy.model = "legacy-saved-model".to_string();
    legacy.model_provider = Some("custom".to_string());
    legacy.model_provider_id = None;

    let root = manager.resolved_route_for_thread(&config, &legacy)?;
    assert_eq!(root.identity.provider, ApiProvider::Custom);
    assert_eq!(root.identity.key, "custom");
    assert_eq!(root.identity.exact_id, None);
    assert_eq!(root.config.deepseek_base_url(), "http://127.0.0.1:18181/v1");

    legacy.model_provider_id = Some("custom".to_string());
    let exact = manager.resolved_route_for_thread(&config, &legacy)?;
    assert_eq!(exact.identity.provider, ApiProvider::Custom);
    assert_eq!(exact.identity.key, "custom");
    assert_eq!(exact.identity.exact_id.as_deref(), Some("custom"));
    assert_eq!(
        exact.config.deepseek_base_url(),
        "http://127.0.0.1:18182/v1"
    );
    let root_only = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18181/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let error = manager
        .resolved_route_for_thread(&root_only, &legacy)
        .expect_err("exact literal table thread must not fall back to root")
        .to_string();
    assert!(error.contains("[providers.custom]"), "{error}");
    assert!(error.contains("will not fall back"), "{error}");
    Ok(())
}

#[tokio::test]
async fn empty_imported_custom_id_fails_closed_when_root_and_table_coexist() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "custom".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18182/v1".to_string()),
            model: Some("table-model".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18181/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    let mut imported = sample_thread("thr_empty_custom_id");
    imported.model_provider = Some("custom".to_string());
    imported.model_provider_id = Some("   ".to_string());
    let error = manager
        .resolved_route_for_thread(&config, &imported)
        .expect_err("malformed imported identity must not acquire the root route")
        .to_string();
    assert!(error.contains("empty exact provider id"), "{error}");

    let before = manager.store.list_threads()?.len();
    let request_error = manager
        .create_thread(CreateThreadRequest {
            model_provider: Some("custom".to_string()),
            model_provider_id: Some(String::new()),
            ..CreateThreadRequest::default()
        })
        .await
        .expect_err("malformed create request must fail before persistence")
        .to_string();
    assert!(
        request_error.contains("empty exact provider id"),
        "{request_error}"
    );
    assert_eq!(manager.store.list_threads()?.len(), before);
    Ok(())
}

#[tokio::test]
async fn thread_records_and_create_requests_preserve_provider_kind_id_pairing() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "openai".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18183/v1".to_string()),
            model: Some("custom-openai-model".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("openai".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..crate::config::ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    for provider_id in [None, Some("openai".to_string())] {
        let mut built_in = sample_thread("thr_builtin_openai_collision");
        built_in.model_provider = Some("openai".to_string());
        built_in.model_provider_id = provider_id;
        let error = manager
            .resolved_route_for_thread(&config, &built_in)
            .expect_err("built-in thread must not route through same-key custom endpoint")
            .to_string();
        assert!(error.contains("requires built-in 'openai'"), "{error}");
        assert!(error.contains("shadows"), "{error}");
    }

    let mut exact_custom = sample_thread("thr_custom_openai_collision");
    exact_custom.model = "custom-openai-model".to_string();
    exact_custom.model_provider = Some("custom".to_string());
    exact_custom.model_provider_id = Some("openai".to_string());
    let route = manager.resolved_route_for_thread(&config, &exact_custom)?;
    assert_eq!(route.identity.provider, ApiProvider::Custom);
    assert_eq!(route.identity.key, "openai");
    assert_eq!(
        route.config.deepseek_base_url(),
        "http://127.0.0.1:18183/v1"
    );

    let mut auto_thread = exact_custom.clone();
    auto_thread.id = "thr_auto_openai_collision".to_string();
    auto_thread.model = "auto".to_string();
    manager.store.save_thread(&auto_thread)?;
    let mut restored_turn = sample_turn(
        &auto_thread.id,
        "turn_openai_collision",
        RuntimeTurnStatus::Completed,
    );
    restored_turn.effective_provider = Some("openai".to_string());
    restored_turn.effective_provider_id = None;
    restored_turn.effective_model = Some("custom-openai-model".to_string());
    manager.store.save_turn(&restored_turn)?;
    let turn_error = manager
        .resolved_route_for_thread(&config, &auto_thread)
        .expect_err("restored built-in turn must not be captured by custom endpoint")
        .to_string();
    assert!(
        turn_error.contains("requires built-in 'openai'"),
        "{turn_error}"
    );

    restored_turn.effective_provider = Some("custom".to_string());
    restored_turn.effective_provider_id = Some("openai".to_string());
    manager.store.save_turn(&restored_turn)?;
    let restored_custom = manager.resolved_route_for_thread(&config, &auto_thread)?;
    assert_eq!(restored_custom.identity.provider, ApiProvider::Custom);
    assert_eq!(restored_custom.identity.key, "openai");
    assert_eq!(restored_custom.model, "custom-openai-model");

    let request_error = manager
        .create_thread(CreateThreadRequest {
            model_provider: Some("openai".to_string()),
            model_provider_id: Some("openai".to_string()),
            ..CreateThreadRequest::default()
        })
        .await
        .expect_err("built-in request must fail closed under exact custom shadow")
        .to_string();
    assert!(
        request_error.contains("requires built-in 'openai'"),
        "{request_error}"
    );

    let created = manager
        .create_thread(CreateThreadRequest {
            model_provider: Some("custom".to_string()),
            model_provider_id: Some("openai".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    assert_eq!(created.model_provider.as_deref(), Some("custom"));
    assert_eq!(created.model_provider_id.as_deref(), Some("openai"));
    assert_eq!(created.model, "custom-openai-model");
    Ok(())
}

#[tokio::test]
async fn config_reload_updates_next_turn_route_without_mutating_engine_route() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
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
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("local-model".to_string()),
            model_provider: Some("lm-studio".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;

    let mut reloaded = config;
    let provider = reloaded
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("named custom provider");
    provider.base_url = Some("http://127.0.0.1:18182/v1".to_string());
    provider.api_key = Some("new-local-test-key".to_string());
    manager.reload_config(reloaded).await?;

    let refreshed = manager.resolved_route_for_thread(&manager.read_config(), &thread)?;
    assert_eq!(refreshed.identity.key, "lm-studio");
    assert_eq!(
        refreshed.config.deepseek_base_url(),
        "http://127.0.0.1:18182/v1"
    );
    for _ in 0..3 {
        let op = harness.rx_op.recv().await.expect("runtime control op");
        assert!(
            matches!(
                op,
                Op::SetCompaction { .. }
                    | Op::SetStreamChunkTimeout { .. }
                    | Op::SetSubagentRuntimeConfig { .. }
            ),
            "reload must not mutate an engine provider route: {op:?}"
        );
    }
    let compact_turn = manager
        .compact_thread(
            &thread.id,
            CompactThreadRequest {
                reason: Some("verify refreshed route".to_string()),
            },
        )
        .await?;
    assert_eq!(compact_turn.effective_provider.as_deref(), Some("custom"));
    assert_eq!(
        compact_turn.effective_provider_id.as_deref(),
        Some("lm-studio")
    );
    assert_eq!(compact_turn.effective_model.as_deref(), Some("local-model"));
    match harness.rx_op.recv().await {
        Some(Op::CompactContext { route, compaction }) => {
            assert_eq!(route.identity.key, "lm-studio");
            assert_eq!(
                route.config.deepseek_base_url(),
                "http://127.0.0.1:18182/v1"
            );
            assert_eq!(compaction.model, "local-model");
            assert_eq!(
                compaction.effective_context_window,
                Some(crate::route_budget::route_context_window_tokens(
                    ApiProvider::Custom,
                    "local-model",
                    crate::route_budget::known_route_limits(route.candidate.limits()),
                ))
            );
        }
        other => panic!("expected typed compact route, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn config_sync_reports_removed_named_custom_route_and_keeps_mailbox_clean() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
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
    let manager = RuntimeThreadManager::open(
        config,
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("local-model".to_string()),
            model_provider: Some("lm-studio".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;

    let err = manager
        .reload_config(Config::default())
        .await
        .expect_err("removed named custom route must fail config reload");

    let message = err.to_string();
    assert!(message.contains(&thread.id), "{message}");
    assert!(message.contains("lm-studio"), "{message}");
    assert!(harness.rx_op.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn create_thread_uses_requested_named_custom_provider_default_model() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
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
                ..Default::default()
            },
        );
    }
    let config = Config {
        provider: Some("custom-b".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Default::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    let thread = manager
        .create_thread(CreateThreadRequest {
            model_provider: Some("custom-a".to_string()),
            ..Default::default()
        })
        .await?;

    assert_eq!(thread.model_provider.as_deref(), Some("custom"));
    assert_eq!(thread.model_provider_id.as_deref(), Some("custom-a"));
    assert_eq!(thread.model, "model-a");
    let route = manager.resolved_route_for_thread(&config, &thread)?;
    assert_eq!(route.identity.key, "custom-a");
    assert_eq!(
        route.config.deepseek_base_url(),
        "http://127.0.0.1:18181/v1"
    );
    Ok(())
}

#[tokio::test]
async fn create_thread_uses_requested_non_current_builtin_default_model() -> Result<()> {
    let config = Config {
        provider: Some("openrouter".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        ..Default::default()
    };
    let manager = RuntimeThreadManager::open(
        config,
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    let thread = manager
        .create_thread(CreateThreadRequest {
            model_provider: Some("zai".to_string()),
            ..Default::default()
        })
        .await?;

    assert_eq!(thread.model_provider.as_deref(), Some("zai"));
    assert_eq!(thread.model, crate::config::DEFAULT_ZAI_MODEL);
    Ok(())
}

#[tokio::test]
async fn simultaneous_named_custom_auto_threads_keep_exact_routes() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
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
                ..Default::default()
            },
        );
    }
    let manager = RuntimeThreadManager::open(
        Config {
            provider: Some("custom-b".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread_a = manager
        .create_thread(CreateThreadRequest {
            model: Some("auto".to_string()),
            model_provider: Some("custom-a".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_b = manager
        .create_thread(CreateThreadRequest {
            model: Some("auto".to_string()),
            model_provider: Some("custom-b".to_string()),
            ..Default::default()
        })
        .await?;
    let mut harness_a = install_mock_engine(&manager, &thread_a.id).await;
    let mut harness_b = install_mock_engine(&manager, &thread_b.id).await;

    let request_a = manager.start_turn(
        &thread_a.id,
        StartTurnRequest {
            prompt: "route A".to_string(),
            ..Default::default()
        },
    );
    let request_b = manager.start_turn(
        &thread_b.id,
        StartTurnRequest {
            prompt: "route B".to_string(),
            ..Default::default()
        },
    );
    let (turn_a, turn_b) = tokio::join!(request_a, request_b);
    let turn_a = turn_a?;
    let turn_b = turn_b?;

    assert_eq!(turn_a.effective_provider.as_deref(), Some("custom"));
    assert_eq!(turn_a.effective_provider_id.as_deref(), Some("custom-a"));
    assert_eq!(turn_a.effective_model.as_deref(), Some("model-a"));
    assert_eq!(turn_b.effective_provider.as_deref(), Some("custom"));
    assert_eq!(turn_b.effective_provider_id.as_deref(), Some("custom-b"));
    assert_eq!(turn_b.effective_model.as_deref(), Some("model-b"));
    match harness_a.rx_op.recv().await {
        Some(Op::SendMessage { route, .. }) => {
            assert_eq!(route.identity.provider, ApiProvider::Custom);
            assert_eq!(route.identity.key, "custom-a");
            assert_eq!(route.model, "model-a");
        }
        other => panic!("expected custom A send, got {other:?}"),
    }
    match harness_b.rx_op.recv().await {
        Some(Op::SendMessage { route, .. }) => {
            assert_eq!(route.identity.provider, ApiProvider::Custom);
            assert_eq!(route.identity.key, "custom-b");
            assert_eq!(route.model, "model-b");
        }
        other => panic!("expected custom B send, got {other:?}"),
    }
    Ok(())
}

#[test]
fn turn_record_persists_billing_surface_without_raw_endpoint() {
    let mut turn = sample_turn("thr_surface", "turn_surface", RuntimeTurnStatus::Completed);
    turn.effective_provider = Some(ApiProvider::Stepfun.as_str().to_string());
    turn.effective_billing_surface = Some(crate::pricing::STEPFUN_PAYG_BILLING_SURFACE.to_string());
    turn.effective_model = Some("step-3.7-flash".to_string());

    let value = serde_json::to_value(turn).expect("serialize turn");
    assert_eq!(
        value["effective_billing_surface"],
        crate::pricing::STEPFUN_PAYG_BILLING_SURFACE
    );
    assert!(value.get("base_url").is_none());
    assert!(value.get("effective_base_url").is_none());
}

#[tokio::test]
async fn aggregate_usage_keeps_codex_tokens_without_api_dollar_pricing() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let mut thread = sample_thread("thr_mixed_routes");
    thread.model = "auto".to_string();
    manager.store.save_thread(&thread)?;

    let usage = Usage {
        input_tokens: 10_000,
        output_tokens: 1_000,
        ..Usage::default()
    };
    let mut deepseek = sample_turn(&thread.id, "turn_deepseek", RuntimeTurnStatus::Completed);
    deepseek.usage = Some(usage.clone());
    deepseek.effective_provider = Some(ApiProvider::Deepseek.as_str().to_string());
    deepseek.effective_model = Some("deepseek-v4-flash".to_string());
    manager.store.save_turn(&deepseek)?;

    let mut codex = sample_turn(&thread.id, "turn_codex", RuntimeTurnStatus::Completed);
    codex.usage = Some(usage);
    codex.effective_provider = Some(ApiProvider::OpenaiCodex.as_str().to_string());
    codex.effective_model = Some("gpt-5.5".to_string());
    manager.store.save_turn(&codex)?;

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Provider)
        .await?;
    assert_eq!(report.totals.turns, 2);
    assert_eq!(report.totals.input_tokens, 20_000);
    assert!(report.totals.cost_usd > 0.0);

    let deepseek_bucket = report
        .buckets
        .iter()
        .find(|bucket| bucket.key == ApiProvider::Deepseek.as_str())
        .expect("DeepSeek bucket");
    let codex_bucket = report
        .buckets
        .iter()
        .find(|bucket| bucket.key == ApiProvider::OpenaiCodex.as_str())
        .expect("Codex bucket");
    assert!(deepseek_bucket.cost_usd > 0.0);
    assert_eq!(codex_bucket.cost_usd, 0.0);
    assert_eq!(codex_bucket.input_tokens, 10_000);
    assert_eq!(report.totals.cost_usd, deepseek_bucket.cost_usd);
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_prices_each_turn_at_its_recorded_time() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let mut thread = sample_thread("thr_historical_pricing");
    thread.model = "claude-sonnet-5".to_string();
    manager.store.save_thread(&thread)?;

    let usage = Usage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        ..Usage::default()
    };
    for (turn_id, created_at) in [
        ("turn_intro", "2026-08-31T23:59:59Z"),
        ("turn_standard", "2026-09-01T00:00:00Z"),
    ] {
        let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
        turn.created_at = created_at.parse().expect("recorded turn time");
        turn.usage = Some(usage.clone());
        turn.effective_provider = Some(ApiProvider::Anthropic.as_str().to_string());
        turn.effective_model = Some("claude-sonnet-5".to_string());
        manager.store.save_turn(&turn)?;
    }

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Model)
        .await?;

    assert_eq!(report.totals.turns, 2);
    assert!((report.totals.cost_usd - 5.0).abs() < f64::EPSILON);
    assert_eq!(report.buckets.len(), 1);
    assert!((report.buckets[0].cost_usd - 5.0).abs() < f64::EPSILON);
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_prices_only_stepfun_payg_surface() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let mut thread = sample_thread("thr_stepfun_surfaces");
    thread.model = "step-3.7-flash".to_string();
    manager.store.save_thread(&thread)?;

    let usage = Usage {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        prompt_cache_hit_tokens: Some(250_000),
        ..Usage::default()
    };
    for (turn_id, surface) in [
        (
            "turn_stepfun_payg",
            crate::pricing::STEPFUN_PAYG_BILLING_SURFACE,
        ),
        (
            "turn_stepfun_plan",
            crate::pricing::STEPFUN_PLAN_BILLING_SURFACE,
        ),
    ] {
        let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
        turn.usage = Some(usage.clone());
        turn.effective_provider = Some(ApiProvider::Stepfun.as_str().to_string());
        turn.effective_billing_surface = Some(surface.to_string());
        turn.effective_model = Some("step-3.7-flash".to_string());
        manager.store.save_turn(&turn)?;
    }

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Provider)
        .await?;
    assert_eq!(report.totals.turns, 2);
    assert!((report.totals.cost_usd - 0.735).abs() < 1e-12);
    Ok(())
}

fn sample_item(turn_id: &str, item_id: &str, status: TurnItemLifecycleStatus) -> TurnItemRecord {
    TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: item_id.to_string(),
        turn_id: turn_id.to_string(),
        kind: TurnItemKind::Status,
        status,
        summary: "sample item".to_string(),
        detail: None,
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(Utc::now()),
        ended_at: None,
    }
}

async fn install_mock_engine(
    manager: &RuntimeThreadManager,
    thread_id: &str,
) -> crate::core::engine::MockEngineHandle {
    let harness = mock_engine_handle();
    manager
        .install_test_engine(thread_id, harness.handle.clone())
        .await
        .expect("install mock engine");
    harness
}

async fn wait_for_sender_strong_count<T>(
    sender: &tokio::sync::mpsc::Sender<T>,
    minimum: usize,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while sender.strong_count() < minimum {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow!("Timed out waiting for mailbox reservation"))?;
    Ok(())
}

async fn wait_for_terminal_turn(
    manager: &RuntimeThreadManager,
    turn_id: &str,
    timeout: Duration,
) -> Result<TurnRecord> {
    let deadline = Instant::now() + timeout;
    loop {
        let turn = manager.store.load_turn(turn_id)?;
        let terminal = matches!(
            turn.status,
            RuntimeTurnStatus::Completed
                | RuntimeTurnStatus::Failed
                | RuntimeTurnStatus::Interrupted
                | RuntimeTurnStatus::Canceled
        );
        if terminal {
            let receipt_is_durable =
                manager
                    .events_since(&turn.thread_id, None)?
                    .iter()
                    .any(|event| {
                        event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
                    });
            let claim_is_clear = manager
                .active_turn_flags(&turn.thread_id, turn_id)
                .await
                .is_none();
            if receipt_is_durable && claim_is_clear {
                return Ok(turn);
            }
        }
        if Instant::now() >= deadline {
            bail!("Timed out waiting for turn {turn_id}");
        }
        sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn store_load_thread_rejects_newer_schema_version() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

    // Construct a thread record persisted with a future schema version.
    let mut thread = sample_thread("thr_future");
    thread.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

    // Bypass save_thread (which would respect our local schema_version)
    // by writing the JSON directly so we can simulate a future writer.
    let path = store.threads_dir.join(format!("{}.json", thread.id));
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
    let payload = serde_json::to_string(&thread).expect("serialize thread");
    std::fs::write(&path, payload).expect("write thread");

    let err = store
        .load_thread(&thread.id)
        .expect_err("load_thread must reject newer schema");
    let msg = format!("{err:#}");
    assert!(msg.contains("newer than supported"), "got: {msg}");

    // Cleanup so we don't leak across tests.
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn store_open_truncates_only_torn_final_event_record_and_preserves_sequence_gap() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    let first = store
        .append_event("thr_torn_tail", None, None, "first", json!({ "value": 1 }))
        .await
        .expect("append first event");
    let torn = store
        .append_event("thr_torn_tail", None, None, "torn", json!({ "value": 2 }))
        .await
        .expect("append event to tear");
    let path = store.events_path("thr_torn_tail").expect("event path");
    let original_len = std::fs::metadata(&path).expect("event metadata").len();
    assert!(original_len > 16);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open event log for simulated crash")
        .set_len(original_len - 16)
        .expect("tear final event record");
    drop(store);

    let reopened = RuntimeThreadStore::open(dir.clone()).expect("repair torn event tail");
    let replay = reopened
        .events_since("thr_torn_tail", None)
        .expect("replay repaired event log");
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].seq, first.seq);

    let after_repair = reopened
        .append_event(
            "thr_torn_tail",
            None,
            None,
            "after_repair",
            json!({ "value": 3 }),
        )
        .await
        .expect("append after repair");
    assert_eq!(after_repair.seq, torn.seq.saturating_add(1));
    assert_eq!(
        reopened
            .events_since("thr_torn_tail", None)
            .expect("replay repaired and appended events")
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![first.seq, after_repair.seq]
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn store_open_treats_valid_json_without_newline_as_uncommitted_append() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    let committed = store
        .append_event(
            "thr_missing_commit_marker",
            None,
            None,
            "committed",
            json!({ "value": 1 }),
        )
        .await
        .expect("append committed event");
    let uncommitted = store
        .append_event(
            "thr_missing_commit_marker",
            None,
            None,
            "missing_marker",
            json!({ "value": 2 }),
        )
        .await
        .expect("append event whose commit marker will be removed");
    let path = store
        .events_path("thr_missing_commit_marker")
        .expect("event path");
    let encoded = std::fs::read(&path).expect("read event log");
    assert_eq!(encoded.last(), Some(&b'\n'));
    let without_marker = &encoded[..encoded.len() - 1];
    let final_json = without_marker
        .rsplit(|byte| *byte == b'\n')
        .next()
        .expect("final JSON record");
    serde_json::from_slice::<RuntimeEventRecord>(final_json)
        .expect("the unterminated tail must otherwise be valid JSON");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open event log for simulated crash")
        .set_len(u64::try_from(without_marker.len()).expect("event log length fits u64"))
        .expect("remove only the newline commit marker");
    drop(store);

    let reopened = RuntimeThreadStore::open(dir.clone()).expect("repair uncommitted event tail");
    let replay = reopened
        .events_since("thr_missing_commit_marker", None)
        .expect("replay repaired event log");
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].seq, committed.seq);

    let after_repair = reopened
        .append_event(
            "thr_missing_commit_marker",
            None,
            None,
            "after_repair",
            json!({ "value": 3 }),
        )
        .await
        .expect("append after repair");
    assert_eq!(after_repair.seq, uncommitted.seq.saturating_add(1));

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn store_open_does_not_discard_newline_terminated_malformed_event() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    store
        .append_event("thr_bad_tail", None, None, "valid", json!({}))
        .await
        .expect("append valid event");
    let path = store.events_path("thr_bad_tail").expect("event path");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open event log");
    std::io::Write::write_all(&mut file, b"{malformed-but-terminated}\n")
        .expect("append malformed event");
    std::io::Write::flush(&mut file).expect("flush malformed event");
    drop(file);
    drop(store);

    let reopened = RuntimeThreadStore::open(dir.clone()).expect("open terminated event log");
    let error = reopened
        .events_since("thr_bad_tail", None)
        .expect_err("terminated malformed event must fail closed");
    assert!(
        format!("{error:#}").contains("Failed to parse event line"),
        "unexpected replay error: {error:#}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn event_replay_is_bounded_and_tail_cursor_skips_only_omitted_history() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread_id = "thr_bounded_replay";
    let path = manager.store.events_path(thread_id)?;
    let mut encoded = Vec::new();
    for seq in 1_u64..=600 {
        let event = RuntimeEventRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            seq,
            timestamp: Utc::now(),
            thread_id: thread_id.to_string(),
            turn_id: None,
            item_id: None,
            event: "test.event".to_string(),
            payload: json!({ "seq": seq }),
        };
        serde_json::to_writer(&mut encoded, &event)?;
        encoded.push(b'\n');
    }
    std::fs::write(path, encoded)?;

    let mut full = manager.replay_events(thread_id, None, None).await?;
    assert_eq!(full.base_seq, 0);
    let mut full_sequences = Vec::new();
    while let Some(batch) = full.batches.recv().await {
        let batch = batch.map_err(anyhow::Error::msg)?;
        assert!(
            batch.len() <= RUNTIME_EVENT_REPLAY_BATCH_SIZE,
            "replay batch exceeded its memory bound"
        );
        full_sequences.extend(batch.into_iter().map(|event| event.seq));
    }
    assert_eq!(full_sequences, (1_u64..=600).collect::<Vec<_>>());

    let mut tail = manager.replay_events(thread_id, None, Some(10)).await?;
    assert_eq!(tail.base_seq, 590);
    let mut tail_sequences = Vec::new();
    while let Some(batch) = tail.batches.recv().await {
        tail_sequences.extend(
            batch
                .map_err(anyhow::Error::msg)?
                .into_iter()
                .map(|event| event.seq),
        );
    }
    assert_eq!(tail_sequences, (591_u64..=600).collect::<Vec<_>>());

    let mut empty_tail = manager.replay_events(thread_id, None, Some(0)).await?;
    assert_eq!(empty_tail.base_seq, 600);
    assert!(empty_tail.batches.recv().await.is_none());
    assert!(
        manager
            .replay_events(
                thread_id,
                None,
                Some(MAX_RUNTIME_EVENT_REPLAY_TAIL.saturating_add(1)),
            )
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn event_reader_ignores_an_unterminated_live_append_tail() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let committed = store
        .append_event("thr_live_tail", None, None, "committed", json!({}))
        .await?;
    let path = store.events_path("thr_live_tail")?;
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    std::io::Write::write_all(&mut file, br#"{"schema_version":2,"seq":999"#)?;
    std::io::Write::flush(&mut file)?;

    let replay = store.events_since("thr_live_tail", None)?;
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].seq, committed.seq);
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

#[cfg(unix)]
#[test]
fn store_open_rejects_symlinked_state_file() {
    let dir = test_runtime_dir();
    std::fs::create_dir_all(&dir).expect("mkdir runtime dir");
    let target = dir.join("outside-state.json");
    let link = dir.join("state.json");
    std::fs::write(
        &target,
        serde_json::to_string(&RuntimeStoreState::default()).unwrap(),
    )
    .expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink state");

    let err = RuntimeThreadStore::open(dir.clone()).expect_err("symlink state should fail");
    assert!(format!("{err:#}").contains("must not be a symlink"));

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn event_append_rollback_rejects_a_swapped_symlink_target() -> Result<()> {
    let dir = test_runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let outside = dir.join("outside.jsonl");
    let events_path = dir.join("events.jsonl");
    std::fs::write(&outside, b"must-remain-intact\n")?;
    std::os::unix::fs::symlink(&outside, &events_path)?;

    let error = rollback_failed_event_append(&events_path, 0)
        .expect_err("rollback followed a swapped symlink target");
    assert!(format!("{error:#}").contains("must not be a symlink"));
    assert_eq!(std::fs::read(&outside)?, b"must-remain-intact\n");
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn store_open_rejects_root_traversal() {
    let dir = test_runtime_dir();
    let bad_root = dir.join("runtime").join("..").join("outside");

    let err = RuntimeThreadStore::open(bad_root).expect_err("traversal root should fail");
    assert!(format!("{err:#}").contains("cannot contain '..'"));

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn store_open_rejects_symlinked_store_directory() {
    let dir = test_runtime_dir();
    std::fs::create_dir_all(&dir).expect("mkdir runtime dir");
    let outside = dir.join("outside-items");
    let link = dir.join("items");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink items dir");

    let err = RuntimeThreadStore::open(dir.clone()).expect_err("symlink items dir should fail");
    assert!(
        format!("{err:#}").contains("directory must not be a symlink"),
        "got: {err:#}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn store_list_items_rejects_symlinked_item_file() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    let item = sample_item("turn_link", "item_link", TurnItemLifecycleStatus::Completed);
    let target = dir.join("outside-item.json");
    let link = store.items_dir.join(format!("{}.json", item.id));
    std::fs::write(&target, serde_json::to_string(&item).unwrap()).expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink item");

    let err = store
        .list_items_for_turn(&item.turn_id)
        .expect_err("symlink item should fail");
    assert!(format!("{err:#}").contains("must not be a symlink"));

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn store_list_items_rejects_swapped_symlinked_store_directory() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    let outside = dir.join("outside-items");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::fs::remove_dir_all(&store.items_dir).expect("remove items dir");
    std::os::unix::fs::symlink(&outside, &store.items_dir).expect("symlink items dir");

    let err = store
        .list_items_for_turn("turn_link")
        .expect_err("swapped symlink items dir should fail");
    assert!(
        format!("{err:#}").contains("directory must not be a symlink"),
        "got: {err:#}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_load_thread_defaults_missing_session_id() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");
    let thread = sample_thread("thr_legacy_session");
    let path = store.threads_dir.join(format!("{}.json", thread.id));
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
    let mut payload = serde_json::to_value(&thread).expect("serialize thread");
    payload
        .as_object_mut()
        .expect("thread object")
        .remove("session_id");
    std::fs::write(
        &path,
        serde_json::to_string(&payload).expect("encode thread"),
    )
    .expect("write thread");

    let loaded = store
        .load_thread(&thread.id)
        .expect("legacy thread should load");
    assert_eq!(loaded.session_id, None);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn seed_thread_keeps_tool_results_on_preceding_turn() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread = sample_thread("thr_seed_blocks");
    manager.store.save_thread(&thread)?;
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "check the files".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Thinking {
                    thinking: "need a tool".to_string(),
                    signature: Some("sig-1".to_string()),
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "shell".to_string(),
                    input: json!({ "cmd": "one" }),
                    caller: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "shell".to_string(),
                    input: json!({ "cmd": "two" }),
                    caller: None,
                },
            ],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "one".to_string(),
                is_error: None,
                content_blocks: Some(vec![json!({
                    "type": "text",
                    "text": "structured one"
                })]),
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-2".to_string(),
                content: "two".to_string(),
                is_error: Some(true),
                content_blocks: None,
            }],
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
                cache_control: None,
            }],
        },
    ];

    manager
        .seed_thread_from_messages(&thread.id, &messages)
        .await?;
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 1);

    let restored = manager.reconstruct_messages_from_turns(&turns)?;
    let roles = restored
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    assert_eq!(restored[2].content.len(), 2);

    match &restored[2].content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            content_blocks,
        } => {
            assert_eq!(tool_use_id, "tool-1");
            assert_eq!(content, "one");
            assert_eq!(*is_error, None);
            assert_eq!(
                content_blocks
                    .as_ref()
                    .and_then(|blocks| blocks[0].get("text")),
                Some(&json!("structured one"))
            );
        }
        other => panic!("expected first tool result, got {other:?}"),
    }
    match &restored[2].content[1] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            content_blocks,
        } => {
            assert_eq!(tool_use_id, "tool-2");
            assert_eq!(content, "two");
            assert_eq!(*is_error, Some(true));
            assert!(content_blocks.is_none());
        }
        other => panic!("expected second tool result, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

#[test]
fn current_runtime_schema_version_is_two_on_v066() {
    // Locks the bump in (issue #124). Bump deliberately when persisted
    // shape changes.
    assert_eq!(CURRENT_RUNTIME_SCHEMA_VERSION, 2);
}

#[test]
fn store_rejects_path_like_record_ids() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

    let err = store
        .load_thread("../outside")
        .expect_err("path traversal id should fail");
    assert!(
        format!("{err:#}").contains("unsupported characters"),
        "got: {err:#}"
    );

    let mut thread = sample_thread("thr_bad/id");
    let err = store
        .save_thread(&thread)
        .expect_err("path separator id should fail");
    assert!(
        format!("{err:#}").contains("unsupported characters"),
        "got: {err:#}"
    );

    thread.id = " thr_bad".to_string();
    let err = store
        .save_thread(&thread)
        .expect_err("whitespace id should fail");
    assert!(format!("{err:#}").contains("whitespace"), "got: {err:#}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_load_turn_rejects_newer_schema_version() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

    let mut turn = sample_turn("thr_t", "trn_future", RuntimeTurnStatus::InProgress);
    turn.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

    let path = store.turns_dir.join(format!("{}.json", turn.id));
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
    std::fs::write(&path, serde_json::to_string(&turn).expect("serialize turn"))
        .expect("write turn");

    let err = store
        .load_turn(&turn.id)
        .expect_err("load_turn must reject newer schema");
    assert!(
        format!("{err:#}").contains("newer than supported"),
        "got: {err:#}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_load_item_rejects_newer_schema_version() {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

    let mut item = sample_item("trn_t", "itm_future", TurnItemLifecycleStatus::InProgress);
    item.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

    let path = store.items_dir.join(format!("{}.json", item.id));
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
    std::fs::write(&path, serde_json::to_string(&item).expect("serialize item"))
        .expect("write item");

    let err = store
        .load_item(&item.id)
        .expect_err("load_item must reject newer schema");
    assert!(
        format!("{err:#}").contains("newer than supported"),
        "got: {err:#}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enforce_lru_capacity_does_not_loop_when_all_threads_are_active() {
    let mut active = ActiveThreads::default();
    let harness_a = mock_engine_handle();
    let harness_b = mock_engine_handle();

    active.engines.insert(
        "thr_a".to_string(),
        ActiveThreadState {
            engine: harness_a.handle,
            active_turn: Some(ActiveTurnState {
                turn_id: "turn_a".to_string(),
                interrupt_requested: false,
                auto_approve: true,
                trust_mode: false,
            }),
            route_identity: crate::config::ProviderIdentity {
                provider: ApiProvider::Deepseek,
                key: "deepseek".to_string(),
                exact_id: Some("deepseek".to_string()),
            },
            route_model: DEFAULT_TEXT_MODEL.to_string(),
            sensitive_user_input_values: HashSet::new(),
            client_preflight_required: false,
        },
    );
    active.engines.insert(
        "thr_b".to_string(),
        ActiveThreadState {
            engine: harness_b.handle,
            active_turn: Some(ActiveTurnState {
                turn_id: "turn_b".to_string(),
                interrupt_requested: false,
                auto_approve: true,
                trust_mode: false,
            }),
            route_identity: crate::config::ProviderIdentity {
                provider: ApiProvider::Deepseek,
                key: "deepseek".to_string(),
                exact_id: Some("deepseek".to_string()),
            },
            route_model: DEFAULT_TEXT_MODEL.to_string(),
            sensitive_user_input_values: HashSet::new(),
            client_preflight_required: false,
        },
    );
    active.lru.push_back("thr_a".to_string());
    active.lru.push_back("thr_b".to_string());

    let evicted = enforce_lru_capacity(&mut active, 2);
    assert!(evicted.is_empty(), "no idle threads should be evicted");
    assert_eq!(active.engines.len(), 2);
    assert_eq!(active.lru.len(), 2);
}

#[test]
fn approval_decision_keeps_trust_mode_out_of_tool_approval() {
    assert!(matches!(
        RuntimeThreadManager::approval_decision(false, false, false),
        RuntimeApprovalDecision::DenyTool
    ));
    assert!(matches!(
        RuntimeThreadManager::approval_decision(false, true, false),
        RuntimeApprovalDecision::DenyTool
    ));
    assert!(matches!(
        RuntimeThreadManager::approval_decision(true, false, false),
        RuntimeApprovalDecision::ApproveTool
    ));
    assert!(matches!(
        RuntimeThreadManager::approval_decision(true, false, true),
        RuntimeApprovalDecision::DenyTool
    ));
    assert!(matches!(
        RuntimeThreadManager::approval_decision(true, true, true),
        RuntimeApprovalDecision::RetryWithFullAccess
    ));
}

#[test]
fn open_recovers_queued_and_in_progress_turns() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(runtime_dir.clone())?;
    let thread = sample_thread("thr_recover");
    store.save_thread(&thread)?;

    let mut queued_turn = sample_turn(&thread.id, "turn_queued", RuntimeTurnStatus::Queued);
    let mut in_progress_turn =
        sample_turn(&thread.id, "turn_running", RuntimeTurnStatus::InProgress);
    let completed_turn = sample_turn(&thread.id, "turn_done", RuntimeTurnStatus::Completed);

    let queued_item = sample_item(
        &queued_turn.id,
        "item_queued",
        TurnItemLifecycleStatus::Queued,
    );
    let in_progress_item = sample_item(
        &in_progress_turn.id,
        "item_running",
        TurnItemLifecycleStatus::InProgress,
    );
    let completed_item = sample_item(
        &completed_turn.id,
        "item_done",
        TurnItemLifecycleStatus::Completed,
    );

    queued_turn.item_ids = vec![queued_item.id.clone()];
    in_progress_turn.item_ids = vec![in_progress_item.id.clone()];

    store.save_item(&queued_item)?;
    store.save_item(&in_progress_item)?;
    store.save_item(&completed_item)?;
    store.save_turn(&queued_turn)?;
    store.save_turn(&in_progress_turn)?;
    store.save_turn(&completed_turn)?;

    let manager = test_manager(runtime_dir)?;

    let queued_turn = manager.store.load_turn(&queued_turn.id)?;
    assert_eq!(queued_turn.status, RuntimeTurnStatus::Interrupted);
    assert_eq!(queued_turn.error.as_deref(), Some(RUNTIME_RESTART_REASON));
    assert!(queued_turn.ended_at.is_some());
    assert!(queued_turn.duration_ms.is_some());

    let in_progress_turn = manager.store.load_turn(&in_progress_turn.id)?;
    assert_eq!(in_progress_turn.status, RuntimeTurnStatus::Interrupted);
    assert_eq!(
        in_progress_turn.error.as_deref(),
        Some(RUNTIME_RESTART_REASON)
    );
    assert!(in_progress_turn.ended_at.is_some());
    assert!(in_progress_turn.duration_ms.is_some());

    let completed_turn = manager.store.load_turn(&completed_turn.id)?;
    assert_eq!(completed_turn.status, RuntimeTurnStatus::Completed);
    assert!(completed_turn.error.is_none());

    let queued_item = manager.store.load_item("item_queued")?;
    assert_eq!(queued_item.status, TurnItemLifecycleStatus::Interrupted);
    assert!(queued_item.ended_at.is_some());

    let in_progress_item = manager.store.load_item("item_running")?;
    assert_eq!(
        in_progress_item.status,
        TurnItemLifecycleStatus::Interrupted
    );
    assert!(in_progress_item.ended_at.is_some());

    let completed_item = manager.store.load_item("item_done")?;
    assert_eq!(completed_item.status, TurnItemLifecycleStatus::Completed);

    Ok(())
}

#[tokio::test]
async fn thread_lifecycle_persists_across_restart() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let manager = test_manager(runtime_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_turn_1".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageStarted { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageDelta {
                    index: 0,
                    content: "mock response".to_string(),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageComplete { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 12,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "first prompt".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    let completed = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(completed.status, RuntimeTurnStatus::Completed);

    drop(manager);

    let reopened = test_manager(runtime_dir)?;
    let detail = reopened.get_thread_detail(&thread.id).await?;
    assert_eq!(detail.thread.id, thread.id);
    assert_eq!(detail.turns.len(), 1);
    assert!(detail.latest_seq >= 1);
    assert!(!detail.items.is_empty());
    let events = reopened.events_since(&thread.id, None)?;
    assert!(
        events.iter().any(|ev| ev.event == "turn.completed"),
        "expected turn.completed event after restart"
    );
    Ok(())
}

#[tokio::test]
async fn completed_turn_without_engine_output_fails() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_empty_turn".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 0,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "empty turn".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;

    let failed = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(failed.status, RuntimeTurnStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some(EMPTY_TURN_REASON));

    let events = manager.events_since(&thread.id, None)?;
    assert!(events.iter().any(|ev| {
        ev.event == "item.failed"
            && ev
                .payload
                .get("item")
                .and_then(|item| item.get("kind"))
                .and_then(Value::as_str)
                == Some("error")
    }));
    assert!(events.iter().any(|ev| {
        ev.event == "turn.completed"
            && ev
                .payload
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
    }));
    Ok(())
}

#[tokio::test]
async fn preturn_control_status_does_not_make_empty_turn_succeed() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::AgentComplete {
                    id: "stale_agent".to_string(),
                    result: "stale completion".to_string(),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::status("Compaction settings updated"))
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_empty_after_control_status".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage::default(),
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "empty after setup".to_string(),
                ..Default::default()
            },
        )
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Failed);
    assert_eq!(terminal.error.as_deref(), Some(EMPTY_TURN_REASON));
    assert!(
        manager
            .store
            .list_items_for_turn(&turn.id)?
            .iter()
            .all(|item| {
                item.summary != "Compaction settings updated"
                    && !item.summary.contains("stale_agent")
            })
    );
    Ok(())
}

#[tokio::test]
async fn engine_error_remains_failed_after_nominal_turn_complete() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_error_then_complete".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::error(
                    crate::error_taxonomy::ErrorEnvelope::fatal("provider exploded"),
                ))
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage::default(),
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "surface the failure".to_string(),
                ..Default::default()
            },
        )
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Failed);
    assert_eq!(terminal.error.as_deref(), Some("provider exploded"));
    Ok(())
}

#[tokio::test]
async fn create_thread_defaults_auto_approve_to_false() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    assert!(!thread.auto_approve);
    Ok(())
}

#[tokio::test]
async fn update_thread_workspace_persists_event_and_evicts_idle_engine() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let old_workspace = std::env::temp_dir().join("codewhale-runtime-old-workspace");
    let new_workspace = std::env::temp_dir().join("codewhale-runtime-new-workspace");
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: Some(old_workspace.clone()),
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let updated = manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                workspace: Some(new_workspace.clone()),
                ..UpdateThreadRequest::default()
            },
        )
        .await?;

    assert_eq!(updated.workspace, new_workspace);
    assert_eq!(
        manager.store.load_thread(&thread.id)?.workspace,
        new_workspace
    );
    {
        let active = manager.active.lock().await;
        assert!(
            !active.engines.contains_key(&thread.id),
            "workspace changes must evict the stale cached engine"
        );
        assert!(!active.lru.iter().any(|id| id == &thread.id));
    }

    match tokio::time::timeout(Duration::from_secs(1), rx_op.recv()).await {
        Ok(Some(Op::Shutdown)) => {}
        other => panic!("expected cached engine shutdown, got {other:?}"),
    }

    let events = manager.events_since(&thread.id, None)?;
    let event = events
        .iter()
        .rev()
        .find(|event| event.event == "thread.updated")
        .expect("thread.updated event");
    let workspace_value = serde_json::to_value(&updated.workspace)?;
    assert_eq!(
        event
            .payload
            .get("changes")
            .and_then(|changes| changes.get("workspace")),
        Some(&workspace_value)
    );
    Ok(())
}

#[tokio::test]
async fn update_thread_workspace_rejects_empty_path() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let err = manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                workspace: Some(PathBuf::new()),
                ..UpdateThreadRequest::default()
            },
        )
        .await
        .expect_err("empty workspace must be rejected");
    assert!(format!("{err:#}").contains("workspace must not be empty"));
    Ok(())
}

#[tokio::test]
async fn update_thread_workspace_rejects_active_turn() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let old_workspace = std::env::temp_dir().join("codewhale-runtime-active-old");
    let new_workspace = std::env::temp_dir().join("codewhale-runtime-active-new");
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: Some(old_workspace.clone()),
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    {
        let mut active = manager.active.lock().await;
        let state = active.engines.get_mut(&thread.id).expect("mock engine");
        state.active_turn = Some(ActiveTurnState {
            turn_id: "turn_live".to_string(),
            interrupt_requested: false,
            auto_approve: false,
            trust_mode: false,
        });
    }

    let err = manager
        .update_thread(
            &thread.id,
            UpdateThreadRequest {
                workspace: Some(new_workspace),
                ..UpdateThreadRequest::default()
            },
        )
        .await
        .expect_err("workspace update during active turn must fail");

    assert!(format!("{err:#}").contains("active turn"));
    assert_eq!(
        manager.store.load_thread(&thread.id)?.workspace,
        old_workspace
    );
    {
        let active = manager.active.lock().await;
        assert!(
            active.engines.contains_key(&thread.id),
            "active engine should stay cached after rejected update"
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rx_op.recv())
            .await
            .is_err(),
        "rejected workspace update must not shut down the active engine"
    );
    Ok(())
}

#[tokio::test]
async fn start_turn_passes_effective_auto_approve_to_engine() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(false),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let _turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "override approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                ..Default::default()
            },
        )
        .await?;

    match rx_op.recv().await {
        Some(Op::SendMessage { auto_approve, .. }) => assert!(auto_approve),
        other => panic!("expected SendMessage op, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn start_turn_can_override_thread_auto_approve_to_false() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(true),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let _turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "disable approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(false),
                ..Default::default()
            },
        )
        .await?;

    match rx_op.recv().await {
        Some(Op::SendMessage { auto_approve, .. }) => assert!(!auto_approve),
        other => panic!("expected SendMessage op, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn compact_thread_preserves_thread_auto_approve_policy() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(false),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let turn = manager
        .compact_thread(&thread.id, CompactThreadRequest::default())
        .await?;

    assert!(matches!(
        rx_op.recv().await,
        Some(Op::CompactContext { .. })
    ));
    assert_eq!(
        manager.active_turn_flags(&thread.id, &turn.id).await,
        Some((false, false))
    );

    Ok(())
}

#[tokio::test]
async fn closed_compaction_mailbox_rolls_back_durable_records_and_active_claim() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let before_active = {
        let active = manager.active.lock().await;
        let state = active.engines.get(&thread.id).expect("installed engine");
        (
            state.active_turn.as_ref().map(|turn| turn.turn_id.clone()),
            state.route_identity.clone(),
            state.route_model.clone(),
            active.lru.clone(),
        )
    };
    let before_thread = serde_json::to_value(manager.get_thread(&thread.id).await?)?;
    let before_events = serde_json::to_value(manager.events_since(&thread.id, None)?)?;
    drop(harness.rx_op);

    let error = manager
        .compact_thread(&thread.id, CompactThreadRequest::default())
        .await
        .expect_err("closed mailbox must reject compaction")
        .to_string();
    assert!(error.contains("Failed to trigger compaction"), "{error}");

    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    assert_eq!(
        serde_json::to_value(manager.get_thread(&thread.id).await?)?,
        before_thread
    );
    assert_eq!(
        serde_json::to_value(manager.events_since(&thread.id, None)?)?,
        before_events
    );
    let after_active = {
        let active = manager.active.lock().await;
        let state = active.engines.get(&thread.id).expect("installed engine");
        (
            state.active_turn.as_ref().map(|turn| turn.turn_id.clone()),
            state.route_identity.clone(),
            state.route_model.clone(),
            active.lru.clone(),
        )
    };
    assert_eq!(after_active, before_active);
    Ok(())
}

#[tokio::test]
async fn compact_thread_receipt_keeps_exact_named_custom_identity() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            model: Some("local-code-model".to_string()),
            ..Default::default()
        },
    );
    let manager = RuntimeThreadManager::open(
        Config {
            provider: Some("lm-studio".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let turn = manager
        .compact_thread(&thread.id, CompactThreadRequest::default())
        .await?;

    assert!(matches!(
        rx_op.recv().await,
        Some(Op::CompactContext { .. })
    ));
    assert_eq!(turn.effective_provider.as_deref(), Some("custom"));
    assert_eq!(turn.effective_provider_id.as_deref(), Some("lm-studio"));
    Ok(())
}

#[tokio::test]
async fn compact_thread_with_real_engine_reaches_terminal_status() -> Result<()> {
    let manager = RuntimeThreadManager::open(
        Config {
            // This test intentionally crosses the real-engine boundary. Give
            // client preflight a hermetic credential and closed-loopback URL;
            // the assertion permits the resulting terminal failure.
            api_key: Some("runtime-thread-test-key".to_string()),
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            ..Config::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let turn = manager
        .compact_thread(&thread.id, CompactThreadRequest::default())
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;

    assert!(matches!(
        terminal.status,
        RuntimeTurnStatus::Completed | RuntimeTurnStatus::Failed
    ));
    assert!(
        terminal.ended_at.is_some(),
        "manual compaction should reach a terminal turn state"
    );
    assert_eq!(manager.active_turn_flags(&thread.id, &turn.id).await, None);

    let expected_status = match terminal.status {
        RuntimeTurnStatus::Completed => "completed",
        RuntimeTurnStatus::Failed => "failed",
        other => panic!("unexpected non-terminal compaction status: {other:?}"),
    };
    let events = manager.events_since(&thread.id, None)?;
    assert!(events.iter().any(|ev| {
        ev.event == "turn.completed"
            && ev
                .payload
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                == Some(expected_status)
    }));
    Ok(())
}

#[tokio::test]
async fn multi_turn_continuity_same_thread() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        let mut turn_index = 0u8;
        while let Some(op) = rx_op.recv().await {
            if !matches!(op, Op::SendMessage { .. }) {
                continue;
            }
            turn_index = turn_index.saturating_add(1);
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: format!("engine_turn_{turn_index}"),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageStarted { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageDelta {
                    index: 0,
                    content: format!("reply {turn_index}"),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageComplete { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 5,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
            if turn_index >= 2 {
                break;
            }
        }
    });

    let turn_1 = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "first".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    let turn_1 = wait_for_terminal_turn(&manager, &turn_1.id, Duration::from_secs(2)).await?;
    assert_eq!(turn_1.status, RuntimeTurnStatus::Completed);

    let turn_2 = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "second".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    let turn_2 = wait_for_terminal_turn(&manager, &turn_2.id, Duration::from_secs(2)).await?;
    assert_eq!(turn_2.status, RuntimeTurnStatus::Completed);

    let detail = manager.get_thread_detail(&thread.id).await?;
    assert_eq!(
        detail.thread.latest_turn_id.as_deref(),
        Some(turn_2.id.as_str())
    );
    assert_eq!(detail.turns.len(), 2);
    assert!(detail.items.iter().any(|item| {
        item.kind == TurnItemKind::UserMessage && item.detail.as_deref() == Some("first")
    }));
    assert!(detail.items.iter().any(|item| {
        item.kind == TurnItemKind::UserMessage && item.detail.as_deref() == Some("second")
    }));

    let events = manager.events_since(&thread.id, None)?;
    let started = events
        .iter()
        .filter(|ev| ev.event == "turn.started")
        .count();
    let completed = events
        .iter()
        .filter(|ev| ev.event == "turn.completed")
        .count();
    assert_eq!(started, 2);
    assert_eq!(completed, 2);
    Ok(())
}

#[tokio::test]
async fn get_thread_detail_batches_items_by_turn_without_losing_order() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let base = Utc::now();
    let mut first_turn = sample_turn(
        &thread.id,
        "turn_detail_batch_first",
        RuntimeTurnStatus::Completed,
    );
    first_turn.created_at = base;
    let mut second_turn = sample_turn(
        &thread.id,
        "turn_detail_batch_second",
        RuntimeTurnStatus::Completed,
    );
    second_turn.created_at = base + chrono::Duration::seconds(1);
    manager.store.save_turn(&first_turn)?;
    manager.store.save_turn(&second_turn)?;

    let mut first_late = sample_item(
        &first_turn.id,
        "item_detail_first_late",
        TurnItemLifecycleStatus::Completed,
    );
    first_late.started_at = Some(base + chrono::Duration::seconds(5));
    let mut first_early = sample_item(
        &first_turn.id,
        "item_detail_first_early",
        TurnItemLifecycleStatus::Completed,
    );
    first_early.started_at = Some(base + chrono::Duration::seconds(1));
    let mut second_item = sample_item(
        &second_turn.id,
        "item_detail_second",
        TurnItemLifecycleStatus::Completed,
    );
    second_item.started_at = Some(base + chrono::Duration::seconds(2));
    let unrelated = sample_item(
        "turn_detail_batch_unrelated",
        "item_detail_unrelated",
        TurnItemLifecycleStatus::Completed,
    );

    manager.store.save_item(&first_late)?;
    manager.store.save_item(&second_item)?;
    manager.store.save_item(&unrelated)?;
    manager.store.save_item(&first_early)?;

    let detail = manager.get_thread_detail(&thread.id).await?;
    let item_ids: Vec<&str> = detail.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        item_ids,
        vec![
            "item_detail_first_early",
            "item_detail_first_late",
            "item_detail_second"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn interrupt_turn_marks_interrupted_after_cleanup() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    let cancel_token = harness.cancel_token;
    let cleanup_delay = Duration::from_millis(140);
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_turn_interrupt".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageStarted { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageDelta {
                    index: 0,
                    content: "partial".to_string(),
                })
                .await;
            cancel_token.cancelled().await;
            sleep(cleanup_delay).await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "interrupt me".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;

    sleep(Duration::from_millis(20)).await;
    let interrupted_at = Instant::now();
    let interrupt_result = manager.interrupt_turn(&thread.id, &turn.id).await?;
    assert_eq!(interrupt_result.status, RuntimeTurnStatus::InProgress);

    let final_turn = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(3)).await?;
    assert_eq!(final_turn.status, RuntimeTurnStatus::Interrupted);
    assert!(
        interrupted_at.elapsed() >= cleanup_delay,
        "turn transitioned before cleanup finished"
    );

    let events = manager.events_since(&thread.id, None)?;
    let interrupt_seq = events
        .iter()
        .find(|ev| ev.event == "turn.interrupt_requested")
        .map(|ev| ev.seq)
        .context("missing turn.interrupt_requested event")?;
    let completed = events
        .iter()
        .find(|ev| ev.event == "turn.completed")
        .context("missing turn.completed event")?;
    assert!(completed.seq > interrupt_seq);
    assert_eq!(
        completed
            .payload
            .get("turn")
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str),
        Some("interrupted")
    );
    Ok(())
}

#[tokio::test]
async fn approval_required_with_stale_active_turn_is_denied() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(true),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                ..Default::default()
            },
        )
        .await?;

    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    {
        let mut active = manager.active.lock().await;
        let state = active
            .engines
            .get_mut(&thread.id)
            .context("missing active thread state")?;
        state.active_turn = None;
    }

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "test_key".to_string(),
            approval_grouping_key: "test_key".to_string(),
            id: "tool_stale".to_string(),
            tool_name: "exec_command".to_string(),
            description: "stale approval".to_string(),
            input: serde_json::json!({}),
            intent_summary: None,
            approval_force_prompt: false,
        })
        .await?;

    assert_eq!(
        harness.recv_approval_event().await,
        Some(MockApprovalEvent::Denied {
            id: "tool_stale".to_string(),
        })
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            },
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;

    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn approval_required_awaits_external_decision_allow() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let _turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "key1".to_string(),
            approval_grouping_key: "key1".to_string(),
            id: "tool_external_allow".to_string(),
            tool_name: "exec_command".to_string(),
            description: "external allow".to_string(),
            input: serde_json::json!({}),
            intent_summary: Some("I will update the config file.".to_string()),
            approval_force_prompt: false,
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(manager.pending_approvals_count(), 1);

    let detail = manager.get_thread_detail(&thread.id).await?;
    assert_eq!(detail.pending_approvals.len(), 1);
    assert_eq!(detail.pending_approvals[0].id, "tool_external_allow");
    assert_eq!(detail.pending_approvals[0].turn_id, _turn.id);
    assert_eq!(detail.pending_approvals[0].tool_name, "exec_command");
    assert_eq!(detail.pending_user_inputs.len(), 0);

    let events = manager.events_since(&thread.id, None)?;
    let approval_event = events
        .iter()
        .rev()
        .find(|event| event.event == "approval.required")
        .context("missing approval.required event")?;
    assert_eq!(
        approval_event
            .payload
            .get("intent_summary")
            .and_then(Value::as_str),
        Some("I will update the config file.")
    );

    assert!(manager.deliver_external_approval(
        "tool_external_allow",
        ExternalApprovalDecision::Allow { remember: false },
    ));
    assert_eq!(
        harness.recv_approval_event().await,
        Some(MockApprovalEvent::Approved {
            id: "tool_external_allow".to_string(),
        })
    );
    assert_eq!(manager.pending_approvals_count(), 0);
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_approvals
            .is_empty()
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn user_input_snapshot_survives_reload_and_clears_after_submission() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs a choice".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::UserInputRequired {
            id: "input_reload".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: vec![crate::tools::user_input::UserInputQuestion {
                    header: "Continue".to_string(),
                    id: "continue".to_string(),
                    question: "Continue with the check?".to_string(),
                    options: vec![
                        crate::tools::user_input::UserInputOption {
                            label: "Yes".to_string(),
                            description: "Continue now".to_string(),
                        },
                        crate::tools::user_input::UserInputOption {
                            label: "No".to_string(),
                            description: "Stop here".to_string(),
                        },
                    ],
                    allow_free_text: false,
                    multi_select: false,
                }],
            },
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    let detail = loop {
        let detail = manager.get_thread_detail(&thread.id).await?;
        if !detail.pending_user_inputs.is_empty() {
            break detail;
        }
        if Instant::now() >= deadline {
            bail!("pending user input did not reach the canonical snapshot");
        }
        sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(detail.pending_approvals.len(), 0);
    assert_eq!(detail.pending_user_inputs.len(), 1);
    assert_eq!(detail.pending_user_inputs[0].id, "input_reload");
    assert_eq!(detail.pending_user_inputs[0].turn_id, turn.id);
    assert_eq!(
        detail.pending_user_inputs[0].request.questions[0].question,
        "Continue with the check?"
    );

    manager
        .submit_user_input(
            &thread.id,
            "input_reload",
            crate::tools::user_input::UserInputResponse {
                answers: vec![crate::tools::user_input::UserInputAnswer {
                    id: "continue".to_string(),
                    label: "Yes".to_string(),
                    value: "Yes".to_string(),
                }],
            },
        )
        .await?;
    match harness.recv_user_input_submission().await {
        Some((id, response)) => {
            assert_eq!(id, "input_reload");
            assert_eq!(response.answers[0].id, "continue");
        }
        other => panic!("expected submitted user input, got {other:?}"),
    }
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_user_inputs
            .is_empty()
    );
    assert!(manager.events_since(&thread.id, None)?.iter().any(|event| {
        event.event == "user_input.answered"
            && event.payload.get("input_id").and_then(Value::as_str) == Some("input_reload")
    }));

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn unknown_user_input_id_is_not_delivered_to_engine() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let delivered = manager
        .submit_user_input(
            &thread.id,
            "input_missing",
            crate::tools::user_input::UserInputResponse {
                answers: vec![crate::tools::user_input::UserInputAnswer {
                    id: "choice".to_string(),
                    label: "Missing".to_string(),
                    value: "must-not-enter-engine-mailbox".to_string(),
                }],
            },
        )
        .await?;
    assert!(!delivered);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            harness.recv_user_input_submission()
        )
        .await
        .is_err(),
        "unknown request entered the engine mailbox"
    );
    assert!(manager.events_since(&thread.id, None)?.iter().all(|event| {
        !matches!(
            event.event.as_str(),
            "user_input.answered" | "user_input.canceled"
        )
    }));
    Ok(())
}

#[tokio::test]
async fn user_input_receipt_append_failure_restores_request_without_delivery() -> Result<()> {
    const SECRET: &str = "answer-only-for-engine-after-retry";
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    manager.register_pending_user_input(
        &thread.id,
        PendingUserInputRequest {
            id: "input_retry".to_string(),
            turn_id: "turn_retry".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: Vec::new(),
            },
        },
    );
    let response = || crate::tools::user_input::UserInputResponse {
        answers: vec![crate::tools::user_input::UserInputAnswer {
            id: "choice".to_string(),
            label: "Retry".to_string(),
            value: SECRET.to_string(),
        }],
    };

    let fault_guard = EventAppendFaultGuard::arm(&thread.id, EventAppendTestFault::AfterSync);
    let error = manager
        .submit_user_input(&thread.id, "input_retry", response())
        .await
        .expect_err("injected receipt append unexpectedly succeeded");
    drop(fault_guard);
    assert!(format!("{error:#}").contains("rolled back"));
    assert_eq!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_user_inputs
            .len(),
        1,
        "retry-safe append failure removed the authoritative prompt"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            harness.recv_user_input_submission()
        )
        .await
        .is_err(),
        "answer reached the engine before its receipt was durable"
    );

    assert!(
        manager
            .submit_user_input(&thread.id, "input_retry", response())
            .await?,
        "restored request was not retryable"
    );
    let (_, delivered) =
        tokio::time::timeout(Duration::from_secs(2), harness.recv_user_input_submission())
            .await
            .context("retried answer did not reach the engine")?
            .context("retried answer was canceled")?;
    assert_eq!(delivered.answers[0].value, SECRET);
    let events = manager.events_since(&thread.id, None)?;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event == "user_input.answered")
            .count(),
        1
    );
    assert!(!serde_json::to_string(&events)?.contains(SECRET));
    Ok(())
}

#[tokio::test]
async fn user_input_settlement_outlives_canceled_api_future() -> Result<()> {
    const SECRET: &str = "answer-survives-request-disconnect";
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    manager.register_pending_user_input(
        &thread.id,
        PendingUserInputRequest {
            id: "input_detached".to_string(),
            turn_id: "turn_detached".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: Vec::new(),
            },
        },
    );

    let emit_guard = manager.event_emit.lock().await;
    let submit_manager = manager.clone();
    let thread_id = thread.id.clone();
    let submission = tokio::spawn(async move {
        submit_manager
            .submit_user_input(
                &thread_id,
                "input_detached",
                crate::tools::user_input::UserInputResponse {
                    answers: vec![crate::tools::user_input::UserInputAnswer {
                        id: "choice".to_string(),
                        label: "Continue".to_string(),
                        value: SECRET.to_string(),
                    }],
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager
                .pending_user_inputs
                .lock()
                .get(&(thread.id.clone(), "input_detached".to_string()))
                .is_some_and(|entry| entry.settling)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("submission did not claim the pending request")?;
    submission.abort();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            harness.recv_user_input_submission()
        )
        .await
        .is_err(),
        "answer reached the engine before its receipt append was released"
    );
    drop(emit_guard);

    let (_, delivered) =
        tokio::time::timeout(Duration::from_secs(2), harness.recv_user_input_submission())
            .await
            .context("detached settlement did not reach the engine")?
            .context("detached settlement was canceled")?;
    assert_eq!(delivered.answers[0].value, SECRET);
    let detail = manager.get_thread_detail(&thread.id).await?;
    assert!(detail.pending_user_inputs.is_empty());
    let events = manager.events_since(&thread.id, None)?;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event == "user_input.answered")
            .count(),
        1
    );
    assert!(!serde_json::to_string(&events)?.contains(SECRET));
    Ok(())
}

#[tokio::test]
async fn terminal_user_input_cancellation_is_durable_before_engine_delivery() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    manager.register_pending_user_input(
        &thread.id,
        PendingUserInputRequest {
            id: "input_terminal_order".to_string(),
            turn_id: "turn_terminal_order".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: Vec::new(),
            },
        },
    );

    let emit_guard = manager.event_emit.lock().await;
    let engine = harness.handle.clone();
    let settle_manager = manager.clone();
    let thread_id = thread.id.clone();
    let settlement = tokio::spawn(async move {
        settle_manager
            .settle_user_inputs_for_terminal_turn(&thread_id, "turn_terminal_order", Some(engine))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager
                .pending_user_inputs
                .lock()
                .get(&(thread.id.clone(), "input_terminal_order".to_string()))
                .is_some_and(|entry| entry.settling)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("terminal cancellation did not claim the request")?;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            harness.recv_user_input_submission()
        )
        .await
        .is_err(),
        "terminal cancellation reached the engine before durable append"
    );
    drop(emit_guard);
    settlement
        .await
        .context("terminal settlement task panicked")??;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), harness.recv_user_input_submission())
            .await
            .context("engine did not receive terminal cancellation")?
            .is_none(),
        "terminal cancellation delivered a submitted response"
    );
    let events = manager.events_since(&thread.id, None)?;
    let canceled = events
        .iter()
        .find(|event| {
            event.event == "user_input.canceled"
                && event.payload.get("input_id").and_then(Value::as_str)
                    == Some("input_terminal_order")
        })
        .context("missing terminal cancellation receipt")?;
    assert_eq!(canceled.payload["terminal"], true);
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_user_inputs
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn thread_detail_cursor_precedes_projection_reads_at_terminal_boundary() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "complete while the snapshot is paused".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    manager.set_snapshot_test_hook(hook_tx);
    let snapshot_manager = manager.clone();
    let snapshot_thread_id = thread.id.clone();
    let snapshot_task = tokio::spawn(async move {
        snapshot_manager
            .get_thread_detail(&snapshot_thread_id)
            .await
    });

    let point = tokio::time::timeout(Duration::from_secs(2), hook_rx.recv())
        .await
        .context("snapshot did not capture its replay cursor")?
        .context("snapshot test hook closed")?;
    assert_eq!(point.thread_id, thread.id);

    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "snapshot_terminal".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;

    assert!(
        tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if manager
                    .events_since(&thread.id, Some(point.latest_seq))?
                    .iter()
                    .any(|event| {
                        event.turn_id.as_deref() == Some(&turn.id)
                            && event.event == "turn.completed"
                    })
                {
                    break Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_err(),
        "terminal publication crossed a snapshot projection boundary"
    );
    point
        .resume
        .send(())
        .map_err(|_| anyhow!("snapshot dropped its resume barrier"))?;

    let detail = snapshot_task.await.context("snapshot task panicked")??;
    assert_eq!(detail.latest_seq, point.latest_seq);
    assert_eq!(
        detail
            .turns
            .iter()
            .find(|record| record.id == turn.id)
            .map(|record| record.status),
        Some(RuntimeTurnStatus::InProgress),
        "the paused snapshot must retain the pre-terminal projection"
    );

    let completed_event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(event) = manager
                .events_since(&thread.id, Some(detail.latest_seq))?
                .into_iter()
                .find(|event| {
                    event.turn_id.as_deref() == Some(&turn.id) && event.event == "turn.completed"
                })
            {
                break Ok::<_, anyhow::Error>(event);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("terminal event did not follow the released snapshot boundary")??;
    assert!(completed_event.seq > detail.latest_seq);
    assert!(
        manager
            .events_since(&thread.id, Some(detail.latest_seq))?
            .iter()
            .any(|event| event.seq == completed_event.seq),
        "the same terminal transition must remain replayable from the snapshot cursor"
    );
    Ok(())
}

#[tokio::test]
async fn thread_detail_does_not_reenter_recovery_while_projection_is_locked() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let turn_id = "turn_recovery_queued_during_snapshot";
    let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
    turn.ended_at = Some(Utc::now());
    turn.duration_ms = Some(1);
    manager.store.save_turn(&turn)?;
    {
        let _thread_mutation = manager.store.thread_mutation.lock();
        let mut persisted_thread = manager.store.load_thread(&thread.id)?;
        persisted_thread.latest_turn_id = Some(turn_id.to_string());
        manager.store.save_thread(&persisted_thread)?;
    }

    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    manager.set_snapshot_test_hook(hook_tx);
    let snapshot_manager = manager.clone();
    let snapshot_thread_id = thread.id.clone();
    let snapshot_task = tokio::spawn(async move {
        snapshot_manager
            .get_thread_detail(&snapshot_thread_id)
            .await
    });
    let point = tokio::time::timeout(Duration::from_secs(2), hook_rx.recv())
        .await
        .context("snapshot did not acquire its projection boundary")?
        .context("snapshot test hook closed")?;

    // Queue after get_thread_detail's initial recovery flush, while its
    // projection lock is held. A nested get_thread call would see this receipt
    // and deadlock trying to reacquire that same lock.
    manager.queue_recovery_receipt(RecoveredTurnReceipt {
        turn: turn.clone(),
        unresolved_dynamic_tools: Vec::new(),
    });
    point
        .resume
        .send(())
        .map_err(|_| anyhow!("snapshot dropped its resume barrier"))?;
    let detail = tokio::time::timeout(Duration::from_secs(2), snapshot_task)
        .await
        .context("snapshot re-entered recovery while holding its projection lock")?
        .context("snapshot task panicked")??;
    assert_eq!(detail.thread.id, thread.id);
    assert_eq!(detail.latest_seq, point.latest_seq);
    assert!(manager.recovery_receipts.lock().contains_key(&thread.id));

    // The next top-level observation flushes the receipt after the prior
    // snapshot has released its projection boundary.
    manager.get_thread(&thread.id).await?;
    let completed = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| {
            event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].payload["recovered"], true);
    assert!(!manager.recovery_receipts.lock().contains_key(&thread.id));
    Ok(())
}

#[tokio::test]
async fn thread_detail_materializes_stream_prefixes_before_their_delta_cursor() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "snapshot both streamed prefixes".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "delta_snapshot".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "durable message prefix".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::ThinkingStarted { index: 1 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::ThinkingDelta {
            index: 1,
            content: "durable reasoning prefix".to_string(),
        })
        .await?;

    let deltas = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let deltas = manager
                .events_since(&thread.id, None)?
                .into_iter()
                .filter(|event| {
                    event.turn_id.as_deref() == Some(&turn.id) && event.event == "item.delta"
                })
                .collect::<Vec<_>>();
            if deltas.len() == 2 {
                break Ok::<_, anyhow::Error>(deltas);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("stream deltas were not durably sequenced")??;

    let detail = manager.get_thread_detail(&thread.id).await?;
    let latest_delta_seq = deltas.iter().map(|event| event.seq).max().unwrap_or(0);
    assert!(detail.latest_seq >= latest_delta_seq);
    let message = detail
        .items
        .iter()
        .find(|item| item.kind == TurnItemKind::AgentMessage)
        .context("snapshot omitted the streaming message item")?;
    let reasoning = detail
        .items
        .iter()
        .find(|item| item.kind == TurnItemKind::AgentReasoning)
        .context("snapshot omitted the streaming reasoning item")?;
    assert_eq!(message.status, TurnItemLifecycleStatus::InProgress);
    assert_eq!(message.detail.as_deref(), Some("durable message prefix"));
    assert_eq!(reasoning.status, TurnItemLifecycleStatus::InProgress);
    assert_eq!(
        reasoning.detail.as_deref(),
        Some("durable reasoning prefix")
    );
    assert_eq!(
        manager.store.load_item(&message.id)?.detail,
        message.detail,
        "message prefix must already be on disk before its delta cursor"
    );
    assert_eq!(
        manager.store.load_item(&reasoning.id)?.detail,
        reasoning.detail,
        "reasoning prefix must already be on disk before its delta cursor"
    );
    assert!(
        manager
            .events_since(&thread.id, Some(detail.latest_seq))?
            .iter()
            .all(|event| event.event != "item.delta"),
        "the snapshot itself must carry every delta at or before latest_seq"
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Interrupted,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn thread_detail_delta_boundary_is_replay_idempotent() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "pause a snapshot across streamed deltas".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "delta_boundary".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;

    let started = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(event) = manager
                .events_since(&thread.id, None)?
                .into_iter()
                .find(|event| {
                    event.turn_id.as_deref() == Some(&turn.id)
                        && event.event == "item.started"
                        && event.payload.pointer("/item/kind").and_then(Value::as_str)
                            == Some("agent_message")
                })
            {
                break Ok::<_, anyhow::Error>(event);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("message item did not start")??;
    let item_id = started.item_id.context("started event omitted item id")?;

    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    manager.set_snapshot_test_hook(hook_tx);
    let snapshot_manager = manager.clone();
    let snapshot_thread_id = thread.id.clone();
    let snapshot_task = tokio::spawn(async move {
        snapshot_manager
            .get_thread_detail(&snapshot_thread_id)
            .await
    });
    let point = tokio::time::timeout(Duration::from_secs(2), hook_rx.recv())
        .await
        .context("snapshot did not capture its replay cursor")?
        .context("snapshot test hook closed")?;
    assert!(point.latest_seq >= started.seq);

    for content in ["A", "B"] {
        harness
            .tx_event
            .send(EngineEvent::MessageDelta {
                index: 0,
                content: content.to_string(),
            })
            .await?;
    }
    sleep(STREAM_DELTA_BATCH_MAX_LATENCY + Duration::from_millis(50)).await;
    assert!(
        manager
            .events_since(&thread.id, Some(point.latest_seq))?
            .iter()
            .all(|event| event.event != "item.delta"),
        "a delta must not publish while the snapshot holds its projection boundary"
    );

    point
        .resume
        .send(())
        .map_err(|_| anyhow!("snapshot dropped its resume barrier"))?;
    let detail = snapshot_task.await.context("snapshot task panicked")??;
    let snapshotted = detail
        .items
        .iter()
        .find(|item| item.id == item_id)
        .context("snapshot omitted the streaming item")?;
    assert_eq!(snapshotted.detail.as_deref(), Some(""));

    let delta = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(event) = manager
                .events_since(&thread.id, Some(detail.latest_seq))?
                .into_iter()
                .find(|event| {
                    event.item_id.as_deref() == Some(&item_id) && event.event == "item.delta"
                })
            {
                break Ok::<_, anyhow::Error>(event);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("batched delta did not publish after snapshot release")??;
    assert_eq!(
        delta.payload.get("delta").and_then(Value::as_str),
        Some("AB")
    );
    assert_eq!(
        manager.store.load_item(&item_id)?.detail.as_deref(),
        Some("AB")
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Interrupted,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn terminal_turn_cancels_pending_user_input_and_clears_snapshot() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs input before completion".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::UserInputRequired {
            id: "input_terminal".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: vec![crate::tools::user_input::UserInputQuestion {
                    header: "Continue".to_string(),
                    id: "continue".to_string(),
                    question: "Continue?".to_string(),
                    options: vec![crate::tools::user_input::UserInputOption {
                        label: "Yes".to_string(),
                        description: "Continue now".to_string(),
                    }],
                    allow_free_text: false,
                    multi_select: false,
                }],
            },
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_user_inputs
            .is_empty()
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!("pending user input did not reach the canonical snapshot");
        }
        sleep(Duration::from_millis(20)).await;
    }

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let canceled = tokio::time::timeout(
        Duration::from_secs(2),
        harness.recv_user_input_cancellation(),
    )
    .await
    .expect("terminal user-input cancellation timed out");
    assert_eq!(canceled.as_deref(), Some("input_terminal"));

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let detail = manager.get_thread_detail(&thread.id).await?;
        if detail.pending_user_inputs.is_empty()
            && manager.events_since(&thread.id, None)?.iter().any(|event| {
                event.event == "user_input.canceled"
                    && event.turn_id.as_deref() == Some(turn.id.as_str())
                    && event.payload.get("input_id").and_then(Value::as_str)
                        == Some("input_terminal")
                    && event.payload.get("terminal").and_then(Value::as_bool) == Some(true)
            })
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!(
                "terminal user input was not cleared from the snapshot with a cancellation event"
            );
        }
        sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_result_settles_snapshot_and_emits_one_safe_resolution() -> Result<()> {
    use crate::tools::spec::DynamicToolExecutor;

    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "run an external lookup".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "dynamic_result".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;

    const RESULT_SECRET: &str = "dynamic-result-secret";
    let executor = manager.clone();
    let executor_thread_id = thread.id.clone();
    let execution = tokio::spawn(async move {
        DynamicToolExecutor::execute_dynamic_tool(
            &executor,
            Some(executor_thread_id),
            Some("bench".to_string()),
            "lookup".to_string(),
            json!({ "record_id": "record-7" }),
        )
        .await
    });

    let pending = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let detail = manager.get_thread_detail(&thread.id).await?;
            if let Some(call) = detail.pending_dynamic_tool_calls.first() {
                break Ok::<_, anyhow::Error>((detail.latest_seq, call.clone()));
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("dynamic tool call did not reach the canonical snapshot")??;
    let (snapshot_seq, call) = pending;
    assert_eq!(call.thread_id, thread.id);
    assert_eq!(call.turn_id, turn.id);
    assert_eq!(call.namespace.as_deref(), Some("bench"));
    assert_eq!(call.tool, "lookup");
    assert_eq!(call.arguments["record_id"], "record-7");
    let requested = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .find(|event| {
            event.event == "tool_call.requested"
                && event.payload.get("call_id").and_then(Value::as_str)
                    == Some(call.call_id.as_str())
        })
        .context("dynamic tool request was not durable")?;
    assert!(requested.seq <= snapshot_seq);
    assert!(
        manager
            .events_since(&thread.id, Some(snapshot_seq))?
            .iter()
            .all(|event| event.event != "tool_call.requested"),
        "the pending call must be recoverable from the snapshot once replay starts at latest_seq"
    );

    assert!(
        manager
            .deliver_dynamic_tool_result(
                &thread.id,
                &turn.id,
                &call.call_id,
                DynamicToolCallResult {
                    success: true,
                    content: vec![DynamicToolCallContent::InputText {
                        text: RESULT_SECRET.to_string(),
                    }],
                },
            )
            .await?
    );
    let result = execution.await.context("dynamic tool task panicked")??;
    assert_eq!(
        result.content, RESULT_SECRET,
        "the model-facing result changed"
    );
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_dynamic_tool_calls
            .is_empty()
    );
    let resolved = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| {
            event.event == "tool_call.resolved"
                && event.payload.get("call_id").and_then(Value::as_str)
                    == Some(call.call_id.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].payload["status"], "resolved");
    assert_eq!(resolved[0].payload["success"], true);
    assert!(
        !serde_json::to_string(&resolved)?.contains(RESULT_SECRET),
        "terminal dynamic-tool lifecycle must not echo result content"
    );
    assert!(
        !manager
            .deliver_dynamic_tool_result(
                &thread.id,
                &turn.id,
                &call.call_id,
                DynamicToolCallResult {
                    success: true,
                    content: Vec::new(),
                },
            )
            .await?,
        "a duplicate result must not settle or emit twice"
    );
    assert_eq!(
        manager
            .events_since(&thread.id, None)?
            .iter()
            .filter(|event| event.event == "tool_call.resolved")
            .count(),
        1
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_result_receipt_outlives_canceled_delivery_future() -> Result<()> {
    use crate::tools::spec::DynamicToolExecutor;

    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "settle a result after its HTTP future disappears".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "dynamic_detached_settlement".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;

    const RESULT_SECRET: &str = "detached-dynamic-result-secret";
    let executor = manager.clone();
    let executor_thread_id = thread.id.clone();
    let execution = tokio::spawn(async move {
        DynamicToolExecutor::execute_dynamic_tool(
            &executor,
            Some(executor_thread_id),
            Some("bench".to_string()),
            "detached_lookup".to_string(),
            json!({ "record_id": "record-detached" }),
        )
        .await
    });

    let call = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(call) = manager
                .get_thread_detail(&thread.id)
                .await?
                .pending_dynamic_tool_calls
                .first()
                .cloned()
            {
                break Ok::<_, anyhow::Error>(call);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("dynamic tool call did not become pending")??;

    // Stall terminal publication, submit the result, and wait until that path
    // owns the call. Canceling the API future from this point must not cancel
    // its detached receipt task or wake the model before the receipt is durable.
    let emit_guard = manager.event_emit.lock().await;
    let delivery_manager = manager.clone();
    let delivery_thread_id = thread.id.clone();
    let delivery_turn_id = turn.id.clone();
    let delivery_call_id = call.call_id.clone();
    let delivery = tokio::spawn(async move {
        delivery_manager
            .deliver_dynamic_tool_result(
                &delivery_thread_id,
                &delivery_turn_id,
                &delivery_call_id,
                DynamicToolCallResult {
                    success: true,
                    content: vec![DynamicToolCallContent::InputText {
                        text: RESULT_SECRET.to_string(),
                    }],
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let settling = manager
                .pending_dynamic_tools
                .lock()
                .get(&call.call_id)
                .is_some_and(|entry| entry.sender.is_none());
            if settling {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("result delivery never claimed the pending call")?;
    assert!(
        !execution.is_finished(),
        "the model consumed the result before its terminal receipt could commit"
    );
    assert!(
        !manager
            .deliver_dynamic_tool_result(
                &thread.id,
                &turn.id,
                &call.call_id,
                DynamicToolCallResult {
                    success: false,
                    content: Vec::new(),
                },
            )
            .await?,
        "a duplicate result stole a call whose terminal receipt was settling"
    );
    delivery.abort();
    assert!(
        delivery
            .await
            .expect_err("delivery API future must be canceled")
            .is_cancelled()
    );
    assert!(
        !execution.is_finished(),
        "canceling the delivery future woke the model without a receipt"
    );

    drop(emit_guard);
    let model_result = tokio::time::timeout(Duration::from_secs(2), execution)
        .await
        .context("model did not receive the result after terminal publication")?
        .context("dynamic tool task panicked")??;
    assert_eq!(model_result.content, RESULT_SECRET);

    let detail = manager.get_thread_detail(&thread.id).await?;
    assert!(detail.pending_dynamic_tool_calls.is_empty());
    let terminal = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| {
            matches!(
                event.event.as_str(),
                "tool_call.resolved" | "tool_call.timeout" | "tool_call.canceled"
            ) && event.payload.get("call_id").and_then(Value::as_str) == Some(call.call_id.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].event, "tool_call.resolved");
    assert_eq!(terminal[0].payload["success"], true);
    assert!(
        !serde_json::to_string(&terminal)?.contains(RESULT_SECRET),
        "the terminal receipt exposed result content"
    );
    assert!(
        !manager
            .deliver_dynamic_tool_result(
                &thread.id,
                &turn.id,
                &call.call_id,
                DynamicToolCallResult {
                    success: true,
                    content: Vec::new(),
                },
            )
            .await?,
        "a duplicate retry settled an already terminal call"
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_result_acceptance_survives_receiver_close_before_append() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let receiver = manager.register_pending_dynamic_tool_for_test(
        &thread.id,
        "turn_closed_receiver",
        "call_closed_receiver",
    )?;
    let emit_guard = manager.event_emit.lock().await;
    let delivery_manager = manager.clone();
    let delivery_thread_id = thread.id.clone();
    let delivery = tokio::spawn(async move {
        delivery_manager
            .deliver_dynamic_tool_result(
                &delivery_thread_id,
                "turn_closed_receiver",
                "call_closed_receiver",
                DynamicToolCallResult {
                    success: true,
                    content: vec![DynamicToolCallContent::InputText {
                        text: "closed-receiver-secret".to_string(),
                    }],
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager
                .pending_dynamic_tools
                .lock()
                .get("call_closed_receiver")
                .is_some_and(|entry| entry.sender.is_none())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("result did not claim the call before append")?;
    drop(receiver);
    drop(emit_guard);

    assert!(
        delivery.await.context("delivery task panicked")??,
        "durably accepted result was reported as missing after receiver close"
    );
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_dynamic_tool_calls
            .is_empty()
    );
    let terminal = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| {
            event.payload.get("call_id").and_then(Value::as_str) == Some("call_closed_receiver")
                && matches!(
                    event.event.as_str(),
                    "tool_call.resolved" | "tool_call.timeout" | "tool_call.canceled"
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].event, "tool_call.resolved");
    assert_eq!(terminal[0].payload["result_accepted"], true);
    assert!(
        !serde_json::to_string(&terminal)?.contains("closed-receiver-secret"),
        "closed-receiver acceptance exposed result content"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn dynamic_tool_receipt_append_failure_rolls_back_for_retry() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut receiver = manager.register_pending_dynamic_tool_for_test(
        &thread.id,
        "turn_retry_after_append_failure",
        "call_retry_after_append_failure",
    )?;
    let claim = match manager.claim_pending_dynamic_tool(
        &thread.id,
        "turn_retry_after_append_failure",
        "call_retry_after_append_failure",
    ) {
        PendingDynamicToolClaim::Claimed(claim) => claim,
        PendingDynamicToolClaim::Settling(_)
        | PendingDynamicToolClaim::Indeterminate
        | PendingDynamicToolClaim::Missing => {
            bail!("failed to claim the append-failure fixture")
        }
    };

    // Replace this throwaway thread's event log with a symlink. Runtime store
    // hardening rejects the append deterministically, exercising settlement
    // rollback without relying on platform permission behavior.
    let events_path = manager.store.events_path(&thread.id)?;
    let backup_path = events_path.with_extension("jsonl.append-failure-backup");
    std::fs::rename(&events_path, &backup_path)?;
    std::os::unix::fs::symlink(&backup_path, &events_path)?;
    let ack = manager.spawn_dynamic_tool_settlement(
        claim,
        DynamicToolTerminalOutcome::Resolved(DynamicToolCallResult {
            success: true,
            content: vec![DynamicToolCallContent::InputText {
                text: "discarded-before-retry".to_string(),
            }],
        }),
    );
    let failed = RuntimeThreadManager::await_dynamic_tool_settlement(ack).await;
    std::fs::remove_file(&events_path)?;
    std::fs::rename(&backup_path, &events_path)?;
    assert!(
        failed.is_err(),
        "symlinked event append unexpectedly succeeded"
    );

    {
        let pending = manager.pending_dynamic_tools.lock();
        let entry = pending
            .get("call_retry_after_append_failure")
            .context("failed receipt append stranded or removed the pending call")?;
        assert!(
            entry
                .sender
                .as_ref()
                .is_some_and(|sender| !sender.is_closed()),
            "failed receipt append left a Settling entry without its sender"
        );
    }
    assert_eq!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_dynamic_tool_calls
            .len(),
        1,
        "rollback removed the snapshot-authoritative pending request"
    );

    assert!(
        manager
            .deliver_dynamic_tool_result(
                &thread.id,
                "turn_retry_after_append_failure",
                "call_retry_after_append_failure",
                DynamicToolCallResult {
                    success: true,
                    content: vec![DynamicToolCallContent::InputText {
                        text: "retry-result".to_string(),
                    }],
                },
            )
            .await?,
        "retry did not settle the restored call"
    );
    let delivered = tokio::time::timeout(Duration::from_secs(2), &mut receiver)
        .await
        .context("restored receiver was not woken by retry")??;
    assert_eq!(
        delivered.content,
        vec![DynamicToolCallContent::InputText {
            text: "retry-result".to_string(),
        }]
    );
    let resolved = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| {
            event.event == "tool_call.resolved"
                && event.payload.get("call_id").and_then(Value::as_str)
                    == Some("call_retry_after_append_failure")
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1);
    assert!(
        !serde_json::to_string(&resolved)?.contains("retry-result"),
        "retried terminal receipt exposed result content"
    );
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_post_write_failures_rollback_without_duplicate_receipts() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;

    for (index, fault) in [
        EventAppendTestFault::AfterFlush,
        EventAppendTestFault::AfterSync,
    ]
    .into_iter()
    .enumerate()
    {
        let turn_id = format!("turn_post_write_{index}");
        let call_id = format!("call_post_write_{index}");
        let result_text = format!("post-write-result-{index}");
        let mut receiver =
            manager.register_pending_dynamic_tool_for_test(&thread.id, &turn_id, &call_id)?;
        let fault_guard = EventAppendFaultGuard::arm(&thread.id, fault);
        let error = manager
            .deliver_dynamic_tool_result(
                &thread.id,
                &turn_id,
                &call_id,
                DynamicToolCallResult {
                    success: true,
                    content: vec![DynamicToolCallContent::InputText {
                        text: result_text.clone(),
                    }],
                },
            )
            .await
            .expect_err("injected post-write failure unexpectedly settled");
        drop(fault_guard);
        assert!(
            error.to_string().contains("rolled back"),
            "post-write failure was not classified retry-safe: {error}"
        );

        let failed_snapshot = manager.get_thread_detail(&thread.id).await?;
        assert!(
            failed_snapshot
                .pending_dynamic_tool_calls
                .iter()
                .any(|call| call.call_id == call_id),
            "rolled-back call disappeared from the canonical snapshot"
        );
        assert!(
            manager
                .events_since(&thread.id, Some(failed_snapshot.latest_seq))?
                .is_empty(),
            "failed append left a replay-visible terminal suffix"
        );
        assert!(
            manager.events_since(&thread.id, None)?.iter().all(|event| {
                event.payload.get("call_id").and_then(Value::as_str) != Some(call_id.as_str())
            }),
            "failed append left a visible terminal record before retry"
        );

        assert!(
            manager
                .deliver_dynamic_tool_result(
                    &thread.id,
                    &turn_id,
                    &call_id,
                    DynamicToolCallResult {
                        success: true,
                        content: vec![DynamicToolCallContent::InputText {
                            text: result_text.clone(),
                        }],
                    },
                )
                .await?,
            "retry did not durably accept the rolled-back result"
        );
        let delivered = tokio::time::timeout(Duration::from_secs(2), &mut receiver)
            .await
            .context("retried result did not reach its model receiver")??;
        assert_eq!(
            delivered.content,
            vec![DynamicToolCallContent::InputText {
                text: result_text.clone(),
            }]
        );

        let replay = manager.events_since(&thread.id, Some(failed_snapshot.latest_seq))?;
        let terminal = replay
            .iter()
            .filter(|event| {
                event.event == "tool_call.resolved"
                    && event.payload.get("call_id").and_then(Value::as_str)
                        == Some(call_id.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(terminal[0].seq > failed_snapshot.latest_seq);
        assert_eq!(terminal[0].payload["result_accepted"], true);
        assert!(
            !serde_json::to_string(&terminal)?.contains(&result_text),
            "retried terminal receipt exposed result content"
        );
        let settled_snapshot = manager.get_thread_detail(&thread.id).await?;
        assert!(
            settled_snapshot
                .pending_dynamic_tool_calls
                .iter()
                .all(|call| call.call_id != call_id)
        );
        assert!(settled_snapshot.latest_seq >= terminal[0].seq);
    }
    Ok(())
}

#[tokio::test]
async fn restart_recovers_terminal_turn_after_dynamic_receipt_append_failure() -> Result<()> {
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let turn_id = "turn_terminal_recovery";
    let call_id = "call_terminal_recovery";
    let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
    turn.ended_at = Some(Utc::now());
    turn.duration_ms = Some(1);
    manager.store.save_turn(&turn)?;
    {
        let _thread_mutation = manager.store.thread_mutation.lock();
        let mut persisted_thread = manager.store.load_thread(&thread.id)?;
        persisted_thread.latest_turn_id = Some(turn_id.to_string());
        manager.store.save_thread(&persisted_thread)?;
    }
    let params = DynamicToolCallParams {
        thread_id: thread.id.clone(),
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        namespace: Some("recovery".to_string()),
        tool: "recover_lookup".to_string(),
        arguments: json!({ "record": "recovery-only" }),
    };
    manager
        .emit_event_for_test(
            &thread.id,
            Some(turn_id),
            "tool_call.requested",
            json!(&params),
        )
        .await?;
    let receiver = manager.register_pending_dynamic_tool(params)?;

    let fault_guard = EventAppendFaultGuard::arm(&thread.id, EventAppendTestFault::AfterSync);
    let error = manager
        .deliver_dynamic_tool_result(
            &thread.id,
            turn_id,
            call_id,
            DynamicToolCallResult {
                success: true,
                content: vec![DynamicToolCallContent::InputText {
                    text: "never-committed-result".to_string(),
                }],
            },
        )
        .await
        .expect_err("injected terminal receipt append unexpectedly succeeded");
    drop(fault_guard);
    assert!(error.to_string().contains("rolled back"));
    assert!(manager.events_since(&thread.id, None)?.iter().all(|event| {
        event.event != "turn.completed"
            && !matches!(
                event.event.as_str(),
                "tool_call.resolved" | "tool_call.canceled" | "tool_call.timeout"
            )
    }));
    drop(receiver);
    drop(manager);

    let recovered = test_manager(data_dir.clone())?;
    // Opening is synchronous; the first async observation flushes queued
    // recovery receipts in terminal-call-before-turn order.
    let recovered_turn = recovered.get_thread(&thread.id).await?;
    assert_eq!(recovered_turn.latest_turn_id.as_deref(), Some(turn_id));
    let events = recovered.events_since(&thread.id, None)?;
    let canceled = events
        .iter()
        .filter(|event| {
            event.event == "tool_call.canceled"
                && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
        })
        .collect::<Vec<_>>();
    let completed = events
        .iter()
        .filter(|event| {
            event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].payload["reason"], "process_restart");
    assert_eq!(canceled[0].payload["recovered"], true);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].payload["recovered"], true);
    assert!(canceled[0].seq < completed[0].seq);
    assert_eq!(
        completed[0]
            .payload
            .pointer("/turn/status")
            .and_then(Value::as_str),
        Some("completed")
    );

    // Re-observation and a second manager restart both remain idempotent.
    recovered.get_thread(&thread.id).await?;
    assert_eq!(
        recovered
            .events_since(&thread.id, None)?
            .iter()
            .filter(|event| {
                event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
            })
            .count(),
        1
    );
    drop(recovered);
    let reopened = test_manager(data_dir)?;
    reopened.get_thread(&thread.id).await?;
    let reopened_events = reopened.events_since(&thread.id, None)?;
    assert_eq!(
        reopened_events
            .iter()
            .filter(|event| {
                event.event == "tool_call.canceled"
                    && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
            })
            .count(),
        1
    );
    assert_eq!(
        reopened_events
            .iter()
            .filter(|event| {
                event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
            })
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn restart_reconciles_unresolved_dynamic_call_after_existing_turn_completion() -> Result<()> {
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let turn_id = "turn_legacy_completed_request";
    let call_id = "call_legacy_completed_request";
    let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
    turn.ended_at = Some(Utc::now());
    turn.duration_ms = Some(1);
    manager.store.save_turn(&turn)?;
    let params = DynamicToolCallParams {
        thread_id: thread.id.clone(),
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        namespace: Some("legacy".to_string()),
        tool: "legacy_lookup".to_string(),
        arguments: json!({ "record": "persisted-before-terminal-receipts" }),
    };
    manager
        .emit_event_for_test(
            &thread.id,
            Some(turn_id),
            "tool_call.requested",
            json!(&params),
        )
        .await?;
    manager
        .emit_event_for_test(
            &thread.id,
            Some(turn_id),
            "turn.completed",
            json!({ "turn": &turn }),
        )
        .await?;
    drop(manager);

    let recovered = test_manager(data_dir)?;
    recovered.get_thread(&thread.id).await?;
    let events = recovered.events_since(&thread.id, None)?;
    let terminal_calls = events
        .iter()
        .filter(|event| {
            event.turn_id.as_deref() == Some(turn_id)
                && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
                && matches!(
                    event.event.as_str(),
                    "tool_call.resolved" | "tool_call.canceled" | "tool_call.timeout"
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_calls.len(), 1);
    assert_eq!(terminal_calls[0].event, "tool_call.canceled");
    assert_eq!(terminal_calls[0].payload["reason"], "process_restart");
    assert_eq!(terminal_calls[0].payload["recovered"], true);
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
            })
            .count(),
        1,
        "recovery duplicated an already durable turn completion"
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_recovery_dedupe_scans_emit_each_terminal_receipt_once() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let turn_id = "turn_concurrent_recovery_dedupe";
    let call_id = "call_concurrent_recovery_dedupe";
    let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
    turn.ended_at = Some(Utc::now());
    let params = DynamicToolCallParams {
        thread_id: thread.id.clone(),
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        namespace: Some("recovery".to_string()),
        tool: "dedupe_lookup".to_string(),
        arguments: json!({ "record": "same-terminal-receipt" }),
    };

    let (first_call, second_call) = tokio::join!(
        manager.emit_recovered_dynamic_cancellation_if_missing(&params),
        manager.emit_recovered_dynamic_cancellation_if_missing(&params),
    );
    assert_eq!(
        usize::from(first_call?) + usize::from(second_call?),
        1,
        "the event_emit boundary must linearize dynamic-terminal dedupe"
    );

    let (first_turn, second_turn) = tokio::join!(
        manager.emit_turn_completed_if_missing(&turn, true),
        manager.emit_turn_completed_if_missing(&turn, true),
    );
    assert_eq!(
        usize::from(first_turn?) + usize::from(second_turn?),
        1,
        "the event_emit boundary must linearize turn-completion dedupe"
    );

    let events = manager.events_since(&thread.id, None)?;
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event == "tool_call.canceled"
                    && event.turn_id.as_deref() == Some(turn_id)
                    && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
            })
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
            })
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn consecutive_dynamic_receipt_failures_queue_in_process_recovery() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let turn_id = "turn_consecutive_receipt_failures";
    let call_id = "call_consecutive_receipt_failures";
    let turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::InProgress);
    manager.store.save_turn(&turn)?;
    {
        let _thread_mutation = manager.store.thread_mutation.lock();
        let mut persisted_thread = manager.store.load_thread(&thread.id)?;
        persisted_thread.latest_turn_id = Some(turn_id.to_string());
        manager.store.save_thread(&persisted_thread)?;
    }
    let params = DynamicToolCallParams {
        thread_id: thread.id.clone(),
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        namespace: Some("recovery".to_string()),
        tool: "retry_lookup".to_string(),
        arguments: json!({ "record": "in-process" }),
    };
    let requested = manager
        .emit_event_for_test(
            &thread.id,
            Some(turn_id),
            "tool_call.requested",
            json!(&params),
        )
        .await?;
    let mut receiver = manager.register_pending_dynamic_tool(params)?;

    // The submitted result and the monitor's terminal cancellation both fail
    // after fsync, with each JSONL line transactionally removed. The monitor
    // must retain an in-process recovery path instead of evicting the engine
    // with an Awaiting call and no future owner.
    let fault_guard =
        EventAppendFaultGuard::arm_repeated(&thread.id, EventAppendTestFault::AfterSync, 2);
    let result_error = manager
        .deliver_dynamic_tool_result(
            &thread.id,
            turn_id,
            call_id,
            DynamicToolCallResult {
                success: true,
                content: vec![DynamicToolCallContent::InputText {
                    text: "rolled-back-result".to_string(),
                }],
            },
        )
        .await
        .expect_err("first injected receipt failure unexpectedly succeeded");
    assert!(result_error.to_string().contains("rolled back"));
    manager
        .settle_claimed_turn_failure(&thread.id, turn_id, "forced monitor failure")
        .await;
    drop(fault_guard);

    assert!(
        manager
            .recovery_receipts
            .lock()
            .get(&thread.id)
            .is_some_and(|receipts| receipts.iter().any(|receipt| receipt.turn.id == turn_id)),
        "second retry-safe failure did not queue in-process recovery"
    );
    assert_eq!(manager.pending_dynamic_tools_count(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut receiver)
            .await
            .is_err(),
        "failed cancellation unexpectedly closed the model receiver"
    );
    assert!(manager.events_since(&thread.id, None)?.iter().all(|event| {
        event.event != "turn.completed"
            && !matches!(
                event.event.as_str(),
                "tool_call.resolved" | "tool_call.canceled" | "tool_call.timeout"
            )
    }));

    // The next async observation owns the queued retry. It durably cancels
    // the call before publishing exactly one recovered turn completion.
    manager.get_thread(&thread.id).await?;
    let closed = tokio::time::timeout(Duration::from_secs(2), &mut receiver)
        .await
        .context("recovery did not wake the model receiver")?;
    assert!(closed.is_err(), "terminal recovery delivered a tool result");
    let events = manager.events_since(&thread.id, None)?;
    let canceled = events
        .iter()
        .filter(|event| {
            event.event == "tool_call.canceled"
                && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
        })
        .collect::<Vec<_>>();
    let completed = events
        .iter()
        .filter(|event| {
            event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].payload["reason"], "turn_terminal");
    assert_eq!(canceled[0].payload["terminal"], true);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].payload["recovered"], true);
    assert!(canceled[0].seq < completed[0].seq);
    assert!(
        canceled[0].seq > requested.seq.saturating_add(1),
        "rolled-back append sequence values were unexpectedly reused"
    );
    assert_eq!(manager.pending_dynamic_tools_count(), 0);
    assert!(!manager.recovery_receipts.lock().contains_key(&thread.id));

    manager.get_thread(&thread.id).await?;
    let replay = manager.events_since(&thread.id, None)?;
    assert_eq!(
        replay
            .iter()
            .filter(|event| {
                event.event == "tool_call.canceled"
                    && event.payload.get("call_id").and_then(Value::as_str) == Some(call_id)
            })
            .count(),
        1
    );
    assert_eq!(
        replay
            .iter()
            .filter(|event| {
                event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
            })
            .count(),
        1
    );
    Ok(())
}

#[test]
fn pending_dynamic_tool_registry_rejects_duplicates_and_is_bounded() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let mut receivers = Vec::with_capacity(MAX_PENDING_DYNAMIC_TOOL_CALLS);
    receivers.push(manager.register_pending_dynamic_tool_for_test(
        "thread-bound",
        "turn-bound",
        "call-0",
    )?);
    assert!(
        manager
            .register_pending_dynamic_tool_for_test("thread-bound", "turn-bound", "call-0",)
            .is_err(),
        "duplicate call IDs must not replace an existing result channel"
    );

    for index in 1..MAX_PENDING_DYNAMIC_TOOL_CALLS {
        receivers.push(manager.register_pending_dynamic_tool_for_test(
            "thread-bound",
            "turn-bound",
            &format!("call-{index}"),
        )?);
    }
    assert_eq!(
        manager.pending_dynamic_tools_count(),
        MAX_PENDING_DYNAMIC_TOOL_CALLS
    );
    let error = manager
        .register_pending_dynamic_tool_for_test("thread-bound", "turn-bound", "call-over-limit")
        .expect_err("pending dynamic tool registry exceeded its hard limit");
    assert!(
        error
            .to_string()
            .contains("pending dynamic tool call limit")
    );
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_timeout_clears_snapshot_and_emits_once() -> Result<()> {
    use crate::tools::spec::{DynamicToolExecutor, ToolError};

    let _timeout_guard = test_dynamic_tool_timeout_ms(25);
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "let an external lookup time out".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "dynamic_timeout".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;

    let error = DynamicToolExecutor::execute_dynamic_tool(
        &manager,
        Some(thread.id.clone()),
        None,
        "slow_lookup".to_string(),
        json!({ "marker": "request-only" }),
    )
    .await
    .expect_err("dynamic tool unexpectedly resolved");
    assert!(matches!(error, ToolError::Timeout { .. }));
    assert!(
        manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_dynamic_tool_calls
            .is_empty()
    );
    let timeout_events = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| event.event == "tool_call.timeout")
        .collect::<Vec<_>>();
    assert_eq!(timeout_events.len(), 1);
    assert_eq!(timeout_events[0].turn_id.as_deref(), Some(turn.id.as_str()));
    assert_eq!(timeout_events[0].payload["status"], "timeout");
    assert_eq!(timeout_events[0].payload["timeout_secs"], 0);
    assert!(timeout_events[0].payload.get("arguments").is_none());

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn terminal_turn_cancels_pending_dynamic_tool_exactly_once() -> Result<()> {
    use crate::tools::spec::DynamicToolExecutor;

    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "cancel an external lookup with the turn".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "dynamic_cancel".to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;

    let executor = manager.clone();
    let executor_thread_id = thread.id.clone();
    let execution = tokio::spawn(async move {
        DynamicToolExecutor::execute_dynamic_tool(
            &executor,
            Some(executor_thread_id),
            None,
            "cancel_lookup".to_string(),
            json!({ "id": "pending" }),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !manager
                .get_thread_detail(&thread.id)
                .await?
                .pending_dynamic_tool_calls
                .is_empty()
            {
                break Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("dynamic call did not become pending")??;

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Interrupted,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    execution
        .await
        .context("dynamic tool task panicked")?
        .expect_err("terminal turn unexpectedly resolved the dynamic tool");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let detail = manager.get_thread_detail(&thread.id).await?;
            let canceled = manager
                .events_since(&thread.id, None)?
                .into_iter()
                .filter(|event| event.event == "tool_call.canceled")
                .collect::<Vec<_>>();
            if detail.pending_dynamic_tool_calls.is_empty() && canceled.len() == 1 {
                assert_eq!(canceled[0].payload["status"], "canceled");
                assert_eq!(canceled[0].payload["terminal"], true);
                break Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("terminal dynamic call did not disappear exactly once")??;
    assert_eq!(
        turn.id,
        manager
            .get_thread(&thread.id)
            .await?
            .latest_turn_id
            .unwrap()
    );
    Ok(())
}

#[tokio::test]
async fn approval_required_external_deny_is_denied() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let _turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "key2".to_string(),
            approval_grouping_key: "key2".to_string(),
            id: "tool_external_deny".to_string(),
            tool_name: "exec_command".to_string(),
            description: "external deny".to_string(),
            input: serde_json::json!({}),
            intent_summary: None,
            approval_force_prompt: false,
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(manager.pending_approvals_count(), 1);

    assert!(manager.deliver_external_approval(
        "tool_external_deny",
        ExternalApprovalDecision::Deny { remember: false },
    ));
    assert_eq!(
        harness.recv_approval_event().await,
        Some(MockApprovalEvent::Denied {
            id: "tool_external_deny".to_string(),
        })
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn approval_timeout_denies_clears_ui_and_next_turn_can_start() -> Result<()> {
    let _timeout_guard = test_approval_timeout_ms(25);
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "timeout_key".to_string(),
            approval_grouping_key: "timeout_key".to_string(),
            id: "tool_timeout".to_string(),
            tool_name: "exec_command".to_string(),
            description: "external timeout".to_string(),
            input: serde_json::json!({}),
            intent_summary: None,
            approval_force_prompt: false,
        })
        .await?;

    let decision = tokio::time::timeout(Duration::from_secs(2), harness.recv_approval_event())
        .await
        .context("approval timeout should deny the engine")?;
    assert_eq!(
        decision,
        Some(MockApprovalEvent::Denied {
            id: "tool_timeout".to_string(),
        })
    );
    assert_eq!(manager.pending_approvals_count(), 0);

    let events = manager.events_since(&thread.id, None)?;
    assert!(
        events.iter().any(|event| {
            event.event == "approval.timeout"
                && event.payload.get("approval_id").and_then(Value::as_str) == Some("tool_timeout")
        }),
        "timeout event should be persisted"
    );
    assert!(
        events.iter().any(|event| {
            event.event == "approval.decided"
                && event.payload.get("approval_id").and_then(Value::as_str) == Some("tool_timeout")
                && event.payload.get("decision").and_then(Value::as_str) == Some("deny")
                && event.payload.get("timeout").and_then(Value::as_bool) == Some(true)
        }),
        "timeout should also emit approval.decided so clients can clear pending UI"
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);

    let _next = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "after timeout".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(
        matches!(harness.rx_op.recv().await, Some(Op::SendMessage { .. })),
        "thread should accept a fresh turn after approval timeout cleanup"
    );

    Ok(())
}

#[tokio::test]
async fn thinking_delta_emits_agent_reasoning_item() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(true),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let mut event_rx = manager.subscribe_events();
    let _turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "show your thinking".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::ThinkingStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::ThinkingDelta {
            index: 0,
            content: "Let me reason about this.".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::ThinkingComplete { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;

    // A busy or constrained runner can be quiet for more than one 200 ms poll
    // even though the engine is still making progress. Keep polling until the
    // actual deadline instead of treating the first quiet interval as failure.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut delta_seen = false;
    let mut completed_seen = false;
    while Instant::now() < deadline && (!delta_seen || !completed_seen) {
        match tokio::time::timeout(Duration::from_millis(200), event_rx.recv()).await {
            Ok(Ok(record)) => {
                if record.event == "item.delta"
                    && record.payload.get("kind").and_then(|v| v.as_str())
                        == Some("agent_reasoning")
                {
                    delta_seen = true;
                    assert_eq!(
                        record.payload.get("delta").and_then(|v| v.as_str()),
                        Some("Let me reason about this.")
                    );
                }
                if record.event == "item.completed"
                    && record
                        .payload
                        .get("item")
                        .and_then(|v| v.get("kind"))
                        .and_then(|v| v.as_str())
                        == Some("agent_reasoning")
                {
                    completed_seen = true;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    assert!(delta_seen, "expected item.delta with kind=agent_reasoning");
    assert!(
        completed_seen,
        "expected item.completed for the reasoning item"
    );
    Ok(())
}

#[tokio::test]
async fn deliver_external_approval_for_unknown_id_returns_false() {
    let manager = test_manager(test_runtime_dir()).expect("manager");
    assert!(!manager.deliver_external_approval(
        "no_such_approval",
        ExternalApprovalDecision::Allow { remember: false },
    ));
    assert_eq!(manager.pending_approvals_count(), 0);
}

#[tokio::test]
async fn approval_required_remember_flips_thread_auto_approve() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    assert!(!manager.store.load_thread(&thread.id)?.auto_approve);

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs approval".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "key3".to_string(),
            approval_grouping_key: "key3".to_string(),
            id: "tool_remember".to_string(),
            tool_name: "exec_command".to_string(),
            description: "remember=true".to_string(),
            input: serde_json::json!({}),
            intent_summary: None,
            approval_force_prompt: false,
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
        sleep(Duration::from_millis(20)).await;
    }
    assert!(manager.deliver_external_approval(
        "tool_remember",
        ExternalApprovalDecision::Allow { remember: true },
    ));
    let _ = harness.recv_approval_event().await;

    assert!(
        manager.store.load_thread(&thread.id)?.auto_approve,
        "remember=true should flip thread auto_approve"
    );
    assert_eq!(
        manager.active_turn_flags(&thread.id, &turn.id).await,
        Some((true, false)),
        "remember=true should update the active turn used by subsequent approvals"
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn elevation_required_with_stale_active_turn_is_denied() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: Some(true),
            auto_approve: Some(true),
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "needs elevation".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: Some(true),
                auto_approve: Some(true),
                ..Default::default()
            },
        )
        .await?;

    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    {
        let mut active = manager.active.lock().await;
        let state = active
            .engines
            .get_mut(&thread.id)
            .context("missing active thread state")?;
        state.active_turn = None;
    }

    harness
        .tx_event
        .send(EngineEvent::ElevationRequired {
            tool_id: "tool_stale_elevated".to_string(),
            tool_name: "exec_command".to_string(),
            command: None,
            denial_reason: "sandbox denied".to_string(),
            blocked_network: false,
            blocked_write: false,
        })
        .await?;

    assert_eq!(
        harness.recv_approval_event().await,
        Some(MockApprovalEvent::Denied {
            id: "tool_stale_elevated".to_string(),
        })
    );

    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            },
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;

    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn steer_turn_on_active_turn_records_item_and_event() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let mut rx_steer = harness.rx_steer;
    let tx_event = harness.tx_event;
    let (steer_seen_tx, steer_seen_rx) = oneshot::channel::<String>();
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_turn_steer".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            if let Some(steer) = rx_steer.recv().await {
                let _ = steer_seen_tx.send(steer);
            }
            let _ = tx_event
                .send(EngineEvent::MessageStarted { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageDelta {
                    index: 0,
                    content: "steered response".to_string(),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageComplete { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 8,
                        output_tokens: 9,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: None,
                    base_url: None,
                })
                .await;
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "initial".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;

    let steer_text = "add bullet list".to_string();
    let steered_turn = manager
        .steer_turn(
            &thread.id,
            &turn.id,
            SteerTurnRequest {
                prompt: steer_text.clone(),
            },
        )
        .await?;
    assert_eq!(steered_turn.steer_count, 1);
    let observed_steer = steer_seen_rx
        .await
        .context("driver did not receive steer")?;
    assert_eq!(observed_steer, steer_text);

    let final_turn = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(final_turn.status, RuntimeTurnStatus::Completed);
    assert_eq!(final_turn.steer_count, 1);

    let events = manager.events_since(&thread.id, None)?;
    assert!(events.iter().any(|ev| ev.event == "turn.steered"));
    assert!(events.iter().any(|ev| {
        ev.event == "item.completed"
            && ev
                .payload
                .get("item")
                .and_then(|item| item.get("detail"))
                .and_then(Value::as_str)
                == Some("add bullet list")
    }));
    Ok(())
}

#[tokio::test]
async fn steer_receipts_outlive_caller_cancellation_after_engine_acceptance() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let mut rx_steer = harness.rx_steer;
    let tx_event = harness.tx_event;

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "initial".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(rx_op.recv().await, Some(Op::SendMessage { .. })));

    // Hold publication after durable persistence and mailbox acceptance so the
    // API future can be cancelled while the detached receipt task is pending.
    let emit_guard = manager.event_emit.lock().await;
    let steer_manager = manager.clone();
    let thread_id = thread.id.clone();
    let turn_id = turn.id.clone();
    let steer_task = tokio::spawn(async move {
        steer_manager
            .steer_turn(
                &thread_id,
                &turn_id,
                SteerTurnRequest {
                    prompt: "keep the accepted steer".to_string(),
                },
            )
            .await
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx_steer.recv()).await?,
        Some("keep the accepted steer".to_string())
    );
    steer_task.abort();
    assert!(
        steer_task
            .await
            .expect_err("caller task must be cancelled")
            .is_cancelled()
    );
    drop(emit_guard);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let events = manager.events_since(&thread.id, None)?;
        let steered = events.iter().any(|event| event.event == "turn.steered");
        let completed = events.iter().any(|event| {
            event.event == "item.completed"
                && event
                    .payload
                    .get("item")
                    .and_then(|item| item.get("detail"))
                    .and_then(Value::as_str)
                    == Some("keep the accepted steer")
        });
        if steered && completed {
            break;
        }
        if Instant::now() >= deadline {
            bail!("detached steer receipts were not persisted after caller cancellation");
        }
        tokio::task::yield_now().await;
    }

    let persisted_turn = manager.store.load_turn(&turn.id)?;
    assert_eq!(persisted_turn.steer_count, 1);
    let items = manager.store.list_items_for_turn(&turn.id)?;
    let steer_item = items
        .iter()
        .find(|item| item.detail.as_deref() == Some("keep the accepted steer"))
        .context("accepted steer item must remain durable")?;
    assert!(persisted_turn.item_ids.contains(&steer_item.id));

    tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "accepted steer completed".to_string(),
        })
        .await?;
    tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn steer_rejects_a_terminal_durable_turn_without_dispatch_or_item() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let mut rx_steer = harness.rx_steer;
    let tx_event = harness.tx_event;

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "initial".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(rx_op.recv().await, Some(Op::SendMessage { .. })));
    let original_item_ids = turn.item_ids.clone();
    {
        let _turn_mutation = manager.store.turn_mutation.lock();
        let mut terminal = manager.store.load_turn(&turn.id)?;
        terminal.status = RuntimeTurnStatus::Completed;
        terminal.ended_at = Some(Utc::now());
        manager.store.save_turn(&terminal)?;
    }

    let error = manager
        .steer_turn(
            &thread.id,
            &turn.id,
            SteerTurnRequest {
                prompt: "must be rejected".to_string(),
            },
        )
        .await
        .expect_err("terminal turn must reject steering");
    assert!(error.to_string().contains("no longer in progress"));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rx_steer.recv())
            .await
            .is_err(),
        "rejected terminal steer must not reach the engine"
    );
    let persisted = manager.store.load_turn(&turn.id)?;
    assert_eq!(persisted.steer_count, 0);
    assert_eq!(persisted.item_ids, original_item_ids);
    assert_eq!(manager.store.list_items_for_turn(&turn.id)?.len(), 1);

    // Restore the synthetic record and let the real monitor settle normally.
    {
        let _turn_mutation = manager.store.turn_mutation.lock();
        let mut active = manager.store.load_turn(&turn.id)?;
        active.status = RuntimeTurnStatus::InProgress;
        active.ended_at = None;
        manager.store.save_turn(&active)?;
    }
    tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "terminal rejection test completed".to_string(),
        })
        .await?;
    tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn concurrent_event_publication_keeps_live_and_durable_sequence_order() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut live_rx = manager.subscribe_events();

    let mut emitters = Vec::new();
    for index in 0..24_u64 {
        let emitter = manager.clone();
        let thread_id = thread.id.clone();
        emitters.push(tokio::spawn(async move {
            emitter
                .emit_event(
                    &thread_id,
                    None,
                    None,
                    "test.concurrent",
                    json!({ "index": index }),
                )
                .await
        }));
    }
    for emitter in emitters {
        emitter.await??;
    }

    let mut live = Vec::new();
    for _ in 0..24 {
        live.push(tokio::time::timeout(Duration::from_secs(2), live_rx.recv()).await??);
    }
    assert!(live.windows(2).all(|pair| pair[0].seq < pair[1].seq));

    let durable: Vec<_> = manager
        .events_since(&thread.id, None)?
        .into_iter()
        .filter(|event| event.event == "test.concurrent")
        .collect();
    assert_eq!(durable.len(), 24);
    assert_eq!(
        live.iter()
            .map(|event| (event.seq, event.payload.clone()))
            .collect::<Vec<_>>(),
        durable
            .iter()
            .map(|event| (event.seq, event.payload.clone()))
            .collect::<Vec<_>>(),
        "broadcast order must exactly match append order"
    );
    Ok(())
}

#[tokio::test]
async fn closed_engine_event_stream_fails_turn_items_and_evicts_engine() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "engine stream will close".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(rx_op.recv().await, Some(Op::SendMessage { .. })));
    drop(tx_event);

    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Failed);
    let terminal_error = terminal.error.as_deref().unwrap_or_default();
    assert!(
        terminal.error.as_deref().is_some_and(|error| {
            error.contains("Failed to monitor") || error.contains("without producing any output")
        }),
        "unexpected terminal error: {terminal_error:?}"
    );
    assert!(
        manager
            .store
            .list_items_for_turn(&turn.id)?
            .iter()
            .all(|item| !matches!(
                item.status,
                TurnItemLifecycleStatus::Queued | TurnItemLifecycleStatus::InProgress
            ))
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !manager.active.lock().await.engines.contains_key(&thread.id) {
            break;
        }
        if Instant::now() >= deadline {
            bail!("failed engine was not evicted");
        }
        tokio::task::yield_now().await;
    }
    assert!(matches!(rx_op.recv().await, Some(Op::Shutdown)));
    Ok(())
}

#[tokio::test]
async fn failed_turn_cancels_pending_user_input_and_clears_snapshot() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "input required, then the engine stream closes".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::UserInputRequired {
            id: "input_failed_turn".to_string(),
            request: crate::tools::user_input::UserInputRequest {
                questions: vec![crate::tools::user_input::UserInputQuestion {
                    header: "Continue".to_string(),
                    id: "continue".to_string(),
                    question: "Continue?".to_string(),
                    options: vec![crate::tools::user_input::UserInputOption {
                        label: "Yes".to_string(),
                        description: "Continue now".to_string(),
                    }],
                    allow_free_text: false,
                    multi_select: false,
                }],
            },
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !manager
            .get_thread_detail(&thread.id)
            .await?
            .pending_user_inputs
            .is_empty()
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!("pending user input did not reach the canonical snapshot");
        }
        sleep(Duration::from_millis(20)).await;
    }

    // Fail the turn mid-prompt: closing the event stream settles the turn as
    // failed through the monitor-failure path.
    harness.close_event_stream();

    let canceled = tokio::time::timeout(
        Duration::from_secs(2),
        harness.recv_user_input_cancellation(),
    )
    .await
    .expect("failure-path user-input cancellation timed out");
    assert_eq!(canceled.as_deref(), Some("input_failed_turn"));

    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Failed);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let detail = manager.get_thread_detail(&thread.id).await?;
        if detail.pending_user_inputs.is_empty()
            && manager.events_since(&thread.id, None)?.iter().any(|event| {
                event.event == "user_input.canceled"
                    && event.turn_id.as_deref() == Some(turn.id.as_str())
                    && event.payload.get("input_id").and_then(Value::as_str)
                        == Some("input_failed_turn")
                    && event.payload.get("terminal").and_then(Value::as_bool) == Some(true)
            })
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!("failed turn left a stale pending user input in the snapshot");
        }
        sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

#[tokio::test]
async fn compaction_lifecycle_emits_item_events_with_compaction_counts() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        let mut op_count = 0usize;
        while let Some(op) = rx_op.recv().await {
            match op {
                Op::SendMessage { .. } => {
                    op_count = op_count.saturating_add(1);
                    let _ = tx_event
                        .send(EngineEvent::TurnStarted {
                            turn_id: "engine_turn_auto".to_string(),
                            created_at: chrono::Utc::now(),
                            route: None,
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::CompactionStarted {
                            id: "auto_compact_1".to_string(),
                            auto: true,
                            message: "auto compact begin".to_string(),
                        })
                        .await;
                    let history_summary = format!(
                        "## 📋 {}\n\nkey facts.",
                        crate::compaction::COMPACTION_SUMMARY_MARKER
                    );
                    let _ = tx_event
                        .send(EngineEvent::SessionUpdated {
                            session_id: "runtime_compaction_session".to_string(),
                            messages: vec![crate::compaction::compaction_summary_message(
                                history_summary.clone(),
                                true,
                            )],
                            system_prompt: None,
                            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
                            workspace: PathBuf::from("."),
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::CompactionCompleted {
                            id: "auto_compact_1".to_string(),
                            auto: true,
                            message: "auto compact done".to_string(),
                            messages_before: Some(7),
                            messages_after: Some(3),
                            summary_prompt: Some(history_summary.clone()),
                        })
                        .await;
                    let goal_continuation = "Continue working toward the active goal.";
                    let _ = tx_event
                        .send(EngineEvent::SessionUpdated {
                            session_id: "runtime_compaction_session".to_string(),
                            messages: vec![
                                crate::compaction::compaction_summary_message(
                                    history_summary,
                                    true,
                                ),
                                Message {
                                    role: "assistant".to_string(),
                                    content: vec![ContentBlock::Text {
                                        text: "first post-compaction answer".to_string(),
                                        cache_control: None,
                                    }],
                                },
                                Message {
                                    role: "user".to_string(),
                                    content: vec![ContentBlock::Text {
                                        text: goal_continuation.to_string(),
                                        cache_control: None,
                                    }],
                                },
                                Message {
                                    role: "assistant".to_string(),
                                    content: vec![ContentBlock::Text {
                                        text: "second post-continuation answer".to_string(),
                                        cache_control: None,
                                    }],
                                },
                            ],
                            system_prompt: None,
                            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
                            workspace: PathBuf::from("."),
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::TurnComplete {
                            usage: Usage {
                                input_tokens: 3,
                                output_tokens: 3,
                                ..Usage::default()
                            },
                            status: TurnOutcomeStatus::Completed,
                            error: None,
                            tool_catalog: None,
                            base_url: None,
                        })
                        .await;
                }
                Op::CompactContext { .. } => {
                    op_count = op_count.saturating_add(1);
                    let _ = tx_event
                        .send(EngineEvent::CompactionStarted {
                            id: "manual_compact_1".to_string(),
                            auto: false,
                            message: "manual compact begin".to_string(),
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::CompactionCompleted {
                            id: "manual_compact_1".to_string(),
                            auto: false,
                            message: "manual compact done".to_string(),
                            messages_before: Some(5),
                            messages_after: Some(2),
                            summary_prompt: Some(
                                "## 📋 Conversation Summary (Auto-Generated)\n\nkey facts."
                                    .to_string(),
                            ),
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::TurnComplete {
                            usage: Usage {
                                input_tokens: 1,
                                output_tokens: 1,
                                ..Usage::default()
                            },
                            status: TurnOutcomeStatus::Completed,
                            error: None,
                            tool_catalog: None,
                            base_url: None,
                        })
                        .await;
                }
                _ => {}
            }
            if op_count >= 2 {
                break;
            }
        }
    });

    let auto_turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "trigger auto".to_string(),
                input_summary: None,
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                ..Default::default()
            },
        )
        .await?;
    let auto_turn = wait_for_terminal_turn(&manager, &auto_turn.id, Duration::from_secs(2)).await?;
    assert_eq!(auto_turn.status, RuntimeTurnStatus::Completed);

    let manual_turn = manager
        .compact_thread(
            &thread.id,
            CompactThreadRequest {
                reason: Some("manual request".to_string()),
            },
        )
        .await?;
    let manual_turn =
        wait_for_terminal_turn(&manager, &manual_turn.id, Duration::from_secs(2)).await?;
    assert_eq!(manual_turn.status, RuntimeTurnStatus::Completed);

    let events = manager.events_since(&thread.id, None)?;
    assert!(events.iter().any(|ev| {
        ev.event == "item.started"
            && ev
                .payload
                .get("item")
                .and_then(|item| item.get("kind"))
                .and_then(Value::as_str)
                == Some("context_compaction")
            && ev.payload.get("auto").and_then(Value::as_bool) == Some(true)
    }));
    assert!(events.iter().any(|ev| {
        ev.event == "item.completed"
            && ev
                .payload
                .get("item")
                .and_then(|item| item.get("kind"))
                .and_then(Value::as_str)
                == Some("context_compaction")
            && ev.payload.get("auto").and_then(Value::as_bool) == Some(true)
            && ev.payload.get("messages_before").and_then(Value::as_u64) == Some(7)
            && ev.payload.get("messages_after").and_then(Value::as_u64) == Some(3)
    }));
    assert!(events.iter().any(|ev| {
        ev.event == "item.completed"
            && ev
                .payload
                .get("item")
                .and_then(|item| item.get("kind"))
                .and_then(Value::as_str)
                == Some("context_compaction")
            && ev.payload.get("auto").and_then(Value::as_bool) == Some(false)
            && ev.payload.get("messages_before").and_then(Value::as_u64) == Some(5)
            && ev.payload.get("messages_after").and_then(Value::as_u64) == Some(2)
    }));

    // Compaction lifecycle metadata must never mutate the persisted system
    // prefix. Real summaries persist through SessionUpdated history instead.
    let record = manager.get_thread(&thread.id).await?;
    assert_eq!(record.system_prompt, None);
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    let reconstructed = manager.reconstruct_messages_from_turns(&turns)?;
    assert!(
        reconstructed.iter().any(|message| {
            crate::compaction::compaction_summary_text(message)
                .is_some_and(|text| text.contains("key facts."))
        }),
        "durable reconstruction must restore the summary as history"
    );
    assert_eq!(
        reconstructed
            .iter()
            .flat_map(|message| message.content.iter())
            .filter(|block| {
                matches!(block, ContentBlock::Text { text, .. } if text == "Continue working toward the active goal.")
            })
            .count(),
        1,
        "terminal compaction snapshot must retain the synthetic continuation exactly once"
    );
    assert!(reconstructed.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text, .. } if text == "second post-continuation answer")
        })
    }));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn restart_prefers_durable_compaction_snapshot_over_stale_linked_session() -> Result<()> {
    let _env_lock = lock_test_env();
    let owned_home = test_runtime_dir();
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
    let runtime_dir = owned_home.join("runtime");

    let provider_name = "runtime-restart-test";
    let config = Config {
        provider: Some(provider_name.to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom: std::collections::HashMap::from([(
                provider_name.to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("http://127.0.0.1:18191/v1".to_string()),
                    model: Some("restart-test-model".to_string()),
                    api_key: Some("local-test-key".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let manager =
        RuntimeThreadManager::open(config, PathBuf::from("."), test_manager_config(runtime_dir))?;

    let session_id = "stale_linked_session";
    let stale_message = Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "stale linked session history".to_string(),
            cache_control: None,
        }],
    };
    let sessions_dir = crate::session_manager::default_sessions_dir()?;
    let sessions = crate::session_manager::SessionManager::new(sessions_dir)?;
    let stale_session = crate::session_manager::create_saved_session_with_id_and_mode(
        session_id.to_string(),
        std::slice::from_ref(&stale_message),
        "restart-test-model",
        Path::new("."),
        0,
        None,
        Some("agent"),
    );
    sessions.save_session(&stale_session)?;

    let history_summary = crate::compaction::compaction_summary_message(
        format!(
            "## 📋 {}\n\ndurable compacted history",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    );
    let terminal_reply = Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "durable post-compaction reply".to_string(),
            cache_control: None,
        }],
    };
    let durable_messages = vec![history_summary, terminal_reply];

    let mut thread = sample_thread("thr_compaction_restart");
    thread.model = "restart-test-model".to_string();
    thread.model_provider = Some("custom".to_string());
    thread.model_provider_id = Some(provider_name.to_string());
    thread.session_id = Some(session_id.to_string());
    let mut turn = sample_turn(
        &thread.id,
        "turn_compaction_restart",
        RuntimeTurnStatus::Completed,
    );
    turn.item_ids = vec!["item_compaction_restart".to_string()];
    let item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: turn.item_ids[0].clone(),
        turn_id: turn.id.clone(),
        kind: TurnItemKind::ContextCompaction,
        status: TurnItemLifecycleStatus::Completed,
        summary: "Context compacted".to_string(),
        detail: None,
        metadata: Some(compaction_history_snapshot_metadata(
            &durable_messages,
            "turn_terminal",
            &HashSet::new(),
        )),
        artifact_refs: Vec::new(),
        started_at: turn.started_at,
        ended_at: turn.started_at,
    };
    manager.store.save_thread(&thread)?;
    manager.store.save_turn(&turn)?;
    manager.store.save_item(&item)?;

    let engine = manager.get_engine(&thread.id).await?;
    let (tx, rx) = oneshot::channel();
    engine
        .send(Op::GetSessionSnapshot {
            tx: Arc::new(std::sync::Mutex::new(Some(tx))),
        })
        .await?;
    let restored = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .context("engine snapshot timed out")?
        .context("engine snapshot sender dropped")?;

    assert_eq!(restored.messages, durable_messages);
    assert!(!restored.messages.contains(&stale_message));
    assert_eq!(
        restored
            .messages
            .iter()
            .filter(|message| crate::compaction::is_compaction_summary_message(message))
            .count(),
        1
    );
    engine.send(Op::Shutdown).await?;
    Ok(())
}

#[tokio::test]
async fn latest_compaction_checkpoint_is_private_and_supersedes_prior_checkpoint() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let first_messages = vec![crate::compaction::compaction_summary_message(
        format!(
            "## {}\n\nfirst checkpoint",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    )];
    let second_messages = vec![
        crate::compaction::compaction_summary_message(
            format!(
                "## {}\n\nsecond checkpoint",
                crate::compaction::COMPACTION_SUMMARY_MARKER
            ),
            true,
        ),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "latest tail".to_string(),
                cache_control: None,
            }],
        },
    ];

    let mut first_turn = sample_turn(
        &thread.id,
        "turn_first_checkpoint",
        RuntimeTurnStatus::Completed,
    );
    first_turn.item_ids = vec!["item_first_checkpoint".to_string()];
    manager.store.save_turn(&first_turn)?;
    let mut first_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: first_turn.item_ids[0].clone(),
        turn_id: first_turn.id.clone(),
        kind: TurnItemKind::ContextCompaction,
        status: TurnItemLifecycleStatus::Completed,
        summary: "first checkpoint".to_string(),
        detail: Some("first checkpoint".to_string()),
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: first_turn.started_at,
        ended_at: first_turn.ended_at,
    };
    manager
        .set_latest_compaction_history_snapshot(
            &thread.id,
            &mut first_item,
            &first_messages,
            "turn_terminal",
            &HashSet::new(),
        )
        .await?;
    let mut unsavable_replacement = TurnItemRecord {
        id: TEST_HISTORY_SNAPSHOT_SAVE_FAILURE_ITEM_ID.to_string(),
        ..first_item.clone()
    };
    assert!(
        manager
            .set_latest_compaction_history_snapshot(
                &thread.id,
                &mut unsavable_replacement,
                &second_messages,
                "turn_terminal",
                &HashSet::new(),
            )
            .await
            .is_err()
    );
    assert!(
        item_has_compaction_history_snapshot(&manager.store.load_item(&first_item.id)?),
        "a failed replacement save must not retire the last recoverable checkpoint"
    );

    let mut second_turn = sample_turn(
        &thread.id,
        "turn_second_checkpoint",
        RuntimeTurnStatus::Completed,
    );
    second_turn.created_at = first_turn.created_at + chrono::Duration::milliseconds(1);
    second_turn.item_ids = vec!["item_second_checkpoint".to_string()];
    manager.store.save_turn(&second_turn)?;
    let mut second_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: second_turn.item_ids[0].clone(),
        turn_id: second_turn.id.clone(),
        kind: TurnItemKind::ContextCompaction,
        status: TurnItemLifecycleStatus::Completed,
        summary: "second checkpoint".to_string(),
        detail: Some("second checkpoint".to_string()),
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: second_turn.started_at,
        ended_at: second_turn.ended_at,
    };
    manager
        .set_latest_compaction_history_snapshot(
            &thread.id,
            &mut second_item,
            &second_messages,
            "turn_terminal",
            &HashSet::new(),
        )
        .await?;

    assert!(!item_has_compaction_history_snapshot(
        &manager.store.load_item(&first_item.id)?
    ));
    assert!(item_has_compaction_history_snapshot(
        &manager.store.load_item(&second_item.id)?
    ));
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(
        manager.reconstruct_messages_from_turns(&turns)?,
        second_messages
    );

    let detail = manager.get_thread_detail(&thread.id).await?;
    assert!(
        detail
            .items
            .iter()
            .all(|item| !item_has_compaction_history_snapshot(item))
    );
    assert!(!serde_json::to_string(&detail)?.contains(HISTORY_SNAPSHOT_MESSAGES_KEY));
    Ok(())
}

#[tokio::test]
async fn checkpoint_retirement_scans_items_once_and_same_item_update_skips_scan() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let old_messages = vec![crate::compaction::compaction_summary_message(
        format!(
            "## {}\n\nold checkpoint",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    )];
    let mut old_checkpoint_id = String::new();

    for turn_index in 0..12 {
        let mut turn = sample_turn(
            &thread.id,
            &format!("turn_checkpoint_scan_{turn_index}"),
            RuntimeTurnStatus::Completed,
        );
        for item_index in 0..4 {
            let item_id = format!("item_checkpoint_scan_{turn_index}_{item_index}");
            let mut item = sample_item(&turn.id, &item_id, TurnItemLifecycleStatus::Completed);
            if turn_index == 0 && item_index == 0 {
                item.kind = TurnItemKind::ContextCompaction;
                item.metadata = Some(compaction_history_snapshot_metadata(
                    &old_messages,
                    "turn_terminal",
                    &HashSet::new(),
                ));
                old_checkpoint_id.clone_from(&item.id);
            }
            manager.store.save_item(&item)?;
            turn.item_ids.push(item.id);
        }
        manager.store.save_turn(&turn)?;
    }

    let mut replacement_turn = sample_turn(
        &thread.id,
        "turn_checkpoint_scan_replacement",
        RuntimeTurnStatus::Completed,
    );
    let mut replacement = sample_item(
        &replacement_turn.id,
        "item_checkpoint_scan_replacement",
        TurnItemLifecycleStatus::Completed,
    );
    replacement.kind = TurnItemKind::ContextCompaction;
    replacement_turn.item_ids.push(replacement.id.clone());
    manager.store.save_turn(&replacement_turn)?;
    manager.store.save_item(&replacement)?;

    let replacement_messages = vec![crate::compaction::compaction_summary_message(
        format!(
            "## {}\n\nreplacement checkpoint",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    )];
    manager.store.reset_item_directory_scan_count();
    manager
        .set_latest_compaction_history_snapshot(
            &thread.id,
            &mut replacement,
            &replacement_messages,
            "compaction_point",
            &HashSet::new(),
        )
        .await?;
    assert_eq!(
        manager.store.item_directory_scan_count(),
        1,
        "checkpoint replacement must scan the items directory once, independent of turn count"
    );
    assert!(!item_has_compaction_history_snapshot(
        &manager.store.load_item(&old_checkpoint_id)?
    ));

    let terminal_messages = vec![
        replacement_messages[0].clone(),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "terminal tail".to_string(),
                cache_control: None,
            }],
        },
    ];
    manager.store.reset_item_directory_scan_count();
    manager
        .set_latest_compaction_history_snapshot(
            &thread.id,
            &mut replacement,
            &terminal_messages,
            "turn_terminal",
            &HashSet::new(),
        )
        .await?;
    assert_eq!(
        manager.store.item_directory_scan_count(),
        0,
        "updating the current checkpoint at turn completion must not repeat retirement"
    );
    let stored = manager.store.load_item(&replacement.id)?;
    let stored_messages: Vec<Message> = serde_json::from_value(
        stored
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get(HISTORY_SNAPSHOT_MESSAGES_KEY))
            .cloned()
            .context("missing terminal checkpoint messages")?,
    )?;
    assert_eq!(stored_messages, terminal_messages);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn export_and_engine_load_scan_once_without_blocking_runtime_heartbeat() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut messages = Vec::new();
    for index in 0..12 {
        messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!("user turn {index}"),
                cache_control: None,
            }],
        });
        messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: format!("assistant turn {index}"),
                cache_control: None,
            }],
        });
    }
    manager
        .seed_thread_from_messages(&thread.id, &messages)
        .await?;

    manager.store.reset_item_directory_scan_count();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    manager
        .store
        .set_item_directory_scan_test_hook(entered_tx, resume_rx);
    let export_manager = manager.clone();
    let thread_id = thread.id.clone();
    let export = tokio::spawn(async move {
        export_manager
            .completed_thread_export_snapshot(&thread_id)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await?
        .context("export did not enter its item scan")?;

    // This test uses a single-thread Tokio runtime. The timer can fire only
    // when filesystem scanning is on spawn_blocking rather than this worker.
    tokio::time::timeout(Duration::from_millis(250), async {
        sleep(Duration::from_millis(10)).await;
    })
    .await
    .context("runtime heartbeat stalled behind export reconstruction")?;
    resume_tx
        .send(())
        .map_err(|_| anyhow!("export scan resume channel closed"))?;
    let snapshot = export.await.context("export task panicked")??;
    assert_eq!(snapshot.messages.len(), messages.len());
    for message in &messages {
        assert!(snapshot.messages.contains(message));
    }
    assert_eq!(
        manager.store.item_directory_scan_count(),
        1,
        "export must scan the item directory once, independent of turn count"
    );

    manager.store.reset_item_directory_scan_count();
    let (history_entered_tx, history_entered_rx) = oneshot::channel();
    let (history_resume_tx, history_resume_rx) = std::sync::mpsc::channel();
    manager
        .store
        .set_item_directory_scan_test_hook(history_entered_tx, history_resume_rx);
    let history_manager = manager.clone();
    let history_thread_id = thread.id.clone();
    let history = tokio::spawn(async move {
        history_manager
            .durable_thread_history(&history_thread_id)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), history_entered_rx)
        .await?
        .context("engine-load reconstruction did not enter its item scan")?;
    tokio::time::timeout(Duration::from_millis(250), async {
        sleep(Duration::from_millis(10)).await;
    })
    .await
    .context("runtime heartbeat stalled behind engine-load reconstruction")?;
    history_resume_tx
        .send(())
        .map_err(|_| anyhow!("engine-load scan resume channel closed"))?;
    let (_, reconstructed) = history.await.context("history task panicked")??;
    assert_eq!(reconstructed.len(), messages.len());
    assert_eq!(
        manager.store.item_directory_scan_count(),
        1,
        "engine-load reconstruction must scan once, independent of turn count"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn linked_session_load_does_not_block_runtime_heartbeat() -> Result<()> {
    let expected = vec![Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "linked session history".to_string(),
            cache_control: None,
        }],
    }];
    let returned = expected.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let load = tokio::spawn(load_linked_session_messages_with(
        "linked-session-heartbeat".to_string(),
        move |session_id| {
            assert_eq!(session_id, "linked-session-heartbeat");
            entered_tx
                .send(())
                .map_err(|_| anyhow!("linked-session load entry receiver closed"))?;
            resume_rx
                .recv()
                .map_err(|_| anyhow!("linked-session load resume sender closed"))?;
            Ok(returned)
        },
    ));
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await?
        .context("linked-session load did not enter its blocking worker")?;

    // A single-thread runtime can service this timer only when session-file
    // I/O is outside the async worker used by engine admission.
    tokio::time::timeout(Duration::from_millis(250), async {
        sleep(Duration::from_millis(10)).await;
    })
    .await
    .context("runtime heartbeat stalled behind linked-session loading")?;
    resume_tx
        .send(())
        .map_err(|_| anyhow!("linked-session load resume receiver closed"))?;
    assert_eq!(
        load.await.context("linked-session load task panicked")??,
        expected
    );
    Ok(())
}

#[tokio::test]
async fn completed_export_conflicts_when_turn_starts_after_prevalidation() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    manager
        .seed_thread_from_messages(
            &thread.id,
            &[Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "completed history".to_string(),
                    cache_control: None,
                }],
            }],
        )
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    manager.set_export_snapshot_test_hook(hook_tx);
    let export_manager = manager.clone();
    let export_thread_id = thread.id.clone();
    let export = tokio::spawn(async move {
        export_manager
            .completed_thread_export_snapshot(&export_thread_id)
            .await
    });
    let point = tokio::time::timeout(Duration::from_secs(2), hook_rx.recv())
        .await?
        .context("export did not reach post-validation barrier")?;
    assert_eq!(point.thread_id, thread.id);

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "race the export".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    point
        .resume
        .send(())
        .map_err(|_| anyhow!("export barrier receiver closed"))?;
    let error = export
        .await
        .context("export task panicked")?
        .expect_err("export admitted a newly active turn")
        .to_string();
    assert!(error.contains("already has an active turn"), "{error}");

    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: turn.id.clone(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    Ok(())
}

#[tokio::test]
async fn seeded_runtime_history_reconstructs_when_linked_session_is_missing() -> Result<()> {
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "runtime-seed-test".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18192/v1".to_string()),
            model: Some("runtime-seed-model".to_string()),
            ..Default::default()
        },
    );
    let manager = RuntimeThreadManager::open(
        Config {
            provider: Some("runtime-seed-test".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("runtime-seed-model".to_string()),
            model_provider: Some("runtime-seed-test".to_string()),
            ..Default::default()
        })
        .await?;
    let messages = vec![
        crate::compaction::compaction_summary_message(
            format!(
                "## {}\n\nseeded compacted history",
                crate::compaction::COMPACTION_SUMMARY_MARKER
            ),
            true,
        ),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "post-compaction tail".to_string(),
                cache_control: None,
            }],
        },
    ];
    manager
        .seed_thread_from_messages(&thread.id, &messages)
        .await?;
    manager
        .set_thread_session_id(&thread.id, "session_file_does_not_exist")
        .await?;

    assert_eq!(
        manager.messages_for_session_export(&thread.id).await?,
        messages
    );
    let engine = manager.get_engine(&thread.id).await?;
    let restored = engine.get_session_snapshot().await?;
    assert_eq!(restored.messages, messages);
    engine.send(Op::Shutdown).await?;
    Ok(())
}

#[test]
fn compaction_history_snapshot_redacts_request_user_input_answers() {
    let secret = "free-text-secret-that-must-not-persist";
    let short_secret = "123456";
    let pre_compaction = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "input-call".to_string(),
                name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
                input: json!({"question": "Continue?"}),
                caller: None,
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "input-call".to_string(),
                content: json!({
                    "answers": [
                        {
                            "id": "continue",
                            "label": "Other",
                            "value": secret,
                        },
                        {
                            "id": "pin",
                            "label": "Other",
                            "value": short_secret,
                        }
                    ]
                })
                .to_string(),
                is_error: None,
                content_blocks: None,
            }],
        },
    ];
    let mut sensitive_values = HashSet::new();
    collect_sensitive_user_input_values(&pre_compaction, &mut sensitive_values);
    let compacted = vec![
        crate::compaction::compaction_summary_message(
            format!(
                "## {}\n\nThe user answered {secret} with PIN {short_secret}.",
                crate::compaction::COMPACTION_SUMMARY_MARKER
            ),
            true,
        ),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: format!("ordinary durable context echoed {secret}"),
                cache_control: None,
            }],
        },
    ];

    let metadata =
        compaction_history_snapshot_metadata(&compacted, "turn_terminal", &sensitive_values);
    let serialized = serde_json::to_string(&metadata).expect("serialize snapshot metadata");
    assert!(!serialized.contains(secret));
    assert!(!serialized.contains(short_secret));
    assert!(serialized.contains("[redacted user input]"));
    assert!(serialized.contains("ordinary durable context"));
}

#[test]
fn typed_user_input_provenance_redacts_short_free_text_but_keeps_long_fixed_option() {
    let short_secret = "123456";
    let unknown_id_secret = "654321";
    let fixed_option = "All product surfaces";
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "typed-input".to_string(),
                name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
                input: json!({
                    "questions": [
                        {
                            "header": "PIN",
                            "id": "pin",
                            "question": "Enter the PIN",
                            "options": [
                                {"label": "Skip", "description": "Do not enter it"},
                                {"label": "Cancel", "description": "Stop here"}
                            ],
                            "allow_free_text": true
                        },
                        {
                            "header": "Scope",
                            "id": "scope",
                            "question": "Choose scope",
                            "options": [
                                {"label": fixed_option, "description": "Use every surface"},
                                {"label": "TUI only", "description": "Use only the terminal"}
                            ]
                        }
                    ]
                }),
                caller: None,
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "typed-input".to_string(),
                content: json!({
                    "answers": [
                        {"id": "pin", "label": "Other", "value": short_secret},
                        {"id": "scope", "label": "Scope", "value": fixed_option},
                        {"id": "unknown", "label": unknown_id_secret, "value": unknown_id_secret}
                    ]
                })
                .to_string(),
                is_error: None,
                content_blocks: None,
            }],
        },
    ];

    let mut sensitive_values = HashSet::new();
    collect_sensitive_user_input_values(&messages, &mut sensitive_values);
    assert!(sensitive_values.contains(short_secret));
    assert!(sensitive_values.contains(unknown_id_secret));
    assert!(!sensitive_values.contains(fixed_option));
}

#[test]
fn durable_history_clone_redacts_every_structured_string_leaf() -> Result<()> {
    const SECRET: &str = "structured-secret-493812";
    let mut sensitive_values = HashSet::new();
    sensitive_values.insert(SECRET.to_string());
    let messages = vec![
        crate::compaction::compaction_summary_message(
            format!(
                "## {}\n\nSummary echoed {SECRET}",
                crate::compaction::COMPACTION_SUMMARY_MARKER
            ),
            true,
        ),
        Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: format!("text {SECRET}"),
                    cache_control: None,
                },
                ContentBlock::ImageUrl {
                    image_url: crate::models::ImageUrlContent {
                        url: format!("https://example.invalid/{SECRET}"),
                    },
                },
                ContentBlock::Thinking {
                    thinking: format!("signed {SECRET}"),
                    signature: Some("provider-signature".to_string()),
                },
                ContentBlock::ToolUse {
                    id: "tool-structural-id".to_string(),
                    name: "exec_shell".to_string(),
                    input: json!({"nested": [{"command": format!("echo {SECRET}")}]}),
                    caller: None,
                },
                ContentBlock::ServerToolUse {
                    id: "server-structural-id".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": SECRET}),
                },
                ContentBlock::ToolSearchToolResult {
                    tool_use_id: "search-result-id".to_string(),
                    content: json!({"matches": [SECRET]}),
                },
                ContentBlock::CodeExecutionToolResult {
                    tool_use_id: "code-result-id".to_string(),
                    content: json!({"stdout": SECRET}),
                },
            ],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "ordinary-result-id".to_string(),
                content: format!("result {SECRET}"),
                is_error: None,
                content_blocks: Some(vec![json!({"deep": {"value": SECRET}})]),
            }],
        },
    ];

    let redacted = redacted_durable_history_clone(&messages, &sensitive_values);
    let serialized = serde_json::to_string(&redacted)?;
    assert!(!serialized.contains(SECRET));
    assert!(serialized.contains("tool-structural-id"));
    assert!(serialized.contains("exec_shell"));
    assert!(serialized.contains(crate::compaction::COMPACTION_SUMMARY_MARKER));
    assert!(matches!(
        &redacted[1].content[2],
        ContentBlock::Thinking { thinking, signature }
            if thinking == "[redacted user input]" && signature.is_none()
    ));
    Ok(())
}

#[tokio::test]
async fn seeded_runtime_history_redacts_private_and_public_resume_projections() -> Result<()> {
    const SECRET: &str = "resume-private-answer-481516";
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let messages = vec![
        crate::compaction::compaction_summary_message(
            format!(
                "## {}\n\nEarlier answer was {SECRET}",
                crate::compaction::COMPACTION_SUMMARY_MARKER
            ),
            true,
        ),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "resume-input".to_string(),
                name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
                input: json!({
                    "questions": [{
                        "header": "Secret",
                        "id": "secret",
                        "question": "Enter it",
                        "options": [
                            {"label": "Skip", "description": "Skip"},
                            {"label": "Cancel", "description": "Cancel"}
                        ],
                        "allow_free_text": true
                    }]
                }),
                caller: None,
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "resume-input".to_string(),
                content: serde_json::to_string(&json!({
                    "answers": [{"id": "secret", "label": "Other", "value": SECRET}]
                }))?,
                is_error: None,
                content_blocks: None,
            }],
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "echo-tool".to_string(),
                name: "exec_shell".to_string(),
                input: json!({"command": format!("echo {SECRET}")}),
                caller: None,
            }],
        },
    ];

    manager
        .seed_thread_from_messages(&thread.id, &messages)
        .await?;
    let detail = manager.get_thread_detail(&thread.id).await?;
    let events = manager.events_since(&thread.id, None)?;
    let exported = manager.messages_for_session_export(&thread.id).await?;
    assert!(!serde_json::to_string(&detail)?.contains(SECRET));
    assert!(!serde_json::to_string(&events)?.contains(SECRET));
    assert!(!serde_json::to_string(&exported)?.contains(SECRET));

    for directory in ["threads", "turns", "items", "events"] {
        let path = data_dir.join(directory);
        if !path.exists() {
            continue;
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                assert!(
                    !std::fs::read_to_string(entry.path())?.contains(SECRET),
                    "raw answer persisted in {}",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_sensitive_session_resume_uses_redacted_checkpoint_through_recompaction()
-> Result<()> {
    const SECRET: &str = "ordinary-resume-secret-317905";
    let _env_lock = lock_test_env();
    let owned_home = test_runtime_dir();
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
    let runtime_dir = owned_home.join("runtime");
    let provider_name = "ordinary-sensitive-resume";
    let config = Config {
        provider: Some(provider_name.to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom: std::collections::HashMap::from([(
                provider_name.to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("http://127.0.0.1:18193/v1".to_string()),
                    model: Some("ordinary-resume-model".to_string()),
                    api_key: Some("local-test-key".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let manager = RuntimeThreadManager::open(
        config.clone(),
        PathBuf::from("."),
        test_manager_config(runtime_dir.clone()),
    )?;
    let raw_messages = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "ordinary-resume-input".to_string(),
                name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
                input: json!({
                    "questions": [{
                        "header": "Credential",
                        "id": "credential",
                        "question": "Enter the credential",
                        "options": [
                            {"label": "Skip", "description": "Skip it"},
                            {"label": "Cancel", "description": "Cancel"}
                        ],
                        "allow_free_text": true
                    }]
                }),
                caller: None,
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "ordinary-resume-input".to_string(),
                content: json!({
                    "answers": [{
                        "id": "credential",
                        "label": "Other",
                        "value": SECRET
                    }]
                })
                .to_string(),
                is_error: None,
                content_blocks: None,
            }],
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "ordinary-echo".to_string(),
                name: "exec_shell".to_string(),
                input: json!({"command": format!("echo {SECRET}")}),
                caller: None,
            }],
        },
    ];
    let session_id = "ordinary_sensitive_session";
    let sessions = crate::session_manager::SessionManager::new(
        crate::session_manager::default_sessions_dir()?,
    )?;
    let raw_session = crate::session_manager::create_saved_session_with_id_and_mode(
        session_id.to_string(),
        &raw_messages,
        "ordinary-resume-model",
        Path::new("."),
        0,
        None,
        Some("agent"),
    );
    sessions.save_session(&raw_session)?;

    let thread = manager
        .create_thread(CreateThreadRequest {
            model: Some("ordinary-resume-model".to_string()),
            model_provider: Some("custom".to_string()),
            model_provider_id: Some(provider_name.to_string()),
            ..Default::default()
        })
        .await?;
    manager
        .seed_thread_from_messages(&thread.id, &raw_messages)
        .await?;
    manager
        .set_thread_session_id(&thread.id, session_id)
        .await?;

    let seeded_detail = manager.get_thread_detail(&thread.id).await?;
    assert!(!serde_json::to_string(&seeded_detail)?.contains(SECRET));
    assert!(seeded_detail.items.iter().any(|item| {
        item.kind == TurnItemKind::ContextCompaction
            && item.summary == RESTORED_RUNTIME_HISTORY_RECEIPT
    }));
    let private_items = manager.store.list_items_for_turns_map(
        &seeded_detail
            .turns
            .iter()
            .map(|turn| turn.id.clone())
            .collect::<Vec<_>>(),
    )?;
    assert!(
        private_items
            .values()
            .flatten()
            .any(item_has_compaction_history_snapshot)
    );
    assert!(!serde_json::to_string(&private_items)?.contains(SECRET));

    // The linked session intentionally remains raw. Engine loading must still
    // choose the authoritative private checkpoint and never sync those bytes.
    let engine = manager.get_engine(&thread.id).await?;
    let restored = engine.get_session_snapshot().await?;
    assert!(!serde_json::to_string(&restored.messages)?.contains(SECRET));
    assert!(serde_json::to_string(&restored.messages)?.contains("[redacted user input]"));
    let removed = {
        let mut active = manager.active.lock().await;
        active.lru.retain(|id| id != &thread.id);
        active.engines.remove(&thread.id)
    }
    .context("loaded engine was not active")?;
    removed.engine.send(Op::Shutdown).await?;

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "recompact the restored history".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: turn.id.clone(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionStarted {
            id: "ordinary-recompaction".to_string(),
            auto: true,
            message: "ordinary recompaction".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::SessionUpdated {
            session_id: session_id.to_string(),
            messages: restored.messages,
            system_prompt: None,
            model: "ordinary-resume-model".to_string(),
            workspace: PathBuf::from("."),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionCompleted {
            id: "ordinary-recompaction".to_string(),
            auto: true,
            message: "ordinary recompaction complete".to_string(),
            messages_before: Some(3),
            messages_after: Some(1),
            summary_prompt: Some("summarize".to_string()),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;

    assert!(
        !serde_json::to_string(&manager.get_thread_detail(&thread.id).await?)?.contains(SECRET)
    );
    assert!(!serde_json::to_string(&manager.events_since(&thread.id, None)?)?.contains(SECRET));
    assert!(
        !serde_json::to_string(&manager.messages_for_session_export(&thread.id).await?)?
            .contains(SECRET)
    );
    manager.shutdown();
    drop(harness);
    drop(manager);

    let restarted = RuntimeThreadManager::open(
        config,
        PathBuf::from("."),
        test_manager_config(runtime_dir.clone()),
    )?;
    assert!(
        !serde_json::to_string(&restarted.get_thread_detail(&thread.id).await?)?.contains(SECRET)
    );
    assert!(
        !serde_json::to_string(&restarted.messages_for_session_export(&thread.id).await?)?
            .contains(SECRET)
    );
    for directory in ["threads", "turns", "items", "events"] {
        for entry in std::fs::read_dir(runtime_dir.join(directory))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                assert!(!std::fs::read_to_string(entry.path())?.contains(SECRET));
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn sensitive_answer_taint_survives_turns_recompaction_and_restart() -> Result<()> {
    const SECRET: &str = "482915";
    const UNKNOWN_ID_SECRET: &str = "739204";
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let request = crate::tools::user_input::UserInputRequest {
        questions: vec![crate::tools::user_input::UserInputQuestion {
            header: "PIN".to_string(),
            id: "pin".to_string(),
            question: "Enter the six-digit PIN".to_string(),
            options: vec![
                crate::tools::user_input::UserInputOption {
                    label: "Skip".to_string(),
                    description: "Continue without it".to_string(),
                },
                crate::tools::user_input::UserInputOption {
                    label: "Cancel".to_string(),
                    description: "Stop the operation".to_string(),
                },
            ],
            allow_free_text: true,
            multi_select: false,
        }],
    };

    let first = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "first turn".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: first.id.clone(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::UserInputRequired {
            id: "pin-input".to_string(),
            request: request.clone(),
        })
        .await?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if manager
                .get_thread_detail(&thread.id)
                .await
                .is_ok_and(|detail| !detail.pending_user_inputs.is_empty())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    manager
        .submit_user_input(
            &thread.id,
            "pin-input",
            crate::tools::user_input::UserInputResponse {
                answers: vec![
                    crate::tools::user_input::UserInputAnswer {
                        id: "pin".to_string(),
                        label: "Other".to_string(),
                        value: SECRET.to_string(),
                    },
                    crate::tools::user_input::UserInputAnswer {
                        id: "unknown-pin".to_string(),
                        label: UNKNOWN_ID_SECRET.to_string(),
                        value: UNKNOWN_ID_SECRET.to_string(),
                    },
                ],
            },
        )
        .await?;
    assert!(matches!(
        harness.recv_user_input_submission().await,
        Some((id, _)) if id == "pin-input"
    ));
    harness
        .tx_event
        .send(EngineEvent::CompactionStarted {
            id: "compact-first".to_string(),
            auto: true,
            message: "first compaction".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::SessionUpdated {
            session_id: "taint-session".to_string(),
            messages: vec![crate::compaction::compaction_summary_message(
                format!(
                    "## {}\n\nThe first summary retained PINs {SECRET} and {UNKNOWN_ID_SECRET}",
                    crate::compaction::COMPACTION_SUMMARY_MARKER
                ),
                true,
            )],
            system_prompt: None,
            model: DEFAULT_TEXT_MODEL.to_string(),
            workspace: PathBuf::from("."),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionCompleted {
            id: "compact-first".to_string(),
            auto: true,
            message: "first compaction complete".to_string(),
            messages_before: Some(3),
            messages_after: Some(1),
            summary_prompt: Some("summarize".to_string()),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    wait_for_terminal_turn(&manager, &first.id, Duration::from_secs(2)).await?;

    let second = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "second turn".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: second.id.clone(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionStarted {
            id: "compact-second".to_string(),
            auto: true,
            message: "second compaction".to_string(),
        })
        .await?;
    // The raw request/result pair is already gone. Only the active engine's
    // session-lifetime taint can redact this later compaction.
    harness
        .tx_event
        .send(EngineEvent::SessionUpdated {
            session_id: "taint-session".to_string(),
            messages: vec![crate::compaction::compaction_summary_message(
                format!(
                    "## {}\n\nA later summary still remembered {SECRET} and {UNKNOWN_ID_SECRET}",
                    crate::compaction::COMPACTION_SUMMARY_MARKER
                ),
                true,
            )],
            system_prompt: None,
            model: DEFAULT_TEXT_MODEL.to_string(),
            workspace: PathBuf::from("."),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionCompleted {
            id: "compact-second".to_string(),
            auto: true,
            message: "second compaction complete".to_string(),
            messages_before: Some(2),
            messages_after: Some(1),
            summary_prompt: Some("summarize".to_string()),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    wait_for_terminal_turn(&manager, &second.id, Duration::from_secs(2)).await?;

    let detail = manager.get_thread_detail(&thread.id).await?;
    let events = manager.events_since(&thread.id, None)?;
    let exported = manager.messages_for_session_export(&thread.id).await?;
    assert!(!serde_json::to_string(&detail)?.contains(SECRET));
    assert!(!serde_json::to_string(&detail)?.contains(UNKNOWN_ID_SECRET));
    assert!(!serde_json::to_string(&events)?.contains(SECRET));
    assert!(!serde_json::to_string(&events)?.contains(UNKNOWN_ID_SECRET));
    assert!(!serde_json::to_string(&exported)?.contains(SECRET));
    assert!(!serde_json::to_string(&exported)?.contains(UNKNOWN_ID_SECRET));
    for item in &detail.items {
        let private = manager.store.load_item(&item.id)?;
        assert!(!serde_json::to_string(&private)?.contains(SECRET));
        assert!(!serde_json::to_string(&private)?.contains(UNKNOWN_ID_SECRET));
    }

    manager.shutdown();
    drop(harness);
    drop(manager);
    let restarted = test_manager(data_dir.clone())?;
    let restarted_detail = restarted.get_thread_detail(&thread.id).await?;
    let restarted_export = restarted.messages_for_session_export(&thread.id).await?;
    assert!(!serde_json::to_string(&restarted_detail)?.contains(SECRET));
    assert!(!serde_json::to_string(&restarted_detail)?.contains(UNKNOWN_ID_SECRET));
    assert!(!serde_json::to_string(&restarted_export)?.contains(SECRET));
    assert!(!serde_json::to_string(&restarted_export)?.contains(UNKNOWN_ID_SECRET));
    for directory in ["threads", "turns", "items", "events"] {
        for entry in std::fs::read_dir(data_dir.join(directory))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                assert!(!std::fs::read_to_string(entry.path())?.contains(SECRET));
                assert!(!std::fs::read_to_string(entry.path())?.contains(UNKNOWN_ID_SECRET));
            }
        }
    }
    Ok(())
}

#[test]
fn summary_redaction_cannot_damage_runtime_ownership_sentinel() {
    let sentinel_fragment = "codewhale:compaction_summary";
    let mut sensitive_values = HashSet::new();
    sensitive_values.insert(sentinel_fragment.to_string());
    let messages = vec![crate::compaction::compaction_summary_message(
        format!(
            "## {}\n\nThe answer echoed {sentinel_fragment} in the generated body.",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    )];

    let projected = project_messages_for_durable_history_snapshot(&messages, &sensitive_values);

    assert!(crate::compaction::is_compaction_summary_message(
        &projected[0]
    ));
    let payload = crate::compaction::compaction_summary_text(&projected[0])
        .expect("summary ownership survives redaction");
    assert!(payload.contains("[redacted user input]"));
}

#[test]
fn modal_ids_labels_and_common_short_answers_do_not_corrupt_summary_words() {
    let pre_compaction = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "short-answer".to_string(),
                name: REQUEST_USER_INPUT_TOOL_NAME.to_string(),
                input: json!({}),
                caller: None,
            }],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "short-answer".to_string(),
                content: json!({
                    "answers": [
                        {"id": "scope", "label": "Continue", "value": "Continue"},
                        {"id": "yes", "label": "Other", "value": "yes"},
                        {"id": "letter", "label": "Other", "value": "a"},
                        {"id": "scope-again", "label": "Other", "value": "scope"}
                    ]
                })
                .to_string(),
                is_error: None,
                content_blocks: None,
            }],
        },
    ];
    let mut sensitive_values = HashSet::new();
    collect_sensitive_user_input_values(&pre_compaction, &mut sensitive_values);
    let compacted = vec![crate::compaction::compaction_summary_message(
        format!(
            "## {}\n\nContinue with yes for scope a; the user chose a path and retained data.",
            crate::compaction::COMPACTION_SUMMARY_MARKER
        ),
        true,
    )];

    let metadata =
        compaction_history_snapshot_metadata(&compacted, "turn_terminal", &sensitive_values);
    let serialized = serde_json::to_string(&metadata).expect("serialize snapshot metadata");
    assert!(serialized.contains("Continue with yes for scope a"));
    assert!(serialized.contains("chose a path and retained data"));
    assert!(!serialized.contains("[redacted user input]"));
}

#[test]
fn signed_thinking_with_modal_answer_is_dropped_with_its_signature() {
    let secret = "signed-secret-answer";
    let mut sensitive_values = HashSet::new();
    sensitive_values.insert(secret.to_string());
    let messages = vec![Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Thinking {
            thinking: format!("Reasoning includes {secret}"),
            signature: Some("provider-signature".to_string()),
        }],
    }];

    let projected = project_messages_for_durable_history_snapshot(&messages, &sensitive_values);
    assert!(matches!(
        &projected[0].content[0],
        ContentBlock::Thinking { thinking, signature }
            if thinking == "[redacted user input]" && signature.is_none()
    ));
}

#[tokio::test]
async fn failed_compaction_persists_partial_history_repair() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            workspace: None,
            ..Default::default()
        })
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        while let Some(op) = rx_op.recv().await {
            if matches!(op, Op::SendMessage { .. }) {
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: "failed_compaction_turn".to_string(),
                        created_at: chrono::Utc::now(),
                        route: None,
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::CompactionStarted {
                        id: "failed_compaction".to_string(),
                        auto: true,
                        message: "emergency compact begin".to_string(),
                    })
                    .await;
                let partial_summary = crate::compaction::compaction_summary_message(
                    format!(
                        "## {}\n\npartial repair retained",
                        crate::compaction::COMPACTION_SUMMARY_MARKER
                    ),
                    true,
                );
                let _ = tx_event
                    .send(EngineEvent::SessionUpdated {
                        session_id: "failed_compaction_session".to_string(),
                        messages: vec![partial_summary],
                        system_prompt: None,
                        model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
                        workspace: PathBuf::from("."),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::CompactionFailed {
                        id: "failed_compaction".to_string(),
                        auto: true,
                        message: "still above budget".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage::default(),
                        status: TurnOutcomeStatus::Failed,
                        error: Some("still above budget".to_string()),
                        tool_catalog: None,
                        base_url: None,
                    })
                    .await;
                break;
            }
        }
    });

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "trigger failed recovery".to_string(),
                ..Default::default()
            },
        )
        .await?;
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Failed);

    let events = manager.events_since(&thread.id, None)?;
    let failed_event = events
        .iter()
        .find(|event| {
            event.event == "item.failed"
                && event
                    .payload
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("context_compaction")
        })
        .context("missing failed compaction event")?;
    assert!(
        failed_event
            .payload
            .get("item")
            .and_then(|item| item.get("metadata"))
            .is_none_or(Value::is_null),
        "public failed event must not duplicate the durable transcript snapshot"
    );

    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    let durable_compaction = manager
        .store
        .list_items_for_turn(&turn.id)?
        .into_iter()
        .find(|item| item.kind == TurnItemKind::ContextCompaction)
        .context("missing durable failed compaction item")?;
    assert_eq!(
        durable_compaction
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("history_snapshot_version"))
            .and_then(Value::as_u64),
        Some(1)
    );
    let reconstructed = manager.reconstruct_messages_from_turns(&turns)?;
    assert!(reconstructed.iter().any(|message| {
        crate::compaction::compaction_summary_text(message)
            .is_some_and(|text| text.contains("partial repair retained"))
    }));
    Ok(())
}

#[test]
fn summarize_text_truncates() {
    let out = summarize_text("abcdefghijklmnopqrstuvwxyz", 10);
    assert_eq!(out, "abcdefg...");
}

#[test]
fn approval_decision_requires_auto_approve_and_trust_for_full_access() {
    assert_eq!(
        RuntimeThreadManager::approval_decision(false, false, false),
        RuntimeApprovalDecision::DenyTool
    );
    assert_eq!(
        RuntimeThreadManager::approval_decision(false, true, false),
        RuntimeApprovalDecision::DenyTool
    );
    assert_eq!(
        RuntimeThreadManager::approval_decision(true, false, false),
        RuntimeApprovalDecision::ApproveTool
    );
    assert_eq!(
        RuntimeThreadManager::approval_decision(true, false, true),
        RuntimeApprovalDecision::DenyTool
    );
    assert_eq!(
        RuntimeThreadManager::approval_decision(true, true, true),
        RuntimeApprovalDecision::RetryWithFullAccess
    );
}

#[test]
fn opening_manager_recovers_stale_queued_and_in_progress_work() -> Result<()> {
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let started_at = Utc::now() - chrono::Duration::seconds(5);
    let created_at = started_at - chrono::Duration::seconds(1);

    let thread = ThreadRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "thr_restart".to_string(),
        created_at,
        updated_at: created_at,
        model: DEFAULT_TEXT_MODEL.to_string(),
        model_provider: None,
        model_provider_id: None,
        workspace: PathBuf::from("."),
        mode: "agent".to_string(),
        allow_shell: false,
        trust_mode: false,
        auto_approve: false,
        latest_turn_id: Some("turn_in_progress".to_string()),
        latest_response_bookmark: None,
        archived: false,
        system_prompt: None,
        task_id: None,
        title: None,
        session_id: None,
    };
    manager.store.save_thread(&thread)?;

    let completed_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_completed".to_string(),
        turn_id: "turn_in_progress".to_string(),
        kind: TurnItemKind::Status,
        status: TurnItemLifecycleStatus::Completed,
        summary: "done".to_string(),
        detail: None,
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(started_at),
        ended_at: Some(started_at + chrono::Duration::seconds(1)),
    };
    let in_progress_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_in_progress".to_string(),
        turn_id: "turn_in_progress".to_string(),
        kind: TurnItemKind::ToolCall,
        status: TurnItemLifecycleStatus::InProgress,
        summary: "running".to_string(),
        detail: None,
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(started_at),
        ended_at: None,
    };
    let queued_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_queued".to_string(),
        turn_id: "turn_queued".to_string(),
        kind: TurnItemKind::ToolCall,
        status: TurnItemLifecycleStatus::Queued,
        summary: "queued".to_string(),
        detail: None,
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: None,
        ended_at: None,
    };
    manager.store.save_item(&completed_item)?;
    manager.store.save_item(&in_progress_item)?;
    manager.store.save_item(&queued_item)?;

    manager.store.save_turn(&TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "turn_in_progress".to_string(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::InProgress,
        input_summary: "hello".to_string(),
        created_at,
        started_at: Some(started_at),
        ended_at: None,
        duration_ms: None,
        usage: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_model: None,
        error: None,
        item_ids: vec![completed_item.id.clone(), in_progress_item.id.clone()],
        steer_count: 0,
    })?;
    manager.store.save_turn(&TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "turn_queued".to_string(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::Queued,
        input_summary: "later".to_string(),
        created_at,
        started_at: None,
        ended_at: None,
        duration_ms: None,
        usage: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_model: None,
        error: None,
        item_ids: vec![queued_item.id.clone()],
        steer_count: 0,
    })?;
    drop(manager);

    let recovered = test_manager(data_dir)?;

    let recovered_thread = recovered.store.load_thread(&thread.id)?;
    assert!(recovered_thread.updated_at >= thread.updated_at);

    let recovered_in_progress_turn = recovered.store.load_turn("turn_in_progress")?;
    assert_eq!(
        recovered_in_progress_turn.status,
        RuntimeTurnStatus::Interrupted
    );
    assert_eq!(
        recovered_in_progress_turn.error.as_deref(),
        Some(RUNTIME_RESTART_REASON)
    );
    assert!(recovered_in_progress_turn.ended_at.is_some());
    assert!(
        recovered_in_progress_turn
            .duration_ms
            .is_some_and(|duration| duration >= 5_000)
    );

    let recovered_queued_turn = recovered.store.load_turn("turn_queued")?;
    assert_eq!(recovered_queued_turn.status, RuntimeTurnStatus::Interrupted);
    assert_eq!(
        recovered_queued_turn.error.as_deref(),
        Some(RUNTIME_RESTART_REASON)
    );
    assert!(recovered_queued_turn.ended_at.is_some());
    assert_eq!(recovered_queued_turn.duration_ms, None);

    assert_eq!(
        recovered.store.load_item(&completed_item.id)?.status,
        TurnItemLifecycleStatus::Completed
    );
    let recovered_in_progress_item = recovered.store.load_item(&in_progress_item.id)?;
    assert_eq!(
        recovered_in_progress_item.status,
        TurnItemLifecycleStatus::Interrupted
    );
    assert!(recovered_in_progress_item.ended_at.is_some());

    let recovered_queued_item = recovered.store.load_item(&queued_item.id)?;
    assert_eq!(
        recovered_queued_item.status,
        TurnItemLifecycleStatus::Interrupted
    );
    assert!(recovered_queued_item.ended_at.is_some());

    Ok(())
}

#[test]
fn parse_mode_defaults_to_agent() {
    assert_eq!(parse_mode("unknown"), AppMode::Agent);
    assert_eq!(parse_mode("plan"), AppMode::Plan);
}

#[test]
fn parse_mode_opt_resolves_explicit_tokens_and_aliases() {
    assert_eq!(parse_mode_opt("agent"), Some(AppMode::Agent));
    assert_eq!(parse_mode_opt("1"), Some(AppMode::Agent));
    assert_eq!(parse_mode_opt("plan"), Some(AppMode::Plan));
    assert_eq!(parse_mode_opt("2"), Some(AppMode::Plan));
    assert_eq!(parse_mode_opt("auto"), Some(AppMode::Agent));
    assert_eq!(parse_mode_opt("3"), None);
    assert_eq!(parse_mode_opt("yolo"), Some(AppMode::Yolo));
    assert_eq!(parse_mode_opt("4"), Some(AppMode::Yolo));
    assert_eq!(parse_mode_opt(" PLAN "), Some(AppMode::Plan));
}

#[test]
fn parse_mode_opt_rejects_prompt_fragments() {
    for input in [
        "plan a trip to Tokyo",
        "switch the agent on",
        "enter yolo mode",
        "agent of chaos",
        "mode",
    ] {
        assert_eq!(parse_mode_opt(input), None);
    }
}

#[test]
fn parse_mode_wrapper_defaults_and_resolves_numeric_aliases() {
    assert_eq!(parse_mode("plan a trip to Tokyo"), AppMode::Agent);
    assert_eq!(parse_mode("auto"), AppMode::Agent);
    assert_eq!(parse_mode("1"), AppMode::Agent);
    assert_eq!(parse_mode("2"), AppMode::Plan);
    assert_eq!(parse_mode("3"), AppMode::Agent);
    assert_eq!(parse_mode("4"), AppMode::Yolo);
}

fn rebind_event(event: &str, agent_id: &str, seq: u64) -> RuntimeEventRecord {
    RuntimeEventRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        seq,
        timestamp: Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: event.to_string(),
        payload: json!({ "agent_id": agent_id }),
    }
}

#[test]
fn collect_agent_rebind_hints_resumes_a_mid_fanout_session() {
    // Mirror what runtime_threads persists during a real fanout: three
    // workers spawned, two finished, one still running when the session
    // was killed. The TUI re-attach must rebuild placeholders for the
    // running worker AND the two completed workers (the fanout card
    // tracks all of them so the dot-grid stays accurate post-resume).
    let events = vec![
        rebind_event("agent.spawned", "agent_a", 1),
        rebind_event("agent.spawned", "agent_b", 2),
        rebind_event("agent.spawned", "agent_c", 3),
        rebind_event("agent.progress", "agent_a", 4),
        rebind_event("agent.completed", "agent_a", 5),
        rebind_event("agent.progress", "agent_b", 6),
        rebind_event("agent.completed", "agent_b", 7),
        rebind_event("agent.progress", "agent_c", 8),
    ];
    let hints = collect_agent_rebind_hints(&events);
    assert_eq!(hints.len(), 3, "every fanout worker must be rebound");
    let by_id: std::collections::BTreeMap<&str, AgentRebindStatus> = hints
        .iter()
        .map(|h| (h.agent_id.as_str(), h.status))
        .collect();
    assert_eq!(by_id.get("agent_a"), Some(&AgentRebindStatus::Completed));
    assert_eq!(by_id.get("agent_b"), Some(&AgentRebindStatus::Completed));
    assert_eq!(
        by_id.get("agent_c"),
        Some(&AgentRebindStatus::InProgress),
        "in-flight worker must rebind in InProgress, not downgrade"
    );
}

#[test]
fn collect_agent_rebind_hints_ignores_unrelated_events() {
    // Status / tool events should not produce phantom hints — only the
    // agent.* family carries the contract we re-bind from.
    let events = vec![
        RuntimeEventRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            seq: 1,
            timestamp: Utc::now(),
            thread_id: "thr".to_string(),
            turn_id: None,
            item_id: None,
            event: "tool.completed".to_string(),
            payload: json!({"name": "read_file"}),
        },
        rebind_event("agent.spawned", "agent_x", 2),
        RuntimeEventRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            seq: 3,
            timestamp: Utc::now(),
            thread_id: "thr".to_string(),
            turn_id: None,
            item_id: None,
            event: "compaction.completed".to_string(),
            payload: json!({"messages_after": 12}),
        },
    ];
    let hints = collect_agent_rebind_hints(&events);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].agent_id, "agent_x");
}

#[test]
fn collect_agent_rebind_hints_does_not_downgrade_completed_to_in_progress() {
    // Out-of-order replay: a stale `agent.progress` arriving after the
    // completed event must NOT clobber the terminal status. This matters
    // when an event log is concatenated from interrupted segments.
    let events = vec![
        rebind_event("agent.spawned", "agent_y", 1),
        rebind_event("agent.completed", "agent_y", 2),
        rebind_event("agent.progress", "agent_y", 3),
    ];
    let hints = collect_agent_rebind_hints(&events);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].status, AgentRebindStatus::Completed);
}

/// Helper for the `fork_at_user_message` tests: write a sequence of
/// (user, assistant) turns under the given thread id. Each turn gets
/// one UserMessage item carrying `user_text` in `detail` plus one
/// AgentMessage item. Turn `created_at` is monotonically increasing
/// so the chronological sort in `list_turns_for_thread` is stable.
fn seed_turns_with_user_messages(
    manager: &RuntimeThreadManager,
    thread_id: &str,
    user_texts: &[&str],
) -> Result<Vec<String>> {
    let mut turn_ids = Vec::new();
    let base = Utc::now();
    for (offset, text) in user_texts.iter().enumerate() {
        let created_at = base + chrono::Duration::milliseconds(offset as i64);
        let turn_id = format!("turn_test_{offset}");
        let user_item_id = format!("item_user_{offset}");
        let asst_item_id = format!("item_asst_{offset}");
        manager.store.save_item(&TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: user_item_id.clone(),
            turn_id: turn_id.clone(),
            kind: TurnItemKind::UserMessage,
            status: TurnItemLifecycleStatus::Completed,
            summary: (*text).to_string(),
            detail: Some((*text).to_string()),
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(created_at),
            ended_at: Some(created_at),
        })?;
        manager.store.save_item(&TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: asst_item_id.clone(),
            turn_id: turn_id.clone(),
            kind: TurnItemKind::AgentMessage,
            status: TurnItemLifecycleStatus::Completed,
            summary: format!("reply {offset}"),
            detail: Some(format!("reply {offset}")),
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(created_at),
            ended_at: Some(created_at),
        })?;
        manager.store.save_turn(&TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: turn_id.clone(),
            thread_id: thread_id.to_string(),
            status: RuntimeTurnStatus::Completed,
            input_summary: (*text).to_string(),
            created_at,
            started_at: Some(created_at),
            ended_at: Some(created_at),
            duration_ms: Some(0),
            usage: None,
            effective_provider: None,
            effective_provider_id: None,
            effective_billing_surface: None,
            effective_model: None,
            error: None,
            item_ids: vec![user_item_id, asst_item_id],
            steer_count: 0,
        })?;
        turn_ids.push(turn_id);
    }
    Ok(turn_ids)
}

#[tokio::test]
async fn fork_at_user_message_drops_tail_and_returns_user_text() -> Result<()> {
    // Seed three completed user/assistant turns. Backtracking with
    // depth=0 should drop only the most recent turn ("third") and
    // hand back its original text so the caller can refill the
    // composer.
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    seed_turns_with_user_messages(&manager, &thread.id, &["first", "second", "third"])?;

    let (forked, original_text) = manager.fork_at_user_message(&thread.id, 0).await?;
    assert_eq!(original_text.as_deref(), Some("third"));
    assert_ne!(forked.id, thread.id);

    let forked_turns = manager.store.list_turns_for_thread(&forked.id)?;
    assert_eq!(
        forked_turns.len(),
        2,
        "depth=0 should drop the most recent turn"
    );
    let summaries: Vec<&str> = forked_turns
        .iter()
        .map(|t| t.input_summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["first", "second"]);
    Ok(())
}

#[tokio::test]
async fn fork_at_user_message_depth_one_drops_two_turns() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    seed_turns_with_user_messages(&manager, &thread.id, &["a", "b", "c", "d"])?;

    let (forked, original_text) = manager.fork_at_user_message(&thread.id, 1).await?;
    assert_eq!(original_text.as_deref(), Some("c"));
    let forked_turns = manager.store.list_turns_for_thread(&forked.id)?;
    let summaries: Vec<&str> = forked_turns
        .iter()
        .map(|t| t.input_summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["a", "b"]);
    Ok(())
}

#[tokio::test]
async fn fork_at_user_message_out_of_range_errors() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    seed_turns_with_user_messages(&manager, &thread.id, &["only"])?;

    let err = manager.fork_at_user_message(&thread.id, 5).await.err();
    assert!(err.is_some(), "depth past the end should bail out");
    Ok(())
}

#[tokio::test]
async fn fork_at_user_message_does_not_mutate_source() -> Result<()> {
    // The source thread must be untouched: turns still present, items
    // still present, latest_turn_id still pointing at the original
    // tail. Backtrack creates a sibling, never edits in place.
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            ..Default::default()
        })
        .await?;
    let turn_ids = seed_turns_with_user_messages(&manager, &thread.id, &["x", "y", "z"])?;

    let _ = manager.fork_at_user_message(&thread.id, 0).await?;

    let source_turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(
        source_turns.len(),
        3,
        "source thread must still hold every turn after fork"
    );
    for tid in &turn_ids {
        assert!(
            manager.store.load_turn(tid).is_ok(),
            "turn {tid} must remain on disk"
        );
    }
    Ok(())
}
