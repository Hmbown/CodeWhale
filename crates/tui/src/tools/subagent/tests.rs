use super::*;
use tempfile::tempdir;

fn make_assignment() -> SubAgentAssignment {
    SubAgentAssignment::new("prompt".to_string(), Some("worker".to_string()))
}

fn make_snapshot(status: SubAgentStatus) -> SubAgentResult {
    SubAgentResult {
        agent_id: "agent_test".to_string(),
        agent_type: SubAgentType::General,
        assignment: make_assignment(),
        model: "deepseek-v4-flash".to_string(),
        nickname: None,
        status,
        result: None,
        steps_taken: 0,
        duration_ms: 0,
        from_prior_session: false,
    }
}

fn message_text(message: &Message) -> &str {
    match message.content.first() {
        Some(ContentBlock::Text { text, .. }) => text.as_str(),
        other => panic!("expected text content block, got {other:?}"),
    }
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
        structured_state_block: Some(
            "## Cycle State (Auto-Preserved)\n- Mode: `AGENT`".to_string(),
        ),
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
    assert!(message_text(&messages[1]).contains("<deepseek:fork_state>"));
    assert_eq!(messages[2].role, "system");
    assert!(message_text(&messages[2]).contains("<deepseek:subagent_context>"));
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
fn test_parse_assign_request_accepts_aliases() {
    let input = json!({
        "id": "agent_1234",
        "objective": "re-check failing tests",
        "agent_role": "explorer",
        "input": "focus on tests only",
        "interrupt": false
    });
    let request = parse_assign_request(&input).expect("assign request should parse");
    assert_eq!(request.agent_id, "agent_1234");
    assert_eq!(request.objective.as_deref(), Some("re-check failing tests"));
    assert_eq!(request.role.as_deref(), Some("explorer"));
    assert_eq!(request.message.as_deref(), Some("focus on tests only"));
    assert!(!request.interrupt);
}

#[test]
fn test_parse_assign_request_rejects_invalid_role() {
    let input = json!({
        "agent_id": "agent_1234",
        "role": "unknown"
    });
    let err = parse_assign_request(&input).expect_err("invalid role should fail");
    assert!(err.to_string().contains("Invalid role alias"));
}

#[test]
fn test_parse_assign_request_requires_update_fields() {
    let input = json!({
        "agent_id": "agent_1234"
    });
    let err = parse_assign_request(&input).expect_err("missing update fields should fail");
    assert!(
        err.to_string().contains(
            "Provide at least one of objective, role/agent_role, message/input, or items"
        )
    );
}

#[test]
fn test_send_input_schema_does_not_require_message_field() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    let schema = AgentSendInputTool::new(manager, "send_input").input_schema();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !required
            .iter()
            .any(|entry| entry.as_str().is_some_and(|name| name == "message")),
        "send_input schema should allow items-only payloads"
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
fn test_wait_mode_condition_any_and_all() {
    let one_done = vec![
        make_snapshot(SubAgentStatus::Running),
        make_snapshot(SubAgentStatus::Completed),
    ];
    let all_done = vec![
        make_snapshot(SubAgentStatus::Completed),
        make_snapshot(SubAgentStatus::Cancelled),
    ];

    assert!(WaitMode::Any.condition_met(&one_done));
    assert!(!WaitMode::All.condition_met(&one_done));
    assert!(WaitMode::All.condition_met(&all_done));
}

#[test]
fn test_parse_wait_mode() {
    assert_eq!(parse_wait_mode(&json!({})).unwrap(), WaitMode::Any);
    assert_eq!(
        parse_wait_mode(&json!({"wait_mode": "all"})).unwrap(),
        WaitMode::All
    );
    assert_eq!(
        parse_wait_mode(&json!({"wait_mode": "first"})).unwrap(),
        WaitMode::Any
    );
    assert!(parse_wait_mode(&json!({"wait_mode": "invalid"})).is_err());
}

#[test]
fn test_parse_wait_ids_accepts_aliases() {
    let ids = parse_wait_ids(&json!({
        "ids": ["agent_a", "agent_b"],
        "agent_id": "agent_c",
        "id": "agent_a"
    }));

    assert_eq!(ids, vec!["agent_a", "agent_b", "agent_c"]);
}

#[test]
fn test_parse_wait_ids_empty_when_omitted() {
    let ids = parse_wait_ids(&json!({}));
    assert!(ids.is_empty());
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
fn subagent_auto_model_routes_unconfigured_assignments() {
    let runtime = stub_runtime().with_auto_model(true);

    assert_eq!(
        fallback_subagent_assignment_route(&runtime, None, "implement the release fix").model,
        "deepseek-v4-pro"
    );
    assert_eq!(
        fallback_subagent_assignment_route(&runtime, None, "say hello").model,
        "deepseek-v4-flash"
    );
}

#[test]
fn subagent_auto_route_respects_explicit_or_role_model() {
    let runtime = stub_runtime().with_auto_model(true);

    assert_eq!(
        fallback_subagent_assignment_route(
            &runtime,
            Some("deepseek-v4-flash".to_string()),
            "implement the release fix"
        )
        .model,
        "deepseek-v4-flash"
    );
}

#[test]
fn subagent_auto_reasoning_resolves_to_distinct_v4_tiers() {
    let runtime = stub_runtime().with_reasoning_effort(Some("high".to_string()), true);

    assert_eq!(
        fallback_subagent_assignment_route(&runtime, None, "quick lookup").reasoning_effort,
        Some("high".to_string())
    );
    assert_eq!(
        fallback_subagent_assignment_route(&runtime, None, "debug this release failure")
            .reasoning_effort,
        Some("max".to_string())
    );
}

#[test]
fn fixed_model_subagent_auto_reasoning_skips_flash_router() {
    let runtime = stub_runtime().with_reasoning_effort(Some("high".to_string()), true);

    assert!(
        !should_use_subagent_flash_router(&runtime),
        "fixed-model auto thinking should resolve locally without a hidden router request"
    );
}

#[test]
fn auto_model_subagent_assignments_still_use_flash_router() {
    let runtime = stub_runtime().with_auto_model(true);

    assert!(
        should_use_subagent_flash_router(&runtime),
        "auto-model sub-agent assignments still need router guidance"
    );
}

#[test]
fn subagent_router_prompt_frames_assignment_as_auto_routing() {
    let runtime = stub_runtime()
        .with_auto_model(true)
        .with_reasoning_effort(Some("high".to_string()), true);
    let prompt = subagent_router_prompt(&runtime, "inspect one file");

    assert!(prompt.contains("Parent selected model mode: auto"));
    assert!(prompt.contains("Parent selected thinking mode: auto"));
    assert!(prompt.contains("inspect one file"));
}

#[test]
fn test_subagent_tool_registry_reports_unavailable_tools() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        Some(vec!["read_file".to_string(), "missing_tool".to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    assert_eq!(
        registry.unavailable_allowed_tools(),
        vec!["missing_tool".to_string()]
    );
}

#[tokio::test]
async fn test_wait_for_result_reports_timeout_when_still_running() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 2)));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    let agent_id = agent.id.clone();
    {
        let mut guard = manager.write().await;
        guard.agents.insert(agent_id.clone(), agent);
    }

    let (snapshot, timed_out) = wait_for_result(&manager, &agent_id, Duration::from_millis(10))
        .await
        .expect("wait_for_result should succeed");
    assert!(timed_out);
    assert_eq!(snapshot.status, SubAgentStatus::Running);
}

