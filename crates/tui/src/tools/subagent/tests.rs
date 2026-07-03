use super::*;
use crate::tools::{AgentToolSurfaceOptions, ToolRegistryBuilder};
use crate::worker_profile::ShellPolicy;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;

fn make_assignment() -> SubAgentAssignment {
    SubAgentAssignment::new("prompt".to_string(), Some("worker".to_string()))
}

fn make_snapshot(status: SubAgentStatus) -> SubAgentResult {
    SubAgentResult {
        name: "agent_test".to_string(),
        agent_id: "agent_test".to_string(),
        context_mode: "fresh".to_string(),
        fork_context: false,
        workspace: None,
        git_branch: None,
        agent_type: SubAgentType::General,
        assignment: make_assignment(),
        model: "deepseek-v4-flash".to_string(),
        nickname: None,
        status,
        worker_status: None,
        parent_run_id: None,
        spawn_depth: 0,
        result: None,
        steps_taken: 0,
        checkpoint: None,
        needs_input: None,
        duration_ms: 0,
        from_prior_session: false,
    }
}

fn make_worker_spec(worker_id: &str, workspace: PathBuf) -> AgentWorkerSpec {
    let tool_profile =
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "grep_files".to_string()]);
    let mut runtime_profile = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
    runtime_profile.tools =
        ToolScope::Explicit(vec!["read_file".to_string(), "grep_files".to_string()]);
    runtime_profile.model = ModelRoute::Fixed("deepseek-v4-flash".to_string());
    runtime_profile.max_spawn_depth = DEFAULT_MAX_SPAWN_DEPTH.saturating_sub(1);
    AgentWorkerSpec {
        worker_id: worker_id.to_string(),
        run_id: worker_id.to_string(),
        parent_run_id: None,
        session_name: Some(worker_id.to_string()),
        objective: "inspect the repo".to_string(),
        role: Some("explorer".to_string()),
        agent_type: SubAgentType::Explore,
        model: "deepseek-v4-flash".to_string(),
        workspace,
        git_branch: None,
        context_mode: "fresh".to_string(),
        fork_context: false,
        tool_profile,
        runtime_profile,
        max_steps: 8,
        spawn_depth: 1,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
    }
}

#[test]
fn headless_worker_record_tracks_lifecycle_without_tui_projection() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    manager.register_worker(make_worker_spec(
        "agent_worker_contract",
        tmp.path().to_path_buf(),
    ));

    manager.record_worker_event(
        "agent_worker_contract",
        AgentWorkerStatus::Queued,
        Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
        None,
        None,
    );
    manager.record_worker_progress(
        "agent_worker_contract",
        "step 1: requesting model response".to_string(),
    );
    manager.record_worker_progress(
        "agent_worker_contract",
        "step 1: running tool 'read_file'".to_string(),
    );

    let mut result = make_snapshot(SubAgentStatus::Completed);
    result.agent_id = "agent_worker_contract".to_string();
    result.name = "agent_worker_contract".to_string();
    result.result = Some("worker summary".to_string());
    result.steps_taken = 1;
    manager.complete_worker_from_result("agent_worker_contract", &result);

    let record = manager
        .get_worker_record("agent_worker_contract")
        .expect("worker record");
    assert_eq!(record.status, AgentWorkerStatus::Completed);
    assert_eq!(record.spec.run_id, "agent_worker_contract");
    assert_eq!(record.actor_kind, "subagent");
    assert_eq!(record.spec.agent_type, SubAgentType::Explore);
    assert_eq!(
        record.spec.tool_profile,
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "grep_files".to_string()])
    );
    assert_eq!(record.spec.runtime_profile.role, SubAgentType::Explore);
    assert!(!record.spec.runtime_profile.permissions.write);
    assert_eq!(
        record.spec.runtime_profile.tools,
        ToolScope::Explicit(vec!["read_file".to_string(), "grep_files".to_string()])
    );
    assert_eq!(
        record.spec.runtime_profile.model,
        ModelRoute::Fixed("deepseek-v4-flash".to_string())
    );
    assert_eq!(record.result_summary.as_deref(), Some("worker summary"));
    assert_eq!(record.steps_taken, 1);
    assert_eq!(record.follow_up.tool, "handle_read");
    assert_eq!(record.follow_up.agent_id.as_str(), "agent_worker_contract");
    assert_eq!(record.recommended_action.action, "verify_self_report");
    assert_eq!(
        record.recommended_action.tool.as_deref(),
        Some("handle_read")
    );
    assert!(record.takeover.supported);
    assert!(
        record
            .takeover
            .instructions
            .contains("transcript_handle with handle_read")
    );
    assert_eq!(record.usage.status, "unknown");
    assert_eq!(record.verification.status, "self_report_only");
    assert!(
        record
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "transcript")
    );
    let statuses: Vec<_> = record.events.iter().map(|event| event.status).collect();
    assert!(statuses.contains(&AgentWorkerStatus::Queued));
    assert!(statuses.contains(&AgentWorkerStatus::ModelWait));
    assert!(statuses.contains(&AgentWorkerStatus::RunningTool));
    assert!(statuses.contains(&AgentWorkerStatus::Completed));
    assert!(
        record
            .events
            .iter()
            .any(|event| event.tool_name.as_deref() == Some("read_file"))
    );
}

#[test]
fn worker_record_usage_accumulates_provider_tokens() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    manager.register_worker(make_worker_spec("agent_usage", tmp.path().to_path_buf()));

    manager.record_worker_usage(
        "agent_usage",
        &Usage {
            input_tokens: 100,
            output_tokens: 25,
            prompt_cache_hit_tokens: Some(70),
            prompt_cache_miss_tokens: Some(30),
            ..Usage::default()
        },
    );
    manager.record_worker_usage(
        "agent_usage",
        &Usage {
            input_tokens: 40,
            output_tokens: 10,
            ..Usage::default()
        },
    );

    let record = manager
        .get_worker_record("agent_usage")
        .expect("worker record");
    assert_eq!(record.usage.status, "reported");
    assert_eq!(record.usage.input_tokens, Some(140));
    assert_eq!(record.usage.output_tokens, Some(35));
    assert_eq!(record.usage.total_tokens, Some(175));
    assert_eq!(record.usage.token_budget, None);
    assert!(
        record.usage.note.contains("175 tokens"),
        "usage note includes reported total: {}",
        record.usage.note
    );
}

#[test]
fn token_budget_scope_is_shared_across_nested_workers_and_blocks_when_spent() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut manager =
        SubAgentManager::new(workspace.clone(), 4).with_default_token_budget(Some(100));

    manager.register_worker(make_worker_spec("agent_root", workspace.clone()));
    let root_scope = manager
        .resolve_spawn_budget_scope("agent_root", None, None)
        .expect("root budget resolves")
        .expect("root budget present");
    manager.attach_budget_scope("agent_root", root_scope);
    manager.record_worker_usage(
        "agent_root",
        &Usage {
            input_tokens: 40,
            output_tokens: 10,
            ..Usage::default()
        },
    );

    let mut child_spec = make_worker_spec("agent_child", workspace);
    child_spec.parent_run_id = Some("agent_root".to_string());
    let child_scope = manager
        .resolve_spawn_budget_scope("agent_child", Some("agent_root"), None)
        .expect("child inherits budget")
        .expect("child budget present");
    assert_eq!(child_scope.scope_id, "agent_root");
    assert_eq!(child_scope.limit, 100);
    assert_eq!(child_scope.spent, 50);
    manager.register_worker(child_spec);
    manager.attach_budget_scope("agent_child", child_scope);
    manager.record_worker_usage(
        "agent_child",
        &Usage {
            input_tokens: 30,
            output_tokens: 20,
            ..Usage::default()
        },
    );

    let root = manager.get_worker_record("agent_root").expect("root");
    let child = manager.get_worker_record("agent_child").expect("child");
    assert_eq!(root.usage.budget_spent_tokens, Some(100));
    assert_eq!(child.usage.budget_spent_tokens, Some(100));
    assert_eq!(root.usage.budget_remaining_tokens, Some(0));
    assert_eq!(child.usage.budget_remaining_tokens, Some(0));
    assert_eq!(root.usage.status, "budget_exhausted");

    let err = manager
        .resolve_spawn_budget_scope("agent_grandchild", Some("agent_child"), None)
        .expect_err("spent shared budget blocks further child spawn");
    assert!(
        err.to_string().contains("token budget exhausted"),
        "actionable exhaustion error: {err}"
    );

    let override_scope = manager
        .resolve_spawn_budget_scope("agent_override", Some("agent_child"), Some(20))
        .expect("explicit override starts new scope")
        .expect("override budget present");
    assert_eq!(override_scope.scope_id, "agent_override");
    assert_eq!(override_scope.limit, 20);
    assert_eq!(override_scope.spent, 0);
}

#[test]
fn agent_worker_profile_derives_from_parent_without_escalation() {
    let mut runtime = stub_runtime();
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
    runtime.spawn_depth = 1;
    runtime.max_spawn_depth = DEFAULT_MAX_SPAWN_DEPTH;
    let tool_profile =
        AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(), "write_file".to_string()]);

    let profile = worker_profile_for_spawn(
        &runtime,
        &SubAgentType::Implementer,
        &tool_profile,
        "deepseek-v4-pro",
        Some(ModelRoute::Fixed("deepseek-v4-pro".to_string())),
    );

    assert_eq!(profile.role, SubAgentType::Implementer);
    assert!(
        !profile.permissions.write,
        "child cannot gain write permission from a read-only parent profile"
    );
    assert_eq!(profile.shell, ShellPolicy::ReadOnly);
    assert_eq!(profile.max_spawn_depth, DEFAULT_MAX_SPAWN_DEPTH - 1);
    assert_eq!(
        profile.model,
        ModelRoute::Fixed("deepseek-v4-pro".to_string())
    );
    assert_eq!(
        profile.tools,
        ToolScope::Explicit(vec!["read_file".to_string(), "write_file".to_string()])
    );
}

#[test]
fn subagent_progress_displays_shell_tools_as_bash() {
    assert_eq!(subagent_progress_tool_display_name("exec_shell"), "Bash");
    assert_eq!(subagent_progress_tool_display_name("exec_wait"), "Bash");
    assert_eq!(
        subagent_progress_tool_display_name("task_shell_wait"),
        "Bash"
    );
    assert_eq!(
        subagent_progress_tool_display_name("read_file"),
        "read_file"
    );
}

#[test]
fn agent_progress_preserves_event_channel_headroom_under_load() {
    let (tx, mut rx) = mpsc::channel(40);
    for _ in 0..8 {
        tx.try_send(Event::status("filler")).expect("fill channel");
    }
    assert_eq!(tx.capacity(), 32);

    emit_agent_progress(
        Some(&tx),
        "agent_busy",
        "step 1: requesting model response".to_string(),
        None,
        1,
    );
    assert_eq!(
        tx.capacity(),
        32,
        "routine progress should preserve reserved event-channel headroom"
    );

    emit_agent_progress(
        Some(&tx),
        "agent_waiting",
        "waiting for user input".to_string(),
        None,
        1,
    );
    assert_eq!(
        tx.capacity(),
        31,
        "high-value progress should still reach the UI when headroom is reserved"
    );

    for _ in 0..8 {
        assert!(matches!(rx.try_recv(), Ok(Event::Status { .. })));
    }
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::AgentProgress { id, status, .. })
            if id == "agent_waiting" && status == "waiting for user input"
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn agent_progress_uses_small_event_channels_without_headroom_reservation() {
    let (tx, mut rx) = mpsc::channel(8);

    emit_agent_progress(
        Some(&tx),
        "agent_small_channel",
        "step 1: requesting model response".to_string(),
        None,
        1,
    );

    assert_eq!(tx.capacity(), 7);
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::AgentProgress { id, status, .. })
            if id == "agent_small_channel" && status == "step 1: requesting model response"
    ));
}

#[test]
fn headless_worker_records_persist_with_subagent_state() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());
    manager.register_worker(make_worker_spec(
        "agent_persisted",
        tmp.path().to_path_buf(),
    ));

    let mut result = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    result.agent_id = "agent_persisted".to_string();
    result.name = "agent_persisted".to_string();
    result.steps_taken = 3;
    manager.complete_worker_from_result("agent_persisted", &result);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut loaded = SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    loaded.load_state().expect("load state");

    let record = loaded.get_worker_record("agent_persisted").expect("record");
    assert_eq!(record.spec.run_id, "agent_persisted");
    assert_eq!(record.follow_up.agent_id, "agent_persisted");
    assert!(record.takeover.supported);
    assert_eq!(record.status, AgentWorkerStatus::Failed);
    assert_eq!(record.error.as_deref(), Some("boom"));
    assert_eq!(record.steps_taken, 3);
    assert!(
        record
            .events
            .iter()
            .any(|event| event.status == AgentWorkerStatus::Failed)
    );
}

fn init_subagent_git_repo() -> tempfile::TempDir {
    let dir = tempdir().expect("tempdir");

    let init = Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("git init should run");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let autocrlf = Command::new("git")
        .args(["config", "core.autocrlf", "false"])
        .current_dir(dir.path())
        .output()
        .expect("git config core.autocrlf should run");
    assert!(
        autocrlf.status.success(),
        "git config core.autocrlf failed: {}",
        String::from_utf8_lossy(&autocrlf.stderr)
    );

    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ])
        .current_dir(dir.path())
        .output()
        .expect("git commit should run");
    assert!(
        commit.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    dir
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn text_message(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
    }
}

fn make_checkpoint(agent_id: &str, steps_taken: u32, messages: Vec<Message>) -> SubAgentCheckpoint {
    build_subagent_checkpoint(agent_id, "test_checkpoint", &messages, steps_taken, true)
}

fn message_text(message: &Message) -> &str {
    match message.content.first() {
        Some(ContentBlock::Text { text, .. }) => text.as_str(),
        other => panic!("expected text content block, got {other:?}"),
    }
}

async fn delayed_chat_client(
    first_delay: Duration,
    response_text: &str,
) -> (
    DeepSeekClient,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<Value>>>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            let bodies = Arc::clone(&bodies);
            move |Json(body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let bodies = Arc::clone(&bodies);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    bodies
                        .lock()
                        .expect("request body recorder mutex poisoned")
                        .push(body);
                    if attempt == 1 {
                        tokio::time::sleep(first_delay).await;
                    }
                    Json(json!({
                        "id": format!("chatcmpl-test-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake chat client");
    (client, calls, bodies)
}

async fn transient_header_timeout_then_success_chat_client(
    response_text: &str,
) -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt == 1 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": {
                                    "message": "SSE stream request did not receive response headers after 45s"
                                }
                            })),
                        )
                            .into_response();
                    }
                    Json(json!({
                        "id": format!("chatcmpl-test-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2
                        }
                    }))
                    .into_response()
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake transient chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake transient chat client");
    (client, calls)
}

