//! Hermetic #5305 route-receipt regressions kept out of the oversized parent
//! sub-agent test module.

use super::*;

fn personal_consultant_runtime(
    workspace: &std::path::Path,
    manager: SharedSubAgentManager,
) -> SubAgentRuntime {
    let providers = crate::config::ProvidersConfig {
        deepseek: crate::config::ProviderConfig {
            api_key: Some("deepseek-test-key".to_string()),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            ..Default::default()
        },
        openai_codex: crate::config::ProviderConfig {
            api_key: Some("codex-test-key".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = crate::config::Config {
        api_key: Some("deepseek-test-key".to_string()),
        provider: Some("deepseek".to_string()),
        providers: Some(providers),
        ..Default::default()
    };
    let client = DeepSeekClient::new(&config).expect("DeepSeek parent client");
    SubAgentRuntime::new(
        client,
        "deepseek-v4-flash".to_string(),
        ToolContext::new(workspace.to_path_buf()),
        false,
        None,
        manager,
    )
    .with_api_config(config)
}

fn write_personal_consultant(home: &std::path::Path) {
    let agents = home.join("agents");
    std::fs::create_dir_all(&agents).expect("personal agents directory");
    std::fs::write(
        agents.join("consultant.toml"),
        concat!(
            "id = \"consultant\"\n",
            "role_hint = \"consultant\"\n",
            "provider = \"openai-codex\"\n",
            "model = \"gpt-5.6-sol\"\n",
            "reasoning_effort = \"high\"\n",
        ),
    )
    .expect("personal consultant profile");
}

async fn start_personal_consultant(
    workspace: &std::path::Path,
) -> (
    SharedSubAgentManager,
    ToolContext,
    crate::tools::spec::ToolResult,
) {
    let manager = new_shared_subagent_manager(workspace.to_path_buf(), 4);
    let context = ToolContext::new(workspace.to_path_buf());
    let tool = AgentTool::new(
        manager.clone(),
        personal_consultant_runtime(workspace, manager.clone()),
    );
    let result = tool
        .execute(
            json!({
                "action": "start",
                "type": "consultant",
                "prompt": "inspect the request without writing files",
            }),
            &context,
        )
        .await
        .expect("profile-pinned consultant starts");
    (manager, context, result)
}

fn receipt_from(result: &crate::tools::spec::ToolResult) -> serde_json::Value {
    let content: serde_json::Value =
        serde_json::from_str(&result.content).expect("start result is JSON");
    let metadata = result.metadata.as_ref().expect("ToolResult metadata");
    assert_eq!(content["child_route"], metadata["child_route"]);
    content["child_route"].clone()
}

async fn cancel_started(manager: &SharedSubAgentManager, result: &crate::tools::spec::ToolResult) {
    let agent_id = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("start agent id")
        .to_string();
    manager
        .write()
        .await
        .cancel_agent(&agent_id)
        .expect("cancel test child");
}

#[tokio::test]
async fn issue_5305_first_personal_profile_receipt_precedes_status_poll() {
    let _env_lock = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("home tempdir");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
    let _codex_token =
        crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    write_personal_consultant(home.path());
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    let (manager, _context, start) = start_personal_consultant(workspace.path()).await;
    assert!(start.content.len() < 1024, "receipt must remain compact");
    let receipt = receipt_from(&start);
    assert_eq!(receipt["requested_type"], json!("consultant"));
    assert_eq!(receipt["requested_profile"], serde_json::Value::Null);
    assert_eq!(receipt["resolved_profile_id"], json!("consultant"));
    assert_eq!(receipt["profile_origin"], json!("personal"));
    assert_eq!(receipt["canonical_role"], json!("consultant"));
    assert_eq!(receipt["provider_id"], json!("openai-codex"));
    assert_eq!(receipt["model_id"], json!("gpt-5.6-sol"));
    assert_eq!(receipt["route_source"], json!("agent_profile.model"));
    assert_eq!(receipt["requested_reasoning"], json!("inherit"));
    assert_eq!(receipt["effective_reasoning"], json!("high"));
    assert!(receipt["runtime_version"].as_str().is_some());
    assert!(receipt["runtime_build_sha"].as_str().is_some());
    cancel_started(&manager, &start).await;
}

#[tokio::test]
async fn issue_5305_receipt_survives_status_peek_and_immutable_config_changes() {
    let _env_lock = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("home tempdir");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
    let _codex_token =
        crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    write_personal_consultant(home.path());
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let (manager, context, start) = start_personal_consultant(workspace.path()).await;
    let receipt = receipt_from(&start);
    let agent_id = start.metadata.as_ref().unwrap()["agent_id"]
        .as_str()
        .expect("agent id")
        .to_string();

    std::fs::write(
        home.path().join("agents/consultant.toml"),
        "id = \"consultant\"\nrole_hint = \"consultant\"\nprovider = \"deepseek\"\nmodel = \"changed-model\"\n",
    )
    .expect("mutate source profile after admission");
    let inspect = AgentTool::new(
        manager.clone(),
        personal_consultant_runtime(workspace.path(), manager.clone()),
    );
    let status = inspect
        .execute(json!({"action": "status", "agent_id": agent_id}), &context)
        .await
        .expect("status");
    let status_json: serde_json::Value =
        serde_json::from_str(&status.content).expect("status json");
    assert_eq!(status_json["child_route"], receipt);
    assert_eq!(status.metadata.as_ref().unwrap()["child_route"], receipt);

    let peek = inspect
        .execute(json!({"action": "peek", "agent_id": agent_id}), &context)
        .await
        .expect("peek");
    let peek_json: serde_json::Value = serde_json::from_str(&peek.content).expect("peek json");
    assert_eq!(peek_json["child_route"], receipt);
    cancel_started(&manager, &start).await;
}

#[tokio::test]
async fn issue_5305_explicit_profile_matches_type_resolution_and_conflicts_refuse() {
    let _env_lock = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("home tempdir");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
    let _codex_token =
        crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    write_personal_consultant(home.path());
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let (manager, context, type_start) = start_personal_consultant(workspace.path()).await;
    let type_receipt = receipt_from(&type_start);
    cancel_started(&manager, &type_start).await;

    let explicit_tool = AgentTool::new(
        manager.clone(),
        personal_consultant_runtime(workspace.path(), manager.clone()),
    );
    let explicit = explicit_tool
        .execute(
            json!({"action":"start", "profile":"consultant", "prompt":"same route"}),
            &context,
        )
        .await
        .expect("explicit profile starts");
    let explicit_receipt = receipt_from(&explicit);
    for field in [
        "resolved_profile_id",
        "profile_origin",
        "canonical_role",
        "provider_id",
        "model_id",
        "route_source",
        "requested_reasoning",
        "effective_reasoning",
    ] {
        assert_eq!(explicit_receipt[field], type_receipt[field], "{field}");
    }
    assert_eq!(explicit_receipt["requested_profile"], json!("consultant"));
    cancel_started(&manager, &explicit).await;

    let conflict = explicit_tool
        .execute(
            json!({"action":"start", "type":"scout", "profile":"consultant", "prompt":"must refuse"}),
            &context,
        )
        .await
        .expect_err("conflicting type/profile is refused");
    assert!(conflict.to_string().contains("conflicting explicit type"));
}

#[tokio::test]
async fn issue_5305_unbuildable_route_refuses_before_worktree_admission() {
    let _env_lock = crate::test_support::lock_test_env();
    let home = tempfile::tempdir().expect("home tempdir");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
    let agents = home.path().join("agents");
    std::fs::create_dir_all(&agents).expect("agents");
    std::fs::write(
        agents.join("consultant.toml"),
        "id = \"consultant\"\nrole_hint = \"consultant\"\nprovider = \"deepseek\"\nmodel = \"not-a-deepseek-model\"\n",
    )
    .expect("bad profile");
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let manager = new_shared_subagent_manager(workspace.path().to_path_buf(), 1);
    let context = ToolContext::new(workspace.path().to_path_buf());
    let tool = AgentTool::new(
        manager.clone(),
        personal_consultant_runtime(workspace.path(), manager.clone()),
    );
    let worktree = workspace.path().join("must-not-exist");
    let err = tool
        .execute(
            json!({
                "action":"start", "type":"consultant", "prompt":"refuse before admission",
                "worktree": true, "cwd": worktree,
            }),
            &context,
        )
        .await
        .expect_err("unbuildable model refuses before admission");
    assert!(err.to_string().contains("model"), "{err}");
    assert!(manager.read().await.list_filtered(true).is_empty());
    assert!(!worktree.exists());
}

#[tokio::test]
async fn issue_5305_unbuildable_provider_refuses_before_admission() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let manager = new_shared_subagent_manager(workspace.path().to_path_buf(), 1);
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.path().to_path_buf());
    runtime.manager = manager.clone();
    runtime.api_config = None;
    let mut profile = custom_fleet_profile("consultant");
    profile.provider = Some("openai-codex".to_string());
    profile.model = Some("gpt-5.6-sol".to_string());
    runtime.fleet_roster = std::sync::Arc::new(FleetRoster::from_members(vec![
        crate::fleet::profile::AgentProfile {
            id: "consultant".to_string(),
            display_name: None,
            description: None,
            requires: Vec::new(),
            profile,
            source: std::path::PathBuf::from("private/profile.toml"),
            origin: crate::fleet::roster::ProfileOrigin::Personal,
            plugin_authority: None,
        },
    ]));
    let context = runtime.context.clone();
    let err = AgentTool::new(manager.clone(), runtime)
        .execute(
            json!({"action":"start", "type":"consultant", "prompt":"must not admit"}),
            &context,
        )
        .await
        .expect_err("missing provider client is refused");
    assert!(err.to_string().contains("could not be built"), "{err}");
    assert!(manager.read().await.list_filtered(true).is_empty());
}

#[test]
fn issue_5305_builtin_inheritance_and_redaction_are_bounded() {
    let request =
        parse_spawn_request(&json!({"prompt":"x", "type":"consultant"})).expect("request");
    let mut runtime = stub_runtime();
    runtime.model = "deepseek-v4-flash".to_string();
    let member = runtime
        .fleet_roster
        .get("consultant")
        .expect("built-in member");
    let requested_route = RequestedChildRoute {
        requested_type: "consultant".to_string(),
        requested_profile: None,
        requested_reasoning: "inherit".to_string(),
    };
    let receipt = mint_child_route_receipt(
        &requested_route,
        &request,
        Some(member),
        &runtime,
        "deepseek-v4-flash".to_string(),
        "run.model",
    )
    .expect("bounded receipt");
    let encoded = serde_json::to_string(&receipt).expect("receipt json");
    assert!(encoded.len() <= CHILD_ROUTE_RECEIPT_MAX_BYTES);
    assert_eq!(receipt.profile_origin.as_deref(), Some("built-in"));
    assert_eq!(receipt.route_source, "run.model");
    for forbidden in ["test-key", "127.0.0.1", "codewhale-test-stub", "/"] {
        assert!(
            !encoded.contains(forbidden),
            "receipt leaked {forbidden}: {encoded}"
        );
    }
}

#[tokio::test]
async fn issue_5305_receipt_survives_ledger_interruption_completion_and_resume() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let manager = new_shared_subagent_manager(workspace.path().to_path_buf(), 4);
    let receipt = ChildRouteReceipt {
        requested_type: "consultant".to_string(),
        requested_profile: None,
        resolved_profile_id: Some("consultant".to_string()),
        profile_origin: Some("personal".to_string()),
        canonical_role: "consultant".to_string(),
        provider_id: "openai-codex".to_string(),
        model_id: "gpt-5.6-sol".to_string(),
        route_source: "agent_profile.model".to_string(),
        requested_reasoning: "inherit".to_string(),
        effective_reasoning: Some("high".to_string()),
        runtime_version: "test".to_string(),
        runtime_build_sha: "test-build".to_string(),
    };
    let agent_id = {
        let mut guard = manager.write().await;
        let (agent_id, _) = guard.insert_test_interrupted_continuable_agent(
            "receipt-child",
            workspace.path(),
            vec![text_message("assistant", "checkpointed work")],
        );
        guard
            .worker_records
            .get_mut(&agent_id)
            .expect("worker record")
            .spec
            .child_route = Some(receipt.clone());
        let ledger = guard
            .coordination_summary_for(&agent_id, 4)
            .expect("ledger projection");
        assert_eq!(ledger.child_route, Some(receipt.clone()));
        agent_id
    };

    let context = ToolContext::new(workspace.path().to_path_buf());
    let interrupt = AgentsInterruptTool::new(manager.clone())
        .execute(
            json!({"agent_id": agent_id, "reason": "pause for review"}),
            &context,
        )
        .await
        .expect("already interrupted child projects its receipt");
    let interrupt_json: serde_json::Value =
        serde_json::from_str(&interrupt.content).expect("interrupt json");
    assert_eq!(interrupt_json["child_route"], json!(receipt));

    let interrupted = manager
        .read()
        .await
        .get_result(&agent_id)
        .expect("interrupted snapshot");
    assert_eq!(interrupted.child_route.as_ref(), Some(&receipt));
    let completion = subagent_completion_from_result(&interrupted);
    assert!(completion.payload.contains("gpt-5.6-sol"));

    let mut runtime = stub_runtime();
    runtime.manager = manager.clone();
    let resumed = {
        let mut guard = manager.write().await;
        guard
            .resume_from_checkpoint(manager.clone(), runtime, &agent_id, "continue")
            .expect("resume preserves receipt")
    };
    assert_ne!(resumed.agent_id, agent_id);
    assert_eq!(resumed.child_route.as_ref(), Some(&receipt));
    manager
        .write()
        .await
        .cancel_agent(&resumed.agent_id)
        .expect("cancel resumed test child");
}