#[tokio::test]
async fn test_running_count_counts_only_agents_with_live_task_handles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
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
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 0);
}

#[tokio::test]
async fn test_running_count_ignores_finished_task_handles() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;
    let handle = tokio::spawn(async {});
    handle.await.expect("dummy task should finish immediately");
    agent.task_handle = Some(tokio::spawn(async {}));
    if let Some(handle) = agent.task_handle.as_ref() {
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    }
    manager.agents.insert(agent.id.clone(), agent);

    assert_eq!(manager.running_count(), 0);
}

#[test]
fn test_assign_updates_running_agent_and_sends_message() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 2);
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);

    let snapshot = manager
        .assign(
            &agent_id,
            Some("Re-check module boundaries".to_string()),
            Some("explorer".to_string()),
            None,
            true,
        )
        .expect("assignment should succeed");
    assert_eq!(snapshot.assignment.objective, "Re-check module boundaries");
    assert_eq!(snapshot.assignment.role.as_deref(), Some("explorer"));

    let dispatched = input_rx
        .try_recv()
        .expect("running agent should receive assignment update");
    assert!(dispatched.interrupt);
    assert!(dispatched.text.contains("Assignment updated"));
    assert!(dispatched.text.contains("objective"));
}

#[test]
fn test_assign_rejects_message_for_non_running_agent() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Completed;
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);

    let err = manager
        .assign(&agent_id, None, None, Some("keep going".to_string()), true)
        .expect_err("non-running agent cannot receive assignment message");
    assert!(err.to_string().contains("is not running"));
}

#[test]
fn test_assign_updates_non_running_metadata_without_message() {
    let mut manager = SubAgentManager::new(PathBuf::from("."), 1);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        SubAgentType::Plan,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Completed;
    let agent_id = agent.id.clone();
    manager.agents.insert(agent_id.clone(), agent);

    let snapshot = manager
        .assign(
            &agent_id,
            Some("Draft retry plan".to_string()),
            Some("awaiter".to_string()),
            None,
            true,
        )
        .expect("metadata update should succeed");
    assert_eq!(snapshot.assignment.objective, "Draft retry plan");
    assert_eq!(snapshot.assignment.role.as_deref(), Some("awaiter"));
}