fn estimate_tool_description_tokens_conservative(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

#[test]
fn test_agent_type_from_str() {
    assert_eq!(
        SubAgentType::from_str("general"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("explore"),
        Some(SubAgentType::Explore)
    );
    assert_eq!(SubAgentType::from_str("PLAN"), Some(SubAgentType::Plan));
    assert_eq!(
        SubAgentType::from_str("code-review"),
        Some(SubAgentType::Review)
    );
    assert_eq!(
        SubAgentType::from_str("worker"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("default"),
        Some(SubAgentType::General)
    );
    assert_eq!(
        SubAgentType::from_str("explorer"),
        Some(SubAgentType::Explore)
    );
    assert_eq!(SubAgentType::from_str("awaiter"), Some(SubAgentType::Plan));
    assert_eq!(SubAgentType::from_str("invalid"), None);
}

#[test]
fn test_agent_type_implementer_aliases() {
    // #404 — Implementer accepts the obvious aliases the model is
    // likely to reach for when the user says "build this".
    for alias in ["implementer", "implement", "implementation", "builder"] {
        assert_eq!(
            SubAgentType::from_str(alias),
            Some(SubAgentType::Implementer),
            "alias {alias} should resolve to Implementer"
        );
    }
    // Case-insensitive.
    assert_eq!(
        SubAgentType::from_str("IMPLEMENTER"),
        Some(SubAgentType::Implementer)
    );
}

#[test]
fn test_agent_type_verifier_aliases() {
    // #404 — Verifier accepts test/validate aliases distinct from
    // Reviewer, which is for *grading* code rather than *running* it.
    for alias in ["verifier", "verify", "verification", "validator", "tester"] {
        assert_eq!(
            SubAgentType::from_str(alias),
            Some(SubAgentType::Verifier),
            "alias {alias} should resolve to Verifier"
        );
    }
    assert_eq!(
        SubAgentType::from_str("VERIFY"),
        Some(SubAgentType::Verifier)
    );
}

#[test]
fn test_agent_type_round_trips_via_as_str() {
    // Every type should serialize to a string that round-trips back
    // through `from_str`. Catches missed variants when adding a new
    // role.
    for t in [
        SubAgentType::General,
        SubAgentType::Explore,
        SubAgentType::Plan,
        SubAgentType::Review,
        SubAgentType::Implementer,
        SubAgentType::Verifier,
        SubAgentType::Custom,
    ] {
        let label = t.as_str();
        let back = SubAgentType::from_str(label)
            .unwrap_or_else(|| panic!("as_str label {label:?} doesn't round-trip via from_str"));
        assert_eq!(back, t, "round-trip failed for {t:?} via {label:?}");
    }
}

#[test]
fn test_implementer_and_verifier_have_distinct_prompts() {
    // The whole point of adding the types is that they carry distinct
    // posture. Defensive guard: catch the easy bug where copy-paste
    // leaves two new variants with the same prompt as `General`.
    let implementer = SubAgentType::Implementer.system_prompt();
    let verifier = SubAgentType::Verifier.system_prompt();
    let general = SubAgentType::General.system_prompt();
    assert_ne!(
        implementer, general,
        "Implementer prompt must differ from General"
    );
    assert_ne!(
        verifier, general,
        "Verifier prompt must differ from General"
    );
    assert_ne!(
        implementer, verifier,
        "Implementer and Verifier must differ"
    );
    // Sanity: each prompt mentions the role's defining verb so the
    // model has clear direction.
    assert!(
        implementer.to_lowercase().contains("implement")
            || implementer.to_lowercase().contains("write the code"),
        "Implementer prompt should reference its role: {implementer}"
    );
    assert!(
        verifier.to_lowercase().contains("verif")
            || verifier.to_lowercase().contains("test suite")
            || verifier.to_lowercase().contains("validation"),
        "Verifier prompt should reference its role: {verifier}"
    );
}

#[test]
fn test_agent_type_prompts_include_shared_output_contract_once() {
    for (agent_type, marker) in [
        (SubAgentType::General, "general-purpose sub-agent"),
        (SubAgentType::Explore, "exploration sub-agent"),
        (SubAgentType::Plan, "planning sub-agent"),
        (SubAgentType::Review, "code review sub-agent"),
        (SubAgentType::Implementer, "implementation sub-agent"),
        (SubAgentType::Verifier, "verification sub-agent"),
        (SubAgentType::Custom, "custom sub-agent"),
    ] {
        let prompt = agent_type.system_prompt();
        assert!(prompt.contains(marker));
        assert_eq!(
            prompt.matches("## Output contract (mandatory)").count(),
            1,
            "{agent_type:?} prompt should include the shared output contract exactly once"
        );
        assert!(prompt.contains("### SUMMARY") && prompt.contains("### BLOCKERS"));
    }
}

#[test]
fn explore_prompt_orients_before_searching() {
    let prompt = SubAgentType::Explore.system_prompt();
    assert!(prompt.contains("role: `explore`"));
    assert!(prompt.contains("AGENTS.md/README"));
    assert!(prompt.contains("workspace/project root"));
    assert!(prompt.contains("compressed reconnaissance"));
}

#[test]
fn explore_prompt_is_quick_bounded_and_read_only() {
    let prompt = SubAgentType::Explore.system_prompt();
    assert!(prompt.contains("Default to `EFFORT: quick`"));
    assert!(prompt.contains("3-5 tool calls"));
    assert!(prompt.contains("strictly read-only"));
    assert!(prompt.contains("ALREADY_KNOWN"));
    assert!(prompt.contains("STOP_CONDITION"));
    assert!(prompt.contains("Return partial findings"));
}

#[test]
fn implementer_prompt_is_not_forced_into_explorer_cap() {
    let prompt = SubAgentType::Implementer.system_prompt();
    assert!(prompt.contains("not limited to an explorer-style 3-5 tool-call cap"));
    assert!(prompt.contains("Checkpoint before expanding scope"));
    assert!(!prompt.contains("Default to `EFFORT: quick`"));
}

#[test]
fn review_and_verifier_prompts_stop_after_decisive_evidence() {
    let review = SubAgentType::Review.system_prompt();
    let verifier = SubAgentType::Verifier.system_prompt();
    assert!(review.contains("stop after decisive evidence"));
    assert!(verifier.contains("stop after decisive pass/fail evidence"));
}

#[test]
fn agent_description_explains_background_child_and_transcript_handle() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let tool = AgentTool::new(manager, stub_runtime());
    let description = tool.description();

    assert!(description.contains("Start, inspect, peek at, or cancel focused child agent tasks"));
    assert!(description.contains("runs or queues"));
    assert!(description.contains("provider rate-limit"));
    assert!(description.contains("background"));
    assert!(description.contains("transcript_handle"));
    assert!(
        estimate_tool_description_tokens_conservative(description) <= 1024,
        "agent description exceeds the conservative 1024-token budget"
    );
}

#[test]
fn new_session_tools_use_single_agent_name() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    assert_eq!(AgentTool::new(manager, stub_runtime()).name(), "agent");
}

#[test]
fn test_implementer_allowed_tools_include_writes() {
    // Implementer is the write-heavy role; the deprecated
    // `allowed_tools()` advisory list should reflect that the role
    // can write/edit/patch even if today's runtime grants full
    // inheritance.
    #[allow(deprecated)]
    let tools = SubAgentType::Implementer.allowed_tools();
    assert!(tools.contains(&"write_file"));
    assert!(tools.contains(&"edit_file"));
    assert!(tools.contains(&"apply_patch"));
}

#[test]
fn test_verifier_allowed_tools_include_test_runner_but_no_writes() {
    // Verifier runs validation; it should not have write tools in
    // its advisory list. The runtime will still gate writes through
    // approval, but the advisory list signals intent.
    #[allow(deprecated)]
    let tools = SubAgentType::Verifier.allowed_tools();
    assert!(tools.contains(&"run_tests"));
    assert!(tools.contains(&"run_verifiers"));
    assert!(tools.contains(&"diagnostics"));
    assert!(!tools.contains(&"write_file"));
    assert!(!tools.contains(&"apply_patch"));
}

#[test]
fn test_parse_spawn_request_accepts_message_and_agent_type_aliases() {
    let input = json!({
        "message": "Find references to Foo",
        "agent_type": "explorer"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.prompt, "Find references to Foo");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.assignment.role.as_deref(), Some("explorer"));
}

#[test]
fn test_parse_spawn_request_accepts_objective_and_role_alias() {
    let input = json!({
        "objective": "Coordinate and wait",
        "role": "awaiter"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.prompt, "Coordinate and wait");
    assert_eq!(parsed.agent_type, SubAgentType::Plan);
    assert_eq!(parsed.assignment.role.as_deref(), Some("awaiter"));
}

#[test]
fn test_parse_spawn_request_accepts_items_payload() {
    let input = json!({
        "items": [
            {"type": "text", "text": "Analyze module"},
            {"type": "mention", "name": "drive", "path": "app://drive"}
        ],
        "agent_name": "explorer"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.prompt.contains("Analyze module"));
    assert!(parsed.prompt.contains("[mention:$drive](app://drive)"));
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
}

#[test]
fn test_parse_spawn_request_accepts_fork_context() {
    let input = json!({
        "prompt": "continue from here",
        "fork_context": true
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.fork_context);

    let input = json!({
        "prompt": "continue from here",
        "inherit_context": true
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.fork_context);
}

#[test]
fn test_parse_spawn_request_accepts_model_strength() {
    let input = json!({
        "prompt": "scan parser references",
        "type": "explore",
        "model_strength": "faster"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Faster);

    let input = json!({
        "prompt": "apply a release fix",
        "modelStrength": "same"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);
}

#[test]
fn explore_subagent_defaults_to_faster_model_strength() {
    // type: "explore" with no model_strength and no model defaults to Faster:
    // bounded read-only lookup is exactly the cheap-sibling job.
    let input = json!({
        "prompt": "find every caller of normalize_model_name",
        "type": "explore"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Faster);

    // Explicit model_strength: "same" wins for explore too.
    let input = json!({
        "prompt": "explore but stay capable",
        "type": "explore",
        "model_strength": "same"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);

    // An explicit model pins the child (downstream Fixed route) and disables
    // the explore→faster default, so model_strength falls back to Same.
    let input = json!({
        "prompt": "explore on a specific model",
        "type": "explore",
        "model": "GLM-5.2"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);
}

#[test]
fn non_explore_subagents_keep_default_same_model_strength() {
    // Non-explore roles keep the conservative Same default even with no model.
    for role in ["general", "plan", "review", "implementer"] {
        let input = json!({
            "prompt": "do some work",
            "type": role
        });
        let parsed = parse_spawn_request(&input).expect("spawn request should parse");
        assert_eq!(
            parsed.model_strength,
            SubAgentModelStrength::Same,
            "role {role:?} should default to Same"
        );
    }
}

#[test]
fn test_parse_spawn_request_accepts_child_thinking() {
    let input = json!({
        "prompt": "scan parser references",
        "thinking": "off"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.thinking,
        SubAgentThinking::Effort(ReasoningEffort::Off)
    );

    let input = json!({
        "prompt": "design a fix",
        "reasoning_effort": "max"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.thinking,
        SubAgentThinking::Effort(ReasoningEffort::Max)
    );

    let input = json!({
        "prompt": "classify complexity",
        "reasoningEffort": "auto"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.thinking, SubAgentThinking::Auto);
}

#[test]
fn test_parse_spawn_request_rejects_invalid_model_strength() {
    let input = json!({
        "prompt": "scan parser references",
        "model_strength": "automatic"
    });
    let err = parse_spawn_request(&input).expect_err("invalid model_strength should fail");
    assert!(
        err.to_string()
            .contains("model_strength must be one of: same, faster")
    );
}

#[test]
fn test_parse_spawn_request_rejects_invalid_child_thinking() {
    let input = json!({
        "prompt": "scan parser references",
        "thinking": "forever"
    });
    let err = parse_spawn_request(&input).expect_err("invalid thinking should fail");
    assert!(
        err.to_string()
            .contains("thinking must be one of: inherit, auto, off, low, medium, high, max")
    );
}

#[test]
fn test_parse_spawn_request_accepts_session_name_for_agent() {
    let input = json!({
        "name": "review.parser",
        "prompt": "inspect parser",
        "fork_context": true,
        "max_depth": 0
    });
    let parsed = parse_spawn_request(&input).expect("agent request should parse");
    assert_eq!(parsed.session_name.as_deref(), Some("review.parser"));
    assert!(parsed.fork_context);
    assert_eq!(parsed.max_depth, Some(0));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_session_name() {
    let input = json!({
        "name": "bad name",
        "prompt": "inspect parser"
    });
    let err = parse_spawn_request(&input).expect_err("space in name should fail");
    assert!(err.to_string().contains("name must not contain whitespace"));
}

#[test]
fn test_parse_spawn_request_rejects_out_of_range_max_depth() {
    let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
    let input = json!({
        "name": "review.parser",
        "prompt": "inspect parser",
        "max_depth": ceiling + 1
    });
    let err = parse_spawn_request(&input).expect_err("max_depth should be capped at schema range");
    assert!(
        err.to_string()
            .contains(&format!("max_depth must be between 0 and {ceiling}"))
    );
}

#[tokio::test]
async fn session_projection_exposes_forked_prefix_cache_contract() {
    let mut snapshot = make_snapshot(SubAgentStatus::Running);
    snapshot.name = "fanout_review".to_string();
    snapshot.context_mode = "forked".to_string();
    snapshot.fork_context = true;

    let ctx = ToolContext::new(".");
    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.name, "fanout_review");
    assert_eq!(projection.context_mode, "forked");
    assert_eq!(projection.run_id, "agent_test");
    assert_eq!(projection.follow_up.tool, "handle_read");
    assert_eq!(projection.follow_up.agent_id, "agent_test");
    assert!(projection.takeover.supported);
    assert_eq!(projection.usage.status, "unknown");
    assert_eq!(projection.verification.status, "self_report_only");
    assert!(projection.fork_context);
    assert_eq!(projection.prefix_cache.mode, "forked");
    assert_eq!(
        projection.prefix_cache.parent_prefix,
        "preserved_byte_identical_when_available"
    );
    assert_eq!(projection.transcript_handle.kind, "var_handle");
    assert_eq!(projection.transcript_handle.name, "transcript");
}

#[tokio::test]
async fn terminal_session_projection_prefers_full_transcript_handle() {
    let mut snapshot = make_snapshot(SubAgentStatus::Completed);
    snapshot.result = Some("done".to_string());

    let ctx = ToolContext::new(".");
    let full_handle = {
        let mut store = ctx.runtime.handle_store.lock().await;
        store.insert_json(
            "agent:agent_test",
            "full_transcript",
            json!({
                "kind": "subagent_full_transcript",
                "agent_id": "agent_test",
                "messages": [
                    {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "complete child output" }
                        ]
                    }
                ]
            }),
        )
    };

    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.transcript_handle, full_handle);
    assert_eq!(projection.transcript_handle.name, "full_transcript");
}

#[tokio::test]
async fn interrupted_projection_exposes_checkpoint_metadata_and_messages() {
    let mut snapshot = make_snapshot(SubAgentStatus::Interrupted(
        "API call timed out after 10ms".to_string(),
    ));
    let checkpoint = make_checkpoint(
        &snapshot.agent_id,
        1,
        vec![text_message("user", "inspect checkpoint recovery")],
    );
    snapshot.steps_taken = checkpoint.steps_taken;
    snapshot.checkpoint = Some(checkpoint.clone());

    let ctx = ToolContext::new(".");
    let projection = subagent_session_projection(snapshot, false, &ctx, None).await;

    assert_eq!(projection.status, "waiting_for_user");
    assert!(projection.terminal);
    assert!(projection.continuable);
    assert!(projection.needs_continuation);
    assert!(!projection.timed_out_with_checkpoint);
    assert_eq!(
        projection
            .checkpoint
            .as_ref()
            .expect("checkpoint projected")
            .continuation_handle,
        checkpoint.continuation_handle
    );
    assert_eq!(
        projection
            .snapshot
            .checkpoint
            .as_ref()
            .map(|cp| cp.message_count),
        Some(1)
    );
    assert_eq!(
        projection
            .checkpoint
            .as_ref()
            .and_then(|cp| cp.messages.first())
            .map(message_text),
        Some("inspect checkpoint recovery")
    );

    let timed_out_projection =
        subagent_session_projection(projection.snapshot.clone(), true, &ctx, None).await;
    assert!(timed_out_projection.needs_continuation);
    assert!(timed_out_projection.timed_out);
    assert!(timed_out_projection.timed_out_with_checkpoint);
}

#[test]
fn test_delegate_defaults_to_fork_context() {
    let input = with_default_fork_context(json!({ "prompt": "review current work" }), true);
    let parsed = parse_spawn_request(&input).expect("delegate request should parse");
    assert!(parsed.fork_context);

    let input = with_default_fork_context(
        json!({ "prompt": "fresh exploration", "fork_context": false }),
        true,
    );
    let parsed = parse_spawn_request(&input).expect("delegate override should parse");
    assert!(!parsed.fork_context);
}

#[test]
fn spawn_request_parses_token_budget_override() {
    let parsed = parse_spawn_request(&json!({
        "prompt": "fan out safely",
        "token_budget": 12_345
    }))
    .expect("token budget parses");
    assert_eq!(parsed.token_budget, Some(12_345));

    let parsed = parse_spawn_request(&json!({
        "prompt": "fleet-shaped alias",
        "max_tokens": 4_000
    }))
    .expect("max_tokens alias parses");
    assert_eq!(parsed.token_budget, Some(4_000));

    let err = parse_spawn_request(&json!({
        "prompt": "bad budget",
        "token_budget": 0
    }))
    .expect_err("zero budget is invalid in tool input");
    assert!(
        err.to_string().contains("must be greater than zero"),
        "clear token budget error: {err}"
    );
}

#[test]
fn forked_subagent_messages_preserve_parent_prefix_then_append_task() {
    let parent_system = SystemPrompt::Text("parent system".to_string());
    let parent_message = Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "parent turn".to_string(),
            cache_control: None,
        }],
    };
    let fork_context = SubAgentForkContext {
        system: Some(parent_system.clone()),
        messages: vec![parent_message.clone()],
        structured_state_block: Some("## Fork State\n- Mode: `AGENT`".to_string()),
    };

    let assignment = SubAgentAssignment::new("inspect parser".to_string(), Some("worker".into()));
    let messages = build_initial_subagent_messages(
        "inspect parser",
        &assignment,
        &SubAgentType::General,
        Some(&fork_context),
    );

    assert_eq!(
        subagent_request_system_prompt("child system", Some(&fork_context)),
        parent_system
    );
    assert_eq!(messages.first(), Some(&parent_message));
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].role, "system");
    assert!(message_text(&messages[1]).contains("<codewhale:fork_state>"));
    assert_eq!(messages[2].role, "system");
    assert!(message_text(&messages[2]).contains("<codewhale:subagent_context>"));
    assert_eq!(messages[3].role, "user");
    assert!(message_text(&messages[3]).contains("inspect parser"));
}

#[test]
fn fresh_subagent_messages_keep_existing_single_turn_shape() {
    let assignment = SubAgentAssignment::new("list files".to_string(), None);
    let messages =
        build_initial_subagent_messages("list files", &assignment, &SubAgentType::Explore, None);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert!(message_text(&messages[0]).contains("list files"));
}

#[test]
fn test_parse_spawn_request_rejects_text_and_items_together() {
    let input = json!({
        "prompt": "Analyze module",
        "items": [{"type": "text", "text": "dup"}]
    });
    let err = parse_spawn_request(&input).expect_err("text+items should fail");
    assert!(err.to_string().contains("either prompt text or items"));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_role() {
    let input = json!({
        "prompt": "do work",
        "role": "unknown_role"
    });
    let err = parse_spawn_request(&input).expect_err("invalid role should fail");
    assert!(err.to_string().contains("Invalid role alias"));
}

#[test]
fn test_parse_spawn_request_accepts_full_role_vocabulary() {
    // Regression for #2649: roles that `SubAgentType::from_str` accepts must
    // also pass the second `normalize_role_alias` validation pass instead of
    // being rejected with a stale hint.
    for (role, expected_type, expected_role) in [
        ("general", SubAgentType::General, "worker"),
        ("general-purpose", SubAgentType::General, "worker"),
        ("general_purpose", SubAgentType::General, "worker"),
        ("worker", SubAgentType::General, "worker"),
        ("default", SubAgentType::General, "default"),
        ("explore", SubAgentType::Explore, "explorer"),
        ("exploration", SubAgentType::Explore, "explorer"),
        ("explorer", SubAgentType::Explore, "explorer"),
        ("plan", SubAgentType::Plan, "awaiter"),
        ("planning", SubAgentType::Plan, "awaiter"),
        ("planner", SubAgentType::Plan, "awaiter"),
        ("awaiter", SubAgentType::Plan, "awaiter"),
        ("review", SubAgentType::Review, "reviewer"),
        ("code-review", SubAgentType::Review, "reviewer"),
        ("code_review", SubAgentType::Review, "reviewer"),
        ("reviewer", SubAgentType::Review, "reviewer"),
        ("implementer", SubAgentType::Implementer, "implementer"),
        ("implement", SubAgentType::Implementer, "implementer"),
        ("implementation", SubAgentType::Implementer, "implementer"),
        ("builder", SubAgentType::Implementer, "implementer"),
        ("verifier", SubAgentType::Verifier, "verifier"),
        ("verify", SubAgentType::Verifier, "verifier"),
        ("verification", SubAgentType::Verifier, "verifier"),
        ("validator", SubAgentType::Verifier, "verifier"),
        ("tester", SubAgentType::Verifier, "verifier"),
        ("custom", SubAgentType::Custom, "custom"),
    ] {
        assert_eq!(
            SubAgentType::from_str(role),
            Some(expected_type.clone()),
            "from_str should accept role alias {role:?}"
        );
        assert_eq!(
            normalize_role_alias(role),
            Some(expected_role),
            "normalize_role_alias should accept role alias {role:?}"
        );

        let input = json!({ "prompt": "do work", "role": role });
        let parsed = parse_spawn_request(&input)
            .unwrap_or_else(|e| panic!("role {role:?} should parse, got {e}"));
        assert_eq!(parsed.agent_type, expected_type, "type for role {role:?}");
        assert_eq!(
            parsed.assignment.role.as_deref(),
            Some(expected_role),
            "canonical role for {role:?}"
        );
    }
}

#[test]
fn test_invalid_role_error_lists_real_aliases() {
    // The hint must enumerate the actually-accepted vocabulary (#2649).
    let input = json!({ "prompt": "do work", "role": "nonsense" });
    let err = parse_spawn_request(&input)
        .expect_err("invalid role should fail")
        .to_string();
    assert!(err.contains("reviewer"), "hint should list reviewer: {err}");
    assert!(err.contains("verifier"), "hint should list verifier: {err}");
    assert!(err.contains("custom"), "hint should list custom: {err}");
    assert!(
        err.contains("general-purpose"),
        "hint should list general-purpose: {err}"
    );
    assert!(
        err.contains("code_review"),
        "hint should list code_review: {err}"
    );
}

fn schema_property_description<'a>(schema: &'a Value, property: &str) -> &'a str {
    schema["properties"][property]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("missing description for schema property {property:?}"))
}

#[test]
fn subagent_tool_schemas_advertise_real_type_and_role_vocabulary() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();

    let description = schema_property_description(&agent_schema, "type");
    for alias in [
        "general",
        "explore",
        "plan",
        "review",
        "implementer",
        "verifier",
        "custom",
    ] {
        assert!(
            description.contains(alias),
            "type description should list accepted type {alias:?}: {description}"
        );
    }
    assert!(agent_schema["properties"].get("role").is_none());
    assert!(agent_schema["properties"].get("max_depth").is_some());
    let model_strength = schema_property_description(&agent_schema, "model_strength");
    assert!(
        model_strength.contains("type=explore") && model_strength.contains("faster"),
        "model_strength description should teach explore/faster routing: {model_strength}"
    );
    let thinking = schema_property_description(&agent_schema, "thinking");
    assert!(
        thinking.contains("inherit") && thinking.contains("model_strength=faster"),
        "thinking description should teach child thinking control: {thinking}"
    );
    assert!(agent_schema["properties"].get("model").is_some());
    let worktree = schema_property_description(&agent_schema, "worktree");
    assert!(
        worktree.contains("git worktree") && worktree.contains("parallel edit"),
        "worktree description should teach isolated parallel edits: {worktree}"
    );
    assert!(agent_schema["properties"].get("worktree_branch").is_some());
    assert!(agent_schema["properties"].get("worktree_path").is_some());
}

#[test]
fn agent_tool_prompt_schema_prefers_structured_briefs() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();
    let prompt = schema_property_description(&agent_schema, "prompt");
    assert!(prompt.contains("Subagent Brief"));
    assert!(prompt.contains("QUESTION"));
    assert!(prompt.contains("STOP_CONDITION"));
    assert!(prompt.contains("ALREADY_KNOWN"));
}

#[test]
fn agent_tool_schema_advertises_status_peek_cancel_actions() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();

    let action = schema_property_description(&agent_schema, "action");
    assert!(action.contains("status"));
    assert!(action.contains("peek"));
    assert!(action.contains("cancel"));
    assert!(agent_schema["properties"].get("agent_id").is_some());
}

