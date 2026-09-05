use super::*;
use crate::core::engine::{MockApprovalEvent, mock_engine_handle};
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use codewhale_protocol::agent_mail::{AgentMailDeliveryMode, AgentMailSender, AgentMailStatus};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;
use uuid::Uuid;

fn test_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!("deepseek-runtime-threads-{}", Uuid::new_v4()))
}

const EVENT_PROCESS_ROLE_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_ROLE";
const EVENT_PROCESS_ROOT_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_ROOT";
const EVENT_PROCESS_THREAD_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_THREAD";
const EVENT_PROCESS_SIGNAL_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_SIGNAL";
const EVENT_PROCESS_START_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_START";
const EVENT_PROCESS_RELEASE_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_RELEASE";
const EVENT_PROCESS_WORKER_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_WORKER";
const EVENT_PROCESS_COUNT_ENV: &str = "CODEWHALE_TEST_EVENT_PROCESS_COUNT";
const EVENT_PROCESS_HELPER: &str = "runtime_threads::tests::runtime_event_process_child_helper";
// Deadlock ceiling for monitor settlement in the 10k-test lib binary. This is
// a test watchdog, not an expected runtime latency or a customer-facing SLO.
const TURN_SETTLEMENT_DEADLOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
#[ignore = "spawned by real cross-process Runtime event tests"]
fn runtime_event_process_child_helper() {
    let Ok(role) = std::env::var(EVENT_PROCESS_ROLE_ENV) else {
        return;
    };
    let root = PathBuf::from(
        std::env::var_os(EVENT_PROCESS_ROOT_ENV).expect("event child needs a store root"),
    );
    let thread_id = std::env::var(EVENT_PROCESS_THREAD_ENV).expect("event child needs a thread id");
    let signal = PathBuf::from(
        std::env::var_os(EVENT_PROCESS_SIGNAL_ENV).expect("event child needs a signal path"),
    );
    if role == "manager-racer" {
        let start = PathBuf::from(
            std::env::var_os(EVENT_PROCESS_START_ENV).expect("manager racer needs a start barrier"),
        );
        let release = PathBuf::from(
            std::env::var_os(EVENT_PROCESS_RELEASE_ENV)
                .expect("manager racer needs a release barrier"),
        );
        std::fs::write(&signal, b"ready").expect("announce manager racer readiness");
        wait_for_runtime_event_test_file(&start, "manager racer start barrier");
        match test_manager(root) {
            Ok(_manager) => {
                std::fs::write(&signal, b"won").expect("announce manager race winner");
                wait_for_runtime_event_test_file(&release, "manager racer release barrier");
            }
            Err(error) if format!("{error:#}").contains("already active in another process") => {
                std::fs::write(&signal, b"lost").expect("announce manager race loser");
            }
            Err(error) => panic!("unexpected manager race failure: {error:#}"),
        }
        return;
    }
    let store = RuntimeThreadStore::open(root.clone()).expect("event child opens Runtime store");

    match role.as_str() {
        "manager-holder" => {
            let start = PathBuf::from(
                std::env::var_os(EVENT_PROCESS_START_ENV)
                    .expect("manager holder needs a release barrier"),
            );
            drop(store);
            let _manager = test_manager(root).expect("manager child acquires process owner lock");
            std::fs::write(&signal, b"locked").expect("announce held manager owner lock");
            wait_for_runtime_event_test_file(&start, "manager owner release barrier");
        }
        "writer" => {
            let start = PathBuf::from(
                std::env::var_os(EVENT_PROCESS_START_ENV)
                    .expect("event writer needs a start barrier"),
            );
            let worker =
                std::env::var(EVENT_PROCESS_WORKER_ENV).expect("event writer needs a worker id");
            let count = std::env::var(EVENT_PROCESS_COUNT_ENV)
                .expect("event writer needs a count")
                .parse::<u64>()
                .expect("event writer count must be numeric");
            std::fs::write(&signal, b"ready").expect("announce ready event writer");
            wait_for_runtime_event_test_file(&start, "writer start barrier");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build event writer runtime");
            runtime.block_on(async {
                for index in 0..count {
                    store
                        .append_event(
                            &thread_id,
                            None,
                            None,
                            "process.event",
                            json!({ "worker": worker, "index": index }),
                        )
                        .await
                        .expect("append cross-process Runtime event");
                }
            });
        }
        "holder" => {
            store
                .with_event_transaction(Duration::from_secs(5), || {
                    std::fs::write(&signal, b"locked").expect("announce held event lock");
                    std::thread::sleep(Duration::from_secs(60));
                    Ok(())
                })
                .expect("hold Runtime event transaction");
        }
        "torn" => {
            store
                .with_event_transaction(Duration::from_secs(5), || {
                    let mut state = load_runtime_store_state(&store.state_path)?;
                    let seq = state.next_seq;
                    state.next_seq = seq.checked_add(1).context("reserve torn event sequence")?;
                    write_json_atomic(&store.state_path, &state)?;
                    let path = store.events_path(&thread_id)?;
                    let mut file =
                        open_runtime_store_file(&path, "torn Runtime event fixture", |options| {
                            options.create(true).append(true);
                        })?;
                    let record = RuntimeEventRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        seq,
                        timestamp: Utc::now(),
                        thread_id: thread_id.clone(),
                        turn_id: None,
                        item_id: None,
                        event: "torn.process.event".to_string(),
                        payload: json!({}),
                    };
                    file.write_all(&serde_json::to_vec(&record)?)?;
                    file.flush()?;
                    file.sync_all()?;
                    std::fs::write(&signal, b"torn").expect("announce torn event append");
                    std::thread::sleep(Duration::from_secs(60));
                    Ok(())
                })
                .expect("hold torn Runtime event transaction");
        }
        "phantom" => {
            store
                .with_event_transaction(Duration::from_secs(5), || {
                    let path = store.events_path(&thread_id)?;
                    let mut append = open_runtime_store_file(
                        &path,
                        "phantom Runtime event fixture",
                        |options| {
                            options.create(true).append(true);
                        },
                    )?;
                    let rollback = open_runtime_store_file(
                        &path,
                        "phantom Runtime event rollback",
                        |options| {
                            options.write(true);
                        },
                    )?;
                    validate_same_runtime_store_file_handles(&append, &rollback, &path)?;
                    let original_len = append.metadata()?.len();
                    serde_json::to_writer(
                        &mut append,
                        &RuntimeEventRecord {
                            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                            seq: 999,
                            timestamp: Utc::now(),
                            thread_id: thread_id.clone(),
                            turn_id: None,
                            item_id: None,
                            event: "phantom.process.event".to_string(),
                            payload: json!({}),
                        },
                    )?;
                    append.write_all(b"\n")?;
                    append.flush()?;
                    std::fs::write(&signal, b"visible").expect("announce rollback candidate");
                    std::thread::sleep(Duration::from_millis(300));
                    rollback_failed_event_append_handle(&rollback, original_len)
                })
                .expect("roll back phantom Runtime event");
        }
        other => panic!("unknown Runtime event child role {other}"),
    }
}