#[test]
fn test_persist_and_reload_marks_running_agent_as_interrupted() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path());

    let mut manager = SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path);
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let running = SubAgent::new(
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some("Blue".to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        "boot_test".to_string(),
    );
    let running_id = running.id.clone();
    manager.agents.insert(running_id.clone(), running);
    manager.persist_state().expect("persist state");

    let mut reloaded =
        SubAgentManager::new(workspace, 2).with_state_path(default_state_path(tmp.path()));
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
fn test_interrupted_status_name_and_summary() {
    let snapshot = make_snapshot(SubAgentStatus::Interrupted(
        SUBAGENT_RESTART_REASON.to_string(),
    ));
    assert_eq!(subagent_status_name(&snapshot.status), "interrupted");
    assert!(summarize_subagent_result(&snapshot).contains(SUBAGENT_RESTART_REASON));
}

// === Deprecation notice tests ===

/// Helper: build a plain ToolResult with a JSON payload.
fn make_plain_result(payload: serde_json::Value) -> crate::tools::spec::ToolResult {
    crate::tools::spec::ToolResult::json(&payload).expect("json result")
}

#[test]
fn test_wrap_with_deprecation_notice_adds_deprecation_block() {
    let result = make_plain_result(json!({"agent_id": "abc"}));
    let wrapped = wrap_with_deprecation_notice(result, "spawn_agent", "agent_spawn");

    let meta = wrapped.metadata.expect("metadata should be present");
    let dep = &meta["_deprecation"];
    assert_eq!(dep["this_tool"], "spawn_agent");
    assert_eq!(dep["use_instead"], "agent_spawn");
    assert_eq!(dep["removed_in"], DEPRECATION_REMOVAL_VERSION);
    assert!(
        dep["message"]
            .as_str()
            .unwrap_or("")
            .contains("spawn_agent")
    );
}

#[test]
fn test_wrap_with_deprecation_notice_preserves_existing_metadata() {
    let result = make_plain_result(json!({"agent_id": "abc"}))
        .with_metadata(json!({"status": "Running", "snapshot": {}}));
    let wrapped = wrap_with_deprecation_notice(result, "close_agent", "agent_cancel");

    let meta = wrapped.metadata.expect("metadata should be present");
    // Existing metadata key must survive.
    assert_eq!(meta["status"], "Running");
    // Deprecation block must be present alongside.
    assert_eq!(meta["_deprecation"]["this_tool"], "close_agent");
    assert_eq!(meta["_deprecation"]["use_instead"], "agent_cancel");
}

#[test]
fn test_canonical_agent_send_input_has_no_deprecation() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    // The canonical name "agent_send_input" must NOT receive a deprecation notice.
    // We verify this by inspecting the tool's name — the deprecation branch
    // only fires when name == "send_input".
    let tool = AgentSendInputTool::new(manager.clone(), "agent_send_input");
    assert_eq!(tool.name(), "agent_send_input");

    let alias = AgentSendInputTool::new(manager, "send_input");
    assert_eq!(alias.name(), "send_input");
}