#[tokio::test]
async fn agent_tool_status_returns_running_child_projection() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_status_probe".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "probe".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.read().await.current_session_boot_id.clone(),
    );
    agent.status = SubAgentStatus::Running;
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
        manager_guard
            .record_worker_progress(&agent_id, "step 1: requesting model response".to_string());
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "status", "agent_id": agent_id}), &context)
        .await
        .expect("status action succeeds");

    assert_eq!(result.metadata.as_ref().unwrap()["action"], json!("status"));
    assert!(result.content.contains("agent_status_probe"));
    assert!(result.content.contains("running"));
    assert!(result.content.contains("transcript_handle"));
}

#[tokio::test]
async fn agent_tool_status_reconciles_stale_single_agent_projection() {
    let tmp = tempdir().expect("tempdir");
    let inner = SubAgentManager::new(tmp.path().to_path_buf(), 2)
        .with_running_heartbeat_timeout(Duration::from_secs(30));
    let current_boot = inner.session_boot_id().to_string();
    let manager = Arc::new(RwLock::new(inner));
    let agent_id = "agent_stale_single_status".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "probe stale single status".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        current_boot,
    );
    agent.status = SubAgentStatus::Running;
    agent.last_activity_at = Instant::now() - Duration::from_secs(31);
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "status", "agent_id": agent_id}), &context)
        .await
        .expect("status action succeeds");

    let metadata = result.metadata.as_ref().expect("status metadata");
    assert_eq!(metadata["action"], json!("status"));
    assert_eq!(metadata["status"], json!("cancelled"));
    assert_eq!(metadata["terminal"], json!(true));
    assert_eq!(metadata["agent_id"], json!("agent_stale_single_status"));
    assert!(result.content.contains("agent_stale_single_status"));
    assert!(result.content.contains("cancelled"));
    assert!(result.content.contains("Auto-cancelled"));
    assert_eq!(manager.read().await.running_count(), 0);
}

#[tokio::test]
async fn agent_tool_cancel_stops_running_child() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_cancel_probe".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "cancel".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.read().await.current_session_boot_id.clone(),
    );
    agent.status = SubAgentStatus::Running;
    {
        let mut manager_guard = manager.write().await;
        manager_guard.agents.insert(agent_id.clone(), agent);
        manager_guard.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let tool = AgentTool::new(Arc::clone(&manager), stub_runtime());
    let context = ToolContext::new(tmp.path());
    let result = tool
        .execute(json!({"action": "cancel", "agent_id": agent_id}), &context)
        .await
        .expect("cancel action succeeds");

    assert_eq!(result.metadata.as_ref().unwrap()["action"], json!("cancel"));
    assert!(result.content.contains("cancelled"));
    let snapshot = manager
        .read()
        .await
        .get_result("agent_cancel_probe")
        .expect("agent remains listed");
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
}

#[test]
fn test_parse_spawn_request_rejects_conflicting_type_and_role() {
    let input = json!({
        "prompt": "inspect internals",
        "type": "explore",
        "role": "worker"
    });
    let err = parse_spawn_request(&input).expect_err("conflicting type+role should fail");
    assert!(
        err.to_string()
            .contains("Conflicting type/agent_type and role/agent_role")
    );
}

#[test]
fn test_build_allowed_tools_independent_of_allow_shell() {
    // v0.6.6: allow_shell no longer filters at the build_allowed_tools
    // level — the registry builder controls shell-tool registration.
    // Both calls return None (full inheritance) for a default General
    // agent.
    let with_shell = build_allowed_tools(&SubAgentType::General, None, true).unwrap();
    let without_shell = build_allowed_tools(&SubAgentType::General, None, false).unwrap();
    assert!(with_shell.is_none());
    assert!(without_shell.is_none());
}

#[test]
fn test_allowed_tools_are_deduplicated() {
    let tools = build_allowed_tools(
        &SubAgentType::Custom,
        Some(vec![
            "read_file".to_string(),
            "read_file".to_string(),
            "  ".to_string(),
            "grep_files".to_string(),
        ]),
        true,
    )
    .unwrap();
    assert_eq!(
        tools,
        Some(vec!["read_file".to_string(), "grep_files".to_string()])
    );
}

#[test]
fn test_custom_agent_requires_allowed_tools() {
    let err = build_allowed_tools(&SubAgentType::Custom, None, true).unwrap_err();
    assert!(err.to_string().contains("requires"));
}

#[test]
fn role_posture_blocks_writes_and_shell_for_read_only_roles() {
    // #3217: read-only roles may never run write/edit/patch tools, regardless
    // of parent auto-approval, but can always read.
    for role in [
        SubAgentType::Explore,
        SubAgentType::Review,
        SubAgentType::Plan,
        SubAgentType::Verifier,
    ] {
        assert!(
            !role_posture_permits(&role, ApprovalRequirement::Suggest),
            "{role:?} must not run write/edit/patch tools"
        );
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Auto),
            "{role:?} can still read"
        );
    }

    // Write-capable roles keep write access.
    for role in [SubAgentType::Implementer, SubAgentType::General] {
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Suggest),
            "{role:?} writes"
        );
    }

    // Only Full-shell roles may run shell (Required) tools.
    for role in [
        SubAgentType::Verifier,
        SubAgentType::Implementer,
        SubAgentType::General,
    ] {
        assert!(
            role_posture_permits(&role, ApprovalRequirement::Required),
            "{role:?} has full shell"
        );
    }
    for role in [
        SubAgentType::Plan,
        SubAgentType::Explore,
        SubAgentType::Review,
    ] {
        assert!(
            !role_posture_permits(&role, ApprovalRequirement::Required),
            "{role:?} must not run shell tools"
        );
    }

    // Custom is governed by its explicit allowed_tools list, so the posture
    // check permits it (the allowlist is the authority for that role).
    assert!(role_posture_permits(
        &SubAgentType::Custom,
        ApprovalRequirement::Suggest
    ));
    assert!(role_posture_permits(
        &SubAgentType::Custom,
        ApprovalRequirement::Required
    ));
}

#[test]
fn test_build_assignment_prompt_includes_metadata() {
    let assignment = SubAgentAssignment::new(
        "Inspect parser behavior".to_string(),
        Some("explorer".to_string()),
    );
    let prompt = build_assignment_prompt(
        "Inspect parser behavior",
        &assignment,
        &SubAgentType::Explore,
    );
    assert!(prompt.contains("Assignment metadata"));
    assert!(prompt.contains("resolved_type: explore"));
    assert!(prompt.contains("role: explorer"));
}

#[test]
fn subagent_model_strength_defaults_to_parent_even_when_parent_auto_model() {
    let mut runtime = stub_runtime().with_auto_model(true);
    runtime.model = "deepseek-v4-pro".to_string();

    for prompt in ["implement the release fix", "say hello"] {
        let route = fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            prompt,
        );
        assert_eq!(route.model_route, ModelRoute::Inherit);
        assert_eq!(route.model, "deepseek-v4-pro", "prompt {prompt:?}");
    }
}

#[test]
fn subagent_model_strength_faster_uses_known_family_sibling() {
    let mut runtime = stub_runtime().with_auto_model(true);
    runtime.model = "deepseek-v4-pro".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model_route, ModelRoute::Faster);
    assert_eq!(route.model, "deepseek-v4-flash");
    assert_eq!(route.reasoning_effort.as_deref(), Some("off"));
}

#[test]
fn subagent_model_strength_explicit_model_wins_over_faster() {
    let runtime = stub_runtime().with_auto_model(true);

    let route = fallback_subagent_assignment_route(
        &runtime,
        Some("deepseek-v4-pro".to_string()),
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(
        route.model_route,
        ModelRoute::Fixed("deepseek-v4-pro".to_string())
    );
    assert_eq!(route.model, "deepseek-v4-pro");
}

#[test]
fn explicit_child_thinking_overrides_faster_default_off() {
    let mut runtime = stub_runtime().with_reasoning_effort(Some("max".to_string()), false);
    runtime.model = "deepseek-v4-pro".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Effort(ReasoningEffort::High),
        "inspect one file",
    );
    assert_eq!(route.model, "deepseek-v4-flash");
    assert_eq!(route.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(route.tuning.reasoning_effort, Some(ReasoningEffort::High));
}

#[test]
fn explicit_child_auto_thinking_resolves_from_child_prompt() {
    let runtime = stub_runtime().with_reasoning_effort(Some("off".to_string()), false);

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Inherit,
        SubAgentThinking::Auto,
        "debug this release failure",
    );
    assert_eq!(route.reasoning_effort.as_deref(), Some("max"));
}

#[tokio::test]
async fn route_resolution_matrix_uses_explicit_model_strength_routes() {
    let mut runtime = stub_runtime()
        .with_auto_model(false)
        .with_reasoning_effort(Some("max".to_string()), false);
    runtime.model = "deepseek-v4-pro".to_string();

    struct RouteCase {
        agent_type: SubAgentType,
        configured_model: Option<&'static str>,
        requested_route: ModelRoute,
        prompt: &'static str,
        expected_route: ModelRoute,
        expected_model: &'static str,
        expected_reasoning: Option<&'static str>,
        expected_tuning_effort: Option<ReasoningEffort>,
    }

    let cases = vec![
        RouteCase {
            agent_type: SubAgentType::Explore,
            configured_model: None,
            requested_route: ModelRoute::Inherit,
            prompt: "inspect the parser and report what changed",
            expected_route: ModelRoute::Inherit,
            expected_model: "deepseek-v4-pro",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
        RouteCase {
            agent_type: SubAgentType::Explore,
            configured_model: None,
            requested_route: ModelRoute::Faster,
            prompt: "inspect the parser and report what changed",
            expected_route: ModelRoute::Faster,
            expected_model: "deepseek-v4-flash",
            expected_reasoning: Some("off"),
            expected_tuning_effort: Some(ReasoningEffort::Off),
        },
        RouteCase {
            agent_type: SubAgentType::General,
            configured_model: None,
            requested_route: ModelRoute::Inherit,
            prompt: "synthesize the release blocker fix",
            expected_route: ModelRoute::Inherit,
            expected_model: "deepseek-v4-pro",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
        RouteCase {
            agent_type: SubAgentType::Implementer,
            configured_model: Some("deepseek-v4-flash"),
            requested_route: ModelRoute::Inherit,
            prompt: "apply the narrow code edit",
            expected_route: ModelRoute::Fixed("deepseek-v4-flash".to_string()),
            expected_model: "deepseek-v4-flash",
            expected_reasoning: Some("max"),
            expected_tuning_effort: Some(ReasoningEffort::Max),
        },
    ];

    for case in cases {
        let route = resolve_subagent_assignment_route(
            &runtime,
            case.configured_model.map(str::to_string),
            case.prompt,
            &case.agent_type,
            case.requested_route.clone(),
            SubAgentThinking::Inherit,
        )
        .await;
        assert_eq!(
            route.model_route, case.expected_route,
            "{:?}",
            case.agent_type
        );
        assert_eq!(route.model, case.expected_model, "{:?}", case.agent_type);
        assert_eq!(
            route.reasoning_effort.as_deref(),
            case.expected_reasoning,
            "{:?}",
            case.agent_type
        );
        assert_eq!(
            route.tuning.reasoning_effort, case.expected_tuning_effort,
            "{:?}",
            case.agent_type
        );
        assert_eq!(
            route.tuning.max_output_tokens,
            Some(SUBAGENT_RESPONSE_MAX_TOKENS),
            "{:?}",
            case.agent_type
        );
    }
}

#[test]
fn subagent_auto_reasoning_resolves_to_distinct_v4_tiers() {
    let runtime = stub_runtime().with_reasoning_effort(Some("high".to_string()), true);

    assert_eq!(
        fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            "quick lookup",
        )
        .reasoning_effort,
        Some("high".to_string())
    );
    assert_eq!(
        fallback_subagent_assignment_route(
            &runtime,
            None,
            ModelRoute::Inherit,
            SubAgentThinking::Inherit,
            "debug this release failure"
        )
        .reasoning_effort,
        Some("max".to_string())
    );
}

#[test]
fn test_subagent_tool_registry_reports_unavailable_tools() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        Some(vec!["read_file".to_string(), "missing_tool".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    assert_eq!(
        registry.unavailable_allowed_tools(),
        vec!["missing_tool".to_string()]
    );
}

#[test]
fn test_subagent_tools_respect_nested_agent_depth_budget() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.spawn_depth = 1;
    runtime.max_spawn_depth = 2;
    let registry = SubAgentToolRegistry::new(
        runtime.clone(),
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let tools = registry.tools_for_model(&SubAgentType::Explore);
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"agent"),
        "child should keep the single agent launcher while depth budget remains; tools: {names:?}"
    );
    assert!(registry.is_tool_allowed("agent"));

    runtime.spawn_depth = 2;
    let capped = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    let capped_tools = capped.tools_for_model(&SubAgentType::Explore);
    let capped_names: Vec<_> = capped_tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !capped_names.contains(&"agent"),
        "child should lose agent launcher at configured depth cap; tools: {capped_names:?}"
    );
    assert!(!capped.is_tool_allowed("agent"));
}

fn tool_names(tools: Vec<Tool>) -> HashSet<String> {
    tools.into_iter().map(|tool| tool.name).collect()
}

fn enabled_agent_surface_options() -> AgentToolSurfaceOptions {
    let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
    options.apply_patch_enabled = true;
    options.web_search_enabled = true;
    options.memory_tool_enabled = true;
    options.goal_state = Some(crate::tools::goal::new_shared_goal_state());
    options
}

fn disabled_feature_agent_surface_options() -> AgentToolSurfaceOptions {
    let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
    options.goal_state = Some(crate::tools::goal::new_shared_goal_state());
    options
}

#[test]
fn subagent_general_catalog_matches_parent_agent_surface_when_features_enabled() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let todo_list = crate::tools::todo::new_shared_todo_list();
    let plan_state = crate::tools::plan::new_shared_plan_state();

    let parent_registry = ToolRegistryBuilder::new()
        .with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            runtime.agent_tool_surface_options.clone(),
            todo_list.clone(),
            plan_state.clone(),
        )
        .build(runtime.context.clone());
    let child_registry =
        SubAgentToolRegistry::new(runtime, SubAgentType::General, None, todo_list, plan_state);

    let parent_names = tool_names(parent_registry.to_api_tools());
    let child_names = tool_names(child_registry.tools_for_model(&SubAgentType::General));
    assert_eq!(
        child_names, parent_names,
        "default General sub-agent catalog must stay in parity with the parent Agent surface"
    );
}

#[test]
fn subagent_feature_gates_match_parent_agent_surface() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(disabled_feature_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let todo_list = crate::tools::todo::new_shared_todo_list();
    let plan_state = crate::tools::plan::new_shared_plan_state();

    let parent_registry = ToolRegistryBuilder::new()
        .with_full_agent_surface_options(
            Some(runtime.client.clone()),
            runtime.model.clone(),
            runtime.manager.clone(),
            runtime.clone(),
            runtime.agent_tool_surface_options.clone(),
            todo_list.clone(),
            plan_state.clone(),
        )
        .build(runtime.context.clone());
    let child_registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        todo_list,
        plan_state,
    );

    let parent_names = tool_names(parent_registry.to_api_tools());
    let child_names = tool_names(child_registry.tools_for_model(&SubAgentType::Implementer));
    for name in [
        "apply_patch",
        "web_search",
        "fetch_url",
        "web.run",
        "wait_for_dev_server",
        "remember",
    ] {
        assert!(
            !parent_names.contains(name),
            "{name} should be parent-gated"
        );
        assert!(!child_names.contains(name), "{name} should be child-gated");
    }
}

#[test]
fn explore_catalog_inherits_web_but_hides_write_shell_and_fim_tools() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Explore));
    for name in ["web_search", "fetch_url", "web.run", "wait_for_dev_server"] {
        assert!(names.contains(name), "Explore should inherit {name}");
    }
    for name in [
        "write_file",
        "edit_file",
        "apply_patch",
        "fim_edit",
        "exec_shell",
        "task_shell_start",
    ] {
        assert!(!names.contains(name), "Explore must hide {name}");
    }
}