fn wait_for_runtime_event_test_file(path: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {label} at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_runtime_event_child(
    role: &str,
    root: &Path,
    thread_id: &str,
    signal: &Path,
) -> RuntimeEventChildGuard {
    let mut command = std::process::Command::new(
        std::env::current_exe().expect("current test executable for Runtime event child"),
    );
    command
        .arg(EVENT_PROCESS_HELPER)
        .args(["--exact", "--ignored", "--test-threads", "1"])
        .env(EVENT_PROCESS_ROLE_ENV, role)
        .env(EVENT_PROCESS_ROOT_ENV, root)
        .env(EVENT_PROCESS_THREAD_ENV, thread_id)
        .env(EVENT_PROCESS_SIGNAL_ENV, signal)
        .env(EVENT_PROCESS_START_ENV, root.join("process-writers.start"))
        .env(EVENT_PROCESS_WORKER_ENV, signal.as_os_str())
        .env(EVENT_PROCESS_COUNT_ENV, "8")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    RuntimeEventChildGuard::new(command.spawn().expect("spawn Runtime event child"))
}

fn spawn_runtime_manager_racer(
    root: &Path,
    signal: &Path,
    start: &Path,
    release: &Path,
) -> RuntimeEventChildGuard {
    let mut command = std::process::Command::new(
        std::env::current_exe().expect("current test executable for Runtime manager racer"),
    );
    command
        .arg(EVENT_PROCESS_HELPER)
        .args(["--exact", "--ignored", "--test-threads", "1"])
        .env(EVENT_PROCESS_ROLE_ENV, "manager-racer")
        .env(EVENT_PROCESS_ROOT_ENV, root)
        .env(EVENT_PROCESS_THREAD_ENV, "thr_unused")
        .env(EVENT_PROCESS_SIGNAL_ENV, signal)
        .env(EVENT_PROCESS_START_ENV, start)
        .env(EVENT_PROCESS_RELEASE_ENV, release)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    RuntimeEventChildGuard::new(command.spawn().expect("spawn Runtime manager racer"))
}

fn wait_for_runtime_event_test_value(path: &Path, expected: &[&str], label: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim().to_string();
            if expected.contains(&value.as_str()) {
                return value;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {label} at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct RuntimeEventChildGuard(Option<std::process::Child>);

impl RuntimeEventChildGuard {
    const fn new(child: std::process::Child) -> Self {
        Self(Some(child))
    }

    fn wait_success(mut self, label: &str) {
        use wait_timeout::ChildExt as _;

        let outcome = self
            .0
            .as_mut()
            .expect("Runtime event child is present")
            .wait_timeout(Duration::from_secs(30));
        match outcome {
            Ok(Some(status)) => {
                self.0.take();
                assert!(status.success(), "{label} failed with {status}");
            }
            Ok(None) => panic!("timed out waiting for {label}"),
            Err(error) => panic!("failed waiting for {label}: {error}"),
        }
    }

    fn kill_and_reap(mut self) -> std::io::Result<std::process::ExitStatus> {
        let child = self.0.as_mut().expect("Runtime event child is present");
        let kill = child.kill();
        let status = child.wait();
        if status.is_ok() {
            self.0.take();
        }
        kill.and(status)
    }
}

impl Drop for RuntimeEventChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
        reasoning_effort: None,
        allowed_tools: None,
        workspace: PathBuf::from("."),
        mode: AppMode::Agent.as_setting().to_string(),
        permission_posture: Some("ask".to_string()),
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
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: Vec::new(),
        steer_count: 0,
        agent_mail_message_id: None,
    }
}

fn agent_mail_request(
    message_id: &str,
    source_thread_id: &str,
    destination_thread_id: &str,
    sender_identity: &str,
    summary: &str,
    delivery_mode: AgentMailDeliveryMode,
) -> AgentMailSendRequest {
    AgentMailSendRequest {
        message_id: AgentMailMessageId::parse(message_id).expect("valid mail id"),
        source_thread_id: source_thread_id.to_string(),
        destination_thread_id: destination_thread_id.to_string(),
        sender: AgentMailSender {
            identity: sender_identity.to_string(),
            display_label: "Builder A".to_string(),
        },
        summary: summary.to_string(),
        evidence: Vec::new(),
        delivery_mode,
        trigger_turn: delivery_mode == AgentMailDeliveryMode::WakeAtSafeBoundary,
        hop_count: 0,
    }
}

#[tokio::test]
async fn agent_mail_queue_is_durable_idempotent_redacted_and_workspace_scoped() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let workspace = runtime_dir.join("workspace-a");
    let foreign_workspace = runtime_dir.join("workspace-b");
    fs::create_dir_all(&workspace)?;
    fs::create_dir_all(&foreign_workspace)?;
    let manager = test_manager(runtime_dir.clone())?;
    let source = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(workspace.clone()),
            task_id: Some("task_a".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let destination = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(workspace),
            task_id: Some("task_b".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let foreign = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(foreign_workspace),
            task_id: Some("task_foreign".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;

    let request = agent_mail_request(
        "mail_durable_replay",
        &source.id,
        &destination.id,
        "task_a",
        "Inspect OPENAI_API_KEY=sk-fixture-secret at /tmp/private/transcript.json",
        AgentMailDeliveryMode::QueueOnly,
    );
    let queued = manager.queue_agent_mail(request.clone()).await?;
    assert!(!queued.idempotent_replay);
    assert_eq!(queued.envelope.status, AgentMailStatus::Queued);
    let serialized = serde_json::to_string(&queued.envelope)?;
    assert!(!serialized.contains("sk-fixture-secret"), "{serialized}");
    assert!(!serialized.contains("/tmp/private"), "{serialized}");
    assert!(serialized.contains("redacted-credential"), "{serialized}");
    assert!(serialized.contains("redacted-path"), "{serialized}");

    let replay = manager.queue_agent_mail(request.clone()).await?;
    assert!(replay.idempotent_replay);
    assert_eq!(replay.envelope, queued.envelope);
    let queued_events = manager
        .events_since(&destination.id, None)?
        .into_iter()
        .filter(|event| event.event == AGENT_MAIL_EVENT_QUEUED)
        .count();
    assert_eq!(
        queued_events, 1,
        "replay must not duplicate delivery events"
    );

    let raw_transcript = manager
        .queue_agent_mail(agent_mail_request(
            "mail_raw_transcript",
            &source.id,
            &destination.id,
            "task_a",
            "assistant: hidden model output\nuser: forward it",
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await
        .expect_err("raw transcripts must not enter Agent Mail")
        .to_string();
    assert!(
        raw_transcript.contains("not a raw transcript"),
        "{raw_transcript}"
    );

    let denied = manager
        .queue_agent_mail(agent_mail_request(
            "mail_cross_workspace",
            &source.id,
            &foreign.id,
            "task_a",
            "cross workspace attempt",
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await
        .expect_err("cross-workspace mail must fail closed")
        .to_string();
    assert!(denied.contains("ownership denied"), "{denied}");

    drop(manager);
    let reopened = test_manager(runtime_dir)?;
    let inbox = reopened.list_agent_mail_for_thread(&destination.id).await?;
    assert_eq!(inbox, vec![queued.envelope]);
    let replay_after_restart = reopened.queue_agent_mail(request.clone()).await?;
    assert!(replay_after_restart.idempotent_replay);

    let mut delivered = replay_after_restart.envelope;
    delivered.status = AgentMailStatus::Delivered;
    delivered.attempt_count = 1;
    delivered.delivered_at = Some(Utc::now());
    delivered.delivery_turn_id = Some("turn_mail_replay".to_string());
    reopened.store.save_agent_mail(&delivered)?;
    let replay_after_delivery = reopened.queue_agent_mail(request).await?;
    assert!(replay_after_delivery.idempotent_replay);
    assert_eq!(
        replay_after_delivery.envelope.status,
        AgentMailStatus::Delivered
    );
    let lifecycle_events = reopened.events_since(&destination.id, None)?;
    assert_eq!(
        lifecycle_events
            .iter()
            .filter(|event| event.event == AGENT_MAIL_EVENT_QUEUED)
            .count(),
        1,
        "lifecycle replay must not emit queued with a delivered envelope"
    );
    assert_eq!(
        lifecycle_events
            .iter()
            .filter(|event| event.event == AGENT_MAIL_EVENT_DELIVERED)
            .count(),
        1,
        "lifecycle replay should repair the current canonical event"
    );
    Ok(())
}

fn set_test_turn_route(
    turn: &mut TurnRecord,
    provider: ApiProvider,
    provider_identity: &str,
    model: &str,
    billing_surface: Option<&str>,
    billing_mode: crate::cost_status::RouteBillingMode,
) {
    turn.persist_effective_route(&crate::cost_status::EffectiveRouteEnvelope {
        provider,
        provider_identity: provider_identity.to_string(),
        model: model.to_string(),
        billing_surface: billing_surface.map(str::to_string),
        endpoint_fingerprint: None,
        billing_mode,
        dispatched_at: turn.created_at,
    });
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
    // The threshold is 80% of the route window (the meter's denominator),
    // clamped to the spendable input ceiling; here 217_600 fits under it.
    assert_eq!(config.token_threshold, 217_600);
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
    object.remove("effective_endpoint_fingerprint");
    object.remove("effective_billing_mode");
    object.remove("effective_dispatched_at");
    object.remove("effective_model");
    object.remove("routed_usage");

    let restored: TurnRecord = serde_json::from_value(value).expect("deserialize legacy turn");
    assert_eq!(restored.effective_provider, None);
    assert_eq!(restored.effective_billing_surface, None);
    assert_eq!(restored.effective_billing_mode, None);
    assert_eq!(restored.effective_dispatched_at, None);
    assert_eq!(restored.effective_model, None);
    assert!(restored.routed_usage.is_empty());
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
            migrated_legacy_ollama_cloud_route: false,
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
async fn runtime_turn_reasoning_and_tool_defaults_follow_explicit_precedence() -> Result<()> {
    let manager = RuntimeThreadManager::open(
        Config {
            reasoning_effort: Some("high".to_string()),
            ..Config::default()
        },
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;

    let override_thread = manager
        .create_thread(CreateThreadRequest {
            reasoning_effort: Some("mid".to_string()),
            allowed_tools: Some(vec!["read_file".to_string()]),
            ..CreateThreadRequest::default()
        })
        .await?;
    assert_eq!(
        override_thread.reasoning_effort.as_deref(),
        Some("medium"),
        "new API input is persisted in its canonical spelling"
    );
    assert_eq!(
        override_thread.allowed_tools,
        Some(vec!["read_file".to_string()])
    );
    let mut override_harness = install_mock_engine(&manager, &override_thread.id).await;
    manager
        .start_turn(
            &override_thread.id,
            StartTurnRequest {
                prompt: "turn settings win".to_string(),
                reasoning_effort: Some("off".to_string()),
                allowed_tools: Some(Vec::new()),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    match override_harness.rx_op.recv().await {
        Some(Op::SendMessage {
            reasoning_effort,
            reasoning_effort_auto,
            allowed_tools,
            ..
        }) => {
            assert_eq!(reasoning_effort.as_deref(), Some("off"));
            assert!(!reasoning_effort_auto);
            assert_eq!(
                allowed_tools,
                Some(Vec::new()),
                "Some([]) must reach the engine as an empty model-visible catalog gate"
            );
        }
        other => panic!("expected turn override SendMessage op, got {other:?}"),
    }

    let thread_default = manager
        .create_thread(CreateThreadRequest {
            reasoning_effort: Some("medium".to_string()),
            allowed_tools: Some(vec!["read_file".to_string()]),
            ..CreateThreadRequest::default()
        })
        .await?;
    let mut thread_harness = install_mock_engine(&manager, &thread_default.id).await;
    manager
        .start_turn(
            &thread_default.id,
            StartTurnRequest {
                prompt: "thread settings win over config".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    match thread_harness.rx_op.recv().await {
        Some(Op::SendMessage {
            reasoning_effort,
            allowed_tools,
            ..
        }) => {
            assert_eq!(
                reasoning_effort.as_deref(),
                Some("high"),
                "the thread's medium preference is normalized for the exact DeepSeek route"
            );
            assert_eq!(allowed_tools, Some(vec!["read_file".to_string()]));
        }
        other => panic!("expected thread-default SendMessage op, got {other:?}"),
    }

    let configured_default = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut config_harness = install_mock_engine(&manager, &configured_default.id).await;
    manager
        .start_turn(
            &configured_default.id,
            StartTurnRequest {
                prompt: "fall back to config".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    match config_harness.rx_op.recv().await {
        Some(Op::SendMessage {
            reasoning_effort,
            allowed_tools,
            ..
        }) => {
            assert_eq!(reasoning_effort.as_deref(), Some("high"));
            assert_eq!(
                allowed_tools, None,
                "no API allowlist keeps the normal configured tool catalog"
            );
        }
        other => panic!("expected config-default SendMessage op, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn runtime_thread_rejects_unknown_reasoning_effort() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let error = manager
        .create_thread(CreateThreadRequest {
            reasoning_effort: Some("surprise-me".to_string()),
            ..CreateThreadRequest::default()
        })
        .await
        .expect_err("new Runtime API inputs must not silently become max")
        .to_string();
    assert!(error.contains("Unrecognized reasoning effort"), "{error}");
    assert!(manager.store.list_threads()?.is_empty());

    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let error = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "reject invalid turn setting".to_string(),
                reasoning_effort: Some("surprise-me".to_string()),
                ..StartTurnRequest::default()
            },
        )
        .await
        .expect_err("per-turn Runtime API inputs use the same strict parser")
        .to_string();
    assert!(error.contains("Unrecognized reasoning effort"), "{error}");
    assert!(manager.store.list_turns_for_thread(&thread.id)?.is_empty());
    Ok(())
}

#[test]
fn runtime_request_wire_distinguishes_missing_and_empty_tool_allowlists() -> Result<()> {
    let inherited: StartTurnRequest = serde_json::from_value(json!({ "prompt": "inherit" }))?;
    assert_eq!(inherited.operation_key, None);
    assert_eq!(inherited.reasoning_effort, None);
    assert_eq!(inherited.allowed_tools, None);

    let disabled: StartTurnRequest = serde_json::from_value(json!({
        "prompt": "no tools",
        "operation_key": "cwc-turn-42",
        "reasoning_effort": "high",
        "allowed_tools": []
    }))?;
    assert_eq!(disabled.operation_key.as_deref(), Some("cwc-turn-42"));
    assert_eq!(disabled.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(disabled.allowed_tools, Some(Vec::new()));

    let camel: StartTurnRequest = serde_json::from_value(json!({
        "prompt": "compat alias",
        "operationKey": "cwc-turn-camel"
    }))?;
    assert_eq!(camel.operation_key.as_deref(), Some("cwc-turn-camel"));
    Ok(())
}

#[test]
fn runtime_turn_operation_keys_are_bounded_scoped_and_fingerprint_execution_policy() -> Result<()> {
    assert!(validate_runtime_turn_operation_key("valid-key_42").is_ok());
    assert!(
        validate_runtime_turn_operation_key(&"k".repeat(MAX_RUNTIME_TURN_OPERATION_KEY_BYTES))
            .is_ok()
    );
    for invalid in [
        "".to_string(),
        " leading".to_string(),
        "trailing ".to_string(),
        "control\nkey".to_string(),
        "k".repeat(MAX_RUNTIME_TURN_OPERATION_KEY_BYTES + 1),
    ] {
        assert!(
            validate_runtime_turn_operation_key(&invalid).is_err(),
            "invalid operation key was accepted"
        );
    }

    let key = "same-caller-key";
    let owner_a = runtime_turn_operation_key_fingerprint("owner_a", "thr_a", key)?;
    assert_ne!(
        owner_a,
        runtime_turn_operation_key_fingerprint("owner_b", "thr_a", key)?,
        "operation keys must be Runtime-store scoped"
    );
    assert_ne!(
        owner_a,
        runtime_turn_operation_key_fingerprint("owner_a", "thr_b", key)?,
        "operation keys must be thread scoped"
    );

    let mut thread = sample_thread("thr_fingerprint");
    thread.model_provider = Some("deepseek".to_string());
    thread.model_provider_id = Some("deepseek".to_string());
    let policy = RuntimePolicyProjection::from_persisted("agent", Some("ask"), false);
    let tools = vec!["read_file".to_string()];
    let base = runtime_turn_request_fingerprint(
        &thread,
        "same prompt",
        None,
        "deepseek-v4-pro",
        Some(crate::tui::app::ReasoningEffort::High),
        Some(&tools),
        policy,
        false,
        false,
        &[],
        None,
    )?;

    let mut different_provider = thread.clone();
    different_provider.model_provider = Some("openai-codex".to_string());
    different_provider.model_provider_id = Some("openai-codex".to_string());
    let changed = [
        runtime_turn_request_fingerprint(
            &different_provider,
            "same prompt",
            None,
            "deepseek-v4-pro",
            Some(crate::tui::app::ReasoningEffort::High),
            Some(&tools),
            policy,
            false,
            false,
            &[],
            None,
        )?,
        runtime_turn_request_fingerprint(
            &thread,
            "different prompt",
            None,
            "deepseek-v4-pro",
            Some(crate::tui::app::ReasoningEffort::High),
            Some(&tools),
            policy,
            false,
            false,
            &[],
            None,
        )?,
        runtime_turn_request_fingerprint(
            &thread,
            "same prompt",
            None,
            "different-model",
            Some(crate::tui::app::ReasoningEffort::High),
            Some(&tools),
            policy,
            false,
            false,
            &[],
            None,
        )?,
        runtime_turn_request_fingerprint(
            &thread,
            "same prompt",
            None,
            "deepseek-v4-pro",
            Some(crate::tui::app::ReasoningEffort::Medium),
            Some(&tools),
            policy,
            false,
            false,
            &[],
            None,
        )?,
        runtime_turn_request_fingerprint(
            &thread,
            "same prompt",
            None,
            "deepseek-v4-pro",
            Some(crate::tui::app::ReasoningEffort::High),
            Some(&[]),
            policy,
            false,
            false,
            &[],
            None,
        )?,
        runtime_turn_request_fingerprint(
            &thread,
            "same prompt",
            None,
            "deepseek-v4-pro",
            Some(crate::tui::app::ReasoningEffort::High),
            Some(&tools),
            RuntimePolicyProjection::from_persisted("operate", Some("full_access"), true),
            true,
            true,
            &[],
            None,
        )?,
    ];
    assert!(
        changed.iter().all(|fingerprint| fingerprint != &base),
        "provider/model/prompt/reasoning/tool or permission changes must alter the fingerprint"
    );
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
    let mut event_lock = fd_lock::RwLock::new(manager.store.open_event_lock()?);
    let event_state_guard = event_lock.try_write()?;
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
    let terminal =
        wait_for_terminal_turn(&manager, &turn_id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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
async fn operation_key_replays_torn_response_survives_restart_and_rejects_mismatch() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let manager = test_manager(runtime_dir.clone())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let operation_key = "cwc-op-sk-fixture-must-not-persist".to_string();
    let request = StartTurnRequest {
        prompt: "idempotent turn payload".to_string(),
        operation_key: Some(operation_key.clone()),
        reasoning_effort: Some("high".to_string()),
        allowed_tools: Some(Vec::new()),
        ..StartTurnRequest::default()
    };

    // Hold the start receipt so the engine has accepted and persisted the
    // turn while the HTTP-equivalent caller still has no response.
    let mut event_lock = fd_lock::RwLock::new(manager.store.open_event_lock()?);
    let event_state_guard = event_lock.try_write()?;
    let start_manager = manager.clone();
    let start_thread_id = thread.id.clone();
    let first_request = request.clone();
    let start_task = tokio::spawn(async move {
        start_manager
            .start_turn(&start_thread_id, first_request)
            .await
    });
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), harness.rx_op.recv()).await?,
        Some(Op::SendMessage { .. })
    ));
    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 1);
    let original = turns[0].clone();

    // Simulate a torn HTTP response. Retrying immediately returns the same
    // in-progress turn and never places a second operation in the engine.
    start_task.abort();
    let _ = start_task.await;
    let replay = tokio::time::timeout(
        Duration::from_secs(2),
        manager.start_turn(&thread.id, request.clone()),
    )
    .await??;
    assert_eq!(replay.id, original.id);
    assert!(harness.rx_op.try_recv().is_err());

    let key_fingerprint = runtime_turn_operation_key_fingerprint(
        &manager.store.owner_id,
        &thread.id,
        &operation_key,
    )?;
    let binding = manager
        .store
        .load_turn_operation_binding(&key_fingerprint)?
        .context("persisted turn operation binding")?;
    assert_eq!(binding.turn_id, original.id);
    assert_eq!(binding.thread_id, thread.id);
    assert_eq!(binding.operation_key_fingerprint, key_fingerprint);
    assert_eq!(binding.request_fingerprint.len(), 64);
    assert!(
        !serde_json::to_string(&binding)?.contains(&operation_key),
        "raw operation key entered its durable binding"
    );

    drop(event_state_guard);
    harness
        .tx_event
        .send(EngineEvent::MessageStarted { index: 0 })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::MessageDelta {
            index: 0,
            content: "one execution".to_string(),
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
    let terminal =
        wait_for_terminal_turn(&manager, &original.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
    assert_eq!(manager.store.list_turns_for_thread(&thread.id)?.len(), 1);

    let leaked_key_file = ignore::WalkBuilder::new(&runtime_dir)
        .hidden(false)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .find_map(|entry| {
            fs::read_to_string(entry.path())
                .ok()
                .filter(|contents| contents.contains(&operation_key))
                .map(|_| entry.path().to_path_buf())
        });
    assert!(
        leaked_key_file.is_none(),
        "raw operation key leaked into {}",
        leaked_key_file
            .as_deref()
            .map(std::path::Path::display)
            .map(|path| path.to_string())
            .unwrap_or_default()
    );

    drop(harness);
    drop(manager);
    let reopened = test_manager(runtime_dir)?;
    let replay_after_restart = reopened.start_turn(&thread.id, request.clone()).await?;
    assert_eq!(replay_after_restart.id, original.id);
    assert_eq!(replay_after_restart.status, RuntimeTurnStatus::Completed);

    let mut mismatches = Vec::new();
    let mut changed_prompt = request.clone();
    changed_prompt.prompt = "different prompt".to_string();
    mismatches.push(changed_prompt);
    let mut changed_model = request.clone();
    changed_model.model = Some("different-model".to_string());
    mismatches.push(changed_model);
    let mut changed_reasoning = request.clone();
    changed_reasoning.reasoning_effort = Some("medium".to_string());
    mismatches.push(changed_reasoning);
    let mut changed_tools = request.clone();
    changed_tools.allowed_tools = Some(vec!["read_file".to_string()]);
    mismatches.push(changed_tools);
    let mut changed_permission = request.clone();
    changed_permission.permission_posture = Some("full_access".to_string());
    mismatches.push(changed_permission);

    for mismatch in mismatches {
        let error = reopened
            .start_turn(&thread.id, mismatch)
            .await
            .expect_err("operation-key reuse with changed execution must fail closed")
            .to_string();
        assert!(error.contains("different turn request"), "{error}");
    }
    assert_eq!(reopened.store.list_turns_for_thread(&thread.id)?.len(), 1);

    let mut oversized = request;
    oversized.operation_key = Some("k".repeat(MAX_RUNTIME_TURN_OPERATION_KEY_BYTES + 1));
    let error = reopened
        .start_turn(&thread.id, oversized)
        .await
        .expect_err("oversized operation keys must be rejected")
        .to_string();
    assert!(error.contains("cannot exceed"), "{error}");
    Ok(())
}

#[tokio::test]
async fn restart_removes_torn_operation_item_before_reserved_turn_retry() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(runtime_dir.clone())?;
    let thread = sample_thread("thr_torn_operation_item");
    store.save_thread(&thread)?;
    let operation_key = "relay-torn-item-operation";
    let key_fingerprint =
        runtime_turn_operation_key_fingerprint(&store.owner_id, &thread.id, operation_key)?;
    let turn_id = "turn_relay_reserved_retry";
    store.save_turn_operation_binding(&RuntimeTurnOperationBinding {
        schema_version: TURN_OPERATION_BINDING_SCHEMA_VERSION,
        thread_id: thread.id.clone(),
        turn_id: turn_id.to_string(),
        operation_key_fingerprint: key_fingerprint.clone(),
        request_fingerprint: crate::hashing::sha256_hex("torn-request"),
        created_at: Utc::now(),
    })?;
    let mut orphan = sample_item(
        turn_id,
        "item_torn_before_turn_commit",
        TurnItemLifecycleStatus::Completed,
    );
    orphan.kind = TurnItemKind::UserMessage;
    store.save_item(&orphan)?;
    drop(store);

    let manager = test_manager(runtime_dir)?;
    assert!(
        manager
            .store
            .load_turn_operation_binding(&key_fingerprint)?
            .is_none()
    );
    assert!(manager.store.list_items_for_turn(turn_id)?.is_empty());

    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn_with_reserved_id(
            &thread.id,
            StartTurnRequest {
                prompt: "retry after the pre-submit crash".to_string(),
                operation_key: Some(operation_key.to_string()),
                ..StartTurnRequest::default()
            },
            turn_id,
        )
        .await?;
    assert_eq!(turn.id, turn_id);
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    assert_eq!(
        manager.store.list_items_for_turn(turn_id)?.len(),
        1,
        "the retry must own exactly one user item"
    );
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
    let turn = tokio::time::timeout(TURN_SETTLEMENT_DEADLOCK_TIMEOUT, start_task).await???;
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
    let terminal =
        wait_for_terminal_turn(&manager, &turn.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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
    let mut event_lock = fd_lock::RwLock::new(manager.store.open_event_lock()?);
    let event_state_guard = event_lock.try_write()?;
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
            post_input_tokens: None,
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
    match harness.rx_op.recv().await {
        Some(Op::SendMessage { compaction, .. }) => {
            assert_eq!(
                compaction.runtime_cost_owner.as_deref(),
                Some(turn.id.as_str())
            );
        }
        other => panic!("expected SendMessage op, got {other:?}"),
    }
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
        Some(Op::CompactContext {
            route, compaction, ..
        }) => {
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
async fn queued_reload_is_a_hard_boundary_for_a_concurrent_turn_start() -> Result<()> {
    let provider = |base_url: &str, key: &str| crate::config::ProviderConfig {
        kind: Some("openai-compatible".to_string()),
        base_url: Some(base_url.to_string()),
        model: Some("local-model".to_string()),
        api_key: Some(key.to_string()),
        ..crate::config::ProviderConfig::default()
    };
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom: std::collections::HashMap::from([(
                "lm-studio".to_string(),
                provider("http://127.0.0.1:18181/v1", "old-local-test-key"),
            )]),
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

    // Hold one existing admission reader while the reload writer queues. Tokio
    // RwLock is write-preferring, so the later turn reader cannot overtake it.
    let prior_admission = manager.config_admission.read().await;
    let mut reloaded = config;
    reloaded
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("named custom provider")
        .clone_from(&provider("http://127.0.0.1:18182/v1", "new-local-test-key"));
    let reload_manager = manager.clone();
    let mut reload = tokio::spawn(async move { reload_manager.reload_config(reloaded).await });
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut reload)
            .await
            .is_err(),
        "reload must be waiting for the prior admission reader"
    );

    let start_manager = manager.clone();
    let start_thread_id = thread.id.clone();
    let mut start = tokio::spawn(async move {
        start_manager
            .start_turn(
                &start_thread_id,
                StartTurnRequest {
                    prompt: "use the post-reload route".to_string(),
                    ..StartTurnRequest::default()
                },
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut start)
            .await
            .is_err(),
        "a turn queued after the reload writer may not snapshot old config"
    );
    drop(prior_admission);
    reload.await.expect("reload task")?;
    start.await.expect("start task")?;

    loop {
        match harness.rx_op.recv().await {
            Some(Op::SendMessage { route, .. }) => {
                assert_eq!(
                    route.config.deepseek_base_url(),
                    "http://127.0.0.1:18182/v1"
                );
                break;
            }
            Some(
                Op::SetCompaction { .. }
                | Op::SetStreamChunkTimeout { .. }
                | Op::SetSubagentRuntimeConfig { .. },
            ) => {}
            other => panic!("expected post-reload SendMessage, got {other:?}"),
        }
    }
    Ok(())
}

#[tokio::test]
async fn queued_reload_is_a_hard_boundary_for_concurrent_compaction() -> Result<()> {
    let provider = |base_url: &str, key: &str| crate::config::ProviderConfig {
        kind: Some("openai-compatible".to_string()),
        base_url: Some(base_url.to_string()),
        model: Some("local-model".to_string()),
        api_key: Some(key.to_string()),
        ..crate::config::ProviderConfig::default()
    };
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom: std::collections::HashMap::from([(
                "lm-studio".to_string(),
                provider("http://127.0.0.1:18181/v1", "old-local-test-key"),
            )]),
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

    let prior_admission = manager.config_admission.read().await;
    let mut reloaded = config;
    *reloaded
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("named custom provider") =
        provider("http://127.0.0.1:18182/v1", "new-local-test-key");
    let reload_manager = manager.clone();
    let mut reload = tokio::spawn(async move { reload_manager.reload_config(reloaded).await });
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut reload)
            .await
            .is_err()
    );
    let compact_manager = manager.clone();
    let compact_thread_id = thread.id.clone();
    let mut compact = tokio::spawn(async move {
        compact_manager
            .compact_thread(
                &compact_thread_id,
                CompactThreadRequest {
                    reason: Some("post-reload compaction".to_string()),
                },
            )
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut compact)
            .await
            .is_err(),
        "compaction queued after the reload writer may not carry old config"
    );
    drop(prior_admission);
    reload.await.expect("reload task")?;
    compact.await.expect("compact task")?;

    loop {
        match harness.rx_op.recv().await {
            Some(Op::CompactContext { route, .. }) => {
                assert_eq!(
                    route.config.deepseek_base_url(),
                    "http://127.0.0.1:18182/v1"
                );
                break;
            }
            Some(
                Op::SetCompaction { .. }
                | Op::SetStreamChunkTimeout { .. }
                | Op::SetSubagentRuntimeConfig { .. },
            ) => {}
            other => panic!("expected post-reload CompactContext, got {other:?}"),
        }
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
async fn create_thread_scenario() -> Result<()> {
    // Scenario consolidation of: create_thread_uses_requested_non_current_builtin_default_model, create_thread_defaults_auto_approve_to_false
    // from create_thread_uses_requested_non_current_builtin_default_model
    {
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
    }
    // from create_thread_defaults_auto_approve_to_false
    {
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
    }
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
    let fingerprint = "a".repeat(64);
    turn.persist_effective_route(&crate::cost_status::EffectiveRouteEnvelope {
        provider: ApiProvider::Stepfun,
        provider_identity: "stepfun-primary".to_string(),
        model: "step-3.7-flash".to_string(),
        billing_surface: Some(crate::pricing::STEPFUN_PAYG_BILLING_SURFACE.to_string()),
        endpoint_fingerprint: Some(fingerprint.clone()),
        billing_mode: crate::cost_status::RouteBillingMode::Metered,
        dispatched_at: turn.created_at,
    });

    let value = serde_json::to_value(turn).expect("serialize turn");
    assert_eq!(
        value["effective_billing_surface"],
        crate::pricing::STEPFUN_PAYG_BILLING_SURFACE
    );
    assert_eq!(value["effective_billing_mode"], "metered");
    assert_eq!(value["effective_endpoint_fingerprint"], fingerprint);
    assert!(value["effective_dispatched_at"].is_string());
    assert!(value.get("base_url").is_none());
    assert!(value.get("effective_base_url").is_none());
}

#[test]
fn serialized_turn_record_redacts_all_route_and_source_fields() {
    let mut value = serde_json::to_value(sample_turn(
        "thr_secret_route",
        "turn_secret_route",
        RuntimeTurnStatus::Completed,
    ))
    .expect("serialize clean fixture");
    value["effective_provider"] = serde_json::json!("Authorization: Bearer provider-secret");
    value["effective_provider_id"] = serde_json::json!("CUSTOM_API_KEY=sk-provider-secret");
    value["effective_model"] = serde_json::json!("../.ssh/model-secret");
    value["effective_billing_surface"] =
        serde_json::json!("https://alice:password@example.test/v1?token=secret#fragment");
    value["effective_endpoint_fingerprint"] = serde_json::json!("secret-endpoint");
    value["routed_usage_source_ids"] = serde_json::json!(["raw-provider-response-secret"]);
    value["routed_usage"] = serde_json::json!([{
        "route": {
            "provider": "custom",
            "provider_identity": "TOKEN=ghp_child_secret",
            "model": r"relative\secret\model",
            "billing_surface": "https://host.test/v1?key=child-secret",
            "endpoint_fingerprint": "child-endpoint-secret",
            "billing_mode": "metered",
            "dispatched_at": Utc::now(),
        },
        "usage": Usage::default(),
    }]);
    let turn: TurnRecord = serde_json::from_value(value).expect("deserialize hostile fixture");

    let serialized = serde_json::to_string(&turn).expect("serialize turn record");
    for secret in [
        "provider-secret",
        "sk-provider-secret",
        ".ssh",
        "alice",
        "password",
        "token=secret",
        "raw-provider-response-secret",
        "ghp_child_secret",
        "child-secret",
        "child-endpoint-secret",
    ] {
        assert!(
            !serialized.contains(secret),
            "serialized turn leaked {secret:?}: {serialized}"
        );
    }
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
    set_test_turn_route(
        &mut deepseek,
        ApiProvider::Deepseek,
        ApiProvider::Deepseek.as_str(),
        "deepseek-v4-flash",
        // A real dispatch persists the classified surface. Leaving it absent
        // here would price the row at official DeepSeek rates on nothing but
        // the provider name, which is what the route audit now refuses.
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    manager.store.save_turn(&deepseek)?;

    let mut codex = sample_turn(&thread.id, "turn_codex", RuntimeTurnStatus::Completed);
    codex.usage = Some(usage);
    set_test_turn_route(
        &mut codex,
        ApiProvider::OpenaiCodex,
        ApiProvider::OpenaiCodex.as_str(),
        "gpt-5.5",
        Some(crate::pricing::OAUTH_SUBSCRIPTION_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Subscription,
    );
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
    assert_eq!(deepseek_bucket.priced_turns, 1);
    assert_eq!(deepseek_bucket.unpriced_turns, 0);
    assert_eq!(codex_bucket.priced_turns, 0);
    assert_eq!(codex_bucket.nonmetered_turns, 1);
    assert_eq!(codex_bucket.unpriced_turns, 0);
    assert!(codex_bucket.unpriced_reasons.is_empty());
    assert_eq!(report.totals.priced_turns, 1);
    assert_eq!(report.totals.nonmetered_turns, 1);
    assert_eq!(report.totals.unpriced_turns, 0);
    assert!(report.totals.cost_complete);
    assert_eq!(codex_bucket.input_tokens, 10_000);
    assert_eq!(report.totals.cost_usd, deepseek_bucket.cost_usd);
    Ok(())
}

/// `aggregate_usage_for_thread` scopes the same recorded-time pricing the
/// global `/v1/usage` aggregate applies to exactly one thread, in both
/// published currencies: DeepSeek's first-party rows carry native CNY, so a
/// deepseek-priced turn must produce a non-zero `cost_cny` there, while a
/// different thread's spend stays out of the figure. The parent/child split
/// keeps routed-child (sub-agent) spend out of the parent figure — the split
/// session persistence writes into `session_cost_*` vs `subagent_cost_*` —
/// and CNY coverage counts turns under the provider's own CNY row.
#[tokio::test]
async fn aggregate_usage_for_thread_scopes_both_currencies_to_one_thread() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;

    let billed = sample_thread("thr_thread_usage_billed");
    manager.store.save_thread(&billed)?;
    let mut turn = sample_turn(&billed.id, "turn_billed", RuntimeTurnStatus::Completed);
    turn.usage = Some(Usage {
        input_tokens: 10_000,
        output_tokens: 1_000,
        ..Usage::default()
    });
    set_test_turn_route(
        &mut turn,
        ApiProvider::Deepseek,
        ApiProvider::Deepseek.as_str(),
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    turn.routed_usage = vec![crate::cost_status::EffectiveRouteUsage {
        route: crate::cost_status::EffectiveRouteEnvelope {
            provider: ApiProvider::Deepseek,
            provider_identity: ApiProvider::Deepseek.as_str().to_string(),
            model: "deepseek-v4-flash".to_string(),
            billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
            endpoint_fingerprint: None,
            billing_mode: crate::cost_status::RouteBillingMode::Metered,
            dispatched_at: turn.created_at,
        },
        usage: Usage {
            input_tokens: 2_000,
            output_tokens: 200,
            ..Usage::default()
        },
    }];
    manager.store.save_turn(&turn)?;

    // A second thread with its own priced turn must not leak into the
    // first thread's figure.
    let other = sample_thread("thr_thread_usage_other");
    manager.store.save_thread(&other)?;
    let mut other_turn = sample_turn(&other.id, "turn_other", RuntimeTurnStatus::Completed);
    other_turn.usage = Some(Usage {
        input_tokens: 500_000,
        output_tokens: 500_000,
        ..Usage::default()
    });
    set_test_turn_route(
        &mut other_turn,
        ApiProvider::Deepseek,
        ApiProvider::Deepseek.as_str(),
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    manager.store.save_turn(&other_turn)?;

    let split = manager.aggregate_usage_for_thread(&billed.id).await?;
    // The split mirrors the persisted session fields: parent spend in
    // `parent`, routed-child (sub-agent) spend in `routed_children`.
    assert_eq!(split.parent.input_tokens, 10_000);
    assert_eq!(split.parent.output_tokens, 1_000);
    assert_eq!(split.parent.turns, 1);
    assert!(split.parent.cost_usd > 0.0);
    assert!(split.parent.cost_cny > 0.0);
    assert_eq!(split.routed_children.input_tokens, 2_000);
    assert_eq!(split.routed_children.turns, 1);
    assert!(split.routed_children.cost_usd > 0.0);
    assert!(split.routed_children.cost_cny > 0.0);

    // Native CNY row: priced in CNY without any FX projection of the USD
    // column, and the CNY coverage counters see the same money-metered
    // turns the USD counters do.
    assert_eq!(split.parent.priced_turns, 1);
    assert_eq!(split.parent.unpriced_turns, 0);
    assert_eq!(split.parent.cny_priced_turns, 1);
    assert_eq!(split.parent.cny_unpriced_turns, 0);
    assert!(split.parent.cny_unpriced_reasons.is_empty());
    assert_eq!(split.routed_children.cny_priced_turns, 1);
    assert!(split.routed_children.cny_unpriced_reasons.is_empty());

    let totals = split.combined();
    assert_eq!(totals.input_tokens, 12_000);
    assert_eq!(totals.turns, 2);
    assert_eq!(
        totals.cost_usd,
        split.parent.cost_usd + split.routed_children.cost_usd
    );
    assert!(totals.cost_cny > 0.0);
    assert_eq!(totals.priced_turns, 2);
    assert_eq!(totals.unpriced_turns, 0);
    assert_eq!(totals.cny_priced_turns, 2);
    assert!(totals.cost_complete);

    // The scoped figure agrees with the thread bucket of the global
    // aggregate, so the GUI (per-thread endpoint) and `/v1/usage` can never
    // disagree about the same thread.
    let global = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;
    let bucket = global
        .buckets
        .iter()
        .find(|bucket| bucket.key == billed.id)
        .expect("billed thread bucket");
    assert_eq!(bucket.cost_usd, totals.cost_usd);
    assert_eq!(bucket.cost_cny, totals.cost_cny);
    assert_eq!(bucket.cny_priced_turns, totals.cny_priced_turns);

    let missing = manager
        .aggregate_usage_for_thread("thr_does_not_exist")
        .await;
    assert!(missing.is_err());
    Ok(())
}

/// An aggregate in which nothing could be priced is unavailable, not zero.
///
/// `cost_usd` is a `f64` and an all-unknown run leaves it at `0.0`, so the
/// only thing standing between a reader and a fabricated "$0.00 spent" is that
/// the aggregate also reports zero priced turns and incomplete coverage. Those
/// qualifiers are the contract; this pins them at the aggregate level, which is
/// where per-turn honesty is most easily lost by summing.
#[tokio::test]
async fn aggregate_usage_reports_an_all_unknown_run_as_unavailable_not_zero() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_all_unknown");
    manager.store.save_thread(&thread)?;
    let usage = Usage {
        input_tokens: 10_000,
        output_tokens: 1_000,
        ..Usage::default()
    };

    for (index, surface) in [
        // Endpoint classified but unplaceable.
        Some(crate::pricing::UNCLASSIFIED_BILLING_SURFACE),
        // No endpoint classification at all: a provider name is not evidence
        // that the official endpoint served the turn.
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let mut turn = sample_turn(
            &thread.id,
            &format!("turn_unknown_{index}"),
            RuntimeTurnStatus::Completed,
        );
        turn.usage = Some(usage.clone());
        set_test_turn_route(
            &mut turn,
            ApiProvider::Openai,
            ApiProvider::Openai.as_str(),
            "gpt-5.5",
            surface,
            crate::cost_status::RouteBillingMode::Metered,
        );
        manager.store.save_turn(&turn)?;
    }

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;

    assert_eq!(report.totals.turns, 2);
    // Tokens are known and are reported. Only the money is unknown; an unknown
    // price must not suppress usage that the provider did report.
    assert_eq!(report.totals.input_tokens, 20_000);
    assert_eq!(report.totals.priced_turns, 0);
    assert_eq!(report.totals.unpriced_turns, 2);
    assert_eq!(report.totals.nonmetered_turns, 0);
    assert!(
        !report.totals.cost_complete,
        "an all-unknown run must never be a complete total"
    );
    assert_eq!(
        report.totals.cost_usd, 0.0,
        "no spend may be invented for an unpriced run"
    );
    assert!(
        !report.totals.unpriced_reasons.is_empty(),
        "an unpriced aggregate must say why"
    );
    assert_eq!(report.totals.cny_unpriced_turns, 2);
    assert!(
        !report.totals.cny_unpriced_reasons.is_empty(),
        "CNY coverage must travel with the money (#4318)"
    );
    for bucket in &report.buckets {
        assert_eq!(bucket.priced_turns, 0);
        assert!(
            bucket.unpriced_turns > 0,
            "a bucket cannot report a complete zero either"
        );
    }
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_marks_unknown_cost_as_subtotal_and_keeps_cache_writes() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_cost_coverage");
    manager.store.save_thread(&thread)?;
    let usage = Usage {
        input_tokens: 10_000,
        output_tokens: 1_000,
        prompt_cache_write_tokens: Some(321),
        ..Usage::default()
    };

    let mut priced = sample_turn(&thread.id, "turn_priced", RuntimeTurnStatus::Completed);
    priced.usage = Some(usage.clone());
    set_test_turn_route(
        &mut priced,
        ApiProvider::Deepseek,
        ApiProvider::Deepseek.as_str(),
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    manager.store.save_turn(&priced)?;

    let mut unknown = sample_turn(&thread.id, "turn_unknown", RuntimeTurnStatus::Completed);
    unknown.usage = Some(usage);
    set_test_turn_route(
        &mut unknown,
        ApiProvider::Openai,
        ApiProvider::Openai.as_str(),
        "gpt-5.5",
        Some(crate::pricing::UNCLASSIFIED_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Unknown,
    );
    manager.store.save_turn(&unknown)?;

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;
    assert!(report.totals.cost_usd > 0.0, "priced subtotal is retained");
    assert_eq!(report.totals.priced_turns, 1);
    assert_eq!(report.totals.unpriced_turns, 1);
    assert!(!report.totals.cost_complete);
    assert_eq!(report.totals.nonmetered_turns, 0);
    assert_eq!(report.totals.cache_write_tokens, 642);
    assert!(
        report
            .totals
            .unpriced_reasons
            .contains("unknown_billing_basis")
    );
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_prices_slow_predispatch_turn_at_dispatch_boundary() -> Result<()> {
    // The pricing tier is chosen at the turn's *dispatch* time, not its
    // creation time. Sonnet 5's intro window used to exercise this; that
    // increase was cancelled upstream (the $2/$10 rate is now standard), so
    // the DeepSeek V4 Flash off-peak -> peak boundary at 01:00 UTC is the
    // recorded-time rate flip this test now rides: 1M cache-miss input costs
    // $0.22 off-peak and $0.44 in the peak window
    // (https://api-docs.deepseek.com/quick_start/pricing, 2026-08-17).
    let manager = test_manager(test_runtime_dir())?;
    let mut thread = sample_thread("thr_historical_pricing");
    thread.model = "deepseek-v4-flash".to_string();
    manager.store.save_thread(&thread)?;

    let usage = Usage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        ..Usage::default()
    };
    for (turn_id, created_at, dispatched_at) in [
        (
            "turn_off_peak",
            "2026-08-17T00:59:58Z",
            "2026-08-17T00:59:59Z",
        ),
        (
            "turn_slow_snapshot",
            "2026-08-17T00:59:59Z",
            "2026-08-17T01:00:00Z",
        ),
    ] {
        let mut turn = sample_turn(&thread.id, turn_id, RuntimeTurnStatus::Completed);
        turn.created_at = created_at.parse().expect("recorded turn time");
        turn.started_at = Some(turn.created_at);
        turn.usage = Some(usage.clone());
        set_test_turn_route(
            &mut turn,
            ApiProvider::Deepseek,
            ApiProvider::Deepseek.as_str(),
            "deepseek-v4-flash",
            Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
            crate::cost_status::RouteBillingMode::Metered,
        );
        turn.effective_dispatched_at = Some(dispatched_at.parse().expect("dispatch time"));
        manager.store.save_turn(&turn)?;
    }

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Model)
        .await?;

    assert_eq!(report.totals.turns, 2);
    // 0.22 (off-peak) + 0.44 (peak, priced at the dispatch minute).
    assert!(
        (report.totals.cost_usd - 0.66).abs() < 1e-9,
        "{}",
        report.totals.cost_usd
    );
    assert_eq!(report.buckets.len(), 1);
    assert!((report.buckets[0].cost_usd - 0.66).abs() < 1e-9);
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
        set_test_turn_route(
            &mut turn,
            ApiProvider::Stepfun,
            ApiProvider::Stepfun.as_str(),
            "step-3.7-flash",
            Some(surface),
            if surface == crate::pricing::STEPFUN_PLAN_BILLING_SURFACE {
                crate::cost_status::RouteBillingMode::Subscription
            } else {
                crate::cost_status::RouteBillingMode::Metered
            },
        );
        manager.store.save_turn(&turn)?;
    }

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Provider)
        .await?;
    assert_eq!(report.totals.turns, 2);
    assert!((report.totals.cost_usd - 0.735).abs() < 1e-12);
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_includes_exclusive_child_calls_and_zero_usage_receipts() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_child_usage");
    manager.store.save_thread(&thread)?;

    let mut turn = sample_turn(&thread.id, "turn_parent", RuntimeTurnStatus::Completed);
    turn.usage = Some(Usage {
        input_tokens: 100,
        output_tokens: 20,
        prompt_cache_hit_tokens: Some(40),
        prompt_cache_miss_tokens: Some(50),
        prompt_cache_write_tokens: Some(10),
        reasoning_tokens: Some(5),
        ..Usage::default()
    });
    set_test_turn_route(
        &mut turn,
        ApiProvider::Deepseek,
        "deepseek-parent",
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    turn.routed_usage
        .push(crate::cost_status::EffectiveRouteUsage {
            route: crate::cost_status::EffectiveRouteEnvelope {
                provider: ApiProvider::Deepseek,
                provider_identity: "deepseek-child".to_string(),
                model: "deepseek-v4-flash".to_string(),
                // A child call carries the surface its own dispatch classified;
                // the route audit will not price a metered route without one.
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: turn.created_at,
            },
            usage: Usage {
                input_tokens: 30,
                output_tokens: 7,
                prompt_cache_hit_tokens: Some(10),
                prompt_cache_miss_tokens: Some(20),
                reasoning_tokens: Some(6),
                ..Usage::default()
            },
        });
    turn.routed_usage
        .push(crate::cost_status::EffectiveRouteUsage {
            route: crate::cost_status::EffectiveRouteEnvelope {
                provider: ApiProvider::OpenaiCodex,
                provider_identity: "codex-oauth".to_string(),
                model: "gpt-5.5".to_string(),
                billing_surface: Some(
                    crate::pricing::OAUTH_SUBSCRIPTION_BILLING_SURFACE.to_string(),
                ),
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Subscription,
                dispatched_at: turn.created_at,
            },
            usage: Usage::default(),
        });
    manager.store.save_turn(&turn)?;

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;
    assert_eq!(report.totals.turns, 3, "parent plus two child calls");
    assert_eq!(report.totals.input_tokens, 70);
    assert_eq!(report.totals.cached_tokens, 50);
    assert_eq!(report.totals.cache_write_tokens, 10);
    assert_eq!(report.totals.output_tokens, 27);
    assert_eq!(report.totals.reasoning_tokens, 11);
    assert_eq!(report.totals.priced_turns, 2);
    assert_eq!(report.totals.nonmetered_turns, 1);
    assert_eq!(report.totals.unpriced_turns, 0);
    assert!(report.totals.cost_complete);
    assert_eq!(report.totals.route_receipts.len(), 3);
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_filters_each_call_by_its_dispatch_timestamp() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_dispatch_window");
    manager.store.save_thread(&thread)?;
    let window = Utc::now();
    let mut turn = sample_turn(
        &thread.id,
        "turn_dispatch_window",
        RuntimeTurnStatus::Completed,
    );
    turn.created_at = window - chrono::Duration::days(2);
    turn.usage = Some(Usage {
        input_tokens: 100,
        ..Usage::default()
    });
    set_test_turn_route(
        &mut turn,
        ApiProvider::Deepseek,
        "deepseek-parent",
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        crate::cost_status::RouteBillingMode::Metered,
    );
    turn.routed_usage
        .push(crate::cost_status::EffectiveRouteUsage {
            route: crate::cost_status::EffectiveRouteEnvelope {
                provider: ApiProvider::Deepseek,
                provider_identity: "deepseek-child".to_string(),
                model: "deepseek-v4-flash".to_string(),
                billing_surface: None,
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: window,
            },
            usage: Usage {
                input_tokens: 33,
                ..Usage::default()
            },
        });
    manager.store.save_turn(&turn)?;

    let report = manager
        .aggregate_usage(
            Some(window - chrono::Duration::minutes(1)),
            Some(window + chrono::Duration::minutes(1)),
            UsageGroupBy::Thread,
        )
        .await?;
    assert_eq!(report.totals.turns, 1);
    assert_eq!(report.totals.input_tokens, 33);
    Ok(())
}

#[tokio::test]
async fn aggregate_usage_marks_bounded_journal_truncation_incomplete() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_truncated_usage");
    manager.store.save_thread(&thread)?;
    let mut turn = sample_turn(
        &thread.id,
        "turn_truncated_usage",
        RuntimeTurnStatus::Completed,
    );
    turn.routed_usage_dropped_records = 2;
    manager.store.save_turn(&turn)?;

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;
    assert_eq!(report.totals.dropped_usage_records, 2);
    assert_eq!(report.totals.unpriced_turns, 2);
    assert_eq!(report.totals.cny_unpriced_turns, 2);
    assert_eq!(report.totals.turns, 2);
    assert!(!report.totals.cost_complete);
    assert!(
        report
            .totals
            .unpriced_reasons
            .contains("runtime_usage_journal_truncated")
    );
    assert!(
        report
            .totals
            .cny_unpriced_reasons
            .contains("runtime_usage_journal_truncated")
    );
    let bucket = report.buckets.first().expect("thread usage bucket");
    assert_eq!(bucket.dropped_usage_records, 2);
    assert_eq!(bucket.unpriced_turns, 2);
    assert_eq!(bucket.cny_unpriced_turns, 2);
    assert!(
        bucket
            .cny_unpriced_reasons
            .contains("runtime_usage_journal_truncated")
    );
    Ok(())
}

#[test]
fn runtime_usage_sink_survives_parent_terminal_and_restart_exactly_once() -> Result<()> {
    let _cost_scope = crate::cost_status::test_scope();
    let data_dir = test_runtime_dir();
    let manager = test_manager(data_dir.clone())?;
    let thread = sample_thread("thr_compaction_sink");
    manager.store.save_thread(&thread)?;
    let turn = sample_turn(
        &thread.id,
        "turn_compaction_sink",
        RuntimeTurnStatus::InProgress,
    );
    manager.store.save_turn(&turn)?;
    manager.register_runtime_usage_sink(&turn.id);
    let lease = crate::cost_status::acquire_runtime_usage_lease(&turn.id)
        .expect("active runtime owner lease");
    crate::cost_status::finish_runtime_usage_owner(&turn.id);

    let route = crate::cost_status::EffectiveRouteEnvelope::capture(
        None,
        ApiProvider::Deepseek,
        "deepseek-compaction",
        "deepseek-v4-flash",
        Some(ApiProvider::Deepseek.default_base_url()),
        Utc::now(),
    );
    crate::cost_status::report_effective_route_for_runtime(
        crate::cost_status::scope_token(),
        Some(&turn.id),
        "compaction:response:test",
        &route,
        &Usage {
            input_tokens: 17,
            output_tokens: 3,
            ..Usage::default()
        },
    );
    crate::cost_status::report_effective_route_for_runtime(
        crate::cost_status::scope_token(),
        Some(&turn.id),
        "compaction:response:test",
        &route,
        &Usage {
            input_tokens: 17,
            output_tokens: 3,
            ..Usage::default()
        },
    );
    drop(lease);
    drop(manager);

    let reopened = test_manager(data_dir.clone())?;
    reopened.register_runtime_usage_sink(&turn.id);
    crate::cost_status::report_effective_route_for_runtime(
        crate::cost_status::scope_token(),
        Some(&turn.id),
        "compaction:response:test",
        &route,
        &Usage {
            input_tokens: 17,
            output_tokens: 3,
            ..Usage::default()
        },
    );
    crate::cost_status::finish_runtime_usage_owner(&turn.id);
    drop(reopened);
    let reopened = test_manager(data_dir)?;
    let persisted = reopened.store.load_turn(&turn.id)?;
    assert_eq!(persisted.routed_usage.len(), 1);
    assert_eq!(persisted.routed_usage[0].usage.input_tokens, 17);
    assert!(
        crate::cost_status::take_runtime_usage(&turn.id)
            .records
            .is_empty(),
        "successful synchronous persistence must not duplicate into fallback"
    );
    assert!(
        crate::cost_status::drain().is_empty(),
        "runtime-owned usage must not duplicate into the TUI pool"
    );
    Ok(())
}

#[test]
fn routed_usage_append_is_bounded_and_idempotent_for_every_delivery_path() {
    let mut turn = sample_turn(
        "thr_all_paths",
        "turn_all_paths",
        RuntimeTurnStatus::InProgress,
    );
    let route = crate::cost_status::EffectiveRouteEnvelope::capture(
        None,
        ApiProvider::Deepseek,
        "deepseek-primary",
        "deepseek-v4-flash",
        Some(ApiProvider::Deepseek.default_base_url()),
        Utc::now(),
    );
    for index in 0..90 {
        let path = match index % 3 {
            0 => "sink",
            1 => "mailbox",
            _ => "fallback",
        };
        assert!(append_routed_usage_record(
            &mut turn,
            &format!("{path}:response:{index}"),
            crate::cost_status::EffectiveRouteUsage {
                route: route.clone(),
                usage: Usage {
                    input_tokens: index,
                    ..Usage::default()
                },
            },
        ));
    }

    assert_eq!(turn.routed_usage.len(), MAX_ROUTED_USAGE_RECORDS_PER_TURN);
    assert_eq!(turn.routed_usage_dropped_records, 26);
    assert_eq!(turn.routed_usage_source_ids.len(), 90);
    let before = turn.clone();
    assert!(!append_routed_usage_record(
        &mut turn,
        "sink:response:0",
        crate::cost_status::EffectiveRouteUsage {
            route,
            usage: Usage {
                input_tokens: 999,
                ..Usage::default()
            },
        },
    ));
    assert_eq!(turn.routed_usage, before.routed_usage);
    assert_eq!(
        turn.routed_usage_dropped_records,
        before.routed_usage_dropped_records
    );
}

#[tokio::test]
async fn aggregate_usage_fails_closed_for_legacy_reconstructed_route() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = sample_thread("thr_legacy_cost");
    manager.store.save_thread(&thread)?;
    let mut turn = sample_turn(&thread.id, "turn_legacy_cost", RuntimeTurnStatus::Completed);
    turn.usage = Some(Usage {
        input_tokens: 1_000,
        output_tokens: 10,
        ..Usage::default()
    });
    // These mutable-era fields are intentionally insufficient: no exact
    // identity, billing mode, or dispatch timestamp was persisted.
    turn.effective_provider = Some(ApiProvider::Deepseek.as_str().to_string());
    turn.effective_model = Some("deepseek-v4-flash".to_string());
    manager.store.save_turn(&turn)?;

    let report = manager
        .aggregate_usage(None, None, UsageGroupBy::Thread)
        .await?;
    assert_eq!(report.totals.cost_usd, 0.0);
    assert_eq!(report.totals.unpriced_turns, 1);
    assert!(
        report
            .totals
            .unpriced_reasons
            .contains("unknown_provider_route")
    );
    assert!(report.totals.route_receipts.is_empty());
    Ok(())
}

#[test]
fn runtime_usage_accumulators_saturate_tokens_and_currency() {
    let mut total = f64::MAX;
    saturating_add_cost_amount(&mut total, f64::MAX);
    assert_eq!(total, f64::MAX);

    let thread = sample_thread("thr_saturating");
    let turn = sample_turn(&thread.id, "turn_saturating", RuntimeTurnStatus::Completed);
    let mut totals = UsageTotals {
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        cached_tokens: u64::MAX,
        reasoning_tokens: u64::MAX,
        reasoning_replay_tokens: u64::MAX,
        cache_write_tokens: u64::MAX,
        ..UsageTotals::default()
    };
    let mut buckets = std::collections::BTreeMap::new();
    accumulate_runtime_usage_record(
        &mut totals,
        &mut buckets,
        UsageGroupBy::Thread,
        None,
        &Usage {
            input_tokens: 1,
            output_tokens: 1,
            prompt_cache_hit_tokens: Some(1),
            prompt_cache_write_tokens: Some(1),
            reasoning_tokens: Some(1),
            reasoning_replay_tokens: Some(1),
            ..Usage::default()
        },
        &turn,
        &thread,
    );
    assert_eq!(totals.input_tokens, u64::MAX);
    assert_eq!(totals.output_tokens, u64::MAX);
    assert_eq!(totals.cached_tokens, u64::MAX);
    assert_eq!(totals.reasoning_tokens, u64::MAX);
    assert_eq!(totals.reasoning_replay_tokens, u64::MAX);
    assert_eq!(totals.cache_write_tokens, u64::MAX);
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
    tokio::time::timeout(TURN_SETTLEMENT_DEADLOCK_TIMEOUT, async {
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
        let timed_out = Instant::now() >= deadline;
        if terminal || timed_out {
            let receipt_is_durable =
                manager
                    .events_since(&turn.thread_id, None)?
                    .iter()
                    .any(|event| {
                        event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn_id)
                    });
            let active_claim_present = manager
                .active_turn_flags(&turn.thread_id, turn_id)
                .await
                .is_some();
            if terminal && receipt_is_durable && !active_claim_present {
                return Ok(turn);
            }
            if timed_out {
                bail!(
                    "Timed out waiting for turn {turn_id}: status={:?}, \
                     completion_receipt_present={receipt_is_durable}, \
                     active_claim_present={active_claim_present}",
                    turn.status
                );
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn store_load_scenario() {
    // Scenario consolidation of: store_load_thread_rejects_newer_schema_version, store_load_turn_rejects_newer_schema_version, store_load_item_rejects_newer_schema_version
    // from store_load_thread_rejects_newer_schema_version
    {
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
    // from store_load_turn_rejects_newer_schema_version
    {
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
    // from store_load_item_rejects_newer_schema_version
    {
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
}

#[test]
fn store_restart_discards_only_operation_bindings_that_never_acquired_a_turn() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let key_fingerprint = crate::hashing::sha256_hex("scoped-key");
    let binding = RuntimeTurnOperationBinding {
        schema_version: TURN_OPERATION_BINDING_SCHEMA_VERSION,
        thread_id: "thr_torn_binding".to_string(),
        turn_id: "turn_never_persisted".to_string(),
        operation_key_fingerprint: key_fingerprint.clone(),
        request_fingerprint: crate::hashing::sha256_hex("request"),
        created_at: Utc::now(),
    };
    store.save_turn_operation_binding(&binding)?;
    assert!(
        store
            .load_turn_operation_binding(&key_fingerprint)?
            .is_some()
    );

    drop(store);
    let reopened = RuntimeThreadStore::open(dir)?;
    assert!(
        reopened
            .load_turn_operation_binding(&key_fingerprint)?
            .is_none(),
        "a binding written before its turn is safe to discard because engine submission is later"
    );
    Ok(())
}

#[test]
fn runtime_event_sequences_serialize_across_real_processes() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let thread_id = "thr_process_writers";
    let start = dir.join("process-writers.start");
    let writer_count = 4_u64;
    let events_per_writer = 8_u64;
    let mut children = Vec::new();

    for worker in 0..writer_count {
        let signal = dir.join(format!("process-writer-{worker}.ready"));
        children.push((
            spawn_runtime_event_child("writer", &dir, thread_id, &signal),
            signal,
        ));
    }
    for (_, signal) in &children {
        wait_for_runtime_event_test_file(signal, "event writer readiness");
    }
    // The children already opened their stores before this barrier. The old
    // store-local sequence cache therefore allocated duplicates deterministically.
    std::fs::write(&start, b"go")?;
    for (child, _) in children {
        child.wait_success("cross-process event writer");
    }

    let events = store.events_since(thread_id, None)?;
    let expected = writer_count * events_per_writer;
    assert_eq!(u64::try_from(events.len())?, expected);
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=expected).collect::<Vec<_>>()
    );
    let identities = events
        .iter()
        .map(|event| {
            (
                event.payload["worker"].as_str().unwrap().to_string(),
                event.payload["index"].as_u64().unwrap(),
            )
        })
        .collect::<HashSet<_>>();
    assert_eq!(u64::try_from(identities.len())?, expected);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    assert_eq!(runtime.block_on(store.current_seq())?, expected);
    assert_eq!(
        load_runtime_store_state(&store.state_path)?.next_seq,
        expected + 1
    );

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn runtime_process_owner_lock_copy_helps_the_user_recover() {
    assert!(
        RUNTIME_PROCESS_OWNER_LOCK_HELD.contains("already active in another process"),
        "{RUNTIME_PROCESS_OWNER_LOCK_HELD}"
    );
    assert!(
        RUNTIME_PROCESS_OWNER_LOCK_HELD.contains("Close the other Codewhale session"),
        "{RUNTIME_PROCESS_OWNER_LOCK_HELD}"
    );
    assert!(
        RUNTIME_PROCESS_OWNER_LOCK_HELD.contains("CODEWHALE_RUNTIME_DIR"),
        "{RUNTIME_PROCESS_OWNER_LOCK_HELD}"
    );
    assert!(
        !RUNTIME_PROCESS_OWNER_LOCK_HELD.contains("thread store"),
        "{RUNTIME_PROCESS_OWNER_LOCK_HELD}"
    );
}

#[test]
fn runtime_manager_store_has_one_lifetime_process_owner() -> Result<()> {
    let dir = test_runtime_dir();
    let signal = dir.join("manager-owner.ready");
    let release = dir.join("process-writers.start");
    let child = spawn_runtime_event_child("manager-holder", &dir, "thr_unused", &signal);
    wait_for_runtime_event_test_file(&signal, "manager process owner readiness");

    let error = match test_manager(dir.clone()) {
        Ok(_) => panic!("a second process must not open the same Runtime manager store"),
        Err(error) => error,
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("already active in another process"),
        "unexpected owner-lock error: {error:#}"
    );
    assert!(
        message.contains("CODEWHALE_RUNTIME_DIR"),
        "owner-lock error should name the override for a shared root: {error:#}"
    );
    assert!(
        message.contains("Close the other Codewhale session"),
        "owner-lock error should name the recovery action: {error:#}"
    );
    assert!(
        !message.contains("thread store"),
        "owner-lock error should not leak the store implementation: {error:#}"
    );

    let distinct_dir = test_runtime_dir();
    let distinct = test_manager(distinct_dir)?;
    drop(distinct);

    std::fs::write(release, b"release")?;
    child.wait_success("Runtime manager owner holder");
    let reopened = test_manager(dir)?;
    drop(reopened);
    Ok(())
}

#[test]
fn session_scoped_scenario() -> Result<()> {
    // Scenario consolidation of: session_scoped_runtime_default_lives_under_the_session_directory, session_scoped_runtime_roots_do_not_share_the_process_owner_lock
    // from session_scoped_runtime_default_lives_under_the_session_directory
    {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("temp home");
        let home = temp.path().join("cw-home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let _runtime = crate::test_support::EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
        let _legacy = crate::test_support::EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

        let cfg = RuntimeThreadManagerConfig::for_session(home.join("tasks"), "sess-1");
        assert_eq!(
            cfg.data_dir,
            home.join("sessions").join("sess-1").join("runtime")
        );
    }
    // from session_scoped_runtime_roots_do_not_share_the_process_owner_lock
    {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("cw-home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let _runtime = crate::test_support::EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
        let _legacy = crate::test_support::EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");
        let tasks = home.join("tasks");

        let first = RuntimeThreadManager::open(
            Config::default(),
            PathBuf::from("."),
            RuntimeThreadManagerConfig::for_session(tasks.clone(), "session-a"),
        )?;
        let second = RuntimeThreadManager::open(
            Config::default(),
            PathBuf::from("."),
            RuntimeThreadManagerConfig::for_session(tasks, "session-b"),
        )?;
        drop(first);
        drop(second);
    }
    Ok(())
}

#[test]
fn explicit_runtime_dir_override_beats_session_scope() {
    let _lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().join("cw-home");
    let override_dir = temp.path().join("shared-runtime");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
    let _runtime = crate::test_support::EnvVarGuard::set("CODEWHALE_RUNTIME_DIR", &override_dir);
    let _legacy = crate::test_support::EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

    let cfg = RuntimeThreadManagerConfig::for_session(home.join("tasks"), "sess-1");
    assert_eq!(cfg.data_dir, override_dir);
}

#[test]
fn fresh_runtime_manager_store_race_has_exactly_one_process_owner() -> Result<()> {
    let control = test_runtime_dir();
    std::fs::create_dir_all(&control)?;
    let store_root = control.join("fresh-runtime-store");
    let start = control.join("manager-race.start");
    let release = control.join("manager-race.release");
    let signal_a = control.join("manager-race-a.signal");
    let signal_b = control.join("manager-race-b.signal");
    assert!(!store_root.exists(), "race must begin from an absent store");

    let child_a = spawn_runtime_manager_racer(&store_root, &signal_a, &start, &release);
    let child_b = spawn_runtime_manager_racer(&store_root, &signal_b, &start, &release);
    wait_for_runtime_event_test_value(&signal_a, &["ready"], "manager racer A readiness");
    wait_for_runtime_event_test_value(&signal_b, &["ready"], "manager racer B readiness");
    assert!(
        !store_root.exists(),
        "neither child may initialize owner-derived state before the common barrier"
    );

    std::fs::write(&start, b"go")?;
    let mut outcomes = vec![
        wait_for_runtime_event_test_value(&signal_a, &["won", "lost"], "manager racer A outcome"),
        wait_for_runtime_event_test_value(&signal_b, &["won", "lost"], "manager racer B outcome"),
    ];
    outcomes.sort();
    assert_eq!(outcomes, vec!["lost", "won"]);

    let owner_path = store_root.join(AGENT_MAIL_OWNER_FILE);
    let owner: RuntimeStoreOwner = serde_json::from_str(&read_store_file(&owner_path)?)?;
    validated_record_id(&owner.owner_id, "Runtime owner id")?;
    assert!(owner.owner_id.starts_with("owner_"));

    std::fs::write(&release, b"release")?;
    child_a.wait_success("fresh Runtime manager racer A");
    child_b.wait_success("fresh Runtime manager racer B");
    let reopened = test_manager(store_root)?;
    assert_eq!(reopened.test_store().owner_id, owner.owner_id);
    drop(reopened);
    std::fs::remove_dir_all(control)?;
    Ok(())
}

#[test]
fn events_from_offset_does_not_reread_earlier_records() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let thread_id = "thr_cursor";
    for index in 0..3 {
        store.append_event_transaction(
            thread_id.to_string(),
            None,
            None,
            "cursor.event".to_string(),
            json!({ "index": index }),
            Duration::from_secs(1),
        )?;
    }

    let (first, cursor) = store.events_from_offset(thread_id, 0, None)?;
    assert_eq!(first.len(), 3);
    assert!(cursor > 0);
    let (empty, same_cursor) = store.events_from_offset(thread_id, cursor, None)?;
    assert!(empty.is_empty());
    assert_eq!(same_cursor, cursor);

    store.append_event_transaction(
        thread_id.to_string(),
        None,
        None,
        "cursor.event".to_string(),
        json!({ "index": 3 }),
        Duration::from_secs(1),
    )?;
    let (tail, next_cursor) = store.events_from_offset(thread_id, cursor, None)?;
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].payload["index"], 3);
    assert!(next_cursor > cursor);

    let (limited, limited_cursor) = store.events_from_offset(thread_id, 0, Some(2))?;
    assert_eq!(limited.len(), 2);
    assert!(limited_cursor < next_cursor);
    let (rest, _) = store.events_from_offset(thread_id, limited_cursor, None)?;
    assert_eq!(rest.len(), 2);

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn runtime_event_lock_timeout_is_non_mutating_and_holder_death_releases_lock() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let thread_id = "thr_process_holder";
    let signal = dir.join("process-holder.ready");
    let child = spawn_runtime_event_child("holder", &dir, thread_id, &signal);
    wait_for_runtime_event_test_file(&signal, "held Runtime event lock");

    let state_before = std::fs::read(&store.state_path)?;
    let event_path = store.events_path(thread_id)?;
    let started = Instant::now();
    let error = store
        .append_event_transaction(
            thread_id.to_string(),
            None,
            None,
            "must.timeout".to_string(),
            json!({}),
            Duration::from_millis(75),
        )
        .expect_err("contended Runtime event transaction must time out");
    let elapsed = started.elapsed();
    assert!(error.downcast_ref::<RuntimeEventLockTimeout>().is_some());
    assert!(
        elapsed >= Duration::from_millis(75),
        "returned too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout was unbounded: {elapsed:?}"
    );
    assert_eq!(std::fs::read(&store.state_path)?, state_before);
    assert!(!event_path.exists());

    let status = child.kill_and_reap()?;
    assert!(
        !status.success(),
        "killed lock holder unexpectedly succeeded"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let committed = runtime.block_on(store.append_event(
        thread_id,
        None,
        None,
        "after.holder.death",
        json!({}),
    ))?;
    assert_eq!(committed.seq, 1);

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn killed_torn_append_is_repaired_and_preserves_reserved_gap() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let thread_id = "thr_process_torn";
    let signal = dir.join("process-torn.ready");
    let child = spawn_runtime_event_child("torn", &dir, thread_id, &signal);
    wait_for_runtime_event_test_file(&signal, "durable torn Runtime event tail");
    let status = child.kill_and_reap()?;
    assert!(
        !status.success(),
        "killed torn writer unexpectedly succeeded"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let committed = runtime.block_on(store.append_event(
        thread_id,
        None,
        None,
        "after.torn.death",
        json!({}),
    ))?;
    assert_eq!(committed.seq, 2);
    assert_eq!(runtime.block_on(store.current_seq())?, 2);
    let events = store.events_since(thread_id, None)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 2);
    let raw = std::fs::read(store.events_path(thread_id)?)?;
    assert_eq!(raw.last(), Some(&b'\n'));
    for line in raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        serde_json::from_slice::<RuntimeEventRecord>(line)?;
    }

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn event_reader_never_observes_cross_process_rollback_candidate() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    let thread_id = "thr_process_phantom";
    let signal = dir.join("process-phantom.ready");
    let child = spawn_runtime_event_child("phantom", &dir, thread_id, &signal);
    wait_for_runtime_event_test_file(&signal, "visible rollback candidate");

    let started = Instant::now();
    assert!(store.events_since(thread_id, None)?.is_empty());
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "reader did not wait for rollback disposition"
    );
    child.wait_success("phantom rollback child");
    assert!(store.events_since(thread_id, None)?.is_empty());

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn event_history_lock_wait_does_not_block_tokio_worker() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread_id = "thr_async_history_wait";
    let signal = dir.join("async-history-holder.ready");
    let child = spawn_runtime_event_child("holder", &dir, thread_id, &signal);
    wait_for_runtime_event_test_file(&signal, "async history lock holder");

    let history = manager.events_since_async(thread_id, None);
    tokio::pin!(history);
    tokio::select! {
        result = &mut history => panic!("history lock wait returned early: {result:?}"),
        () = sleep(Duration::from_millis(50)) => {}
    }
    let status = child.kill_and_reap()?;
    assert!(!status.success(), "killed history lock holder succeeded");
    assert!(history.await?.is_empty());

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn runtime_event_sequence_exhaustion_fails_before_mutation() -> Result<()> {
    let dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(dir.clone())?;
    store.with_event_transaction(Duration::from_secs(1), || {
        write_json_atomic(
            &store.state_path,
            &RuntimeStoreState {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                next_seq: u64::MAX,
            },
        )
    })?;
    let state_before = std::fs::read(&store.state_path)?;
    let thread_id = "thr_sequence_exhausted";
    let error = store
        .append_event_transaction(
            thread_id.to_string(),
            None,
            None,
            "must.not.append".to_string(),
            json!({}),
            Duration::from_secs(1),
        )
        .expect_err("u64 sequence exhaustion must fail closed");
    assert!(
        format!("{error:#}").contains("Runtime event sequence exhausted"),
        "unexpected exhaustion error: {error:#}"
    );
    assert_eq!(std::fs::read(&store.state_path)?, state_before);
    assert!(!store.events_path(thread_id)?.exists());

    std::fs::remove_dir_all(dir)?;
    Ok(())
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
fn event_append_rollback_handle_rejects_a_swapped_symlink_target() -> Result<()> {
    let dir = test_runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let outside = dir.join("outside.jsonl");
    let events_path = dir.join("events.jsonl");
    std::fs::write(&outside, b"must-remain-intact\n")?;
    std::os::unix::fs::symlink(&outside, &events_path)?;

    let error = open_runtime_store_file(&events_path, "Runtime event rollback", |options| {
        options.write(true);
    })
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
fn store_load_thread_defaults_missing_additive_runtime_defaults() {
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
    payload
        .as_object_mut()
        .expect("thread object")
        .remove("reasoning_effort");
    payload
        .as_object_mut()
        .expect("thread object")
        .remove("allowed_tools");
    std::fs::write(
        &path,
        serde_json::to_string(&payload).expect("encode thread"),
    )
    .expect("write thread");

    let loaded = store
        .load_thread(&thread.id)
        .expect("legacy thread should load");
    assert_eq!(loaded.session_id, None);
    assert_eq!(loaded.reasoning_effort, None);
    assert_eq!(loaded.allowed_tools, None);

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
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "check the files".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "need a tool".to_string(),
                    signature: Some("sig-1".to_string()),
                    state: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "shell".to_string(),
                    input: json!({ "cmd": "one" }),
                    caller: None,
                    thought_signature: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "shell".to_string(),
                    input: json!({ "cmd": "two" }),
                    caller: None,
                    thought_signature: None,
                },
            ],
        },
        Message {
            role: Role::User,
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
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-2".to_string(),
                content: "two".to_string(),
                is_error: Some(true),
                content_blocks: None,
            }],
        },
        Message {
            role: Role::Assistant,
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

/// A resumed session is seeded with one `Utc::now()` shared by every turn and
/// every item. `list_turns_for_thread` sorts on `created_at` and
/// `list_items_for_turn` sorts on `started_at`, so both sorts were a single
/// tie and the surviving order was whatever `read_dir` returned. That order
/// reaches users: `get_thread_detail` feeds the web dashboard's transcript,
/// and `fork_thread`/`fork_at_user_message` rebuild `item_ids` from it, which
/// makes the scramble durable. Seeded records must carry a total order.
#[tokio::test]
async fn seeded_session_records_carry_a_total_order() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread = sample_thread("thr_seed_order");
    manager.store.save_thread(&thread)?;
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "first question".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "consider".to_string(),
                    signature: None,
                    state: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "shell".to_string(),
                    input: json!({ "cmd": "ls" }),
                    caller: None,
                    thought_signature: None,
                },
            ],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "listing".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "first answer".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "second question".to_string(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "second answer".to_string(),
                cache_control: None,
            }],
        },
    ];

    manager
        .seed_thread_from_messages(&thread.id, &messages)
        .await?;

    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    assert_eq!(turns.len(), 2);
    assert!(
        turns[0].created_at < turns[1].created_at,
        "seeded turns must not tie on the only key list_turns_for_thread sorts by: {:?} vs {:?}",
        turns[0].created_at,
        turns[1].created_at
    );
    assert_eq!(turns[0].input_summary, "first question");
    assert_eq!(turns[1].input_summary, "second question");

    for turn in &turns {
        let items = manager.store.list_items_for_turn(&turn.id)?;
        for pair in items.windows(2) {
            assert!(
                pair[0].started_at < pair[1].started_at,
                "seeded items must not tie on the only key list_items_for_turn sorts by"
            );
        }
        let scanned = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            scanned, turn.item_ids,
            "the store scan must reproduce the order the turn recorded"
        );
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
                compaction_id: None,
            }),
            route_identity: crate::config::ProviderIdentity {
                provider: ApiProvider::Deepseek,
                key: "deepseek".to_string(),
                exact_id: Some("deepseek".to_string()),
                migrated_legacy_ollama_cloud_route: false,
            },
            route_model: DEFAULT_TEXT_MODEL.to_string(),
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
                compaction_id: None,
            }),
            route_identity: crate::config::ProviderIdentity {
                provider: ApiProvider::Deepseek,
                key: "deepseek".to_string(),
                exact_id: Some("deepseek".to_string()),
                migrated_legacy_ollama_cloud_route: false,
            },
            route_model: DEFAULT_TEXT_MODEL.to_string(),
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
fn approval_decision_scenario() {
    // Scenario consolidation of: approval_decision_keeps_trust_mode_out_of_tool_approval, approval_decision_requires_auto_approve_and_trust_for_full_access
    // from approval_decision_keeps_trust_mode_out_of_tool_approval
    {
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
    // from approval_decision_requires_auto_approve_and_trust_for_full_access
    {
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
async fn runtime_chat_event_read_flushes_restart_terminal_receipt_once() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(runtime_dir.clone())?;
    let thread = sample_thread("thr_relay_restart_receipt");
    let turn = sample_turn(
        &thread.id,
        "turn_relay_restart_receipt",
        RuntimeTurnStatus::InProgress,
    );
    store.save_thread(&thread)?;
    store.save_turn(&turn)?;
    drop(store);

    let manager = test_manager(runtime_dir)?;
    let events = manager.events_since_async(&thread.id, None).await?;
    let completed = events
        .iter()
        .filter(|event| {
            event.event == "turn.completed" && event.turn_id.as_deref() == Some(turn.id.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].payload["recovered"], true);
    assert_eq!(completed[0].payload["turn"]["status"], "interrupted");

    let replay = manager.events_since_async(&thread.id, None).await?;
    assert_eq!(
        replay
            .iter()
            .filter(|event| {
                event.event == "turn.completed"
                    && event.turn_id.as_deref() == Some(turn.id.as_str())
            })
            .count(),
        1,
        "restart recovery must append one stable terminal receipt"
    );
    Ok(())
}

#[tokio::test]
async fn restart_resumes_item_normalization_after_parent_was_already_interrupted() -> Result<()> {
    let runtime_dir = test_runtime_dir();
    let store = RuntimeThreadStore::open(runtime_dir.clone())?;
    let mut thread = sample_thread("thr_second_crash_recovery");
    thread.latest_turn_id = Some("turn_stale_pointer".to_string());
    let mut turn = sample_turn(
        &thread.id,
        "turn_second_crash_recovery",
        RuntimeTurnStatus::Interrupted,
    );
    turn.error = Some(RUNTIME_RESTART_REASON.to_string());
    turn.ended_at = Some(Utc::now());
    let item = sample_item(
        &turn.id,
        "item_second_crash_recovery",
        TurnItemLifecycleStatus::InProgress,
    );
    turn.item_ids = vec![item.id.clone()];
    store.save_thread(&thread)?;
    store.save_item(&item)?;
    store.save_turn(&turn)?;
    drop(store);

    let manager = test_manager(runtime_dir.clone())?;
    assert_eq!(
        manager.store.load_thread(&thread.id)?.latest_turn_id,
        Some(turn.id.clone()),
        "recovery must repair a turn committed before its thread pointer"
    );
    assert_eq!(
        manager.store.load_item(&item.id)?.status,
        TurnItemLifecycleStatus::Interrupted,
        "a second startup must finish item normalization even when the parent was committed first"
    );
    let first = manager.events_since_async(&thread.id, None).await?;
    assert_eq!(
        first
            .iter()
            .filter(|event| {
                event.event == "turn.completed"
                    && event.turn_id.as_deref() == Some(turn.id.as_str())
            })
            .count(),
        1
    );
    drop(manager);

    let reopened = test_manager(runtime_dir)?;
    assert_eq!(
        reopened.store.load_item(&item.id)?.status,
        TurnItemLifecycleStatus::Interrupted
    );
    let replay = reopened.events_since_async(&thread.id, None).await?;
    assert_eq!(
        replay
            .iter()
            .filter(|event| {
                event.event == "turn.completed"
                    && event.turn_id.as_deref() == Some(turn.id.as_str())
            })
            .count(),
        1,
        "another reopen must not duplicate the recovered terminal receipt"
    );
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
async fn monitor_separates_lifecycle_start_from_billing_dispatch_and_child_usage() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "capture immutable route".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));

    let started_at = Utc::now() - chrono::Duration::minutes(5);
    let dispatched_at = Utc::now() - chrono::Duration::seconds(2);
    let endpoint_fingerprint = "d".repeat(64);
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: "engine_route_receipt".to_string(),
            created_at: started_at,
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::RouteDispatched {
            turn_id: "engine_route_receipt".to_string(),
            route: crate::core::events::TurnRoute {
                provider: ApiProvider::Stepfun,
                provider_identity: "stepfun-payg-primary".to_string(),
                model: "step-3.7-flash".to_string(),
                auto_model: false,
                receipt: None,
                billing: Some(crate::core::events::RouteBillingEnvelope {
                    billing_surface: Some(crate::pricing::STEPFUN_PAYG_BILLING_SURFACE.to_string()),
                    endpoint_fingerprint: Some(endpoint_fingerprint.clone()),
                    billing_mode: crate::cost_status::RouteBillingMode::Metered,
                    dispatched_at,
                }),
                base_url: ApiProvider::Stepfun.default_base_url().to_string(),
                billing_product: crate::route_billing::RouteProduct::Unproven,
            },
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::SubAgentMailbox {
            owner_session_id: thread.id.clone(),
            turn_id: "engine_route_receipt".to_string(),
            seq: 77,
            message: crate::tools::subagent::MailboxMessage::TokenUsage {
                agent_id: "agent_child".to_string(),
                source_id: "response-child".to_string(),
                route: crate::cost_status::EffectiveRouteEnvelope {
                    provider: ApiProvider::OpenaiCodex,
                    provider_identity: "codex-child".to_string(),
                    model: "gpt-5.5".to_string(),
                    billing_surface: Some(
                        crate::pricing::OAUTH_SUBSCRIPTION_BILLING_SURFACE.to_string(),
                    ),
                    endpoint_fingerprint: None,
                    billing_mode: crate::cost_status::RouteBillingMode::Subscription,
                    dispatched_at,
                },
                usage: Usage {
                    input_tokens: 3,
                    output_tokens: 2,
                    reasoning_tokens: Some(2),
                    ..Usage::default()
                },
            },
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage {
                input_tokens: 10,
                output_tokens: 4,
                reasoning_replay_tokens: Some(6),
                server_tool_use: Some(crate::models::ServerToolUsage {
                    code_execution_requests: Some(2),
                    tool_search_requests: Some(3),
                }),
                ..Usage::default()
            },
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            // Completion-time endpoint evidence must be ignored.
            base_url: Some("https://api.stepfun.com/v1/coding".to_string()),
        })
        .await?;

    let completed =
        wait_for_terminal_turn(&manager, &turn.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
    assert_eq!(completed.effective_provider.as_deref(), Some("stepfun"));
    assert_eq!(
        completed.effective_provider_id.as_deref(),
        Some("stepfun-payg-primary")
    );
    assert_eq!(
        completed.effective_billing_surface.as_deref(),
        Some(crate::pricing::STEPFUN_PAYG_BILLING_SURFACE)
    );
    assert_eq!(
        completed.effective_endpoint_fingerprint.as_deref(),
        Some(endpoint_fingerprint.as_str())
    );
    assert_eq!(
        completed.effective_billing_mode,
        Some(crate::cost_status::RouteBillingMode::Metered)
    );
    assert_eq!(completed.effective_dispatched_at, Some(dispatched_at));
    assert_eq!(completed.started_at, Some(started_at));
    assert_eq!(completed.routed_usage.len(), 1);
    assert_eq!(completed.routed_usage[0].usage.reasoning_tokens, Some(2));
    let persisted_usage = completed.usage.expect("parent usage");
    assert_eq!(persisted_usage.reasoning_replay_tokens, Some(6));
    assert_eq!(
        persisted_usage.server_tool_use,
        Some(crate::models::ServerToolUsage {
            code_execution_requests: Some(2),
            tool_search_requests: Some(3),
        })
    );

    // A new engine mailbox starts at the same sequence number. Runtime
    // dedupe is scoped by the engine turn identity, so the second receipt is
    // persisted exactly once instead of colliding with the prior turn.
    let second = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "capture second mailbox".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage { .. })
    ));
    let second_engine_turn = "engine_route_receipt_second";
    harness
        .tx_event
        .send(EngineEvent::TurnStarted {
            turn_id: second_engine_turn.to_string(),
            created_at: Utc::now(),
            route: None,
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::SubAgentMailbox {
            owner_session_id: thread.id.clone(),
            turn_id: second_engine_turn.to_string(),
            seq: 77,
            message: crate::tools::subagent::MailboxMessage::TokenUsage {
                agent_id: "agent-child-second".to_string(),
                source_id: "response-child-second".to_string(),
                route: crate::cost_status::EffectiveRouteEnvelope {
                    provider: ApiProvider::OpenaiCodex,
                    provider_identity: "codex-child".to_string(),
                    model: "gpt-5.5".to_string(),
                    billing_surface: Some(
                        crate::pricing::OAUTH_SUBSCRIPTION_BILLING_SURFACE.to_string(),
                    ),
                    endpoint_fingerprint: None,
                    billing_mode: crate::cost_status::RouteBillingMode::Subscription,
                    dispatched_at: Utc::now(),
                },
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 1,
                    ..Usage::default()
                },
            },
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
    let second = wait_for_terminal_turn(&manager, &second.id, Duration::from_secs(2)).await?;
    assert_eq!(second.routed_usage.len(), 1);
    assert_eq!(second.routed_usage[0].usage.input_tokens, 5);
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
    let thread_id = thread.id.clone();
    tokio::spawn(async move {
        if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            let _ = tx_event
                .send(EngineEvent::AgentComplete {
                    owner_session_id: thread_id,
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
            compaction_id: None,
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

    let turn = manager
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
    assert_eq!(turn.permission_posture.as_deref(), Some("full_access"));

    match rx_op.recv().await {
        Some(Op::SendMessage {
            auto_approve,
            approval_mode,
            ..
        }) => {
            assert!(auto_approve);
            assert_eq!(approval_mode, crate::tui::approval::ApprovalMode::Bypass);
        }
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

    let turn = manager
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
    assert_eq!(turn.permission_posture.as_deref(), Some("ask"));

    match rx_op.recv().await {
        Some(Op::SendMessage {
            auto_approve,
            approval_mode,
            ..
        }) => {
            assert!(!auto_approve);
            assert_eq!(approval_mode, crate::tui::approval::ApprovalMode::Suggest);
        }
        other => panic!("expected SendMessage op, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn start_turn_enforces_and_records_auto_review_without_legacy_bypass() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            permission_posture: Some("ask".to_string()),
            ..Default::default()
        })
        .await?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;

    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "review autonomously".to_string(),
                permission_posture: Some("auto-review".to_string()),
                ..Default::default()
            },
        )
        .await?;

    assert_eq!(turn.permission_posture.as_deref(), Some("auto_review"));
    assert_eq!(
        manager
            .store
            .load_turn(&turn.id)?
            .permission_posture
            .as_deref(),
        Some("auto_review")
    );
    match rx_op.recv().await {
        Some(Op::SendMessage {
            auto_approve,
            approval_mode,
            ..
        }) => {
            assert!(!auto_approve);
            assert_eq!(approval_mode, crate::tui::approval::ApprovalMode::Auto);
        }
        other => panic!("expected SendMessage op, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn active_turn_permission_posture_switches_use_the_engine_live_authority() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            permission_posture: Some("ask".to_string()),
            ..Default::default()
        })
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "keep working while permissions change".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage {
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            ..
        })
    ));

    for (requested, canonical, expected_auto, expected_approval) in [
        (
            "auto-review",
            "auto_review",
            false,
            crate::tui::approval::ApprovalMode::Auto,
        ),
        (
            "full-access",
            "full_access",
            true,
            crate::tui::approval::ApprovalMode::Bypass,
        ),
        (
            "ask",
            "ask",
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        ),
    ] {
        let updated = manager
            .update_thread(
                &thread.id,
                UpdateThreadRequest {
                    permission_posture: Some(requested.to_string()),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(updated.permission_posture.as_deref(), Some(canonical));
        assert_eq!(updated.auto_approve, expected_auto);

        match harness.rx_op.recv().await {
            Some(Op::ChangeMode {
                auto_approve,
                approval_mode,
                ..
            }) => {
                assert_eq!(auto_approve, expected_auto, "{requested}");
                assert_eq!(approval_mode, expected_approval, "{requested}");
            }
            other => panic!("expected ChangeMode for {requested}, got {other:?}"),
        }

        let authority = {
            let active = manager.active.lock().await;
            active
                .engines
                .get(&thread.id)
                .expect("active engine")
                .engine
                .runtime_permission_authority()
        };
        assert_eq!(authority.auto_approve, expected_auto, "{requested}");
        assert_eq!(authority.approval_mode, expected_approval, "{requested}");
        assert_eq!(
            manager.active_turn_flags(&thread.id, &turn.id).await,
            Some((expected_auto, false)),
            "{requested} must replace the running turn's authority immediately"
        );
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

    match rx_op.recv().await {
        Some(Op::CompactContext { compaction, .. }) => {
            assert_eq!(
                compaction.runtime_cost_owner.as_deref(),
                Some(turn.id.as_str())
            );
        }
        other => panic!("expected CompactContext op, got {other:?}"),
    }
    assert_eq!(
        manager.active_turn_flags(&thread.id, &turn.id).await,
        Some((false, false))
    );

    Ok(())
}

#[tokio::test]
async fn compact_interrupt_persists_canceled_item_for_the_exact_request() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;

    let turn = manager
        .compact_thread(&thread.id, CompactThreadRequest::default())
        .await?;
    let compaction_id = match harness.rx_op.recv().await {
        Some(Op::CompactContext { id, .. }) => id,
        other => panic!("expected CompactContext op, got {other:?}"),
    };

    manager.interrupt_turn(&thread.id, &turn.id).await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::CancelCompaction { id }) if id == compaction_id
    ));

    harness
        .tx_event
        .send(EngineEvent::CompactionStarted {
            id: compaction_id.clone(),
            auto: false,
            message: "compaction started".to_string(),
        })
        .await?;
    harness
        .tx_event
        .send(EngineEvent::CompactionCancelled {
            id: compaction_id,
            auto: false,
            message: "compaction canceled".to_string(),
        })
        .await?;
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

    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Interrupted);
    let items = manager.store.list_items_for_turn(&turn.id)?;
    assert!(items.iter().any(|item| {
        item.kind == TurnItemKind::ContextCompaction
            && item.status == TurnItemLifecycleStatus::Canceled
    }));
    let events = manager.events_since(&thread.id, None)?;
    assert!(events.iter().any(|event| {
        event.event == "item.canceled"
            && event
                .payload
                .get("item")
                .and_then(|item| item.get("status"))
                .and_then(Value::as_str)
                == Some("canceled")
    }));
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
    let turn_1 =
        wait_for_terminal_turn(&manager, &turn_1.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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
    let turn_2 =
        wait_for_terminal_turn(&manager, &turn_2.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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

/// A minimal tool definition for `TurnComplete.tool_catalog` receipts in
/// mock-engine goal tests.
fn catalog_tool(name: &str) -> codewhale_core::request::Tool {
    codewhale_core::request::Tool {
        tool_type: None,
        name: name.to_string(),
        description: String::new(),
        input_schema: serde_json::json!({ "type": "object" }),
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

fn active_test_goal(thread_id: &str, goal_id: &str) -> codewhale_protocol::ThreadGoal {
    codewhale_protocol::ThreadGoal {
        thread_id: thread_id.to_string(),
        goal_id: goal_id.to_string(),
        objective: "ship the goal loop".to_string(),
        status: codewhale_protocol::ThreadGoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        created_at: 0,
        updated_at: 0,
    }
}

async fn wait_for_goal_status(
    manager: &RuntimeThreadManager,
    thread_id: &str,
    expected: codewhale_protocol::ThreadGoalStatus,
    timeout: Duration,
) -> Result<codewhale_protocol::ThreadGoal> {
    let deadline = Instant::now() + timeout;
    loop {
        let goal = manager
            .store
            .load_goal(thread_id)?
            .ok_or_else(|| anyhow::anyhow!("goal record missing for {thread_id}"))?;
        if goal.status == expected {
            return Ok(goal);
        }
        if Instant::now() > deadline {
            anyhow::bail!("goal did not reach {expected:?} in time: {goal:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until the thread has `expected` terminal turns in the durable store.
async fn wait_for_terminal_turn_count(
    manager: &RuntimeThreadManager,
    thread_id: &str,
    expected: usize,
    timeout: Duration,
) -> Result<Vec<TurnRecord>> {
    let deadline = Instant::now() + timeout;
    loop {
        let turns = manager.store.list_turns_for_thread(thread_id)?;
        let terminal = turns
            .iter()
            .filter(|turn| {
                matches!(
                    turn.status,
                    RuntimeTurnStatus::Completed
                        | RuntimeTurnStatus::Failed
                        | RuntimeTurnStatus::Interrupted
                        | RuntimeTurnStatus::Canceled
                )
            })
            .count();
        if terminal == expected {
            return Ok(turns);
        }
        if Instant::now() > deadline {
            anyhow::bail!("expected {expected} terminal turns, saw {terminal}: {turns:?}");
        }
        sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn host_goal_loop_kickoff_arms_one_continuation_and_parks_at_engine_cap() -> Result<()> {
    let config = Config {
        goal: Some(crate::config::GoalConfig {
            max_continuations: Some(2),
            continuation_delay_seconds: None,
        }),
        ..Config::default()
    };
    let manager = RuntimeThreadManager::open(
        config,
        PathBuf::from("."),
        test_manager_config(test_runtime_dir()),
    )?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    manager
        .store
        .save_goal(&active_test_goal(&thread.id, "goal_cap"))?;

    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    let passes = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let counter = passes.clone();
    tokio::spawn(async move {
        while let Some(op) = rx_op.recv().await {
            if !matches!(op, Op::SendMessage { .. }) {
                continue;
            }
            let pass = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            // Mirror the real engine: emit_goal_updated lands right before
            // TurnComplete, so settlement sees the final snapshot.
            let snapshot = crate::tools::goal::GoalSnapshot {
                objective: Some("ship the goal loop".to_string()),
                status: "active".to_string(),
                // The kickoff runs pass 0; the armed continuation is pass 2,
                // already at the configured cap.
                continuation_count: if pass == 1 { 0 } else { 2 },
                ..Default::default()
            };
            let _ = tx_event.send(EngineEvent::GoalUpdated { snapshot }).await;
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: format!("engine_goal_{pass}"),
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
                    content: format!("pass {pass}"),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageComplete { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 10,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: Some(vec![catalog_tool("update_goal")]),
                    base_url: None,
                })
                .await;
        }
    });

    manager.activate_thread_goal(&thread.id).await?;

    // Kickoff plus exactly one armed continuation, then the engine cap
    // parks the goal instead of arming a third pass.
    let turns =
        wait_for_terminal_turn_count(&manager, &thread.id, 2, TURN_SETTLEMENT_DEADLOCK_TIMEOUT)
            .await?;
    assert_eq!(turns.len(), 2);
    assert!(
        turns
            .iter()
            .all(|turn| turn.status == RuntimeTurnStatus::Completed)
    );
    assert_eq!(turns[0].input_summary, "goal kickoff");
    assert_eq!(turns[1].input_summary, "goal continuation pass #1");

    let goal = wait_for_goal_status(
        &manager,
        &thread.id,
        codewhale_protocol::ThreadGoalStatus::Paused,
        TURN_SETTLEMENT_DEADLOCK_TIMEOUT,
    )
    .await?;
    // Snapshot authority: the record keeps the engine's counter (2), and
    // both turns' usage was accrued.
    assert_eq!(goal.continuation_count, 2);
    assert_eq!(goal.tokens_used, 40);

    // Grace window for an (incorrect) third pass with delay 0.
    sleep(Duration::from_millis(400)).await;
    assert_eq!(
        passes.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "no pass may be armed after the engine cap parked the goal"
    );

    // Transcripts must read the kickoff as a kickoff, not "pass #0".
    let detail = manager.get_thread_detail(&thread.id).await?;
    assert!(detail.items.iter().any(|item| {
        item.kind == TurnItemKind::UserMessage
            && item.detail.as_deref() == Some("Goal kickoff (host-driven)")
    }));
    assert!(detail.items.iter().any(|item| {
        item.kind == TurnItemKind::UserMessage
            && item.detail.as_deref() == Some("Goal continuation pass #1 (host-driven)")
    }));
    Ok(())
}

#[tokio::test]
async fn host_goal_loop_skips_rearm_without_update_goal_and_after_failed_pass() -> Result<()> {
    // Scenario A: a completed pass whose catalog lacks `update_goal` has no
    // terminal path, so the host must not arm another pass.
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    manager
        .store
        .save_goal(&active_test_goal(&thread.id, "goal_no_update"))?;
    let harness = install_mock_engine(&manager, &thread.id).await;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    let passes = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let counter = passes.clone();
    tokio::spawn(async move {
        while let Some(op) = rx_op.recv().await {
            if !matches!(op, Op::SendMessage { .. }) {
                continue;
            }
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let snapshot = crate::tools::goal::GoalSnapshot {
                objective: Some("ship the goal loop".to_string()),
                status: "active".to_string(),
                ..Default::default()
            };
            let _ = tx_event.send(EngineEvent::GoalUpdated { snapshot }).await;
            let _ = tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_goal_no_update".to_string(),
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
                    content: "work without the goal tool".to_string(),
                })
                .await;
            let _ = tx_event
                .send(EngineEvent::MessageComplete { index: 0 })
                .await;
            let _ = tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 10,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Completed,
                    error: None,
                    tool_catalog: Some(vec![catalog_tool("read")]),
                    base_url: None,
                })
                .await;
        }
    });
    manager.activate_thread_goal(&thread.id).await?;
    wait_for_terminal_turn_count(&manager, &thread.id, 1, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
    // Settlement must have accrued usage before deciding; once visible, the
    // arming decision has been made.
    let deadline = Instant::now() + TURN_SETTLEMENT_DEADLOCK_TIMEOUT;
    let goal = loop {
        let goal = manager
            .store
            .load_goal(&thread.id)?
            .ok_or_else(|| anyhow::anyhow!("goal record missing"))?;
        if goal.tokens_used >= 20 || Instant::now() > deadline {
            break goal;
        }
        sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(goal.status, codewhale_protocol::ThreadGoalStatus::Active);
    sleep(Duration::from_millis(400)).await;
    assert_eq!(
        passes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no pass may be armed without update_goal in the catalog"
    );

    // Scenario B: a failed kickoff pass leaves the goal Active without
    // arming anything.
    let failed_thread = manager
        .create_thread(CreateThreadRequest::default())
        .await?;
    manager
        .store
        .save_goal(&active_test_goal(&failed_thread.id, "goal_failed_pass"))?;
    let failed_harness = install_mock_engine(&manager, &failed_thread.id).await;
    let mut failed_rx_op = failed_harness.rx_op;
    let failed_tx_event = failed_harness.tx_event;
    let failed_passes = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let failed_counter = failed_passes.clone();
    tokio::spawn(async move {
        while let Some(op) = failed_rx_op.recv().await {
            if !matches!(op, Op::SendMessage { .. }) {
                continue;
            }
            failed_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let snapshot = crate::tools::goal::GoalSnapshot {
                objective: Some("ship the goal loop".to_string()),
                status: "active".to_string(),
                ..Default::default()
            };
            let _ = failed_tx_event
                .send(EngineEvent::GoalUpdated { snapshot })
                .await;
            let _ = failed_tx_event
                .send(EngineEvent::TurnStarted {
                    turn_id: "engine_goal_failed".to_string(),
                    created_at: chrono::Utc::now(),
                    route: None,
                })
                .await;
            let _ = failed_tx_event
                .send(EngineEvent::TurnComplete {
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 5,
                        ..Usage::default()
                    },
                    status: TurnOutcomeStatus::Failed,
                    error: Some("provider exploded".to_string()),
                    tool_catalog: Some(vec![catalog_tool("update_goal")]),
                    base_url: None,
                })
                .await;
        }
    });
    manager.activate_thread_goal(&failed_thread.id).await?;
    let failed_turns = wait_for_terminal_turn_count(
        &manager,
        &failed_thread.id,
        1,
        TURN_SETTLEMENT_DEADLOCK_TIMEOUT,
    )
    .await?;
    assert_eq!(failed_turns[0].status, RuntimeTurnStatus::Failed);
    sleep(Duration::from_millis(400)).await;
    assert_eq!(
        failed_passes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a failed pass must not arm a continuation"
    );
    let failed_goal = manager
        .store
        .load_goal(&failed_thread.id)?
        .ok_or_else(|| anyhow::anyhow!("failed-pass goal record missing"))?;
    assert_eq!(
        failed_goal.status,
        codewhale_protocol::ThreadGoalStatus::Active
    );
    Ok(())
}

#[tokio::test]
async fn host_goal_loop_mirrors_terminal_snapshot_and_does_not_rearm() -> Result<()> {
    // A completed pass whose engine snapshot says `complete` (or `blocked`)
    // is mirrored into the durable record and arms nothing more: a restarted
    // host must not resume a goal the verifier already closed.
    let manager = test_manager(test_runtime_dir())?;
    for (goal_id, engine_status, expected) in [
        (
            "goal_complete",
            "complete",
            codewhale_protocol::ThreadGoalStatus::Complete,
        ),
        (
            "goal_blocked",
            "blocked",
            codewhale_protocol::ThreadGoalStatus::Blocked,
        ),
    ] {
        let thread = manager
            .create_thread(CreateThreadRequest::default())
            .await?;
        manager
            .store
            .save_goal(&active_test_goal(&thread.id, goal_id))?;
        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        let passes = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let counter = passes.clone();
        let status = engine_status.to_string();
        tokio::spawn(async move {
            while let Some(op) = rx_op.recv().await {
                if !matches!(op, Op::SendMessage { .. }) {
                    continue;
                }
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let snapshot = crate::tools::goal::GoalSnapshot {
                    objective: Some("ship the goal loop".to_string()),
                    status: status.clone(),
                    continuation_count: 1,
                    ..Default::default()
                };
                let _ = tx_event.send(EngineEvent::GoalUpdated { snapshot }).await;
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: format!("engine_{status}"),
                        created_at: chrono::Utc::now(),
                        route: None,
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 7,
                            output_tokens: 3,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                        tool_catalog: Some(vec![catalog_tool("update_goal")]),
                        base_url: None,
                    })
                    .await;
            }
        });
        manager.activate_thread_goal(&thread.id).await?;
        let goal = wait_for_goal_status(
            &manager,
            &thread.id,
            expected,
            TURN_SETTLEMENT_DEADLOCK_TIMEOUT,
        )
        .await?;
        // Usage and the engine's continuation counter landed on the record.
        assert_eq!(goal.tokens_used, 10);
        assert_eq!(goal.continuation_count, 1);
        sleep(Duration::from_millis(400)).await;
        assert_eq!(
            passes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a terminal snapshot must not arm another pass ({engine_status})"
        );
        let turns = manager.store.list_turns_for_thread(&thread.id)?;
        assert_eq!(
            turns.len(),
            1,
            "exactly the kickoff turn ran ({engine_status})"
        );
    }
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
async fn auto_review_force_prompt_is_denied_without_opening_a_modal() -> Result<()> {
    let manager = test_manager(test_runtime_dir())?;
    let thread = manager
        .create_thread(CreateThreadRequest {
            permission_posture: Some("auto-review".to_string()),
            ..Default::default()
        })
        .await?;
    let mut harness = install_mock_engine(&manager, &thread.id).await;
    let turn = manager
        .start_turn(
            &thread.id,
            StartTurnRequest {
                prompt: "review the gated action".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert!(matches!(
        harness.rx_op.recv().await,
        Some(Op::SendMessage {
            approval_mode: crate::tui::approval::ApprovalMode::Auto,
            ..
        })
    ));

    harness
        .tx_event
        .send(EngineEvent::ApprovalRequired {
            approval_key: "auto_hold".to_string(),
            approval_grouping_key: "auto_hold".to_string(),
            id: "tool_auto_hold".to_string(),
            tool_name: "exec_command".to_string(),
            description: "policy hold under auto review".to_string(),
            input: serde_json::json!({}),
            intent_summary: None,
            approval_force_prompt: true,
        })
        .await?;

    let decision = tokio::time::timeout(Duration::from_secs(2), harness.recv_approval_event())
        .await
        .context("Auto-Review hold should resolve without a modal")?;
    assert_eq!(
        decision,
        Some(MockApprovalEvent::Denied {
            id: "tool_auto_hold".to_string(),
        })
    );
    assert_eq!(manager.pending_approvals_count(), 0);
    assert!(manager.events_since(&thread.id, None)?.iter().any(|event| {
        event.event == "approval.decided"
            && event.payload.get("approval_id").and_then(Value::as_str) == Some("tool_auto_hold")
            && event.payload.get("posture").and_then(Value::as_str) == Some("auto_review")
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
    let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
    assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
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
    let terminal =
        wait_for_terminal_turn(&manager, &turn.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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
    let terminal =
        wait_for_terminal_turn(&manager, &turn.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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

    let terminal =
        wait_for_terminal_turn(&manager, &turn.id, TURN_SETTLEMENT_DEADLOCK_TIMEOUT).await?;
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
    let deadline = Instant::now() + TURN_SETTLEMENT_DEADLOCK_TIMEOUT;
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
                    let _ = tx_event
                        .send(EngineEvent::CompactionCompleted {
                            id: "auto_compact_1".to_string(),
                            auto: true,
                            message: "auto compact done".to_string(),
                            messages_before: Some(7),
                            messages_after: Some(3),
                            summary_prompt: None,
                            post_input_tokens: None,
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
                            post_input_tokens: None,
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

    // The manual compact carried a summary_prompt → it must be persisted into
    // the thread record so engine reloads restore it. The auto compact carried
    // None → exactly one summary section, from the manual pass.
    let record = manager.get_thread(&thread.id).await?;
    let record_prompt = record.system_prompt.expect("record keeps a system prompt");
    assert!(record_prompt.contains(COMPACTION_SUMMARY_BEGIN));
    assert!(record_prompt.contains("Conversation Summary (Auto-Generated)"));
    assert!(record_prompt.contains("key facts."));
    assert_eq!(record_prompt.matches(COMPACTION_SUMMARY_BEGIN).count(), 1);
    Ok(())
}

#[test]
fn summarize_text_truncates() {
    let out = summarize_text("abcdefghijklmnopqrstuvwxyz", 10);
    assert_eq!(out, "abcdefg...");
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
        reasoning_effort: None,
        allowed_tools: None,
        workspace: PathBuf::from("."),
        mode: "agent".to_string(),
        permission_posture: None,
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
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: vec![completed_item.id.clone(), in_progress_item.id.clone()],
        steer_count: 0,
        agent_mail_message_id: None,
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
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: vec![queued_item.id.clone()],
        steer_count: 0,
        agent_mail_message_id: None,
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
fn parse_mode_scenario() {
    // Scenario consolidation of: parse_mode_defaults_to_agent, parse_mode_opt_resolves_explicit_tokens_and_aliases, parse_mode_opt_rejects_prompt_fragments, parse_mode_wrapper_defaults_and_resolves_numeric_aliases
    // from parse_mode_defaults_to_agent
    {
        assert_eq!(parse_mode("unknown"), AppMode::Agent);
        assert_eq!(parse_mode("plan"), AppMode::Plan);
    }
    // from parse_mode_opt_resolves_explicit_tokens_and_aliases
    {
        assert_eq!(parse_mode_opt("agent"), Some(AppMode::Agent));
        assert_eq!(parse_mode_opt("1"), Some(AppMode::Agent));
        assert_eq!(parse_mode_opt("plan"), Some(AppMode::Plan));
        assert_eq!(parse_mode_opt("2"), Some(AppMode::Plan));
        assert_eq!(parse_mode_opt("auto"), Some(AppMode::Agent));
        assert_eq!(parse_mode_opt("operate"), Some(AppMode::Operate));
        assert_eq!(parse_mode_opt("3"), Some(AppMode::Operate));
        // Legacy YOLO spellings resolve to Act; the posture travels separately.
        assert_eq!(parse_mode_opt("yolo"), Some(AppMode::Agent));
        assert_eq!(parse_mode_opt("4"), Some(AppMode::Agent));
        assert_eq!(parse_mode_opt(" PLAN "), Some(AppMode::Plan));
    }
    // from parse_mode_opt_rejects_prompt_fragments
    {
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
    // from parse_mode_wrapper_defaults_and_resolves_numeric_aliases
    {
        assert_eq!(parse_mode("plan a trip to Tokyo"), AppMode::Agent);
        assert_eq!(parse_mode("auto"), AppMode::Agent);
        assert_eq!(parse_mode("1"), AppMode::Agent);
        assert_eq!(parse_mode("2"), AppMode::Plan);
        assert_eq!(parse_mode("3"), AppMode::Operate);
        assert_eq!(parse_mode("4"), AppMode::Agent);
    }
}

#[test]
fn mode_only_override_preserves_legacy_full_access_posture() -> Result<()> {
    let mut thread = sample_thread("thr_legacy_full_access");
    thread.permission_posture = None;
    thread.auto_approve = true;

    let policy = runtime_policy_with_overrides(&thread, Some("plan"), None, None)?;
    assert_eq!(policy.mode, AppMode::Plan);
    assert_eq!(policy.permission_wire(), "full_access");

    let ask = runtime_policy_with_overrides(&thread, Some("act"), None, Some(false))?;
    assert_eq!(ask.permission_wire(), "ask");
    Ok(())
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
            permission_posture: None,
            effective_provider: None,
            effective_provider_id: None,
            effective_billing_surface: None,
            effective_endpoint_fingerprint: None,
            effective_billing_mode: None,
            effective_dispatched_at: None,
            effective_model: None,
            routed_usage: Vec::new(),
            routed_usage_source_ids: Vec::new(),
            routed_usage_dropped_records: 0,
            error: None,
            item_ids: vec![user_item_id, asst_item_id],
            steer_count: 0,
            agent_mail_message_id: None,
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

// ── compaction summary persistence (merge_summary_into_prompt) ──

#[test]
fn summary_merge_scenario() {
    // Scenario consolidation of: summary_merge_appends_section_to_base_prompt, summary_merge_replaces_existing_section_idempotently, summary_merge_handles_missing_base
    // from summary_merge_appends_section_to_base_prompt
    {
        let merged = merge_summary_into_prompt(
            Some("You are a helpful agent."),
            "## 📋 Conversation Summary (Auto-Generated)\n\nUser prefers lists.",
        );
        assert!(merged.starts_with("You are a helpful agent."));
        assert!(merged.contains(COMPACTION_SUMMARY_BEGIN));
        assert!(merged.contains("User prefers lists."));
        assert!(merged.ends_with(COMPACTION_SUMMARY_END));
        // Reload restore keys on the marker: SyncSession migrates this carrier
        // into one ordinary history checkpoint before provider dispatch.
        assert!(merged.contains("Conversation Summary (Auto-Generated)"));
    }
    // from summary_merge_replaces_existing_section_idempotently
    {
        let first = merge_summary_into_prompt(Some("Base prompt."), "summary v1");
        let second = merge_summary_into_prompt(Some(&first), "summary v2");
        assert!(second.contains("summary v2"));
        assert!(!second.contains("summary v1"));
        assert_eq!(
            second.matches(COMPACTION_SUMMARY_BEGIN).count(),
            1,
            "repeated compactions must swap the section, not stack duplicates"
        );
        assert!(second.starts_with("Base prompt."));
    }
    // from summary_merge_handles_missing_base
    {
        let merged = merge_summary_into_prompt(None, "only summary");
        assert!(merged.starts_with(COMPACTION_SUMMARY_BEGIN));
        assert!(merged.contains("only summary"));
        let empty_base = merge_summary_into_prompt(Some(""), "only summary");
        assert!(empty_base.starts_with(COMPACTION_SUMMARY_BEGIN));
    }
}

#[test]
fn summary_strip_scenario() {
    // Scenario consolidation of: summary_strip_preserves_text_after_section, summary_strip_handles_missing_end_sentinel
    // from summary_strip_preserves_text_after_section
    {
        let with_tail = format!(
            "Base.\n\n{COMPACTION_SUMMARY_BEGIN}\nold summary\n{COMPACTION_SUMMARY_END}\n\nTrailing rules."
        );
        let stripped = strip_summary_section(&with_tail);
        assert!(stripped.contains("Base."));
        assert!(stripped.contains("Trailing rules."));
        assert!(!stripped.contains("old summary"));
        // Re-merge keeps the tail intact.
        let merged = merge_summary_into_prompt(Some(&with_tail), "new summary");
        assert!(merged.contains("Trailing rules."));
        assert!(merged.contains("new summary"));
    }
    // from summary_strip_handles_missing_end_sentinel
    {
        let broken = format!("Base.\n\n{COMPACTION_SUMMARY_BEGIN}\ntruncated…");
        let stripped = strip_summary_section(&broken);
        assert_eq!(stripped, "Base.");
    }
}

/// Release acceptance: the full two-task Agent Mail matrix in one run, with
/// evidence written to disk. Run explicitly:
///   CODEWHALE_MAIL_ACCEPTANCE_DIR=/Volumes/VIXinSSD/CW/artifacts/... \
///     cargo test -p codewhale-tui --lib agent_mail_release_acceptance -- --ignored
#[tokio::test]
#[ignore = "release acceptance: run explicitly with CODEWHALE_MAIL_ACCEPTANCE_DIR"]
async fn agent_mail_release_acceptance_two_task_matrix() -> Result<()> {
    use codewhale_protocol::agent_mail::AGENT_MAIL_EVENT_READ;
    use std::time::Instant as StdInstant;

    let runtime_dir = test_runtime_dir();
    let workspace = runtime_dir.join("workspace-shared");
    let foreign_workspace = runtime_dir.join("workspace-foreign");
    fs::create_dir_all(&workspace)?;
    fs::create_dir_all(&foreign_workspace)?;
    let artifacts = std::env::var("CODEWHALE_MAIL_ACCEPTANCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| runtime_dir.join("acceptance-artifacts"));
    fs::create_dir_all(&artifacts)?;

    let manager = test_manager(runtime_dir.clone())?;
    let task_a = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(workspace.clone()),
            task_id: Some("task_a".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let task_b = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(workspace),
            task_id: Some("task_b".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let task_foreign = manager
        .create_thread(CreateThreadRequest {
            workspace: Some(foreign_workspace),
            task_id: Some("task_foreign".to_string()),
            ..CreateThreadRequest::default()
        })
        .await?;
    let harness_b = install_mock_engine(&manager, &task_b.id).await;

    let credential_summary = "Handoff: the staging token OPENAI_API_KEY=[REDACTED-VALUE] \
        must rotate; scratch DB dump lives under /tmp/private/staging.sql. \
        Verify the deploy checklist before the next release cut.";

    // (1) Queue-only mail A -> B; B persists it without sampling (no turn).
    let m1 = manager
        .queue_agent_mail(agent_mail_request(
            "mail_accept_m1",
            &task_a.id,
            &task_b.id,
            "task_a",
            credential_summary,
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await?;
    assert!(!m1.idempotent_replay);
    assert_eq!(m1.envelope.status, AgentMailStatus::Queued);
    let serialized = serde_json::to_string(&m1.envelope)?;
    assert!(!serialized.contains("sk-fixture-secret"), "{serialized}");
    assert!(!serialized.contains("/tmp/private"), "{serialized}");
    assert!(serialized.contains("redacted-credential"), "{serialized}");
    assert!(serialized.contains("redacted-path"), "{serialized}");
    assert!(
        manager.store.list_turns_for_thread(&task_b.id)?.is_empty(),
        "queue-only mail must not sample the destination"
    );

    // (2) Replaying the same envelope is idempotent — no duplicate event.
    let m1_replay = manager
        .queue_agent_mail(agent_mail_request(
            "mail_accept_m1",
            &task_a.id,
            &task_b.id,
            "task_a",
            credential_summary,
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await?;
    assert!(m1_replay.idempotent_replay);
    assert_eq!(m1_replay.envelope, m1.envelope);
    assert_eq!(
        mail_event_count(&manager, &task_b.id, AGENT_MAIL_EVENT_QUEUED),
        1
    );

    // (3) Unauthorized cross-workspace delivery fails closed.
    let denied = manager
        .queue_agent_mail(agent_mail_request(
            "mail_accept_foreign",
            &task_a.id,
            &task_foreign.id,
            "task_a",
            "cross workspace attempt",
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await
        .expect_err("cross-workspace mail must fail closed")
        .to_string();
    assert!(denied.contains("ownership denied"), "{denied}");
    assert!(
        manager
            .list_agent_mail_for_thread(&task_foreign.id)
            .await?
            .is_empty()
    );

    // (4) A real terminal turn on B is the safe boundary: the wake envelope
    // delivers exactly once; the queue-only envelope stays queued.
    let m2 = manager
        .queue_agent_mail(agent_mail_request(
            "mail_accept_m2",
            &task_a.id,
            &task_b.id,
            "task_a",
            "Wake: the fixture suite now passes; land the follow-up step.",
            AgentMailDeliveryMode::WakeAtSafeBoundary,
        ))
        .await?;
    let explicit_turn = manager
        .start_turn(
            &task_b.id,
            StartTurnRequest {
                prompt: "ordinary work turn on task B".to_string(),
                ..StartTurnRequest::default()
            },
        )
        .await?;
    harness_b
        .tx_event
        .send(EngineEvent::MessageComplete { index: 0 })
        .await?;
    harness_b
        .tx_event
        .send(EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: TurnOutcomeStatus::Completed,
            error: None,
            tool_catalog: None,
            base_url: None,
        })
        .await?;
    let explicit_terminal = wait_for_terminal_turn(
        &manager,
        &explicit_turn.id,
        TURN_SETTLEMENT_DEADLOCK_TIMEOUT,
    )
    .await?;
    assert_eq!(explicit_terminal.status, RuntimeTurnStatus::Completed);

    let deadline = StdInstant::now() + Duration::from_secs(10);
    let (m2_delivered, wake_turns) = loop {
        let inbox = manager.list_agent_mail_for_thread(&task_b.id).await?;
        let envelope = inbox
            .iter()
            .find(|mail| mail.message_id.as_str() == "mail_accept_m2")
            .expect("wake envelope present");
        let turns = manager
            .store
            .list_turns_for_thread(&task_b.id)?
            .into_iter()
            .filter(|turn| turn.agent_mail_message_id.as_deref() == Some("mail_accept_m2"))
            .count();
        if envelope.status == AgentMailStatus::Delivered && turns == 1 {
            break (envelope.clone(), turns);
        }
        if StdInstant::now() > deadline {
            anyhow::bail!(
                "wake delivery did not settle: status={:?} turns={turns}",
                envelope.status
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        wake_turns, 1,
        "wake mail must produce exactly one follow-up"
    );
    assert!(m2_delivered.delivery_turn_id.is_some());
    assert_eq!(
        mail_event_count(&manager, &task_b.id, AGENT_MAIL_EVENT_DELIVERING),
        1
    );
    assert_eq!(
        mail_event_count(&manager, &task_b.id, AGENT_MAIL_EVENT_DELIVERED),
        1
    );
    let inbox_after_boundary = manager.list_agent_mail_for_thread(&task_b.id).await?;
    let m1_after = inbox_after_boundary
        .iter()
        .find(|mail| mail.message_id.as_str() == "mail_accept_m1")
        .expect("queue-only envelope present");
    assert_eq!(
        m1_after.status,
        AgentMailStatus::Queued,
        "queue-only mail must wait for an explicit delivery request"
    );

    // (5) Read transition publishes exactly one READ event.
    let m2_read = manager
        .mark_agent_mail_read(&task_b.id, &m2.envelope.message_id)
        .await?;
    assert_eq!(m2_read.status, AgentMailStatus::Read);
    assert!(m2_read.read_at.is_some());
    assert_eq!(
        mail_event_count(&manager, &task_b.id, AGENT_MAIL_EVENT_READ),
        1
    );

    // (6) Queued mail survives a full runtime restart; replay stays idempotent.
    let m3 = manager
        .queue_agent_mail(agent_mail_request(
            "mail_accept_m3",
            &task_a.id,
            &task_b.id,
            "task_a",
            "Post-restart handoff: re-run the acceptance matrix.",
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await?;
    drop(manager);
    let reopened = test_manager(runtime_dir.clone())?;
    let inbox_restarted = reopened.list_agent_mail_for_thread(&task_b.id).await?;
    assert!(
        inbox_restarted
            .iter()
            .any(|mail| mail.message_id == m3.envelope.message_id
                && mail.status == AgentMailStatus::Queued)
    );
    assert!(
        inbox_restarted
            .iter()
            .any(|mail| mail.message_id == m2.envelope.message_id
                && mail.status == AgentMailStatus::Read)
    );
    let m1_restart_replay = reopened
        .queue_agent_mail(agent_mail_request(
            "mail_accept_m1",
            &task_a.id,
            &task_b.id,
            "task_a",
            credential_summary,
            AgentMailDeliveryMode::QueueOnly,
        ))
        .await?;
    assert!(m1_restart_replay.idempotent_replay);
    // one queued event per envelope (m1, m2, m3) — replay added none
    assert_eq!(
        mail_event_count(&reopened, &task_b.id, AGENT_MAIL_EVENT_QUEUED),
        3
    );

    // (7) Evidence: sanitized envelopes, persisted events, turn receipts,
    // restart state, and a SHA-256 manifest.
    let envelopes = reopened.list_agent_mail_for_thread(&task_b.id).await?;
    let envelope_dump = serde_json::to_string_pretty(&envelopes)?;
    assert!(!envelope_dump.contains("sk-fixture-secret"));
    assert!(!envelope_dump.contains("/tmp/private"));
    fs::write(artifacts.join("envelopes-b.sanitized.json"), &envelope_dump)?;

    let events = reopened.events_since(&task_b.id, None)?;
    let mut event_lines = String::new();
    for event in &events {
        event_lines.push_str(&serde_json::to_string(event)?);
        event_lines.push('\n');
    }
    assert!(!event_lines.contains("sk-fixture-secret"));
    fs::write(artifacts.join("events-b.jsonl"), &event_lines)?;

    let turns_b = reopened.store.list_turns_for_thread(&task_b.id)?;
    let turn_receipts: Vec<_> = turns_b
        .iter()
        .map(|turn| {
            serde_json::json!({
                "turn_id": turn.id,
                "status": turn.status,
                "agent_mail_message_id": turn.agent_mail_message_id,
                "input_summary": turn.input_summary,
                "created_at": turn.created_at,
            })
        })
        .collect();
    let stub_request_counts = serde_json::json!({
        "task_b_explicit_user_turns": 1,
        "task_b_agent_mail_follow_up_turns": 1,
        "task_b_turns_total": turn_receipts.len(),
        "task_a_provider_requests": 0,
        "note": "mock engine channel; queue-only mail sampled zero turns",
    });
    fs::write(
        artifacts.join("turns-b.json"),
        serde_json::to_string_pretty(&turn_receipts)?,
    )?;
    fs::write(
        artifacts.join("stub-request-counts.json"),
        serde_json::to_string_pretty(&stub_request_counts)?,
    )?;
    fs::write(
        artifacts.join("restart-inbox-b.json"),
        serde_json::to_string_pretty(&inbox_restarted)?,
    )?;
    fs::write(
        artifacts.join("boundary-receipt.md"),
        format!(
            "# Agent Mail acceptance boundary receipt\n\n\
             explicit turn {} -> {:?}\n\
             wake envelope {} -> {:?} (delivery_turn_id={:?})\n\
             queue-only envelope {} stayed {:?} across the boundary\n",
            explicit_terminal.id,
            explicit_terminal.status,
            m2.envelope.message_id,
            m2_delivered.status,
            m2_delivered.delivery_turn_id,
            m1.envelope.message_id,
            m1_after.status,
        ),
    )?;

    let mut manifest = serde_json::Map::new();
    for name in [
        "envelopes-b.sanitized.json",
        "events-b.jsonl",
        "turns-b.json",
        "stub-request-counts.json",
        "restart-inbox-b.json",
        "boundary-receipt.md",
    ] {
        let bytes = fs::read(artifacts.join(name))?;
        manifest.insert(
            name.to_string(),
            serde_json::json!({
                "sha256": crate::hashing::sha256_hex(&bytes),
                "bytes": bytes.len(),
            }),
        );
    }
    fs::write(
        artifacts.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::Value::Object(manifest))?,
    )?;
    Ok(())
}

fn mail_event_count(manager: &RuntimeThreadManager, thread_id: &str, event_name: &str) -> usize {
    manager
        .events_since(thread_id, None)
        .expect("events readable")
        .into_iter()
        .filter(|event| event.event == event_name)
        .count()
}

/// Regression for #5823: after a Runtime restart, history is rebuilt from the
/// persisted turn-item snapshots. The live engine path stores the tool-call
/// identity in item metadata and overwrites `detail` with the tool output at
/// completion, so the rebuild must restore a paired, non-empty
/// tool_call/tool_result from metadata instead of an empty shell.
#[test]
fn restart_rebuild_restores_tool_call_identity_from_persisted_items() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread = sample_thread("thr_rebuild_5823");
    manager.store.save_thread(&thread)?;

    let now = Utc::now();
    let user_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_u5823".to_string(),
        turn_id: "turn_5823".to_string(),
        kind: TurnItemKind::UserMessage,
        status: TurnItemLifecycleStatus::Completed,
        summary: "read the readme".to_string(),
        detail: Some("read the readme".to_string()),
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(now),
        ended_at: Some(now),
    };
    // Completed live tool call: identity captured at `ToolCallStarted`, result
    // markers merged at `ToolCallComplete`, `detail` holding the tool output.
    let call_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_t5823".to_string(),
        turn_id: "turn_5823".to_string(),
        kind: TurnItemKind::ToolCall,
        status: TurnItemLifecycleStatus::Completed,
        summary: "read: readme body".to_string(),
        detail: Some("# README body".to_string()),
        metadata: Some(json!({
            "tool_use_id": "call_00_abc",
            "tool_name": "read",
            "tool_input": r#"{"path":"README.md"}"#,
            "tool_result_for": "call_00_abc",
            "is_error": false,
        })),
        artifact_refs: Vec::new(),
        started_at: Some(now),
        ended_at: Some(now),
    };
    manager.store.save_item(&user_item)?;
    manager.store.save_item(&call_item)?;
    manager.store.save_turn(&TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "turn_5823".to_string(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::Completed,
        input_summary: "read the readme".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: None,
        usage: None,
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: vec![user_item.id.clone(), call_item.id.clone()],
        steer_count: 0,
        agent_mail_message_id: None,
    })?;

    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    let messages = manager.reconstruct_messages_from_turns(&turns)?;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user", "assistant", "user"]);

    match &messages[1].content[0] {
        ContentBlock::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "call_00_abc");
            assert_eq!(name, "read");
            // This exact string is the provider `tool_calls[].function.arguments`
            // value; the old bug replayed "null" with empty id/name.
            assert_eq!(
                serde_json::to_string(input).unwrap(),
                r#"{"path":"README.md"}"#
            );
        }
        other => panic!("expected restored tool call, got {other:?}"),
    }
    match &messages[2].content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => {
            assert_eq!(tool_use_id, "call_00_abc");
            assert_eq!(content, "# README body");
        }
        other => panic!("expected restored tool result, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

/// A tool call snapshot persisted at `ToolCallStarted` but never completed
/// (restart mid-tool) still carries its identity and must rebuild the call
/// with non-empty id/name/arguments instead of an empty shell.
#[test]
fn restart_rebuild_keeps_in_flight_tool_call_identity() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread = sample_thread("thr_rebuild_5823_inflight");
    manager.store.save_thread(&thread)?;

    let now = Utc::now();
    let call_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_inflight5823".to_string(),
        turn_id: "turn_5823_inflight".to_string(),
        kind: TurnItemKind::ToolCall,
        status: TurnItemLifecycleStatus::InProgress,
        summary: "read started".to_string(),
        detail: Some(r#"{"path":"README.md"}"#.to_string()),
        metadata: Some(json!({
            "tool_use_id": "call_00_def",
            "tool_name": "read",
            "tool_input": r#"{"path":"README.md"}"#,
        })),
        artifact_refs: Vec::new(),
        started_at: Some(now),
        ended_at: None,
    };
    manager.store.save_item(&call_item)?;
    manager.store.save_turn(&TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "turn_5823_inflight".to_string(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::Failed,
        input_summary: "read the readme".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: None,
        usage: None,
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: vec![call_item.id.clone()],
        steer_count: 0,
        agent_mail_message_id: None,
    })?;

    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    let messages = manager.reconstruct_messages_from_turns(&turns)?;
    match &messages[0].content[0] {
        ContentBlock::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "call_00_def");
            assert_eq!(name, "read");
            assert_eq!(
                serde_json::to_string(input).unwrap(),
                r#"{"path":"README.md"}"#
            );
        }
        other => panic!("expected in-flight tool call identity, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

/// Snapshots persisted before tool identity was durable (metadata absent)
/// must contribute nothing to rebuilt history: an empty id/name tool_call
/// with `arguments: "null"` is rejected by strict OpenAI-compatible
/// endpoints, so degrading silently is safer than replaying the shell.
#[test]
fn restart_rebuild_skips_legacy_tool_items_without_identity() -> Result<()> {
    let dir = test_runtime_dir();
    let manager = test_manager(dir.clone())?;
    let thread = sample_thread("thr_rebuild_5823_legacy");
    manager.store.save_thread(&thread)?;

    let now = Utc::now();
    let user_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_legacy_u".to_string(),
        turn_id: "turn_5823_legacy".to_string(),
        kind: TurnItemKind::UserMessage,
        status: TurnItemLifecycleStatus::Completed,
        summary: "hello".to_string(),
        detail: Some("hello".to_string()),
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(now),
        ended_at: Some(now),
    };
    let legacy_tool_item = TurnItemRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "item_legacy_t".to_string(),
        turn_id: "turn_5823_legacy".to_string(),
        kind: TurnItemKind::ToolCall,
        status: TurnItemLifecycleStatus::Completed,
        summary: "read: old output".to_string(),
        detail: Some("old output".to_string()),
        metadata: None,
        artifact_refs: Vec::new(),
        started_at: Some(now),
        ended_at: Some(now),
    };
    manager.store.save_item(&user_item)?;
    manager.store.save_item(&legacy_tool_item)?;
    manager.store.save_turn(&TurnRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: "turn_5823_legacy".to_string(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::Completed,
        input_summary: "hello".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: None,
        usage: None,
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: Vec::new(),
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: vec![user_item.id.clone(), legacy_tool_item.id.clone()],
        steer_count: 0,
        agent_mail_message_id: None,
    })?;

    let turns = manager.store.list_turns_for_thread(&thread.id)?;
    let messages = manager.reconstruct_messages_from_turns(&turns)?;
    let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["user"]);
    for message in &messages {
        for block in &message.content {
            assert!(
                !matches!(
                    block,
                    ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                ),
                "legacy snapshot must not rebuild a tool block, got {block:?}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
