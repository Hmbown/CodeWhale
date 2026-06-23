use super::*;
use crate::core::engine::{MockApprovalEvent, mock_engine_handle};
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
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

fn sample_thread(thread_id: &str) -> ThreadRecord {
    let now = Utc::now();
    ThreadRecord {
        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
        id: thread_id.to_string(),
        created_at: now,
        updated_at: now,
        model: DEFAULT_TEXT_MODEL.to_string(),
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
        error: None,
        item_ids: Vec::new(),
        steer_count: 0,
    }
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
    let mut active = manager.active.lock().await;
    active.engines.insert(
        thread_id.to_string(),
        ActiveThreadState {
            engine: harness.handle.clone(),
            active_turn: None,
        },
    );
    touch_lru(&mut active.lru, thread_id);
    harness
}

async fn wait_for_terminal_turn(
    manager: &RuntimeThreadManager,
    turn_id: &str,
    timeout: Duration,
) -> Result<TurnRecord> {
    let deadline = Instant::now() + timeout;
    loop {
        let turn = manager.store.load_turn(turn_id)?;
        if matches!(
            turn.status,
            RuntimeTurnStatus::Completed
                | RuntimeTurnStatus::Failed
                | RuntimeTurnStatus::Interrupted
                | RuntimeTurnStatus::Canceled
        ) {
            return Ok(turn);
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
fn approval_decision_matches_auto_approve_and_trust_mode() {
    assert!(matches!(
        RuntimeThreadManager::approval_decision(false, false, false),
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

    assert!(matches!(rx_op.recv().await, Some(Op::CompactContext)));
    assert_eq!(
        manager.active_turn_flags(&thread.id, &turn.id).await,
        Some((false, false))
    );

    Ok(())
}

#[tokio::test]
async fn compact_thread_with_real_engine_reaches_terminal_status() -> Result<()> {
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

    let deadline = Instant::now() + Duration::from_secs(2);
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
            _ => break,
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
                Op::CompactContext => {
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
    assert_eq!(parse_mode_opt("yolo"), Some(AppMode::Yolo));
    assert_eq!(parse_mode_opt("3"), Some(AppMode::Yolo));
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
    assert_eq!(parse_mode("1"), AppMode::Agent);
    assert_eq!(parse_mode("2"), AppMode::Plan);
    assert_eq!(parse_mode("3"), AppMode::Yolo);
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