#[test]
fn implementer_catalog_inherits_patch_and_fim_when_enabled() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Implementer));
    for name in ["apply_patch", "fim_edit", "write_file", "edit_file"] {
        assert!(
            names.contains(name),
            "Implementer should inherit write-capable tool {name}"
        );
    }
}

#[tokio::test]
async fn api_timeout_preserves_checkpoint_and_returns_needs_input_without_parking() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_checkpoint_timeout".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect checkpoint behavior".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec![]),
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let (client, calls, _bodies) =
        delayed_chat_client(Duration::from_millis(80), "resumed answer").await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_millis(50));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    let (mailbox, mut mailbox_rx) =
        crate::tools::subagent::mailbox::Mailbox::new(tokio_util::sync::CancellationToken::new());
    runtime.mailbox = Some(mailbox);

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime: runtime.clone(),
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect checkpoint behavior".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };
    let task_handle = tokio::spawn(run_subagent_task(task));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first timed-out API attempt should reach the test server");

    let interrupted_envelope = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            for env in mailbox_rx.drain() {
                if let MailboxMessage::Interrupted {
                    agent_id: id,
                    reason,
                } = env.message
                {
                    return (id, reason);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("API timeout should publish an Interrupted mailbox lifecycle event");
    assert_eq!(interrupted_envelope.0, agent_id);
    assert!(
        interrupted_envelope.1.contains("API call timed out"),
        "reason should carry the timeout context: {}",
        interrupted_envelope.1
    );

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("sub-agent task must not park waiting for checkpoint input")
        .expect("sub-agent task should finish");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "needs-input interruption must not park for continuation or issue a second API request"
    );

    let interrupted = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    assert!(matches!(interrupted.status, SubAgentStatus::Interrupted(_)));
    let checkpoint = interrupted
        .checkpoint
        .as_ref()
        .expect("timeout should preserve checkpoint");
    assert!(checkpoint.continuable);
    assert_eq!(checkpoint.steps_taken, 1);
    assert!(
        checkpoint
            .messages
            .iter()
            .any(|message| message_text(message).contains("Inspect checkpoint behavior")),
        "checkpoint should preserve local child prompt: {checkpoint:?}"
    );
    assert!(interrupted.needs_input.is_some());

    let ctx = runtime.context.clone();
    let worker_record = {
        let manager = manager.read().await;
        manager.get_worker_record(&agent_id)
    };
    let projection =
        subagent_session_projection(interrupted.clone(), false, &ctx, worker_record).await;
    assert_eq!(projection.status, "waiting_for_user");
    assert!(projection.continuable);
    assert!(projection.needs_continuation);
    assert!(projection.checkpoint.is_some());
    assert!(
        projection
            .needs_input
            .as_ref()
            .expect("needs_input should be projected")
            .question
            .contains("Re-dispatch this worker"),
        "projection should tell the parent how to wake/re-dispatch: {:?}",
        projection.needs_input
    );
    assert_eq!(
        projection
            .worker_record
            .as_ref()
            .expect("worker record")
            .status,
        AgentWorkerStatus::WaitingForUser
    );
    assert_eq!(
        projection
            .worker_record
            .as_ref()
            .expect("worker record")
            .recommended_action
            .action,
        "inspect_or_replace"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "projection inspection must not respawn the child implicitly"
    );
}

#[test]
fn transient_provider_classifier_matches_sse_header_timeout() {
    let err = anyhow::anyhow!("SSE stream request did not receive response headers after 45s");

    assert!(is_transient_subagent_provider_error(&err));
}

#[tokio::test]
async fn subagent_retries_transient_provider_header_timeout_before_succeeding() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_transient_provider_retry".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect transient provider recovery".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec![]),
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    let (client, calls) =
        transient_header_timeout_then_success_chat_client("recovered answer").await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_secs(5));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect transient provider recovery".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::spawn(run_subagent_task(task)),
    )
    .await
    .expect("sub-agent task should finish")
    .expect("sub-agent join should succeed");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one transient provider failure should be retried exactly once"
    );
    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    assert_eq!(snapshot.status, SubAgentStatus::Completed);
    assert_eq!(snapshot.result.as_deref(), Some("recovered answer"));
}

#[tokio::test]
async fn spawn_duplicate_session_name_error_names_conflicting_agent() {
    // #2656: the duplicate-name error must identify the conflicting agent so a
    // model can recover deterministically (reuse the id, or pick a new name).
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 5)));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut existing = SubAgent::new(
        "test_agent_existing".to_string(),
        SubAgentType::Explore,
        "scan".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    existing.session_name = "researcher".to_string();
    existing.status = SubAgentStatus::Running;
    let existing_id = existing.id.clone();
    {
        let mut guard = manager.write().await;
        guard.agents.insert(existing_id.clone(), existing);
    }

    let err = {
        let mut guard = manager.write().await;
        guard
            .spawn_background_with_assignment_options(
                manager.clone(),
                stub_runtime(),
                SubAgentType::Explore,
                "new work".to_string(),
                make_assignment(),
                Some(vec!["read_file".to_string()]),
                SubAgentSpawnOptions {
                    name: Some("researcher".to_string()),
                    ..Default::default()
                },
            )
            .expect_err("duplicate session name must error")
    };
    let msg = err.to_string();
    assert!(
        msg.contains(&existing_id),
        "names the conflicting agent_id: {msg}"
    );
    assert!(
        msg.contains("running"),
        "includes the conflicting status: {msg}"
    );
    // #3020: elapsed time lets the parent distinguish a live worker from a
    // stale earlier spawn.
    assert!(
        msg.contains("started ") && msg.contains(" ago"),
        "includes elapsed time since spawn: {msg}"
    );
}

#[tokio::test]
async fn test_running_count_counts_only_agents_with_live_task_handles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_3".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    let handle = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    agent.task_handle = Some(handle);
    let agent_id = agent.id.clone();
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[test]
fn test_running_count_ignores_running_status_without_task_handle() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_4".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 0);
}

#[tokio::test]
async fn test_running_count_counts_running_agents_until_status_reconciles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_5".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    let finished_handle = tokio::spawn(async {});
    while !finished_handle.is_finished() {
        tokio::task::yield_now().await;
    }
    agent.task_handle = Some(finished_handle);
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
}

#[tokio::test]
async fn admission_limit_counts_queued_and_running_workers_separately() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 2).with_admission_limit(4);
    let mut handles = Vec::new();

    for (agent_id, queued) in [
        ("agent_admit_a", false),
        ("agent_admit_b", false),
        ("agent_admit_c", true),
        ("agent_admit_d", true),
    ] {
        let (input_tx, _input_rx) = mpsc::unbounded_channel();
        let mut agent = SubAgent::new(
            agent_id.to_string(),
            SubAgentType::Explore,
            "prompt".to_string(),
            make_assignment(),
            "deepseek-v4-flash".to_string(),
            Some("Blue".to_string()),
            Some(vec!["read_file".to_string()]),
            input_tx,
            PathBuf::from("."),
            "boot_test".to_string(),
        );
        agent.status = SubAgentStatus::Running;
        agent.task_handle = Some(tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }));
        handles.push(agent_id.to_string());
        manager.agents.insert(agent_id.to_string(), agent);
        manager.register_worker(make_worker_spec(agent_id, PathBuf::from(".")));
        if queued {
            manager.record_worker_event(
                agent_id,
                AgentWorkerStatus::Queued,
                Some(SUBAGENT_QUEUED_LAUNCH_REASON.to_string()),
                None,
                None,
            );
        }

        if manager.admitted_count() < 4 {
            manager
                .check_admission_capacity()
                .expect("admission remains below total ceiling");
        }
    }

    assert_eq!(manager.admitted_count(), 4);
    assert_eq!(manager.active_count(), 2);
    assert_eq!(manager.queued_count(), 2);
    let err = manager
        .check_admission_capacity()
        .expect_err("admission ceiling rejects fifth worker");
    let msg = err.to_string();
    assert!(
        msg.contains("max_admitted 4") && msg.contains("running 2") && msg.contains("queued 2"),
        "error distinguishes running vs queued counts: {msg}"
    );

    for agent_id in handles {
        manager
            .agents
            .get_mut(&agent_id)
            .and_then(|agent| agent.task_handle.take())
            .expect("live task handle")
            .abort();
    }
}

#[tokio::test]
async fn cleanup_auto_cancels_stale_running_agent_and_releases_slot() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(1));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_stale".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);
    tokio::time::sleep(Duration::from_millis(5)).await;

    assert_eq!(
        manager.running_count(),
        0,
        "stale running agents must not keep the concurrency slot occupied"
    );
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 1);

    let snapshot = manager
        .get_result(&agent_id)
        .expect("agent should remain inspectable");
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
    assert_eq!(manager.running_count(), 0);
    assert!(
        snapshot
            .result
            .as_deref()
            .unwrap_or_default()
            .contains("Auto-cancelled")
    );
}

#[tokio::test]
async fn status_projection_reconciles_stale_running_agent() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(1));
    let current_boot = inner.session_boot_id().to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_status_stale".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        current_boot,
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    inner.agents.insert(agent.id.clone(), agent);
    tokio::time::sleep(Duration::from_millis(5)).await;

    let manager = Arc::new(RwLock::new(inner));
    let context = ToolContext::new(".");
    let result = inspect_agent_from_input(&json!({"action": "status"}), manager, &context, false)
        .await
        .expect("status projection should succeed");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("status payload should be json");
    let agent = payload["agents"]
        .as_array()
        .and_then(|agents| agents.first())
        .expect("stale current-session agent should remain inspectable");

    assert_eq!(payload["count"], 1);
    assert_eq!(agent["agent_id"], "test_agent_status_stale");
    assert_eq!(agent["status"], "cancelled");
    assert_eq!(agent["terminal"], true);
    assert_eq!(agent["snapshot"]["status"], "Cancelled");
    assert!(
        agent["snapshot"]["result"]
            .as_str()
            .unwrap_or_default()
            .contains("Auto-cancelled")
    );
}

#[tokio::test]
async fn cleanup_keeps_recent_running_agent() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_secs(300));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_recent".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.last_activity_at = Instant::now();
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);

    assert_eq!(manager.running_count(), 1);
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 0);
    assert_eq!(
        manager.get_result(&agent_id).expect("agent").status,
        SubAgentStatus::Running
    );
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[tokio::test]
async fn touch_refreshes_stale_running_agent_heartbeat() {
    // Use a heartbeat timeout that is comfortably larger than the synchronous
    // work between `touch()` and the `cleanup()` assertion below. With a 1ms
    // timeout the test was flaky on loaded CI runners (notably Windows, whose
    // scheduler can deschedule this thread for >1ms): the just-touched agent
    // would tip back over the staleness threshold before `cleanup()` ran and
    // get reaped, so `cleanup()` returned 1 instead of 0. A 50ms timeout keeps
    // the staleness logic exercised while removing the timing race.
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1)
        .with_running_heartbeat_timeout(Duration::from_millis(50));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        "test_agent_touched".to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);
    // Sleep well past the 50ms heartbeat timeout so the agent is reliably stale
    // even if the timer fires early under coarse OS timer granularity.
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(manager.running_count(), 0);
    assert!(manager.touch(&agent_id));
    assert_eq!(manager.running_count(), 1);
    assert_eq!(manager.cleanup(Duration::from_secs(60 * 60)), 0);
    manager
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.task_handle.take())
        .expect("live task handle")
        .abort();
}

#[test]
fn test_persist_and_reload_marks_running_agent_as_interrupted() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");

    let mut manager = SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let running = SubAgent::new(
        "test_agent_9_running".to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    let running_id = running.id.clone();
    manager.agents.insert(running_id.clone(), running);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut reloaded = SubAgentManager::new(workspace, 2)
        .with_state_path(default_state_path(tmp.path()).expect("default state path"));
    reloaded.load_state().expect("load state");
    let snapshot = reloaded
        .get_result(&running_id)
        .expect("reloaded agent should exist");
    assert!(matches!(
        snapshot.status,
        SubAgentStatus::Interrupted(ref message)
            if message.contains(SUBAGENT_RESTART_REASON)
    ));
}

#[test]
fn persist_and_reload_preserves_checkpoint_for_interrupted_running_agent() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");

    let mut manager = SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut running = SubAgent::new(
        "test_agent_checkpoint_reload".to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    running.checkpoint = Some(make_checkpoint(
        &running.id,
        2,
        vec![
            text_message("user", "initial task"),
            text_message("assistant", "partial progress"),
        ],
    ));
    let running_id = running.id.clone();
    manager.agents.insert(running_id.clone(), running);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut reloaded = SubAgentManager::new(workspace, 2)
        .with_state_path(default_state_path(tmp.path()).expect("default state path"));
    reloaded.load_state().expect("load state");
    let snapshot = reloaded
        .get_result(&running_id)
        .expect("reloaded agent should exist");

    assert!(matches!(snapshot.status, SubAgentStatus::Interrupted(_)));
    let checkpoint = snapshot.checkpoint.expect("checkpoint should reload");
    assert!(checkpoint.continuable);
    assert_eq!(checkpoint.steps_taken, 2);
    assert_eq!(checkpoint.messages.len(), 2);
    assert_eq!(message_text(&checkpoint.messages[1]), "partial progress");
}

#[cfg(unix)]
#[test]
fn load_state_rejects_symlinked_state_file() {
    let tmp = tempdir().expect("tempdir");
    let target = tmp.path().join("outside-state.json");
    let link = tmp.path().join(SUBAGENT_STATE_FILE);
    std::fs::write(
        &target,
        serde_json::json!({
            "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
            "agents": [],
            "workers": []
        })
        .to_string(),
    )
    .expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink state");

    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 1).with_state_path(link);
    let err = manager
        .load_state()
        .expect_err("symlinked state should fail");
    assert!(format!("{err:#}").contains("must not traverse symlinks"));
}

#[test]
fn persist_state_rejects_state_path_outside_workspace() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let outside_state = tmp.path().join("outside-state.json");
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");

    let manager = SubAgentManager::new(workspace, 1).with_state_path(outside_state);
    let err = manager
        .persist_state()
        .expect_err("outside state path should fail");

    assert!(format!("{err:#}").contains("must stay within workspace"));
}

#[cfg(unix)]
#[test]
fn persist_state_rejects_symlinked_state_directory() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let outside = tmp.path().join("outside-state");
    let codewhale_dir = workspace.join(".codewhale");
    let state_dir = codewhale_dir.join("state");
    std::fs::create_dir_all(&codewhale_dir).expect("mkdir codewhale");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::os::unix::fs::symlink(&outside, &state_dir).expect("symlink state dir");

    let err = default_state_path(&workspace)
        .expect_err("symlinked state directory should fail before manager construction");
    assert!(
        format!("{err:#}").contains("must stay within workspace")
            || format!("{err:#}").contains("must not traverse symlinks")
    );
}

#[test]
fn test_interrupted_status_name_and_summary() {
    let snapshot = make_snapshot(SubAgentStatus::Interrupted(
        SUBAGENT_RESTART_REASON.to_string(),
    ));
    assert_eq!(subagent_status_name(&snapshot.status), "interrupted");
    assert!(summarize_subagent_result(&snapshot).contains(SUBAGENT_RESTART_REASON));
}

// === v0.6.6 — sub-agent authority unification ===

#[test]
fn build_allowed_tools_general_returns_none_for_full_inheritance() {
    // Default behavior: General agent with no explicit list inherits the
    // parent's full registry (None signals no narrowing).
    let result = build_allowed_tools(&SubAgentType::General, None, true).unwrap();
    assert!(
        result.is_none(),
        "General with no explicit_tools should default to full inheritance (None), got {result:?}"
    );
}

#[test]
fn build_allowed_tools_explore_returns_none_for_full_inheritance() {
    // Per-type allowlists are now advisory — Explore also gets the full
    // surface unless an explicit list is passed.
    let result = build_allowed_tools(&SubAgentType::Explore, None, true).unwrap();
    assert!(
        result.is_none(),
        "Explore with no explicit_tools should default to full inheritance"
    );
}

#[test]
fn build_allowed_tools_custom_requires_explicit_list() {
    // Custom is the one type that REQUIRES explicit allowed_tools.
    let err = build_allowed_tools(&SubAgentType::Custom, None, true).unwrap_err();
    assert!(
        err.to_string().contains("Custom sub-agent requires"),
        "got: {err}"
    );
}

#[test]
fn build_allowed_tools_explicit_list_returned_as_some() {
    let explicit = vec!["read_file".to_string(), "list_dir".to_string()];
    let result = build_allowed_tools(&SubAgentType::Custom, Some(explicit.clone()), true).unwrap();
    assert_eq!(result, Some(explicit));
}

#[test]
fn build_allowed_tools_explicit_list_dedupes_and_trims() {
    let explicit = vec![
        "read_file".to_string(),
        "  read_file  ".to_string(), // trim + dedupe
        "list_dir".to_string(),
        "".to_string(), // skip empty
    ];
    let result = build_allowed_tools(&SubAgentType::Custom, Some(explicit), true).unwrap();
    assert_eq!(
        result,
        Some(vec!["read_file".to_string(), "list_dir".to_string()])
    );
}