#[test]
fn test_wrap_with_deprecation_notice_all_alias_mappings() {
    let cases = [
        ("spawn_agent", "agent_spawn"),
        ("delegate_to_agent", "agent_spawn"),
        ("close_agent", "agent_cancel"),
        ("send_input", "agent_send_input"),
    ];

    for (alias, canonical) in cases {
        let result = make_plain_result(json!({"ok": true}));
        let wrapped = wrap_with_deprecation_notice(result, alias, canonical);
        let meta = wrapped.metadata.expect("metadata for alias {alias}");
        assert_eq!(meta["_deprecation"]["this_tool"], alias, "alias={alias}");
        assert_eq!(
            meta["_deprecation"]["use_instead"], canonical,
            "alias={alias}"
        );
        assert_eq!(
            meta["_deprecation"]["removed_in"], DEPRECATION_REMOVAL_VERSION,
            "alias={alias}"
        );
    }
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
fn build_subagent_system_prompt_appends_role_when_set() {
    let assignment = SubAgentAssignment::new("p".to_string(), Some("worker".to_string()));
    let prompt = build_subagent_system_prompt(&SubAgentType::General, &assignment);
    assert!(
        prompt.ends_with("You are operating in the role of `worker`."),
        "expected role line at end, got: {}",
        &prompt[prompt.len().saturating_sub(80)..]
    );
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
    let sentinel = subagent_done_sentinel("agent_xyz", &res);
    assert!(sentinel.starts_with("<deepseek:subagent.done>"));
    assert!(sentinel.ends_with("</deepseek:subagent.done>"));

    // The inner JSON parses and carries the expected fields.
    let inner = sentinel
        .trim_start_matches("<deepseek:subagent.done>")
        .trim_end_matches("</deepseek:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_xyz");
    assert_eq!(parsed["status"], "completed");
    assert_eq!(parsed["agent_type"], "general");
}

#[test]
fn subagent_failed_sentinel_format_is_well_formed() {
    let sentinel = subagent_failed_sentinel("agent_zzz", "boom");
    let inner = sentinel
        .trim_start_matches("<deepseek:subagent.done>")
        .trim_end_matches("</deepseek:subagent.done>");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("inner JSON parses");
    assert_eq!(parsed["agent_id"], "agent_zzz");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["error"], "boom");
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
fn child_runtime_increments_depth_and_forces_auto_approve() {
    let mut parent = stub_runtime();
    parent.spawn_depth = 1;
    parent.context.auto_approve = false; // parent in suggest mode
    let child = parent.child_runtime();
    assert_eq!(child.spawn_depth, 2, "child depth = parent + 1");
    assert!(
        child.context.auto_approve,
        "child must auto-approve regardless of parent mode (spawning IS the approval)"
    );
    // Parent mode is unchanged — the override is on the child only.
    assert!(!parent.context.auto_approve);
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

    let workspace = std::env::temp_dir().join("deepseek-test-stub");
    let context = ToolContext::new(workspace.clone());
    SubAgentRuntime {
        client: stub_client(),
        model: "deepseek-v4-flash".to_string(),
        auto_model: false,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        role_models: std::collections::HashMap::new(),
        context,
        allow_shell: true,
        event_tx: None,
        manager: new_shared_subagent_manager(workspace, 5),
        spawn_depth: 0,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
        cancel_token: CancellationToken::new(),
        mailbox: None,
        parent_completion_tx: None,
        fork_context: None,
    }
}

/// A minimal stub client. Test helpers below only ever check struct fields
/// (depth, cancel_token, context); they don't call the network. We need a
/// *some* `DeepSeekClient` because `SubAgentRuntime.client` isn't
/// `Option<...>`. `Config::default()` is enough — `DeepSeekClient::new`
/// only validates that an API key field exists, not that the key works.
fn stub_client() -> DeepSeekClient {
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
// `agent_list` defaults to current-session only; `include_archived=true`
// surfaces the prior-session records with the flag set.

fn insert_prior_session_agent(
    manager: &mut SubAgentManager,
    id: &str,
    status: SubAgentStatus,
    boot_id: &str,
) {
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        SubAgentType::General,
        "old prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
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
            .expect("persist round-trip should write");
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
// When a direct child of the engine finishes, `run_subagent_task` emits
// a `SubAgentCompletion` on the runtime's `parent_completion_tx`. The
// engine's turn loop drains that channel before deciding to end the turn.
// These tests cover the gating logic in `emit_parent_completion` so the
// parent isn't flooded with grandchild completions and so the function
// is safe when no channel is wired.

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
fn emit_parent_completion_skips_grandchildren() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(2, Some(tx));

    let sent = emit_parent_completion(&runtime, "agent_grandchild", "ignored");

    assert!(
        !sent,
        "depth=2 grandchild must not fire on the parent channel"
    );
    assert!(
        rx.try_recv().is_err(),
        "channel should remain empty for grandchildren"
    );
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
fn child_runtime_propagates_completion_tx_for_gating() {
    // The channel is cloned through `child_runtime()` so descendants carry
    // it. The gate at the send site (`spawn_depth == 1`) is what limits
    // who actually fires — `child_runtime` simply must not strand it.
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
fn subagent_completion_payload_carries_existing_sentinel_format() {
    // The payload format is the same one already documented in
    // prompts/base.md: human summary on line 1, `<deepseek:subagent.done>`
    // sentinel on line 2. This test pins the format so future refactors
    // don't silently break the model's parsing contract.
    let mut snap = make_snapshot(SubAgentStatus::Completed);
    snap.result = Some("Found three errors.".to_string());

    let summary = summarize_subagent_result(&snap);
    let sentinel = subagent_done_sentinel("agent_test", &snap);
    let payload = format!("{summary}\n{sentinel}");

    let mut lines = payload.lines();
    let first = lines.next().expect("first line is summary");
    let second = lines.next().expect("second line is sentinel");
    assert!(
        !first.starts_with("<deepseek:subagent.done>"),
        "summary should not be the sentinel itself"
    );
    assert!(
        second.starts_with("<deepseek:subagent.done>"),
        "second line is the sentinel"
    );
    assert!(second.ends_with("</deepseek:subagent.done>"));
    assert!(
        second.contains("\"agent_id\":\"agent_test\""),
        "sentinel JSON includes agent_id"
    );
}