#[test]
fn parse_spawn_request_extracts_cwd_when_present() {
    let input = json!({
        "prompt": "build feature A",
        "cwd": ".worktrees/feature-a"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(
        parsed.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        Some(".worktrees/feature-a".to_string())
    );
}

#[test]
fn parse_spawn_request_accepts_worktree_isolation() {
    let input = json!({
        "prompt": "build feature A",
        "worktree": true,
        "worktree_branch": "codex/agent-feature-a",
        "worktree_path": "feature-a",
        "worktree_base": "HEAD"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    let worktree = parsed.worktree.expect("worktree request");
    assert_eq!(worktree.branch.as_deref(), Some("codex/agent-feature-a"));
    assert_eq!(worktree.base_ref.as_deref(), Some("HEAD"));
    assert_eq!(
        worktree
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        Some("feature-a".to_string())
    );
}

#[test]
fn parse_spawn_request_rejects_cwd_with_worktree_isolation() {
    let input = json!({
        "prompt": "build feature A",
        "cwd": ".worktrees/manual",
        "worktree": true
    });
    let err = parse_spawn_request(&input).expect_err("cwd and worktree should conflict");
    assert!(
        err.to_string().contains("either cwd or worktree"),
        "unexpected error: {err}"
    );
}

#[test]
fn parse_spawn_request_cwd_absent_yields_none() {
    let input = json!({ "prompt": "no cwd" });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.cwd.is_none());
}

#[test]
fn parse_spawn_request_cwd_empty_string_yields_none() {
    let input = json!({ "prompt": "empty cwd", "cwd": "   " });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert!(parsed.cwd.is_none(), "whitespace-only cwd should be None");
}

#[test]
fn create_isolated_worktree_creates_branch_checkout_outside_parent_repo() {
    let repo = init_subagent_git_repo();
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-isolated-test".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let path = create_isolated_worktree(
        repo.path(),
        &request,
        Some("isolated-test"),
        &SubAgentType::Implementer,
    )
    .expect("worktree should be created");

    assert!(path.exists(), "worktree path should exist");
    assert!(
        !path.starts_with(repo.path()),
        "generated worktree must be outside the parent checkout"
    );
    assert_eq!(
        current_git_branch(&path).as_deref(),
        Some("codex/agent-isolated-test")
    );
}

#[test]
fn create_isolated_worktree_rejects_invalid_branch_as_input() {
    let repo = init_subagent_git_repo();
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("bad branch name".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(
        repo.path(),
        &request,
        Some("isolated-test"),
        &SubAgentType::Implementer,
    )
    .expect_err("invalid branch should fail");

    assert!(
        err.to_string().contains("Invalid worktree_branch"),
        "unexpected error: {err}"
    );
}

#[test]
fn build_subagent_system_prompt_appends_role_when_set() {
    let assignment = SubAgentAssignment::new("p".to_string(), Some("worker".to_string()));
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(
        prompt.contains("You are operating in the role of `worker`."),
        "expected role line present, got: {}",
        &prompt[prompt.len().saturating_sub(160)..]
    );
    // The shared background-worker / caller framing follows the role line.
    assert!(prompt.contains("background sub-agent"));
}

#[test]
fn build_subagent_system_prompt_skips_role_when_none() {
    let assignment = SubAgentAssignment::new("p".to_string(), None);
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(!prompt.contains("You are operating in the role of"));
}

#[test]
fn build_subagent_system_prompt_skips_role_when_blank() {
    let assignment = SubAgentAssignment::new("p".to_string(), Some("   ".to_string()));
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(!prompt.contains("You are operating in the role of"));
}

#[test]
fn subagent_done_sentinel_format_is_well_formed() {
    let res = make_snapshot(SubAgentStatus::Completed);
    let sentinel = subagent_done_sentinel("agent_xyz", &res, false);
    assert!(sentinel.starts_with("<codewhale:subagent.done>"));
    assert!(sentinel.ends_with("</codewhale:subagent.done>"));

    // The inner JSON parses and carries the expected fields.
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_xyz");
    assert_eq!(parsed["status"], "completed");
    assert_eq!(parsed["agent_type"], "general");
    assert_eq!(parsed["summary_location"], "previous_line");
    // issue #2652: a complete (non-truncated) summary is tagged as such.
    assert_eq!(parsed["summary_kind"], "complete");
    assert!(parsed.get("details").is_none());
    assert!(parsed.get("result_clipped").is_none());
    assert!(parsed.get("summary_complete").is_none());
    assert!(parsed.get("next_action").is_none());
    assert!(parsed.get("summary").is_none());
    assert!(parsed.get("duration_ms").is_none());
    assert!(parsed.get("steps").is_none());
}

#[test]
fn subagent_done_sentinel_keeps_large_result_out_of_metadata() {
    let mut res = make_snapshot(SubAgentStatus::Completed);
    res.result = Some("x".repeat(2048));
    let sentinel = subagent_done_sentinel("agent_big", &res, false);
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_big");
    assert_eq!(parsed["summary_location"], "previous_line");
    assert_eq!(parsed["summary_kind"], "complete");
    assert!(parsed.get("result_clipped").is_none());
    assert!(parsed.get("summary_complete").is_none());
    assert!(parsed.get("next_action").is_none());
    assert!(
        !inner.contains(&"x".repeat(128)),
        "sentinel should not duplicate large result text"
    );
}

#[test]
fn subagent_done_sentinel_marks_truncated_summaries() {
    // issue #2652: when the child summary was length-gated, the sentinel must
    // advertise summary_kind:"truncated" so the parent can steer verification.
    let res = make_snapshot(SubAgentStatus::Completed);
    let sentinel = subagent_done_sentinel("agent_trunc", &res, true);
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["summary_kind"], "truncated");
}

#[test]
fn stamp_subagent_summary_appends_note_when_short() {
    // issue #2652: a short (complete) summary gets the soft self-report note
    // and is NOT marked truncated.
    let (stamped, truncated) = stamp_subagent_summary("All tests pass.");
    assert!(!truncated);
    assert!(stamped.starts_with("All tests pass."));
    assert!(
        stamped.contains("[Sub-agent self-report"),
        "short summary gets the provenance note"
    );
    assert!(
        !stamped.contains("[Sub-agent summary truncated"),
        "short summary must not get the truncation footer"
    );
}

#[test]
fn stamp_subagent_summary_truncates_when_over_budget() {
    // issue #2652: a summary exceeding the budget is head+tail truncated using
    // the existing [Output truncated ...] vocabulary, honestly noting there is
    // no retrieve handle, and is marked truncated.
    let big = "a".repeat(SUBAGENT_SUMMARY_CHAR_BUDGET + 5_000);
    let (stamped, truncated) = stamp_subagent_summary(&big);
    assert!(truncated);
    assert!(
        stamped.contains("[Sub-agent summary truncated"),
        "long summary gets the truncation footer"
    );
    assert!(
        stamped.contains("not in the spillover store"),
        "footer is honest about the missing retrieve handle"
    );
    assert!(
        !stamped.contains("[Sub-agent self-report"),
        "truncated summary must not also get the self-report note"
    );
    // Head and tail slices are present; a run of budget-length 'a's is gone
    // from the middle.
    assert!(stamped.contains(&"a".repeat(SUBAGENT_SUMMARY_HEAD_CHARS)));
    assert!(stamped.contains(&"a".repeat(SUBAGENT_SUMMARY_TAIL_CHARS)));
    assert!(
        stamped.chars().filter(|c| *c == 'a').count() < big.chars().count(),
        "truncation removed middle characters"
    );
}

#[test]
fn subagent_failed_sentinel_format_is_well_formed() {
    let sentinel = subagent_failed_sentinel("agent_zzz", "boom");
    let inner = sentinel
        .trim_start_matches("<codewhale:subagent.done>")
        .trim_end_matches("</codewhale:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_zzz");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["error_location"], "previous_line");
    assert!(parsed.get("details").is_none());
    assert!(parsed.get("next_action").is_none());
    // Stays lean — the error text lives on the previous line, not the sentinel.
    assert!(parsed.get("error").is_none());
}

#[test]
fn annotated_failure_message_composes_class_tag_and_model_hint() {
    // #3884: the failure recorder composes subagent_failure_message (adds the
    // class tag + full chain) with annotate_child_model_error (adds the
    // model-availability hint). Pin the composition the mailbox/update_failed
    // call sites actually perform, not just the helper in isolation.
    let err = anyhow::Error::new(crate::llm_client::LlmError::AuthorizationError(
        "The model `gpt-5.5-codex` does not exist or you do not have access".to_string(),
    ))
    .context("Responses API request failed");

    let annotated = annotate_child_model_error(&subagent_failure_message(&err), "gpt-5.5-codex");

    // Class tag from subagent_failure_message.
    assert!(annotated.starts_with("[auth]"), "{annotated}");
    // Full chain preserved.
    assert!(
        annotated.contains("Responses API request failed"),
        "{annotated}"
    );
    assert!(annotated.contains("does not exist"), "{annotated}");
    // Model-availability hint fired because the real provider text now
    // reaches the classifier (it could not when only the masked outer
    // context string was recorded).
    assert!(annotated.contains("gpt-5.5-codex"), "{annotated}");
    assert!(
        annotated.contains("child model override")
            || annotated.contains("child-agent model config"),
        "{annotated}"
    );
}

#[test]
fn subagent_failure_message_preserves_error_chain() {
    // #3884: `to_string()` on an anyhow error prints only the outermost
    // context ("Responses API request failed"), masking the HTTP status and
    // body detail carried by the source `LlmError`. The failure message must
    // walk the chain and prefix the error class.
    let err = anyhow::Error::new(crate::llm_client::LlmError::InvalidRequest {
        status: 400,
        message: "model `gpt-5.5-codex` is not supported on this endpoint".to_string(),
    })
    .context("Responses API request failed");

    let message = subagent_failure_message(&err);
    assert!(message.starts_with("[invalid_request]"), "{message}");
    assert!(
        message.contains("Responses API request failed"),
        "{message}"
    );
    assert!(message.contains("Invalid request (400)"), "{message}");
    assert!(
        message.contains("not supported on this endpoint"),
        "{message}"
    );

    // Rate limits classify too — the fanout failure shape from the report.
    let err = anyhow::Error::new(crate::llm_client::LlmError::RateLimited {
        message: "please slow down".to_string(),
        retry_after: None,
    })
    .context("Responses API request failed");
    let message = subagent_failure_message(&err);
    assert!(message.starts_with("[rate_limited]"), "{message}");
    assert!(message.contains("please slow down"), "{message}");

    // Plain errors with no LlmError in the chain pass through untagged but
    // still fully chained.
    let err = anyhow::anyhow!("boom").context("outer");
    let message = subagent_failure_message(&err);
    assert_eq!(message, "outer: boom");
}

#[test]
fn annotate_child_model_error_adds_actionable_hint() {
    // #2653: a bare provider 403 becomes actionable by naming the model and the
    // recovery path, while unrelated errors pass through unchanged.
    let auth = annotate_child_model_error("403 Forbidden", "kimi-k2");
    assert!(auth.contains("kimi-k2"), "names the model: {auth}");
    assert!(
        auth.contains("child model override"),
        "names the recovery path: {auth}"
    );
    assert!(
        auth.contains("403 Forbidden"),
        "preserves the original: {auth}"
    );

    let unrelated = annotate_child_model_error("connection reset by peer", "kimi-k2");
    assert_eq!(unrelated, "connection reset by peer");

    // #3020: provider rejections that classify as Internal (not
    // Authorization/State) still get the hint via raw-text matching.
    let not_exist = annotate_child_model_error("Model Not Exist", "kimi-k2");
    assert!(
        not_exist.contains("child-agent model config"),
        "DeepSeek-style rejection gets the hint: {not_exist}"
    );

    let openai_style = annotate_child_model_error(
        "The model `gpt-5.5-nano` does not exist or you do not have access to it.",
        "gpt-5.5-nano",
    );
    assert!(
        openai_style.contains("child-agent model config"),
        "OpenAI-style rejection gets the hint: {openai_style}"
    );
}

#[test]
fn subagent_runtime_default_max_depth_is_three() {
    // Sanity-check the constant — bumping it without a test means stale docs.
    assert_eq!(DEFAULT_MAX_SPAWN_DEPTH, 3);
}

#[test]
fn would_exceed_depth_at_boundary() {
    // depth=2, max=3 → next spawn (depth 3) is allowed (allow-equal).
    // depth=3, max=3 → next spawn (depth 4) exceeds.
    let runtime = stub_runtime();
    let mut at_max = runtime.clone();
    at_max.spawn_depth = 3;
    at_max.max_spawn_depth = 3;
    assert!(
        at_max.would_exceed_depth(),
        "depth 3 + max 3 → next would be 4, exceeds"
    );

    let mut below_max = runtime;
    below_max.spawn_depth = 2;
    below_max.max_spawn_depth = 3;
    assert!(
        !below_max.would_exceed_depth(),
        "depth 2 + max 3 → next is 3, allowed"
    );
}

#[test]
fn clamp_child_max_spawn_depth_enforces_absolute_ceiling() {
    let ceiling = codewhale_config::MAX_SPAWN_DEPTH_CEILING;
    // Deep child re-supplying max_depth cannot push the cap past the ceiling —
    // this is the recursion-ring-limit bypass fix. Once at the ceiling, the
    // resulting cap equals the ceiling, so `would_exceed_depth` blocks.
    assert_eq!(clamp_child_max_spawn_depth(ceiling, 5), ceiling);
    assert_eq!(clamp_child_max_spawn_depth(ceiling - 1, 5), ceiling);
    // A smaller request below the ceiling is still honored (fewer rings).
    assert_eq!(clamp_child_max_spawn_depth(1, 2), 3);
    // Saturating add cannot overflow into a huge cap.
    assert_eq!(clamp_child_max_spawn_depth(u32::MAX, 5), ceiling);

    // End-to-end: a runtime whose cap was set via the clamp at the ceiling
    // cannot spawn another ring.
    let mut rt = stub_runtime();
    rt.spawn_depth = ceiling;
    rt.max_spawn_depth = clamp_child_max_spawn_depth(rt.spawn_depth, 5);
    assert!(
        rt.would_exceed_depth(),
        "at the ceiling, a further spawn must be blocked regardless of max_depth"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn rate_limit_pause_blocks_subagent_spawn() {
    let _guard = crate::retry_status::test_guard();
    crate::retry_status::clear();
    crate::retry_status::clear_rate_limit();
    crate::retry_status::note_rate_limit(Duration::from_secs(30));

    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);

    let err = spawn_subagent_from_input(
        json!({"prompt": "inspect the retry gate"}),
        Arc::clone(&manager),
        runtime,
    )
    .await
    .expect_err("active provider rate-limit pause must refuse new sub-agent work");

    assert!(
        err.to_string().contains("rate-limiting"),
        "error should name the provider throttle: {err}"
    );
    assert!(
        manager.read().await.list().is_empty(),
        "refused spawn must not register or launch a worker"
    );
    crate::retry_status::clear_rate_limit();
}

#[test]
fn child_runtime_increments_depth_and_preserves_auto_approve() {
    let mut parent = stub_runtime();
    parent.spawn_depth = 1;
    parent.context.auto_approve = false; // parent in suggest mode
    let child = parent.child_runtime();
    assert_eq!(child.spawn_depth, 2, "child depth = parent + 1");
    assert_eq!(child.step_api_timeout, DEFAULT_STEP_API_TIMEOUT);
    assert!(
        !child.context.auto_approve,
        "child must inherit parent approval state"
    );
    assert!(!parent.context.auto_approve);

    parent.context.auto_approve = true;
    let auto_child = parent.child_runtime();
    assert!(
        auto_child.context.auto_approve,
        "auto-approved parents should still create auto-approved children"
    );
}

#[test]
fn child_and_background_runtimes_preserve_step_api_timeout() {
    let timeout = Duration::from_secs(7);
    let parent = stub_runtime().with_step_api_timeout(timeout);

    let child = parent.child_runtime();
    assert_eq!(child.step_api_timeout, timeout);

    let background = parent.background_runtime();
    assert_eq!(background.step_api_timeout, timeout);
}

#[tokio::test]
async fn subagent_registry_blocks_approval_tools_without_parent_auto_approve() {
    let mut runtime = stub_runtime();
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        Some(vec!["exec_shell".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("approval-gated child tool should be blocked");

    assert!(
        err.to_string().contains("requires approval"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn implementer_delegation_allows_suggest_write_without_parent_auto_approve() {
    // Issue #1828: implementer agents could not write files even when their
    // whole job is to land code changes, because the registry blocked every
    // approval-gated tool when the parent ran in `suggest` mode. The
    // hardened gate (#1833) delegates `Suggest`-level tools (write_file,
    // edit_file, apply_patch) to write-capable roles.
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let result = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "delegated.txt", "content": "hello"}),
        )
        .await
        .expect("delegated write should be allowed for implementer");

    let written = std::fs::read_to_string(workspace.join("delegated.txt"))
        .expect("file should exist after delegated write");
    assert_eq!(written, "hello");
    assert!(
        !result.contains("requires approval"),
        "successful write should not look like an approval error: {result}"
    );
}

#[tokio::test]
async fn general_delegation_still_blocks_suggest_write_without_parent_auto_approve() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "general.txt", "content": "ok"}),
        )
        .await
        .expect_err("general agent should not silently gain write permission");
    let msg = err.to_string();
    assert!(
        msg.contains("not delegated to general sub-agents"),
        "general writes should be rejected with a role-aware message: {msg}"
    );

    assert!(
        !workspace.join("general.txt").exists(),
        "general write must not land without parent auto-approve"
    );
}

#[tokio::test]
async fn explore_role_still_blocks_suggest_writes_without_parent_auto_approve() {
    // Read-only stances (explore, plan, review, verifier) must not gain
    // write capabilities via delegation — otherwise a parent that asked
    // for "just look at the code" could find files mutated behind its back.
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "should_not_appear.txt", "content": "denied"}),
        )
        .await
        .expect_err("explore agents must not write");
    let msg = err.to_string();
    assert!(
        msg.contains("explore") && msg.contains("not permitted"),
        "explore writes should be rejected with a role-aware message: {msg}"
    );
    assert!(
        !tmp.path().join("should_not_appear.txt").exists(),
        "file must not have been written"
    );
}

#[tokio::test]
async fn explore_role_blocks_writes_even_under_parent_auto_approve() {
    // #3217: the authoritative per-role posture closes the auto-approve bypass —
    // a read-only role cannot mutate the workspace even when the parent session
    // is auto-approved.
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "nope.txt", "content": "denied"}),
        )
        .await
        .expect_err("explore must not write even under parent auto-approve");
    assert!(
        err.to_string().contains("not permitted"),
        "expected posture rejection, got: {err}"
    );
    assert!(
        !tmp.path().join("nope.txt").exists(),
        "file must not have been written under auto-approve"
    );
}

#[tokio::test]
async fn delegated_write_role_still_blocks_required_tools() {
    // Required-level tools (exec_shell, etc.) remain gated behind parent
    // auto-approve regardless of role. Implementer can write files, but it
    // still can't bypass shell approval just because it's a "write" role.
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        Some(vec!["exec_shell".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("Required-level shell must still need parent auto-approve");
    assert!(
        err.to_string().contains(
            "cannot run inside this sub-agent unless the parent session is auto-approved"
        ),
        "expected Required-level approval message, got: {err}"
    );
}

#[tokio::test]
async fn auto_approved_parent_runs_required_tools_in_subagent() {
    // Baseline: when the parent runtime IS auto-approved, every approval
    // class is permitted (same as before the delegation hardening).
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    // Calling exec_shell with interactive=true is what we block via the
    // separate terminal-takeover guard; pick the simpler write-file path
    // to assert that approval gating is off when auto_approve is set.
    registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "auto.txt", "content": "auto"}),
        )
        .await
        .expect("auto-approved parent should allow writes");
}

#[test]
fn subagent_request_budget_allows_large_write_file_arguments() {
    assert_eq!(
        SUBAGENT_RESPONSE_MAX_TOKENS, 16_384,
        "non-streaming sub-agent tool calls need enough output budget for large write_file arguments"
    );
}

#[test]
fn truncated_subagent_tool_calls_return_model_visible_errors() {
    let tool_uses = vec![(
        "toolu_write".to_string(),
        "write_file".to_string(),
        json!({"path": "report.md", "content": "partial"}),
    )];

    let results = truncated_response_tool_results(&tool_uses);

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_write");
            assert_eq!(is_error, &Some(true));
            assert!(content.contains("truncated by max_tokens"));
            assert!(content.contains("write_file"));
            assert!(content.contains("smaller writes"));
        }
        other => panic!("expected tool error result, got {other:?}"),
    }
}

#[test]
fn truncated_subagent_text_response_returns_model_visible_error() {
    let results = truncated_response_text_retry_message();

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("truncated by max_tokens"));
            assert!(text.contains("No complete tool call was available"));
            assert!(text.contains("Retry with a shorter response"));
        }
        other => panic!("expected text retry message, got {other:?}"),
    }
}

#[test]
fn consecutive_truncated_subagent_responses_are_capped() {
    let mut consecutive = 0;

    for _ in 0..MAX_CONSECUTIVE_TRUNCATED_SUBAGENT_RESPONSES {
        record_truncated_subagent_response(&mut consecutive).expect("within truncation cap");
    }

    let err = record_truncated_subagent_response(&mut consecutive)
        .expect_err("one more truncation should stop the sub-agent");
    assert!(err.to_string().contains("truncated by max_tokens"));
    assert!(err.to_string().contains("consecutive"));

    reset_truncated_subagent_responses(&mut consecutive);
    record_truncated_subagent_response(&mut consecutive).expect("reset should allow recovery");
    assert_eq!(consecutive, 1);
}

#[test]
fn child_cancellation_cascades_from_parent() {
    let parent = stub_runtime();
    let child = parent.child_runtime();
    assert!(!child.cancel_token.is_cancelled());
    parent.cancel_token.cancel();
    assert!(
        child.cancel_token.is_cancelled(),
        "parent cancel() must propagate to child via child_token()"
    );
}

#[test]
fn mailbox_propagates_through_child_runtime_chain() {
    use crate::tools::subagent::mailbox::Mailbox;
    let parent_token = CancellationToken::new();
    let (mailbox, _rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox);

    let child = parent.child_runtime();
    let grandchild = child.child_runtime();
    assert!(parent.mailbox.is_some());
    assert!(child.mailbox.is_some(), "child inherits parent mailbox");
    assert!(
        grandchild.mailbox.is_some(),
        "grandchild inherits via the cloned Arc inside Mailbox"
    );
}

#[test]
fn subagent_rejects_interactive_shell_terminal_takeover() {
    let err = reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "python3 -i",
            "interactive": true
        }),
    )
    .expect_err("sub-agents must not inherit the parent terminal");

    let msg = err.to_string();
    assert!(msg.contains("cannot use exec_shell with interactive=true"));
    assert!(msg.contains("parent TUI terminal"));

    reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "cargo check",
            "interactive": false
        }),
    )
    .expect("non-interactive shell remains allowed");
    reject_subagent_terminal_takeover(
        "exec_shell",
        &serde_json::json!({
            "command": "cargo test",
            "background": true
        }),
    )
    .expect("background shell remains allowed");
}

#[tokio::test]
async fn mailbox_close_as_cancel_propagates_to_grandchild_runtime() {
    use crate::tools::subagent::mailbox::Mailbox;
    let parent_token = CancellationToken::new();
    let (mailbox, _rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox.clone());

    let child = parent.child_runtime();
    let grandchild = child.child_runtime();
    assert!(!grandchild.cancel_token.is_cancelled());

    // Close the mailbox via *any* clone — the original or the one stored on
    // the runtime. Cancellation must reach all the way to the grandchild.
    mailbox.close();
    assert!(parent.cancel_token.is_cancelled());
    assert!(child.cancel_token.is_cancelled());
    assert!(
        grandchild.cancel_token.is_cancelled(),
        "close-as-cancel must propagate across max_spawn_depth=3"
    );
}

#[tokio::test]
async fn mailbox_orders_messages_from_parent_and_child_runtimes() {
    use crate::tools::subagent::mailbox::{Mailbox, MailboxMessage};
    let parent_token = CancellationToken::new();
    let (mailbox, mut rx) = Mailbox::new(parent_token.clone());

    let mut parent = stub_runtime();
    parent.cancel_token = parent_token;
    parent.mailbox = Some(mailbox);
    let child = parent.child_runtime();

    // Interleave sends from both runtimes; sequence numbers stay monotonic.
    parent
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("parent_a", "step 1"));
    child
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("child_b", "step 1"));
    parent
        .mailbox
        .as_ref()
        .unwrap()
        .send(MailboxMessage::progress("parent_a", "step 2"));

    let drained = rx.drain();
    assert_eq!(drained.len(), 3);
    assert_eq!(drained[0].seq, 1);
    assert_eq!(drained[1].seq, 2);
    assert_eq!(drained[2].seq, 3);
    // Verify ordering is preserved across publishers.
    match (
        &drained[0].message,
        &drained[1].message,
        &drained[2].message,
    ) {
        (
            MailboxMessage::Progress { agent_id: a, .. },
            MailboxMessage::Progress { agent_id: b, .. },
            MailboxMessage::Progress { agent_id: c, .. },
        ) => {
            assert_eq!(a, "parent_a");
            assert_eq!(b, "child_b");
            assert_eq!(c, "parent_a");
        }
        other => panic!("unexpected message order: {other:?}"),
    }
}

#[test]
fn persisted_empty_allowed_tools_loads_as_full_inheritance() {
    // Backward-compat: a v0.6.5 session that persisted with an empty Vec
    // (or a v0.6.6 session with no narrowing) should load as None on
    // restart, meaning full inheritance.
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("subagents.v1.json");
    let payload = serde_json::json!({
        "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
        "agents": [{
            "id": "agent_test",
            "agent_type": "general",
            "prompt": "p",
            "assignment": { "objective": "p" },
            "status": "Completed",
            "result": null,
            "steps_taken": 0,
            "duration_ms": 0,
            "allowed_tools": [],
            "updated_at_ms": 0
        }]
    });
    std::fs::write(&state_path, payload.to_string()).unwrap();

    let mut manager = SubAgentManager::new(dir.path().to_path_buf(), 5).with_state_path(state_path);
    manager.load_state().expect("load should succeed");
    let agent = manager.agents.get("agent_test").expect("loaded agent");
    assert!(
        agent.allowed_tools.is_none(),
        "empty Vec on disk → None (full inheritance)"
    );
}

#[test]
fn persisted_non_empty_allowed_tools_loads_as_narrow() {
    // Backward-compat the other way: a v0.6.5 session that persisted with
    // an explicit narrow list keeps that list on reload.
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("subagents.v1.json");
    let payload = serde_json::json!({
        "schema_version": SUBAGENT_STATE_SCHEMA_VERSION,
        "agents": [{
            "id": "agent_narrow",
            "agent_type": "custom",
            "prompt": "p",
            "assignment": { "objective": "p" },
            "status": "Completed",
            "result": null,
            "steps_taken": 0,
            "duration_ms": 0,
            "allowed_tools": ["read_file", "list_dir"],
            "updated_at_ms": 0
        }]
    });
    std::fs::write(&state_path, payload.to_string()).unwrap();

    let mut manager = SubAgentManager::new(dir.path().to_path_buf(), 5).with_state_path(state_path);
    manager.load_state().expect("load should succeed");
    let agent = manager.agents.get("agent_narrow").expect("loaded agent");
    assert_eq!(
        agent.allowed_tools.as_deref(),
        Some(&["read_file".to_string(), "list_dir".to_string()][..]),
        "non-empty Vec → Some(list), narrow scope preserved"
    );
}

/// Build a minimal `SubAgentRuntime` for tests that exercise pure runtime
/// helpers (depth, cancellation, child_runtime). Doesn't construct a real
/// HTTP client — calls that hit `runtime.client` would fail, but the
/// helpers we test here don't.
fn stub_runtime() -> SubAgentRuntime {
    use tokio_util::sync::CancellationToken;

    let workspace = std::env::temp_dir().join("codewhale-test-stub");
    let context = ToolContext::new(workspace.clone());
    SubAgentRuntime {
        client: stub_client(),
        model: "deepseek-v4-flash".to_string(),
        auto_model: false,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        role_models: std::collections::HashMap::new(),
        provider_routing: None,
        context,
        allow_shell: true,
        agent_tool_surface_options: AgentToolSurfaceOptions::new(ShellPolicy::Full),
        worker_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
        event_tx: None,
        manager: new_shared_subagent_manager(workspace, 5),
        spawn_depth: 0,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
        cancel_token: CancellationToken::new(),
        mailbox: None,
        parent_agent_id: None,
        parent_completion_tx: None,
        fork_context: None,
        mcp_pool: None,
        step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
        tool_timeout: DEFAULT_TOOL_TIMEOUT,
        speech_output_dir: None,
        todos: crate::tools::todo::new_shared_todo_list(),
    }
}

/// A minimal stub client. Test helpers below only ever check struct fields
/// (depth, cancel_token, context); they don't call the network. We need a
/// *some* `DeepSeekClient` because `SubAgentRuntime.client` isn't
/// `Option<...>`. `Config::default()` is enough — `DeepSeekClient::new`
/// only validates that an API key field exists, not that the key works.
fn stub_runtime_for_provider(provider: &str) -> SubAgentRuntime {
    let mut runtime = stub_runtime();
    runtime.client = stub_client_for_provider(provider);
    runtime
}

fn stub_client_for_provider(provider: &str) -> DeepSeekClient {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut providers = crate::config::ProvidersConfig::default();
    match provider {
        "moonshot" => {
            providers.moonshot = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        "openrouter" => {
            providers.openrouter = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        "zai" => {
            providers.zai = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        // OpenAI Codex (ChatGPT backend). Exercises the faster-lane reasoning
        // rule: GPT-5.5 children stay on GPT-5.5 and resolve Low reasoning.
        "openai-codex" => {
            providers.openai_codex = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
        // Ollama is keyless (local runtime); extend per-provider as needed.
        "ollama" => {}
        other => panic!("extend stub_client_for_provider for provider {other}"),
    }
    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        provider: Some(provider.to_string()),
        providers: Some(providers),
        ..crate::config::Config::default()
    };
    DeepSeekClient::new(&config).expect("stub client should construct")
}

fn stub_client() -> DeepSeekClient {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        ..crate::config::Config::default()
    };
    DeepSeekClient::new(&config).expect("stub client should construct")
}

// ---- #405 session-boundary classification ----
//
// Each manager assigns a fresh session_boot_id; agents stamp the id at
// spawn time. After persist + reload by a *new* manager, those agents
// carry the prior boot id and are classified as `from_prior_session`.
// Listings default to current-session only; `include_archived=true` surfaces
// the prior-session records with the flag set.

fn insert_prior_session_agent(
    manager: &mut SubAgentManager,
    id: &str,
    status: SubAgentStatus,
    boot_id: &str,
) {
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        id.to_string(),
        SubAgentType::General,
        "old prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        manager.workspace.clone(),
        boot_id.to_string(),
    );
    agent.status = status;
    agent.id = id.to_string();
    manager.agents.insert(id.to_string(), agent);
}

#[test]
fn session_boot_ids_are_unique_per_manager() {
    let a = SubAgentManager::new(PathBuf::from("."), 1);
    let b = SubAgentManager::new(PathBuf::from("."), 1);
    assert_ne!(a.session_boot_id(), b.session_boot_id());
}

#[test]
fn list_filtered_drops_prior_session_terminals_by_default() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_running",
        SubAgentStatus::Running,
        &current_boot,
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_completed",
        SubAgentStatus::Completed,
        "boot_old_session",
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_running",
        SubAgentStatus::Running,
        "boot_old_session",
    );

    let listed = manager.list_filtered(false);
    let ids: Vec<&str> = listed.iter().map(|s| s.agent_id.as_str()).collect();
    assert!(ids.contains(&"current_running"), "{ids:?}");
    assert!(
        ids.contains(&"prior_running"),
        "still-running prior-session agents stay visible: {ids:?}"
    );
    assert!(
        !ids.contains(&"prior_completed"),
        "completed prior-session agents are hidden by default: {ids:?}"
    );

    let prior = listed
        .iter()
        .find(|s| s.agent_id == "prior_running")
        .unwrap();
    assert!(prior.from_prior_session);
    let current = listed
        .iter()
        .find(|s| s.agent_id == "current_running")
        .unwrap();
    assert!(!current.from_prior_session);
}

#[test]
fn list_snapshots_refresh_git_branch_from_agent_workspace() {
    let repo = init_subagent_git_repo();
    git(repo.path(), &["checkout", "-b", "feature/agent-old"]);

    let mut manager = SubAgentManager::new(repo.path().to_path_buf(), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_running",
        SubAgentStatus::Running,
        &current_boot,
    );

    let listed = manager.list_filtered(false);
    let agent = listed
        .iter()
        .find(|agent| agent.agent_id == "current_running")
        .expect("current agent should be listed");
    assert_eq!(agent.git_branch.as_deref(), Some("feature/agent-old"));
    assert_eq!(agent.workspace.as_deref(), Some(repo.path()));

    git(repo.path(), &["checkout", "-b", "feature/agent-new"]);

    let refreshed = manager.list_filtered(false);
    let agent = refreshed
        .iter()
        .find(|agent| agent.agent_id == "current_running")
        .expect("current agent should still be listed");
    assert_eq!(agent.git_branch.as_deref(), Some("feature/agent-new"));
}

#[test]
fn list_filtered_with_include_archived_returns_everything() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    let current_boot = manager.session_boot_id().to_string();
    insert_prior_session_agent(
        &mut manager,
        "current_done",
        SubAgentStatus::Completed,
        &current_boot,
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_done",
        SubAgentStatus::Completed,
        "boot_old",
    );
    insert_prior_session_agent(
        &mut manager,
        "prior_failed",
        SubAgentStatus::Failed("boom".to_string()),
        "boot_old",
    );

    let listed = manager.list_filtered(true);
    assert_eq!(listed.len(), 3, "{listed:?}");
    let prior = listed.iter().find(|s| s.agent_id == "prior_done").unwrap();
    assert!(prior.from_prior_session);
    let current = listed
        .iter()
        .find(|s| s.agent_id == "current_done")
        .unwrap();
    assert!(!current.from_prior_session);
}

#[test]
fn agents_with_empty_boot_id_classify_as_prior_session() {
    // Records persisted before #405 land with an empty `session_boot_id`
    // due to `#[serde(default)]`. The manager treats those the same as
    // a non-matching id — i.e. prior session.
    let mut manager = SubAgentManager::new(PathBuf::from("."), 5);
    insert_prior_session_agent(&mut manager, "legacy", SubAgentStatus::Completed, "");

    let listed_default = manager.list_filtered(false);
    assert!(
        listed_default.iter().all(|s| s.agent_id != "legacy"),
        "legacy completed agents are hidden by default"
    );

    let listed_archived = manager.list_filtered(true);
    let legacy = listed_archived
        .iter()
        .find(|s| s.agent_id == "legacy")
        .unwrap();
    assert!(legacy.from_prior_session);
}

#[test]
fn persist_round_trip_preserves_session_boot_id() {
    let dir = tempdir().expect("tempdir");
    let state_path = dir.path().join(SUBAGENT_STATE_FILE);

    let original_boot;
    {
        let mut writer =
            SubAgentManager::new(dir.path().to_path_buf(), 2).with_state_path(state_path.clone());
        original_boot = writer.session_boot_id().to_string();
        insert_prior_session_agent(
            &mut writer,
            "agent_persist",
            SubAgentStatus::Completed,
            &original_boot,
        );
        writer
            .persist_state()
            .expect("persist round-trip should write")
            .join()
            .expect("persist thread");
    }

    // A fresh manager comes up with a *different* boot id and reloads
    // the persisted state; the agent should now be classified prior.
    let mut reader =
        SubAgentManager::new(dir.path().to_path_buf(), 2).with_state_path(state_path.clone());
    reader.load_state().expect("reload should succeed");
    assert_ne!(reader.session_boot_id(), original_boot);

    let listed_default = reader.list_filtered(false);
    assert!(
        !listed_default.iter().any(|s| s.agent_id == "agent_persist"),
        "completed prior-session agent hidden after reload: {listed_default:?}"
    );
    let listed_all = reader.list_filtered(true);
    let snap = listed_all
        .iter()
        .find(|s| s.agent_id == "agent_persist")
        .unwrap();
    assert!(snap.from_prior_session);
}

// === Issue #756: parent-completion wakeup ===
//
// When an agent finishes, `run_subagent_task` emits a `SubAgentCompletion` on
// the runtime's `parent_completion_tx`. For root-spawned agents the engine turn
// loop drains that channel; for nested agents the running parent sub-agent
// owns a local receiver and injects the completion into its own transcript.
// These tests cover the routing logic and no-channel safety.

fn runtime_with_depth(
    spawn_depth: u32,
    parent_completion_tx: Option<mpsc::UnboundedSender<SubAgentCompletion>>,
) -> SubAgentRuntime {
    let mut rt = stub_runtime();
    rt.spawn_depth = spawn_depth;
    rt.parent_completion_tx = parent_completion_tx;
    rt
}

#[test]
fn emit_parent_completion_fires_for_direct_child() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(1, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_abc", "summary line\n<sentinel/>");

    assert!(sent, "depth=1 with channel wired should send");
    let received = rx.try_recv().expect("channel should have one message");
    assert_eq!(received.agent_id, "agent_abc");
    assert_eq!(received.payload, "summary line\n<sentinel/>");
    assert!(rx.try_recv().is_err(), "should be exactly one message");
}

#[test]
fn child_runtime_inherits_speech_output_dir() {
    let output_dir = PathBuf::from("configured-speech-output");
    let runtime = stub_runtime().with_speech_output_dir(Some(output_dir.clone()));

    let child = runtime.child_runtime();

    assert_eq!(child.speech_output_dir, Some(output_dir));
    assert_eq!(
        child.agent_tool_surface_options.speech_output_dir,
        Some(PathBuf::from("configured-speech-output"))
    );
}

#[test]
fn emit_parent_completion_fires_for_nested_child() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(2, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_grandchild", "nested summary");

    assert!(sent, "depth=2 child should send to its wired parent inbox");
    let received = rx.try_recv().expect("nested completion should be routed");
    assert_eq!(received.agent_id, "agent_grandchild");
    assert_eq!(received.payload, "nested summary");
}

#[test]
fn emit_parent_completion_skips_engine_self() {
    // depth 0 is the engine itself — the engine never spawns a task at
    // depth 0, but defend against accidental misuse.
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(0, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_root", "ignored");

    assert!(
        !sent,
        "depth=0 must not fire (only depth=1 direct children)"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn emit_parent_completion_no_channel_is_noop() {
    let runtime = runtime_with_depth(1, None);

    let sent = emit_parent_completion(&runtime, "agent_no_chan", "anything");

    assert!(
        !sent,
        "missing channel should be a silent no-op, not a panic"
    );
}

#[test]
fn emit_parent_completion_dropped_receiver_does_not_panic() {
    let (tx, rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    drop(rx);
    let runtime = runtime_with_depth(1, Some(tx));

    // The send returns an error internally but we discard it — the
    // caller's run_subagent_task does not care whether the engine is
    // still listening (it might be shutting down).
    let sent = emit_parent_completion(&runtime, "agent_orphan", "after-rx-drop");

    assert!(
        sent,
        "we still attempt the send; the engine being gone is not our problem"
    );
}

#[test]
fn terminal_results_excluding_returns_only_current_root_undelivered_agents() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    let current_boot = manager.current_session_boot_id.clone();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();

    let mut root = SubAgent::new(
        "agent_root_done".to_string(),
        SubAgentType::General,
        "root".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx.clone(),
        tmp.path().to_path_buf(),
        current_boot.clone(),
    );
    root.status = SubAgentStatus::Completed;
    root.result = Some("root result".to_string());

    let mut nested = SubAgent::new(
        "agent_nested_done".to_string(),
        SubAgentType::General,
        "nested".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx.clone(),
        tmp.path().to_path_buf(),
        current_boot,
    );
    nested.status = SubAgentStatus::Completed;

    let mut prior = SubAgent::new(
        "agent_prior_done".to_string(),
        SubAgentType::General,
        "prior".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        "prior_boot".to_string(),
    );
    prior.status = SubAgentStatus::Completed;

    manager.agents.insert(root.id.clone(), root);
    manager.agents.insert(nested.id.clone(), nested);
    manager.agents.insert(prior.id.clone(), prior);

    manager.register_worker(make_worker_spec(
        "agent_root_done",
        tmp.path().to_path_buf(),
    ));
    let mut nested_spec = make_worker_spec("agent_nested_done", tmp.path().to_path_buf());
    nested_spec.parent_run_id = Some("agent_root_parent".to_string());
    manager.register_worker(nested_spec);
    manager.register_worker(make_worker_spec(
        "agent_prior_done",
        tmp.path().to_path_buf(),
    ));

    let delivered = HashSet::from(["agent_already_delivered".to_string()]);
    let results = manager.terminal_results_excluding(&delivered);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].agent_id, "agent_root_done");

    let delivered = HashSet::from(["agent_root_done".to_string()]);
    assert!(manager.terminal_results_excluding(&delivered).is_empty());
}

#[tokio::test]
async fn run_subagent_task_emits_parent_completion_before_terminal_update() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 2)));
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_noop".to_string();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "noop".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        task_input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    manager.write().await.agents.insert(agent_id.clone(), agent);

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.manager = Arc::clone(&manager);

    let task = SubAgentTask {
        manager_handle: manager.clone(),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "no-op child run".to_string(),
        assignment: make_assignment(),
        allowed_tools: None,
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 0,
        token_budget: None,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    let manager_lock = manager.write().await;
    let task_handle = tokio::spawn(run_subagent_task(task));

    // While the manager write lock is held, completion can be emitted only if it
    // is sent before the terminal-state manager update (the ordering fixed by
    // issue #1961).
    let completion = tokio::time::timeout(Duration::from_secs(1), completion_rx.recv())
        .await
        .expect("completion should be emitted while manager write lock is still held");
    let completion = completion.expect("completion channel should remain open");
    assert_eq!(completion.agent_id, agent_id);

    drop(manager_lock);
    task_handle
        .await
        .expect("run_subagent_task should complete after lock release");

    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("completed agent should be present")
    };
    assert_eq!(snapshot.status, SubAgentStatus::Completed);
}

#[test]
fn child_runtime_propagates_completion_tx_for_gating() {
    // The channel is cloned through `child_runtime()` so descendants carry
    // it. Running sub-agents replace the channel in the runtime handed to
    // their nested tool registry, so this propagation must not strand it.
    let (tx, _rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let parent = runtime_with_depth(0, Some(tx));

    let child = parent.child_runtime();

    assert_eq!(child.spawn_depth, 1, "child increments depth");
    assert!(
        child.parent_completion_tx.is_some(),
        "child carries the wakeup channel forward"
    );
}

#[test]
fn nested_tool_runtime_routes_child_completions_to_local_inbox() {
    let (root_tx, mut root_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let direct_child_runtime = runtime_with_depth(1, Some(root_tx));
    let fork_context = SubAgentForkContext {
        system: None,
        messages: Vec::new(),
        structured_state_block: None,
    };

    let (tool_runtime, mut local_rx) =
        runtime_for_nested_agent_tools(&direct_child_runtime, "agent_parent", fork_context);
    let nested_child_runtime = tool_runtime.child_runtime();

    let sent = emit_parent_completion(
        &nested_child_runtime,
        "agent_nested",
        "nested child summary\n<codewhale:subagent.done>{}</codewhale:subagent.done>",
    );

    assert!(sent, "nested child should report to the local parent inbox");
    let local = local_rx
        .try_recv()
        .expect("local parent inbox receives nested completion");
    assert_eq!(local.agent_id, "agent_nested");
    assert!(
        root_rx.try_recv().is_err(),
        "root engine must not receive nested child completion directly"
    );
}

#[test]
fn child_completion_runtime_message_preserves_agent_and_provenance_guidance() {
    let message = child_completion_runtime_message(&[SubAgentCompletion {
        agent_id: "agent_nested".to_string(),
        payload: "SUMMARY\n### EVIDENCE\n- src/lib.rs:1-3".to_string(),
    }]);
    assert_eq!(message.role, "user");
    let text = match &message.content[0] {
        ContentBlock::Text { text, .. } => text,
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(text.contains("child_subagent_completion"));
    assert!(text.contains("agent_id: agent_nested"));
    assert!(text.contains("cite the child agent_id and the EVIDENCE lines"));
    assert!(text.contains("src/lib.rs:1-3"));
}

#[test]
fn subagent_runtime_default_step_api_timeout_is_legacy_120s() {
    // The legacy hardcoded constant is now the default field value so existing
    // call sites and tests that construct a runtime without explicit timeout
    // wiring keep their old behavior (#1806, #1808).
    let runtime = stub_runtime();
    assert_eq!(runtime.step_api_timeout, DEFAULT_STEP_API_TIMEOUT);
    assert_eq!(
        DEFAULT_STEP_API_TIMEOUT,
        std::time::Duration::from_secs(crate::config::DEFAULT_SUBAGENT_API_TIMEOUT_SECS)
    );
}

#[test]
fn with_step_api_timeout_overrides_runtime_field() {
    let runtime = stub_runtime().with_step_api_timeout(std::time::Duration::from_secs(900));
    assert_eq!(runtime.step_api_timeout.as_secs(), 900);
}

#[test]
fn tool_timeout_defaults_to_generous_budget_and_survives_spawn() {
    // Track A raised the per-tool timeout from the old 30s (which killed long
    // but legitimate tool runs) to a generous default, and that budget must
    // survive the child/background spawn clone rather than reverting.
    let parent = stub_runtime();
    assert!(
        parent.tool_timeout.as_secs() >= 300,
        "per-tool timeout must be a generous (>=300s) budget, not the old 30s"
    );
    let expected = parent.tool_timeout;
    assert_eq!(parent.child_runtime().tool_timeout, expected);
    assert_eq!(parent.background_runtime().tool_timeout, expected);
}

#[test]
fn child_runtime_preserves_step_api_timeout() {
    // Real sub-agents spawn through `child_runtime()` / `background_runtime()`;
    // forgetting to clone the timeout would silently drop the user's config
    // override and resurrect the 120 s default for every child step.
    let parent = stub_runtime().with_step_api_timeout(std::time::Duration::from_secs(900));
    let child = parent.child_runtime();
    let background = parent.background_runtime();

    assert_eq!(
        child.step_api_timeout.as_secs(),
        900,
        "child_runtime must preserve parent's per-step timeout"
    );
    assert_eq!(
        background.step_api_timeout.as_secs(),
        900,
        "background_runtime (detached) must also preserve the parent's timeout"
    );
}

#[test]
fn subagent_completion_payload_carries_existing_sentinel_format() {
    // The payload format is the same one already documented in
    // prompts/constitution.md: human summary on line 1, `<codewhale:subagent.done>`
    // sentinel on line 2. This test pins the format so future refactors
    // don't silently break the model's parsing contract.
    let mut snap = make_snapshot(SubAgentStatus::Completed);
    snap.result = Some("Found three errors.".to_string());

    let summary = summarize_subagent_result(&snap);
    let sentinel = subagent_done_sentinel("agent_test", &snap, false);
    let payload = format!("{summary}\n{sentinel}");

    let mut lines = payload.lines();
    let first = lines.next().expect("first line is summary");
    let second = lines.next().expect("second line is sentinel");
    assert!(
        !first.starts_with("<codewhale:subagent.done>"),
        "summary should not be the sentinel itself"
    );
    assert!(
        second.starts_with("<codewhale:subagent.done>"),
        "second line is the sentinel"
    );
    assert!(second.ends_with("</codewhale:subagent.done>"));
    assert!(
        second.contains("\"agent_id\":\"agent_test\""),
        "sentinel JSON includes agent_id"
    );
    assert!(
        !second.contains("Found three errors."),
        "sentinel should not duplicate the human summary line"
    );
}

/// #2683 — Verify the model-facing tool catalog only advertises canonical
/// subagent tools and never exposes legacy superseded names.
#[test]
fn model_catalog_only_advertises_canonical_subagent_tools() {
    use crate::tools::ToolRegistryBuilder;

    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = stub_runtime();
    let manager = runtime.manager.clone();
    let ctx = crate::tools::spec::ToolContext::new(tmp.path().to_path_buf());
    let registry = ToolRegistryBuilder::new()
        .with_subagent_tools(manager, runtime)
        .build(ctx);

    let api_names: Vec<String> = registry
        .to_api_tools()
        .into_iter()
        .map(|t| t.name)
        .collect();

    assert_eq!(
        api_names
            .iter()
            .filter(|name| name.as_str() == "agent")
            .count(),
        1,
        "agent should be the only model-facing sub-agent lifecycle tool"
    );
}

// ── #3018: provider-aware auto routing and model validation ─────────────────

#[tokio::test]
async fn faster_route_on_provider_without_known_sibling_stays_on_parent_model() {
    // AC: Ollama must never build a request with a DeepSeek id; even when the
    // model explicitly asks for a faster child, an unknown family stays on the
    // parent model.
    let mut runtime = stub_runtime_for_provider("ollama").with_auto_model(true);
    runtime.model = "qwen3:32b".to_string();

    for prompt in ["hi", "please refactor the whole auth module for security"] {
        let route = resolve_subagent_assignment_route(
            &runtime,
            None,
            prompt,
            &SubAgentType::General,
            ModelRoute::Faster,
            SubAgentThinking::Inherit,
        )
        .await;
        assert_eq!(route.model, "qwen3:32b", "prompt {prompt:?}");
        assert!(
            !route.model.contains("deepseek"),
            "no DeepSeek id may be fabricated: {route:?}"
        );
    }
}

#[test]
fn faster_route_uses_known_deepseek_and_glm_family_siblings() {
    let mut deepseek = stub_runtime();
    deepseek.model = "deepseek-v4-pro".to_string();
    let route = fallback_subagent_assignment_route(
        &deepseek,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model, "deepseek-v4-flash");

    let mut zai = stub_runtime_for_provider("zai");
    zai.model = "GLM-5.2".to_string();
    let route = fallback_subagent_assignment_route(
        &zai,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect docs",
    );
    // GLM-5.2 faster/explore children route to GLM-5-Turbo (same-family fast
    // sibling), not down to GLM-5.1.
    assert_eq!(route.model, "GLM-5-Turbo");
    assert_ne!(route.model, "GLM-5.1");

    let mut openrouter = stub_runtime_for_provider("openrouter");
    openrouter.model = "z-ai/glm-5.2".to_string();
    let route = fallback_subagent_assignment_route(
        &openrouter,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect docs",
    );
    assert_eq!(route.model, "z-ai/glm-5-turbo");
    assert_ne!(route.model, "z-ai/glm-5.1");
}

#[test]
fn gpt55_faster_route_stays_on_gpt55_with_low_reasoning() {
    // AC: a faster/explore child of a GPT-5.5 (OpenAI Codex) parent must stay
    // on GPT-5.5 — there is no cheaper same-provider sibling, so we never
    // fabricate a DeepSeek/GLM id — and resolve Low reasoning rather than Off,
    // because the Codex adapter has no true "off" on the wire.
    //
    // The Codex client validates OAuth credentials at construction time, so we
    // stub the access-token env var for the duration of this test (save/restore
    // to avoid leaking into parallel tests).
    let prev_token = std::env::var_os("OPENAI_CODEX_ACCESS_TOKEN");
    // Safety: this test does not run concurrently with other tests that read
    // OPENAI_CODEX_ACCESS_TOKEN, and we restore the original value below.
    unsafe {
        std::env::set_var("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    }
    let mut codex = stub_runtime_for_provider("openai-codex");
    unsafe {
        match prev_token {
            Some(prev) => std::env::set_var("OPENAI_CODEX_ACCESS_TOKEN", prev),
            None => std::env::remove_var("OPENAI_CODEX_ACCESS_TOKEN"),
        }
    }
    codex.model = "gpt-5.5".to_string();
    let route = fallback_subagent_assignment_route(
        &codex,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "inspect one file",
    );
    assert_eq!(route.model, "gpt-5.5");
    assert!(
        !route.model.contains("deepseek"),
        "no DeepSeek id may be fabricated: {route:?}"
    );
    assert!(
        !route.model.contains("glm"),
        "no GLM id may be fabricated: {route:?}"
    );
    assert_eq!(route.reasoning_effort.as_deref(), Some("low"));
    assert_ne!(route.reasoning_effort.as_deref(), Some("off"));
}

#[test]
fn role_model_validation_accepts_provider_native_ids() {
    // AC: [subagents] worker_model = "kimi-k2.5" on Moonshot must not fail
    // with "Expected a DeepSeek model id".
    let mut runtime = stub_runtime_for_provider("moonshot");
    runtime
        .role_models
        .insert("worker".to_string(), "kimi-k2.5".to_string());

    let model = configured_model_for_role_or_type(&runtime, Some("worker"), &SubAgentType::General)
        .expect("provider-native id is accepted");
    assert_eq!(model.as_deref(), Some("kimi-k2.5"));
}

// ---- #3965 per-role explicit provider routing ----

fn lm_studio_route_config() -> crate::config::Config {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://localhost:1234/v1".to_string()),
            api_key: Some("lm-studio".to_string()),
            ..Default::default()
        },
    );
    crate::config::Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("test-key".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn runtime_with_explore_route() -> SubAgentRuntime {
    let mut runtime = stub_runtime();
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "explore".to_string(),
        crate::config::SubagentRouteConfig {
            provider: "lm-studio".to_string(),
            model: Some("qwen-2.5-7b".to_string()),
        },
    );
    runtime.provider_routing = Some(Arc::new(SubagentProviderRouting {
        routes,
        config: lm_studio_route_config(),
    }));
    runtime
}

#[test]
fn role_provider_route_builds_client_on_routed_provider() {
    // AC (#3965): [subagents.routes.explore] provider = "lm-studio" builds the
    // child's client against the LM Studio custom endpoint — with the route's
    // model — while the parent session stays on DeepSeek.
    let runtime = runtime_with_explore_route();

    let (route, base_config) = configured_provider_route_for_role_or_type(
        &runtime,
        Some("Explore"),
        &SubAgentType::General,
    )
    .expect("role has an explicit provider route (case-insensitive key)");
    assert_eq!(route.provider, "lm-studio");

    let (client, model) =
        subagent_client_for_provider_route(base_config, &route.provider, route.model.as_deref())
            .expect("routed client builds against the custom endpoint");
    assert_eq!(client.api_provider(), crate::config::ApiProvider::Custom);
    assert_eq!(client.base_url(), "http://localhost:1234/v1");
    assert_eq!(model, "qwen-2.5-7b");
    assert_eq!(
        runtime.client.api_provider(),
        crate::config::ApiProvider::Deepseek,
        "parent client is untouched by the child's route"
    );
}

#[test]
fn role_without_provider_route_inherits_parent() {
    // Fallback AC (#3965): roles without a route keep the legacy behavior —
    // no route resolves, so the spawn path inherits the parent's client.
    let runtime = runtime_with_explore_route();
    assert!(
        configured_provider_route_for_role_or_type(
            &runtime,
            Some("general"),
            &SubAgentType::General,
        )
        .is_none()
    );

    let unrouted = stub_runtime();
    assert!(
        configured_provider_route_for_role_or_type(
            &unrouted,
            Some("explore"),
            &SubAgentType::Explore,
        )
        .is_none()
    );
}

#[test]
fn provider_route_with_unknown_provider_fails_clearly() {
    // Graceful-error AC (#3965): a route naming a provider that is neither
    // built-in nor a configured [providers.<name>] entry must fail the spawn
    // with a clear error instead of silently falling back to DeepSeek.
    let err = match subagent_client_for_provider_route(
        &lm_studio_route_config(),
        "no-such-provider",
        None,
    ) {
        Ok(_) => panic!("unknown provider must be rejected"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("no-such-provider"),
        "names the bad provider: {msg}"
    );
    assert!(
        msg.contains("[providers.no-such-provider]"),
        "points at the custom-provider config path: {msg}"
    );
}

#[test]
fn role_model_validation_stays_strict_on_official_deepseek() {
    let mut runtime = stub_runtime();
    runtime
        .role_models
        .insert("worker".to_string(), "kimi-k2.5".to_string());

    let err = configured_model_for_role_or_type(&runtime, Some("worker"), &SubAgentType::General)
        .expect_err("non-DeepSeek id is rejected on the official API");
    let msg = err.to_string();
    assert!(msg.contains("kimi-k2.5"), "names the bad id: {msg}");
    assert!(
        msg.contains("deepseek-v4-pro"),
        "lists accepted ids from model_completion_names_for_provider: {msg}"
    );
}

#[test]
fn normalize_requested_subagent_model_is_provider_aware() {
    assert_eq!(
        normalize_requested_subagent_model(
            "kimi-k2.5",
            "model",
            crate::config::ApiProvider::Moonshot
        )
        .expect("Moonshot accepts its own ids"),
        "kimi-k2.5"
    );
    assert_eq!(
        normalize_requested_subagent_model(
            "qwen3:32b",
            "model",
            crate::config::ApiProvider::Ollama
        )
        .expect("Ollama tags pass through"),
        "qwen3:32b"
    );
    assert!(
        normalize_requested_subagent_model(
            "kimi-k2.5",
            "model",
            crate::config::ApiProvider::Deepseek
        )
        .is_err(),
        "official DeepSeek API rejects foreign ids"
    );
}

// ── #3030: step-counter formatting ──────────────────────────────────────────

#[test]
fn format_step_counter_hides_unbounded_sentinel() {
    // DEFAULT_MAX_STEPS is u32::MAX, meaning "unbounded" — rendering the
    // sentinel as a denominator produced "step 16/4294967295".
    assert_eq!(format_step_counter(16, u32::MAX), "step 16");
}

#[test]
fn format_step_counter_keeps_concrete_budgets() {
    assert_eq!(format_step_counter(3, 25), "step 3/25");
    assert_eq!(format_step_counter(0, 1), "step 0/1");
}

// ── #3095: sub-agent launch gate ─────────────────────────────────────────────

#[test]
fn launch_gate_defaults_to_launch_concurrency_capped_by_max_agents() {
    let tmp = tempdir().expect("tempdir");
    let manager = SubAgentManager::new(tmp.path().to_path_buf(), 10);
    // Unset launch concurrency now seeds the gate to the full agent cap.
    assert_eq!(manager.launch_gate.available_permits(), 10);

    let small = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    assert_eq!(small.launch_gate.available_permits(), 2);

    let custom = SubAgentManager::new(tmp.path().to_path_buf(), 10).with_launch_concurrency(0);
    assert_eq!(custom.launch_gate.available_permits(), 1, "clamps up to 1");

    let oversized = SubAgentManager::new(tmp.path().to_path_buf(), 3).with_launch_concurrency(99);
    assert_eq!(
        oversized.launch_gate.available_permits(),
        3,
        "clamps down to max_agents"
    );
}

#[tokio::test]
async fn launch_gate_queues_extra_direct_children() {
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        4,
    )));

    let (client, _calls, _bodies) = delayed_chat_client(Duration::from_millis(150), "done").await;
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let mut runtime = stub_runtime();
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    runtime.mailbox = Some(mailbox);

    let gate = Arc::new(Semaphore::new(1));
    let held_launch_permit = Arc::clone(&gate)
        .acquire_owned()
        .await
        .expect("test holds the single launch permit");
    let spawn = |agent_id: &str, gate: Option<Arc<Semaphore>>| {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let agent = SubAgent::new(
            agent_id.to_string(),
            SubAgentType::General,
            "Answer".to_string(),
            make_assignment(),
            "deepseek-v4-flash".to_string(),
            None,
            Some(vec![]),
            input_tx,
            tmp.path().to_path_buf(),
            "boot_test".to_string(),
        );
        let task = SubAgentTask {
            manager_handle: Arc::clone(&manager),
            runtime: runtime.clone(),
            agent_id: agent_id.to_string(),
            agent_type: SubAgentType::General,
            prompt: "Answer".to_string(),
            assignment: make_assignment(),
            allowed_tools: Some(vec![]),
            fork_context: false,
            started_at: Instant::now(),
            max_steps: 1,
            token_budget: None,
            input_rx,
            launch_gate: gate,
        };
        (agent, task)
    };

    let (agent_b, task_b) = spawn("agent_gate_b", Some(Arc::clone(&gate)));
    {
        let mut mgr = manager.write().await;
        mgr.agents.insert(agent_b.id.clone(), agent_b);
    }

    // Holding the permit models another direct child occupying the launch
    // gate without relying on wall-clock timing or scheduler fairness.
    tokio::spawn(run_subagent_task(task_b));

    let mut messages = Vec::new();
    let queued = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let Some(envelope) = mailbox_rx.recv().await else {
                break;
            };
            let message = envelope.message;
            let queued_b = matches!(
                &message,
                MailboxMessage::Progress { agent_id, status }
                    if agent_id == "agent_gate_b" && status.contains("queued")
            );
            let started_b = matches!(
                &message,
                MailboxMessage::Started { agent_id, .. } if agent_id == "agent_gate_b"
            );
            messages.push(message);
            assert!(
                !started_b,
                "queued child must not start while the launch permit is held: {messages:?}"
            );
            if queued_b {
                break;
            }
        }
    })
    .await;
    assert!(
        queued.is_ok(),
        "second child must publish a visible queued reason: {messages:?}"
    );
    drop(held_launch_permit);

    let collected = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(envelope) = mailbox_rx.recv().await else {
                break;
            };
            let completed_b = matches!(
                &envelope.message,
                MailboxMessage::Completed { agent_id, .. } if agent_id == "agent_gate_b"
            );
            messages.push(envelope.message);
            if completed_b {
                break;
            }
        }
    })
    .await;
    assert!(collected.is_ok(), "queued child should complete");

    let queued_b = messages.iter().position(|m| {
        matches!(
            m,
            MailboxMessage::Progress { agent_id, status }
                if agent_id == "agent_gate_b" && status.contains("queued")
        )
    });
    assert!(
        queued_b.is_some(),
        "second child must publish a visible queued reason: {messages:?}"
    );
    let queued_b = queued_b.expect("queued progress exists");

    let completed_b = messages
        .iter()
        .position(
            |m| matches!(m, MailboxMessage::Completed { agent_id, .. } if agent_id == "agent_gate_b"),
        )
        .expect("queued child completes");
    let started_b = messages
        .iter()
        .position(
            |m| matches!(m, MailboxMessage::Started { agent_id, .. } if agent_id == "agent_gate_b"),
        )
        .expect("second child eventually starts");
    assert!(
        started_b > queued_b && completed_b > started_b,
        "queued child must start only after queuing, then complete: {messages:?}"
    );
}

/// Stub chat server that always replies with a final assistant text whose
/// `usage` reports the given token counts. Returns the client plus a call
/// counter so tests can assert how many model turns ran before a budget cap
/// fired. Mirrors `delayed_chat_client` but with configurable usage and no
/// artificial latency.
async fn token_heavy_chat_client(
    prompt_tokens: u64,
    completion_tokens: u64,
    response_text: &str,
) -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let response_text = response_text.to_string();
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            let response_text = response_text.clone();
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                let response_text = response_text.clone();
                async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    Json(json!({
                        "id": format!("chatcmpl-budget-{attempt}"),
                        "model": "deepseek-v4-flash",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": response_text
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": prompt_tokens,
                            "completion_tokens": completion_tokens,
                            "total_tokens": prompt_tokens + completion_tokens
                        }
                    }))
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake chat client");
    (client, calls)
}

/// Shared scaffolding for the per-worker token-budget runtime tests: spins up
/// a general worker against `token_heavy_chat_client` with the given cap and
/// returns the manager, agent id, call counter, and spawned task handle.
async fn spawn_budget_capped_worker(
    workspace: &Path,
    prompt_tokens: u64,
    completion_tokens: u64,
    token_budget: Option<u64>,
    max_steps: u32,
) -> (
    Arc<RwLock<SubAgentManager>>,
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        workspace.to_path_buf(),
        2,
    )));
    let agent_id = "agent_budget_worker".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Work within budget".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Budget".to_string()),
        Some(vec![]),
        task_input_tx,
        workspace.to_path_buf(),
        "boot_budget".to_string(),
    );
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, workspace.to_path_buf()));
    }

    let (client, calls) =
        token_heavy_chat_client(prompt_tokens, completion_tokens, "partial answer").await;
    let mut runtime = stub_runtime();
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(workspace.to_path_buf());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime: runtime.clone(),
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Work within budget".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps,
        token_budget,
        input_rx: task_input_rx,
        launch_gate: None,
    };
    let task_handle = tokio::spawn(run_subagent_task(task));
    (manager, agent_id, calls, task_handle)
}

#[tokio::test]
async fn worker_stops_when_per_worker_token_budget_exceeded() {
    let tmp = tempdir().expect("tempdir");
    // 100 tokens/turn (60 in + 40 out) vs a 50-token cap: the worker must
    // stop with `BudgetExhausted` after its very first model turn instead of
    // running on to `max_steps`.
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("budget-capped worker must terminate")
        .expect("task should finish");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "worker must stop after the first over-budget turn, not run to max_steps"
    );

    let result = {
        let manager = manager.read().await;
        manager.get_result(&agent_id).expect("agent registered")
    };
    assert!(
        matches!(result.status, SubAgentStatus::BudgetExhausted),
        "expected BudgetExhausted, got {:?}",
        result.status
    );
}

#[tokio::test]
async fn worker_without_per_worker_token_budget_runs_to_completion() {
    let tmp = tempdir().expect("tempdir");
    // No per-worker cap: a final-text response completes the worker normally
    // even though each turn reports 100 tokens.
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, None, 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("uncapped worker must terminate")
        .expect("task should finish");

    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let result = {
        let manager = manager.read().await;
        manager.get_result(&agent_id).expect("agent registered")
    };
    assert!(
        matches!(result.status, SubAgentStatus::Completed),
        "uncapped worker should complete normally, got {:?}",
        result.status
    );
}

#[tokio::test]
async fn per_worker_token_budget_does_not_double_count_scope_accounting() {
    let tmp = tempdir().expect("tempdir");
    // The per-worker runtime cap stops the worker, but the scope-level
    // accounting (#3319 `aggregate_budget_spent` sums worker_records'
    // `total_tokens`) must reflect the tokens actually consumed exactly once
    // — never inflated by the runtime accumulator that triggered the stop.
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("budget-capped worker must terminate")
        .expect("task should finish");

    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (result, worker_record) = {
        let manager = manager.read().await;
        (
            manager.get_result(&agent_id).expect("agent registered"),
            manager.get_worker_record(&agent_id).expect("worker record"),
        )
    };
    assert!(
        matches!(result.status, SubAgentStatus::BudgetExhausted),
        "expected BudgetExhausted, got {:?}",
        result.status
    );
    // One turn of 60 in + 40 out = 100 tokens, counted exactly once.
    assert_eq!(
        worker_record.usage.total_tokens,
        Some(100),
        "scope accounting must equal the single turn's tokens, not double-count: {:?}",
        worker_record.usage
    );
}

#[test]
fn cleanup_due_gates_write_locked_cleanup_to_a_bounded_cadence() {
    // #3803: a fresh manager is always due (never cleaned); right after a
    // cleanup it is not due again until the interval elapses, so the sidebar
    // refresh (Op::ListSubAgents) renders from the read-only snapshot in
    // between instead of taking the write lock on every request.
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);

    assert!(
        manager.cleanup_due(Duration::from_secs(2)),
        "a never-cleaned manager should be due"
    );

    manager.cleanup(Duration::from_secs(3600));
    assert!(
        !manager.cleanup_due(Duration::from_secs(3600)),
        "immediately after cleanup it should not be due again within the interval"
    );
    assert!(
        manager.cleanup_due(Duration::from_secs(0)),
        "a zero interval is always due"
    );
}

// ── #3882: bounded sub-agent output under Fleet fanout ─────────────────────

/// Serialize-and-restore guard for the shared spillover test root, mirroring
/// the pattern in `tools::truncate::tests`.
fn with_spillover_root<F: FnOnce()>(root: &std::path::Path, f: F) {
    let _guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let prior = crate::tools::truncate::set_test_spillover_root(Some(root.to_path_buf()));
    struct Restore(Option<std::path::PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.0.take());
        }
    }
    let _restore = Restore(prior);
    f();
}

#[test]
fn bounded_tail_messages_keeps_recent_within_budget_and_counts_omitted() {
    let messages: Vec<Message> = (0..10)
        .map(|i| text_message("user", &format!("{i}:{}", "x".repeat(10_000))))
        .collect();

    let (kept, omitted) = bounded_tail_messages(&messages, 35_000);

    assert!(!kept.is_empty());
    assert_eq!(kept.len() + omitted, messages.len());
    assert!(omitted > 0, "a 100 KB history must not fit a 35 KB budget");
    // The tail is the most recent slice, in order.
    let last_kept = message_text(kept.last().expect("tail non-empty"));
    assert!(
        last_kept.starts_with("9:"),
        "kept tail must end at the newest message"
    );
    let total: usize = kept.iter().map(approximate_message_bytes).sum();
    assert!(
        total <= 35_000 + 11_000,
        "kept tail exceeds budget by more than one message: {total}"
    );
}

#[test]
fn bounded_tail_messages_always_keeps_the_final_message() {
    let messages = vec![
        text_message("user", &"a".repeat(50_000)),
        text_message("assistant", &"b".repeat(50_000)),
    ];

    let (kept, omitted) = bounded_tail_messages(&messages, 10);

    assert_eq!(
        kept.len(),
        1,
        "the newest message survives even over budget"
    );
    assert_eq!(omitted, 1);
    assert!(message_text(&kept[0]).starts_with('b'));
}

#[test]
fn checkpoints_are_byte_bounded_under_fanout_scale_output() {
    // Simulates the #3882 report shape: a worker whose tool results are
    // multi-MB build logs. Without bounding, every per-step checkpoint clone
    // carried the whole history; the persisted fleet file and every snapshot
    // multiplied it further.
    let huge = "error: expected `;`\n".repeat(120_000); // ~2.3 MB per message
    let messages: Vec<Message> = (0..6).map(|_| text_message("user", &huge)).collect();

    let checkpoint = make_checkpoint("fleet-worker-1", 6, messages.clone());

    assert_eq!(checkpoint.message_count, messages.len());
    assert!(checkpoint.omitted_messages > 0);
    assert!(
        !checkpoint.messages.is_empty(),
        "checkpoint must stay continuable"
    );
    let serialized = serde_json::to_string(&checkpoint).expect("serialize checkpoint");
    assert!(
        serialized.len() <= SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES + huge.len() + 64 * 1024,
        "checkpoint JSON must be bounded, got {} bytes",
        serialized.len()
    );
    // The raw history is ~14 MB; the checkpoint must not carry it.
    assert!(
        serialized.len() < 4 * 1024 * 1024,
        "checkpoint JSON should be far below the raw transcript size, got {} bytes",
        serialized.len()
    );
}

#[test]
fn checkpoint_without_omitted_field_still_deserializes() {
    // Records persisted before v0.8.67 carry no omitted_messages key.
    let legacy = r#"{
        "checkpoint_id": "a:step:1:ts:1",
        "agent_id": "a",
        "continuation_handle": "agent:a:checkpoint:a:step:1:ts:1",
        "reason": "interrupted",
        "continuable": true,
        "steps_taken": 1,
        "message_count": 1,
        "created_at_ms": 1
    }"#;
    let checkpoint: SubAgentCheckpoint =
        serde_json::from_str(legacy).expect("legacy checkpoint should load");
    assert_eq!(checkpoint.omitted_messages, 0);
}

#[test]
fn subagent_tool_results_spill_to_disk_and_stay_bounded_inline() {
    let tmp = tempdir().expect("tempdir");
    with_spillover_root(tmp.path(), || {
        let raw = "cargo build noise line\n".repeat(220_000); // ~5 MB
        let raw_len = raw.len();

        let (inline, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-42", raw.clone());

        let path = spilled.expect("multi-MB output must spill");
        // Model-visible content is bounded to head + footer.
        assert!(inline.len() <= crate::tools::truncate::SPILLOVER_HEAD_BYTES + 1024);
        assert!(inline.contains("Sub-agent tool output truncated"));
        assert!(inline.contains(&path.display().to_string()));
        assert!(inline.contains("read_file"));
        // Full output remains recoverable from disk.
        let on_disk = std::fs::read_to_string(&path).expect("spill file readable");
        assert_eq!(on_disk.len(), raw_len);

        // Small outputs pass through untouched, no spill file.
        let (small, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-43", "ok".to_string());
        assert_eq!(small, "ok");
        assert!(spilled.is_none());

        // Oversized error output is bounded too: sub-agent errors are
        // routinely full build logs, unlike the root loop's short errors.
        let (bounded_err, spilled) =
            bound_subagent_tool_result("fleet-worker-1", "call-44", format!("Error: {raw}"));
        assert!(spilled.is_some());
        assert!(bounded_err.len() <= crate::tools::truncate::SPILLOVER_HEAD_BYTES + 1024);
        assert!(bounded_err.starts_with("Error:"));
    });
}

#[test]
fn fanout_of_workers_with_huge_outputs_keeps_resident_state_bounded() {
    // Acceptance shape for #3882: multiple workers, each emitting multi-MB
    // tool output. Model-visible content and per-worker checkpoints stay
    // bounded while every full output is recoverable from disk.
    let tmp = tempdir().expect("tempdir");
    with_spillover_root(tmp.path(), || {
        let huge = "warning: unused import `std::mem`\n".repeat(70_000); // ~2.4 MB
        let mut resident_bytes = 0usize;

        for worker in 0..4 {
            let agent_id = format!("fleet-worker-{worker}");
            let mut messages = Vec::new();
            for call in 0..3 {
                let (inline, spilled) =
                    bound_subagent_tool_result(&agent_id, &format!("call-{call}"), huge.clone());
                let path = spilled.expect("should spill");
                assert_eq!(
                    std::fs::read_to_string(&path).expect("readable").len(),
                    huge.len()
                );
                resident_bytes += inline.len();
                messages.push(text_message("user", &inline));
            }
            let checkpoint = make_checkpoint(&agent_id, 3, messages);
            let serialized = serde_json::to_string(&checkpoint).expect("serialize");
            assert!(
                serialized.len() <= SUBAGENT_CHECKPOINT_MESSAGE_BUDGET_BYTES + 128 * 1024,
                "worker {worker} checkpoint too large: {} bytes",
                serialized.len()
            );
            resident_bytes += serialized.len();
        }

        // 4 workers × 3 calls × ~2.4 MB ≈ 29 MB raw. Bounded resident state
        // must stay under 2 MB total.
        assert!(
            resident_bytes < 2 * 1024 * 1024,
            "resident bytes not bounded: {resident_bytes}"
        );
    });
}

#[test]
fn write_json_atomic_survives_concurrent_writers() {
    use std::sync::Arc;
    // Many threads persisting the same state.json concurrently (the real
    // persist_state_best_effort pattern) must never publish a torn file.
    let dir = tempdir().expect("tempdir");
    // Canonicalize so the base matches how write_json_atomic normalizes the
    // workspace (on macOS the tempdir lives under the /var -> /private/var
    // symlink); otherwise the workspace-relative path check would reject it.
    let base = dir.path().canonicalize().expect("canonicalize tempdir");
    let workspace = Arc::new(base.clone());
    let path = Arc::new(base.join(".codewhale").join("subagents").join("state.json"));
    let mut handles = Vec::new();
    for i in 0..16 {
        let ws = Arc::clone(&workspace);
        let p = Arc::clone(&path);
        handles.push(std::thread::spawn(move || {
            let payload = serde_json::json!({ "writer": i, "blob": "x".repeat(8192) });
            let _ = write_json_atomic(&ws, &p, &payload);
        }));
    }
    for h in handles {
        h.join().expect("writer thread");
    }
    // The published file must be complete, valid JSON — not a half-written mix.
    let contents = std::fs::read_to_string(&*path).expect("read state.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&contents).expect("state.json must be complete/valid JSON");
    assert!(parsed.get("writer").is_some());
    // No stray temp files left behind.
    let leftover: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .expect("read subagents dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftover.is_empty(), "temp files leaked: {leftover:?}");
}
