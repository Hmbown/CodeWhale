use super::*;
use crate::fleet::roster::FleetRoster;
use crate::tools::{AgentToolSurfaceOptions, ToolRegistryBuilder};
use crate::worker_profile::ShellPolicy;
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::{Builder as TempDirBuilder, tempdir};

fn built_in_whale_name_that_cannot_be_generated_for(agent_id: &str) -> &'static str {
    WHALE_NICKNAMES
        .iter()
        .chain(WHALE_NICKNAMES_JA)
        .chain(WHALE_NICKNAMES_ZH_HANT)
        .chain(WHALE_NICKNAMES_PT_BR)
        .chain(WHALE_NICKNAMES_ES_419)
        .chain(WHALE_NICKNAMES_VI)
        .chain(WHALE_NICKNAMES_KO)
        .copied()
        .find(|name| generated_whale_name_base(agent_id, name).is_none())
        .expect("the combined pools contain labels not generated for one id")
}

#[test]
fn generated_whale_names_follow_session_language_without_mixing() {
    let localized_pools: &[(&str, &[&str])] = &[
        ("ja", WHALE_NICKNAMES_JA),
        ("zh-Hant", WHALE_NICKNAMES_ZH_HANT),
        ("pt-BR", WHALE_NICKNAMES_PT_BR),
        ("es-419", WHALE_NICKNAMES_ES_419),
        ("vi", WHALE_NICKNAMES_VI),
        ("ko", WHALE_NICKNAMES_KO),
    ];

    for index in 0..64 {
        let id = format!("agent_locale_{index}");
        let english = whale_name_for_id_in_locale(&id, "en");
        let chinese = whale_name_for_id_in_locale(&id, "zh-Hans");

        assert!(english.is_ascii(), "English name leaked locale: {english}");
        assert!(
            !chinese.is_ascii(),
            "Chinese name fell back to English: {chinese}"
        );
        let english_index = WHALE_NICKNAMES
            .iter()
            .position(|candidate| *candidate == english)
            .expect("English generated name belongs to the curated pool");
        assert_eq!(english_index % 2, 0);
        assert_eq!(WHALE_NICKNAMES[english_index + 1], chinese);

        for (locale, pool) in localized_pools {
            let generated = whale_name_for_id_in_locale(&id, locale);
            assert!(
                pool.contains(&generated.as_str()),
                "{locale} generated a name from another language: {generated}"
            );
        }
    }

    assert_eq!(
        whale_name_for_id_in_locale("fallback", "unknown"),
        whale_name_for_id_in_locale("fallback", "en")
    );
}

#[test]
fn locale_matched_whale_collision_suffix_stays_in_language() {
    let id = "agent_locale_collision";
    let base = whale_name_for_id_in_locale(id, "zh-Hans");
    let active = HashSet::from([base.clone()]);
    let unique = assign_unique_whale_name_in_locale(id, &active, "zh-Hans");

    assert_ne!(unique, base);
    assert!(unique.starts_with(&base));
    assert!(!unique.is_ascii());
}

#[test]
fn localized_whale_displays_rederive_legacy_names_from_neutral_ids() {
    let generated_a = whale_name_for_id_in_locale("agent_english_a", "zh-Hans");
    let generated_b = whale_name_for_id_in_locale("agent_english_b", "ja");
    let generated_c = whale_name_for_id_in_locale("agent_english_c", "vi");
    let explicit_whale_id = "agent_explicit_whale";
    let explicit_whale = built_in_whale_name_that_cannot_be_generated_for(explicit_whale_id);
    let displays = localized_whale_display_names(
        [
            ("agent_english_a", Some(generated_a.as_str())),
            ("agent_english_b", Some(generated_b.as_str())),
            ("agent_english_c", Some(generated_c.as_str())),
            ("agent_explicit", Some("docs-fixer")),
            (explicit_whale_id, Some(explicit_whale)),
        ],
        "en",
    );

    for agent_id in ["agent_english_a", "agent_english_b", "agent_english_c"] {
        let display = displays.get(agent_id).expect("generated display");
        assert!(
            display.is_ascii(),
            "English UI leaked a prior-locale whale name: {display}"
        );
        let base = generated_whale_name_base(agent_id, display).expect("English whale display");
        let index = WHALE_NICKNAMES
            .iter()
            .position(|candidate| *candidate == base)
            .expect("English display belongs to the paired pool");
        assert_eq!(index % 2, 0, "English display selected a zh-Hans pair");
    }
    assert_eq!(
        displays.get("agent_explicit").map(String::as_str),
        Some("docs-fixer"),
        "an explicit non-whale nickname remains user-owned"
    );
    assert_eq!(
        displays.get(explicit_whale_id).map(String::as_str),
        Some(explicit_whale),
        "a built-in whale word belonging to another id remains user-owned"
    );
}

#[test]
fn exact_deterministic_whale_match_remains_generated_without_provenance() {
    let agent_id = "agent_ambiguous_whale";
    let generated = whale_name_for_id_in_locale(agent_id, "en");
    let suffixed = format!("{generated} (17)");

    assert_eq!(
        generated_whale_name_base(agent_id, &generated),
        Some(generated.as_str())
    );
    assert_eq!(
        generated_whale_name_base(agent_id, &suffixed),
        Some(generated.as_str()),
        "a collision suffix remains presentation-only"
    );
}

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
        launch_manifest: None,
    }
}

fn make_write_worker_spec(worker_id: &str, workspace: PathBuf, root: &str) -> AgentWorkerSpec {
    let mut spec = make_worker_spec(worker_id, workspace.clone());
    spec.agent_type = SubAgentType::Implementer;
    spec.role = Some("implementer".to_string());
    spec.runtime_profile = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
    spec.launch_manifest = Some(ChildLaunchManifest {
        owner_session: "root".to_string(),
        child_id: worker_id.to_string(),
        profile: spec.runtime_profile.clone(),
        prompt: spec.objective.clone(),
        cwd: Some(workspace.display().to_string()),
        worktree: false,
        writable_roots: vec![root.to_string()],
        writable_files: Vec::new(),
        coordination_contracts: Vec::new(),
        expected_artifact: Some("tested patch".to_string()),
        token_budget: None,
        resume_identity: Some(worker_id.to_string()),
        generation: 1,
    });
    spec
}

#[test]
fn active_worker_records_are_never_pruned_by_history_retention() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    for index in 0..=MAX_AGENT_WORKER_RECORDS {
        manager.register_worker(make_worker_spec(
            &format!("active-worker-{index:03}"),
            tmp.path().to_path_buf(),
        ));
    }

    assert_eq!(
        manager.list_worker_records().len(),
        MAX_AGENT_WORKER_RECORDS + 1
    );
    assert!(manager.get_worker_record("active-worker-000").is_some());
    assert!(
        manager
            .list_worker_records()
            .iter()
            .all(|record| !record.status.is_terminal())
    );
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
    manager.record_worker_event(
        "agent_worker_contract",
        AgentWorkerStatus::ModelWait,
        Some("step 1: requesting model response".to_string()),
        Some(1),
        None,
    );
    manager.record_worker_event(
        "agent_worker_contract",
        AgentWorkerStatus::RunningTool,
        Some("step 1: running tool 'read_file'".to_string()),
        Some(1),
        Some("read_file".to_string()),
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
    let owner = agent_worker_owner_snapshot(&record).expect("worker owner snapshot");
    assert_eq!(owner.external, "worker:agent_worker_contract");
    assert_eq!(owner.state, OwnerState::Completed);
    assert_eq!(owner.seq, record.events.back().expect("terminal event").seq);
    assert_eq!(
        owner.output.as_ref().and_then(EvidenceRef::raw_bytes),
        Some("worker summary".len() as u64),
        "persisted worker results become byte-count receipts, never raw graph output"
    );
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
        subagent_progress_tool_display_name("exec_shell_cancel"),
        "Bash"
    );
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
        AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(1),
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
        AgentProgressEventMeta::new(AgentWorkerStatus::WaitingForUser),
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
        Ok(Event::AgentProgress { id, status, activity, .. })
            if id == "agent_waiting"
                && status == "waiting for user input"
                && activity.worker_status == AgentWorkerStatus::WaitingForUser
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
        AgentProgressEventMeta::new(AgentWorkerStatus::ModelWait).with_step(1),
        None,
        1,
    );

    assert_eq!(tx.capacity(), 7);
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::AgentProgress { id, status, activity, .. })
            if id == "agent_small_channel"
                && status == "step 1: requesting model response"
                && activity.worker_status == AgentWorkerStatus::ModelWait
                && activity.step == Some(1)
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

#[test]
fn coordination_ledger_persists_and_replays_bounded_contracts() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());
    manager
        .coordination
        .register_claim(
            WriteScopeClaim {
                owner: "agent_writer".into(),
                roots: vec!["src".into()],
                exact_files: vec!["Cargo.toml".into()],
                contracts: vec!["public-api".into()],
            },
            false,
            |_| false,
        )
        .expect("claim");
    manager
        .record_coordination_decision(DecisionRecord {
            decision_id: "decision_storage".into(),
            subject: "storage".into(),
            status: DecisionStatus::Accepted,
            owner: "agent_writer".into(),
            scope: vec!["router".into()],
            constraints: vec!["bounded".into()],
            evidence_handles: vec!["test:coordination".into()],
            version: 1,
            sequence: 0,
        })
        .expect("decision");
    manager.persist_state().unwrap().join().unwrap();

    let mut loaded = SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    loaded.load_state().expect("reload coordination");
    assert!(
        loaded
            .validate_write_scope("agent_writer", &["src/lib.rs".into()])
            .is_ok()
    );
    let err = loaded
        .validate_write_scope("agent_writer", &["docs/readme.md".into()])
        .unwrap_err();
    assert!(err.contains("outside") && err.contains("expand"), "{err}");
    let inspect = loaded.inspect_coordination(Some("storage"), 4);
    assert_eq!(inspect["decisions"][0]["decision_id"], "decision_storage");
    assert_eq!(inspect["write_claims"][0]["claim"]["owner"], "agent_writer");
}

#[test]
fn invalid_decision_and_reconciliation_inputs_cannot_poison_persisted_replay() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());

    for bad_id in ["bad\ndecision".to_string(), "x".repeat(513)] {
        let error = manager
            .record_coordination_decision(DecisionRecord {
                decision_id: bad_id,
                subject: "safe-subject".into(),
                status: DecisionStatus::Proposed,
                owner: "agent-a".into(),
                scope: vec!["path:src".into()],
                constraints: vec!["bounded".into()],
                evidence_handles: vec!["receipt:test".into()],
                version: 1,
                sequence: 0,
            })
            .expect_err("invalid decision id must fail before mutation");
        assert!(error.contains("decision id"), "{error}");
        assert!(manager.coordination.decisions.is_empty());
    }

    for (id, owner) in [("candidate-a", "agent-a"), ("candidate-b", "agent-b")] {
        manager
            .record_coordination_decision(DecisionRecord {
                decision_id: id.into(),
                subject: "safe-subject".into(),
                status: DecisionStatus::Proposed,
                owner: owner.into(),
                scope: vec!["path:src".into()],
                constraints: vec!["bounded".into()],
                evidence_handles: vec![format!("receipt:{id}")],
                version: 1,
                sequence: 0,
            })
            .expect("valid candidate decision");
    }
    let duplicate_candidate = manager
        .coordination
        .reconcile(
            "safe-subject".into(),
            "root".into(),
            vec!["candidate-a".into(), "candidate-b".into()],
            "preserve both".into(),
            vec!["receipt:fan-in".into()],
            vec!["branch:a".into(), " branch:a ".into()],
            0,
            3,
            vec!["review:independent".into()],
            vec!["verify:locked".into()],
            "verified".into(),
        )
        .expect_err("whitespace aliases must not satisfy two-candidate fan-in");
    assert!(
        duplicate_candidate.contains("distinct normalized candidate"),
        "{duplicate_candidate}"
    );
    let duplicate_input = manager
        .coordination
        .reconcile(
            "safe-subject".into(),
            "root".into(),
            vec!["candidate-a".into(), " candidate-a ".into()],
            "preserve both".into(),
            vec!["receipt:fan-in".into()],
            vec!["branch:a".into(), "branch:b".into()],
            0,
            3,
            vec!["review:independent".into()],
            vec!["verify:locked".into()],
            "verified".into(),
        )
        .expect_err("whitespace aliases must not satisfy two-decision fan-in");
    assert!(
        duplicate_input.contains("distinct normalized input decisions"),
        "{duplicate_input}"
    );
    assert!(manager.coordination.reconciliations.is_empty());

    manager.persist_state().unwrap().join().unwrap();
    let mut replayed =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    replayed
        .load_state()
        .expect("valid state survives restart after rejected poison inputs");
    assert_eq!(replayed.coordination.decisions.len(), 2);
    assert!(replayed.coordination.reconciliations.is_empty());
}

#[test]
fn coordination_hot_paths_count_only_active_authoritative_owners() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    manager.register_worker(make_worker_spec("agent-live", tmp.path().to_path_buf()));
    manager.register_worker(make_worker_spec("agent-done", tmp.path().to_path_buf()));
    manager
        .worker_records
        .get_mut("agent-done")
        .expect("terminal worker")
        .status = AgentWorkerStatus::Completed;
    for (owner, root) in [("agent-live", "src/live"), ("agent-done", "src/history")] {
        manager
            .coordination
            .register_claim(
                WriteScopeClaim {
                    owner: owner.into(),
                    roots: vec![root.into()],
                    exact_files: Vec::new(),
                    contracts: Vec::new(),
                },
                false,
                |_| false,
            )
            .expect("bounded non-overlapping claim");
    }

    let projection = manager.coordination_detail_projection(None, 24);
    assert_eq!(
        projection.metrics.hottest_paths,
        vec![CoordinationHotPath {
            path: "src/live".into(),
            active_claims: 1,
        }]
    );
}

#[test]
fn headless_worker_registration_enforces_live_claims_and_projects_context() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 8);
    manager
        .record_coordination_decision(DecisionRecord {
            decision_id: "decision-src-a".into(),
            subject: "src-a-contract".into(),
            status: DecisionStatus::Accepted,
            owner: "planner".into(),
            scope: vec!["path:src/a".into()],
            constraints: vec!["preserve the public API".into()],
            evidence_handles: vec!["receipt:planner".into()],
            version: 1,
            sequence: 0,
        })
        .expect("accepted decision");

    let worker = |id: &str, root: &str| {
        let mut spec = make_worker_spec(id, tmp.path().to_path_buf());
        spec.agent_type = SubAgentType::Implementer;
        spec.role = Some("worker".into());
        spec.runtime_profile = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        spec.launch_manifest = Some(ChildLaunchManifest {
            owner_session: "fleet-run".into(),
            child_id: id.into(),
            profile: spec.runtime_profile.clone(),
            prompt: spec.objective.clone(),
            cwd: Some(tmp.path().display().to_string()),
            worktree: false,
            writable_roots: vec![root.into()],
            writable_files: Vec::new(),
            coordination_contracts: Vec::new(),
            expected_artifact: None,
            token_budget: None,
            resume_identity: Some(format!("fleet-{id}")),
            generation: 1,
        });
        spec
    };

    let worker_a = worker("worker-a", "src/a");
    manager
        .preflight_worker_coordination(&worker_a)
        .expect("first worker preflight");
    manager
        .register_worker_with_coordination(worker_a)
        .expect("first worker registration");
    let record = manager
        .get_worker_record("worker-a")
        .expect("worker record");
    assert!(record.spec.objective.contains("src-a-contract"));
    assert!(
        record
            .spec
            .launch_manifest
            .as_ref()
            .expect("launch manifest")
            .prompt
            .contains("src-a-contract")
    );

    let overlap = manager
        .preflight_worker_coordination(&worker("worker-b", "src/a/nested"))
        .expect_err("overlapping live Fleet writer must remain queued");
    assert!(overlap.contains("worker-a"), "{overlap}");
    assert_eq!(manager.coordination.contentions.len(), 1);
    assert!(manager.get_worker_record("worker-b").is_none());

    let worker_c = worker("worker-c", "src/b");
    manager
        .preflight_worker_coordination(&worker_c)
        .expect("non-overlapping worker preflight");
    manager
        .register_worker_with_coordination(worker_c)
        .expect("non-overlapping workers may proceed concurrently");
    assert_eq!(manager.coordination.write_claims.len(), 2);
}

#[test]
fn neutral_reconciliation_requires_the_nearest_common_planner() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 8);

    let mut planner = make_worker_spec("planner", tmp.path().to_path_buf());
    planner.agent_type = SubAgentType::Plan;
    planner.role = Some("planner".into());
    manager.register_worker(planner);
    for worker_id in ["worker-a", "worker-b"] {
        let mut worker = make_worker_spec(worker_id, tmp.path().to_path_buf());
        worker.parent_run_id = Some("planner".into());
        worker.agent_type = SubAgentType::Implementer;
        worker.role = Some("worker".into());
        manager.register_worker(worker);
        manager
            .record_coordination_decision(DecisionRecord {
                decision_id: format!("decision-{worker_id}"),
                subject: "public-api".into(),
                status: DecisionStatus::Proposed,
                owner: worker_id.into(),
                scope: vec!["contract:public-api".into()],
                constraints: vec!["preserve candidate".into()],
                evidence_handles: vec![format!("branch:{worker_id}")],
                version: 1,
                sequence: 0,
            })
            .expect("record candidate decision");
    }
    for (worker_id, agent_type, role) in [
        ("reviewer", SubAgentType::Review, "reviewer"),
        ("verifier", SubAgentType::Verifier, "verifier"),
    ] {
        let mut worker = make_worker_spec(worker_id, tmp.path().to_path_buf());
        worker.parent_run_id = Some("planner".into());
        worker.agent_type = agent_type;
        worker.role = Some(role.into());
        manager.register_worker(worker);
        manager.record_worker_event(
            worker_id,
            AgentWorkerStatus::Completed,
            Some(format!("{role} evidence complete")),
            None,
            None,
        );
    }

    let input_decisions = vec!["decision-worker-a".into(), "decision-worker-b".into()];
    let error = manager
        .reconcile_coordination(
            "public-api".into(),
            "worker-a".into(),
            input_decisions.clone(),
            "combine both candidates".into(),
            vec!["receipt:fanin".into()],
            vec!["branch:worker-a".into(), "branch:worker-b".into()],
            1,
            3,
            vec!["agent:reviewer:review-pass".into()],
            vec!["agent:verifier:locked-tests".into()],
            "verified".into(),
        )
        .unwrap_err();
    assert!(error.contains("'planner'"), "{error}");

    let receipt = manager
        .reconcile_coordination(
            "public-api".into(),
            "planner".into(),
            input_decisions,
            "combine both candidates".into(),
            vec!["receipt:fanin".into()],
            vec!["branch:worker-a".into(), "branch:worker-b".into()],
            1,
            3,
            vec!["agent:reviewer:review-pass".into()],
            vec!["agent:verifier:locked-tests".into()],
            "verified".into(),
        )
        .expect("nearest common planner may reconcile");
    assert_eq!(receipt.owner, "planner");
    assert_eq!(receipt.retry_limit, 3);
    assert_eq!(receipt.candidate_handles.len(), 2);
}

#[test]
fn coordination_acceptance_preserves_scopes_candidates_and_replay() {
    let repo = init_subagent_git_repo();
    let state_path = repo.path().join("coordination-state.json");
    let base_branch = {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(repo.path())
            .output()
            .expect("current branch");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    std::fs::create_dir_all(repo.path().join("src")).expect("src directory");

    git(repo.path(), &["switch", "-c", "candidate-a"]);
    std::fs::write(repo.path().join("src/a.rs"), "pub const A: u8 = 1;\n")
        .expect("candidate A edit");
    git(repo.path(), &["add", "src/a.rs"]);
    git(
        repo.path(),
        &[
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "candidate A",
        ],
    );
    let candidate_a = git_stdout(repo.path(), &["rev-parse", "candidate-a"]);

    git(repo.path(), &["switch", &base_branch]);
    git(repo.path(), &["switch", "-c", "candidate-b"]);
    std::fs::create_dir_all(repo.path().join("src")).expect("candidate B src directory");
    std::fs::write(repo.path().join("src/b.rs"), "pub const B: u8 = 2;\n")
        .expect("candidate B edit");
    git(repo.path(), &["add", "src/b.rs"]);
    git(
        repo.path(),
        &[
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "candidate B",
        ],
    );
    let candidate_b = git_stdout(repo.path(), &["rev-parse", "candidate-b"]);

    let mut manager =
        SubAgentManager::new(repo.path().to_path_buf(), 8).with_state_path(state_path.clone());
    let mut planner = make_worker_spec("parent_session", repo.path().to_path_buf());
    planner.agent_type = SubAgentType::Plan;
    planner.role = Some("planner".into());
    manager.register_worker(planner);
    let agent_a = manager.insert_test_running_agent("a", repo.path());
    let agent_b = manager.insert_test_running_agent("b", repo.path());
    let agent_c = manager.insert_test_running_agent("c", repo.path());

    let claim = |owner: &str, path: &str| WriteScopeClaim {
        owner: owner.into(),
        roots: vec![],
        exact_files: vec![path.into()],
        contracts: vec![],
    };
    manager
        .coordination
        .register_claim(claim(&agent_a, "src/a.rs"), false, |_| false)
        .expect("A claim");
    manager
        .coordination
        .register_claim(claim(&agent_b, "src/b.rs"), false, |_| false)
        .expect("B claim");
    let contention = manager
        .coordination
        .register_claim(claim(&agent_c, "src/a.rs"), false, |owner| {
            owner == agent_a || owner == agent_b
        })
        .expect_err("C cannot collide silently with A");
    assert!(contention.contains(&agent_a), "{contention}");
    assert_eq!(manager.coordination.contentions.len(), 1);

    let scope_expansion = manager
        .expand_write_claim(&agent_a, vec![], vec!["src/b.rs".into()], vec![])
        .expect_err("A expansion into B must visibly replan");
    assert!(scope_expansion.contains("contention"), "{scope_expansion}");

    for (id, subject, owner, scope) in [
        ("accepted-a", "api-a", agent_a.as_str(), "path:src/a.rs"),
        ("accepted-b", "api-b", agent_b.as_str(), "path:src/b.rs"),
    ] {
        manager
            .record_coordination_decision(DecisionRecord {
                decision_id: id.into(),
                subject: subject.into(),
                status: DecisionStatus::Accepted,
                owner: owner.into(),
                scope: vec![scope.into()],
                constraints: vec!["preserve public behavior".into()],
                evidence_handles: vec![format!("commit:{id}")],
                version: 1,
                sequence: 0,
            })
            .expect("accepted scoped decision");
    }
    for (id, owner) in [("merge-a", agent_a.as_str()), ("merge-b", agent_b.as_str())] {
        manager
            .record_coordination_decision(DecisionRecord {
                decision_id: id.into(),
                subject: "merge-strategy".into(),
                status: DecisionStatus::Proposed,
                owner: owner.into(),
                scope: vec!["contract:public-api".into()],
                constraints: vec!["retain both edits".into()],
                evidence_handles: vec![format!("commit:{id}")],
                version: 1,
                sequence: 0,
            })
            .expect("candidate decision");
    }
    let merge_a = manager
        .coordination
        .decisions
        .iter()
        .find(|decision| decision.decision_id == "merge-a")
        .cloned()
        .expect("merge A decision");
    let merge_b = manager
        .coordination
        .decisions
        .iter()
        .find(|decision| decision.decision_id == "merge-b")
        .cloned()
        .expect("merge B decision");
    assert_eq!(merge_a.version, 1);
    assert_eq!(merge_b.version, 2);
    let stale_version = manager
        .update_coordination_decision(
            "merge-b",
            DecisionStatus::Accepted,
            &agent_b,
            merge_a.version,
        )
        .expect_err("a competing stale version cannot replace either candidate");
    assert!(stale_version.contains("version changed"), "{stale_version}");
    assert_eq!(
        manager
            .coordination
            .decisions
            .iter()
            .filter(|decision| {
                decision.subject == "merge-strategy" && decision.status == DecisionStatus::Proposed
            })
            .count(),
        2,
        "both conflicting candidates remain preserved"
    );
    let claim_a = manager
        .coordination
        .write_claims
        .iter()
        .find(|claim| claim.claim.owner == agent_a)
        .expect("claim A")
        .claim
        .clone();
    let claim_b = manager
        .coordination
        .write_claims
        .iter()
        .find(|claim| claim.claim.owner == agent_b)
        .expect("claim B")
        .claim
        .clone();
    let (projection_a, projection_a_receipt) =
        manager
            .coordination
            .project_relevant_decisions(&agent_a, Some(&claim_a), &["File".into()]);
    let (projection_b, projection_b_receipt) =
        manager
            .coordination
            .project_relevant_decisions(&agent_b, Some(&claim_b), &["File".into()]);
    assert!(projection_a.contains("api-a") && !projection_a.contains("api-b"));
    assert!(projection_b.contains("api-b") && !projection_b.contains("api-a"));
    for receipt in [&projection_a_receipt, &projection_b_receipt] {
        assert!(
            receipt.projected_bytes <= coord::COORDINATION_PROJECTION_BYTE_LIMIT,
            "projection byte cap is durable"
        );
        assert!(
            receipt.decision_ids.len() <= coord::COORDINATION_PROJECTION_DECISION_LIMIT,
            "projection decision cap is durable"
        );
        assert_eq!(
            receipt
                .decision_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            receipt.decision_ids.len(),
            "projection ids are deduplicated"
        );
        assert_eq!(receipt.deduplicated, 0);
        assert_eq!(receipt.omitted, 0);
    }

    for (worker_id, agent_type, role) in [
        ("reviewer-agent", SubAgentType::Review, "reviewer"),
        ("verifier-agent", SubAgentType::Verifier, "verifier"),
    ] {
        let mut worker = make_worker_spec(worker_id, repo.path().to_path_buf());
        worker.parent_run_id = Some("parent_session".into());
        worker.agent_type = agent_type;
        worker.role = Some(role.into());
        manager.register_worker(worker);
        manager.record_worker_event(
            worker_id,
            AgentWorkerStatus::Completed,
            Some(format!("{role} evidence complete")),
            None,
            None,
        );
    }

    let receipt = manager
        .reconcile_coordination(
            "merge-strategy".into(),
            "parent_session".into(),
            vec!["merge-a".into(), "merge-b".into()],
            "retry budget exhausted; preserve both candidates for explicit disposition".into(),
            vec!["receipt:neutral-fan-in".into()],
            vec![
                format!("branch:candidate-a@{candidate_a}"),
                format!("branch:candidate-b@{candidate_b}"),
            ],
            3,
            3,
            vec!["agent:reviewer-agent:review-pass".into()],
            vec!["agent:verifier-agent:locked-tests".into()],
            "blocked".into(),
        )
        .expect("nearest common Planner records terminal retry exhaustion");
    assert_eq!(receipt.retry_count, receipt.retry_limit);
    assert_eq!(receipt.candidate_handles.len(), 2);

    manager.persist_state().unwrap().join().unwrap();
    let mut replayed =
        SubAgentManager::new(repo.path().to_path_buf(), 8).with_state_path(state_path);
    replayed.load_state().expect("restart/replay");
    assert_eq!(
        replayed.coordination.schema_version,
        coord::COORDINATION_SCHEMA_VERSION
    );
    assert_eq!(replayed.coordination.contentions.len(), 2);
    assert_eq!(replayed.coordination.projections.len(), 2);
    assert_eq!(replayed.coordination.reconciliations.len(), 1);
    assert!(
        replayed
            .coordination
            .write_claims
            .iter()
            .any(|claim| claim.claim.owner == agent_a)
    );
    let sequences = replayed
        .coordination
        .decisions
        .iter()
        .map(|decision| decision.sequence)
        .chain(
            replayed
                .coordination
                .reconciliations
                .iter()
                .map(|receipt| receipt.sequence),
        )
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|window| window[0] < window[1]));

    assert_eq!(
        git_stdout(repo.path(), &["show", "candidate-a:src/a.rs"]),
        "pub const A: u8 = 1;"
    );
    assert_eq!(
        git_stdout(repo.path(), &["show", "candidate-b:src/b.rs"]),
        "pub const B: u8 = 2;"
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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

#[tokio::test]
async fn tool_free_subagent_omits_chat_tools_and_tool_choice() {
    let tmp = tempdir().expect("tempdir");
    let (client, calls, bodies) = delayed_chat_client(Duration::ZERO, "done").await;
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let mut runtime = stub_runtime();
    runtime.client = client;
    runtime.manager = manager;
    runtime.context = ToolContext::new(tmp.path());
    let (_input_tx, input_rx) = mpsc::unbounded_channel();

    let result = run_subagent(
        &runtime,
        "agent_no_tools_request".to_string(),
        SubAgentType::General,
        "Return a final answer without tools.".to_string(),
        make_assignment(),
        Some(Vec::new()),
        false,
        Instant::now(),
        1,
        None,
        input_rx,
    )
    .await
    .expect("tool-free sub-agent should complete");

    assert_eq!(result.status, SubAgentStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let bodies = bodies.lock().expect("request body recorder mutex poisoned");
    let body = bodies.first().expect("one chat request body");
    assert!(body.get("tools").is_none(), "tools must be omitted: {body}");
    assert!(
        body.get("tool_choice").is_none(),
        "tool_choice must be omitted: {body}"
    );
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

async fn always_rate_limited_chat_client() -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("Retry-After", "0")],
                        Json(json!({
                            "error": {
                                "message": "test provider rate limit"
                            }
                        })),
                    )
                        .into_response()
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake rate-limited chat server");
    let addr = listener.local_addr().expect("fake chat server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        retry: Some(crate::config::RetryConfig {
            enabled: Some(false),
            max_retries: Some(0),
            initial_delay: Some(0.0),
            max_delay: Some(0.0),
            exponential_base: Some(1.0),
        }),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake rate-limited chat client");
    (client, calls)
}

async fn always_invalid_request_chat_client() -> (DeepSeekClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/{*path}",
        post({
            let calls = Arc::clone(&calls);
            move |Json(_body): Json<Value>| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": {
                                "message": "model is not supported on this endpoint"
                            }
                        })),
                    )
                        .into_response()
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake invalid-request chat server");
    let addr = listener
        .local_addr()
        .expect("fake invalid-request server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        base_url: Some(format!("http://{addr}/v1")),
        retry: Some(crate::config::RetryConfig {
            enabled: Some(false),
            max_retries: Some(0),
            initial_delay: Some(0.0),
            max_delay: Some(0.0),
            exponential_base: Some(1.0),
        }),
        ..crate::config::Config::default()
    };
    let client = DeepSeekClient::new(&config).expect("fake invalid-request chat client");
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
fn role_prompts_use_canonical_file_action_contract() {
    let explore = SubAgentType::Explore.system_prompt();
    assert!(
        explore.contains("`File` with actions `list`, `search_name`, `search_content`, and `read`")
    );

    let implementer = SubAgentType::Implementer.system_prompt();
    assert!(implementer.contains("`File` action `read`"));
    assert!(implementer.contains("action `edit`"));
    assert!(implementer.contains("action `patch`"));

    for prompt in [&explore, &implementer] {
        for legacy_name in [
            "list_dir",
            "file_search",
            "grep_files",
            "read_file",
            "edit_file",
            "apply_patch",
        ] {
            assert!(
                !prompt.contains(legacy_name),
                "live role prompt must not teach legacy File alias {legacy_name}: {prompt}"
            );
        }
    }
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

    assert!(description.contains("Start one focused background worker"));
    assert!(description.contains("prompt is enough"));
    assert!(description.contains("multiple starts"));
    assert!(description.contains("agents/list"));
    assert!(description.contains("agents/wait"));
    assert!(description.contains("Fleet profile"));
    assert!(
        estimate_tool_description_tokens_conservative(description) <= 1024,
        "agent description exceeds the conservative 1024-token budget"
    );
}

#[test]
fn deliberate_spawn_requires_delegation_fields() {
    let missing = parse_spawn_request(&json!({
        "prompt": "do a thing",
        "deliberate": true,
    }));
    assert!(
        missing.is_err(),
        "deliberate spawn without fields must fail"
    );
    let err = missing.unwrap_err().to_string();
    assert!(err.contains("expected_artifact"), "{err}");

    let ok = parse_spawn_request(&json!({
        "prompt": "review the diff",
        "deliberate": true,
        "type": "review",
        "workspace_policy": "shared",
        "expected_artifact": "review findings",
        "write_authority": "read_only",
    }))
    .expect("deliberate spawn with all fields");
    assert_eq!(ok.agent_type, SubAgentType::Review);
    assert_eq!(ok.token_budget, None);
    assert_eq!(ok.write_authority, Some(SpawnWriteAuthority::ReadOnly));
    assert_eq!(ok.expected_artifact.as_deref(), Some("review findings"));
    assert!(
        ok.worktree.is_none(),
        "workspace_policy shared must not materialize a worktree"
    );
}

#[test]
fn declared_workspace_policy_worktree_materializes_a_worktree_request() {
    // TUI-DOG-017: a declared policy must be enforced, not decorative. The
    // `worktree` request field is the mechanism that actually creates one.
    let request = parse_spawn_request(&json!({
        "prompt": "isolate this edit",
        "workspace_policy": "worktree",
    }))
    .expect("worktree policy parses");
    assert!(
        request.worktree.is_some(),
        "workspace_policy=worktree must materialize a worktree request"
    );

    let conflict = parse_spawn_request(&json!({
        "prompt": "contradiction",
        "workspace_policy": "shared",
        "worktree": true,
    }));
    assert!(
        conflict.is_err(),
        "shared policy plus explicit worktree must fail closed"
    );
}

#[test]
fn declared_write_authority_parses_and_worktree_write_requires_isolation() {
    let read_only = parse_spawn_request(&json!({
        "prompt": "look around",
        "write_authority": "read_only",
    }))
    .expect("read_only parses without deliberate");
    assert_eq!(
        read_only.write_authority,
        Some(SpawnWriteAuthority::ReadOnly)
    );

    let contradiction = parse_spawn_request(&json!({
        "prompt": "write in a worktree",
        "write_authority": "worktree_write",
    }));
    assert!(
        contradiction.is_err(),
        "worktree_write without worktree isolation must fail closed"
    );

    let ok = parse_spawn_request(&json!({
        "prompt": "write in a worktree",
        "write_authority": "worktree_write",
        "worktree": true,
        "write_roots": ["."],
    }))
    .expect("worktree_write with isolation parses");
    assert_eq!(ok.write_authority, Some(SpawnWriteAuthority::WorktreeWrite));
}

#[test]
fn prompt_only_general_children_default_read_only_instead_of_claiming_the_repo() {
    let request = parse_spawn_request(&json!({
        "prompt": "inspect the subsystem",
    }))
    .expect("prompt-only child remains ergonomic");
    assert_eq!(request.write_authority, Some(SpawnWriteAuthority::ReadOnly));
    assert!(request.write_roots.is_empty());

    let error = parse_spawn_request(&json!({
        "prompt": "edit without a claim",
        "write_authority": "workspace_write",
    }))
    .expect_err("explicit write authority requires an explicit bounded claim")
    .to_string();
    assert!(error.contains("must declare"), "{error}");

    for explicit in [
        json!({"prompt": "implement", "type": "implementer"}),
        json!({"prompt": "general but explicit", "type": "general"}),
        json!({"prompt": "fleet role", "role": "release_lead"}),
    ] {
        let error = parse_spawn_request(&explicit)
            .expect_err("explicit write-capable identity must not silently become read-only")
            .to_string();
        assert!(error.contains("must declare"), "{error}");
    }
}

#[test]
fn spawn_role_and_write_authority_cannot_contradict() {
    let reviewer = parse_spawn_request(&json!({
        "prompt": "review while writing",
        "type": "review",
        "write_authority": "workspace_write",
        "write_roots": ["src"]
    }))
    .expect_err("read-only role cannot request writes")
    .to_string();
    assert!(reviewer.contains("read-only role"), "{reviewer}");

    let implementer = parse_spawn_request(&json!({
        "prompt": "implement without writes",
        "type": "implementer",
        "write_authority": "read_only"
    }))
    .expect_err("implementer cannot claim read-only authority")
    .to_string();
    assert!(implementer.contains("write-capable role"), "{implementer}");
}

#[test]
fn declared_write_scope_is_normalized_and_rejects_traversal() {
    let request = parse_spawn_request(&json!({
        "prompt": "edit bounded files",
        "write_authority": "workspace_write",
        "write_roots": ["./crates/tui/src/", "docs"],
        "exact_files": ["Cargo.toml"],
        "coordination_contracts": ["public-api"],
        "dependencies": ["#4619", "#4619"],
        "acceptance": ["locked tests pass"]
    }))
    .expect("bounded scope parses");
    assert_eq!(request.write_roots, vec!["crates/tui/src", "docs"]);
    assert_eq!(request.exact_files, vec!["Cargo.toml"]);
    assert_eq!(request.coordination_contracts, vec!["public-api"]);
    assert_eq!(request.dependencies, vec!["#4619"]);
    assert_eq!(request.acceptance, vec!["locked tests pass"]);

    let err = parse_spawn_request(&json!({
        "prompt": "escape",
        "write_roots": ["../outside"]
    }))
    .expect_err("traversal must fail")
    .to_string();
    assert!(
        err.contains("repo-relative") || err.contains("traversal"),
        "{err}"
    );
}

#[test]
fn shared_child_cwd_claims_use_one_root_namespace_for_collisions_and_mutations() {
    let repo = tempdir().expect("repo");
    let package = repo.path().join("pkg");
    std::fs::create_dir_all(package.join("src")).expect("package tree");
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname='pkg'\nversion='0.1.0'\n",
    )
    .expect("package manifest");

    let mut manager = SubAgentManager::new(repo.path().to_path_buf(), 4);
    let root_owner = manager.insert_test_running_agent("root_writer", repo.path());
    let child_owner = manager.insert_test_running_agent("child_writer", &package);
    let root_claim = manager
        .namespace_write_claim(
            repo.path(),
            false,
            WriteScopeClaim {
                owner: root_owner.clone(),
                roots: Vec::new(),
                exact_files: vec!["pkg/Cargo.toml".into()],
                contracts: Vec::new(),
            },
        )
        .expect("root claim namespace");
    manager
        .coordination
        .register_claim(root_claim, false, |_| true)
        .expect("root writer claim");
    let child_alias = manager
        .namespace_write_claim(
            &package,
            false,
            WriteScopeClaim {
                owner: child_owner.clone(),
                roots: Vec::new(),
                exact_files: vec!["Cargo.toml".into()],
                contracts: Vec::new(),
            },
        )
        .expect("child claim namespace");
    assert_eq!(child_alias.exact_files, vec!["pkg/Cargo.toml"]);
    let active = manager.active_coordination_owners();
    let collision = manager
        .coordination
        .register_claim(child_alias, false, |owner| active.contains(owner))
        .expect_err("root and child cwd aliases must collide");
    assert!(collision.contains(&root_owner), "{collision}");

    let mut scoped = SubAgentManager::new(repo.path().to_path_buf(), 4);
    let child_owner = scoped.insert_test_running_agent("scoped_child", &package);
    let claim = scoped
        .namespace_write_claim(
            &package,
            false,
            WriteScopeClaim {
                owner: child_owner.clone(),
                roots: vec!["src".into()],
                exact_files: Vec::new(),
                contracts: Vec::new(),
            },
        )
        .expect("scoped child claim namespace");
    assert_eq!(claim.roots, vec!["pkg/src"]);
    scoped
        .coordination
        .register_claim(claim, false, |_| true)
        .expect("scoped child claim");
    scoped
        .validate_write_scope(&child_owner, &["src/lib.rs".into()])
        .expect("child-relative mutation resolves inside persisted root scope");
    let outside = scoped
        .validate_write_scope(&child_owner, &["../other/lib.rs".into()])
        .expect_err("child cwd traversal must remain outside authority");
    assert!(
        outside.contains("repo-relative") || outside.contains("traversal"),
        "{outside}"
    );
}

#[test]
fn resident_file_context_is_workspace_relative_bounded_and_exclusive() {
    let tmp = tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("context.txt"), "bounded context").unwrap();
    let context = ToolContext::new(tmp.path());

    let resident = read_bounded_resident_context(&context, "context.txt")
        .expect("regular in-workspace resident context");
    assert_eq!(resident.display_path, "context.txt");
    assert_eq!(resident.contents, "bounded context");
    assert_eq!(
        resident.lease_key,
        tmp.path()
            .join("context.txt")
            .canonicalize()
            .expect("canonical test resident")
            .display()
            .to_string()
    );

    let absolute = read_bounded_resident_context(
        &context,
        &tmp.path().join("context.txt").display().to_string(),
    )
    .expect_err("absolute resident paths must fail closed")
    .to_string();
    assert!(absolute.contains("repo-relative"), "{absolute}");
    let traversal = read_bounded_resident_context(&context, "../context.txt")
        .expect_err("resident traversal must fail closed")
        .to_string();
    assert!(
        traversal.contains("repo-relative") || traversal.contains("parent traversal"),
        "{traversal}"
    );

    std::fs::write(
        tmp.path().join("oversize.txt"),
        vec![b'x'; usize::try_from(MAX_RESIDENT_CONTEXT_BYTES + 1).unwrap()],
    )
    .unwrap();
    let oversize = read_bounded_resident_context(&context, "oversize.txt")
        .expect_err("oversize resident context must fail closed")
        .to_string();
    assert!(oversize.contains("bounded context limit"), "{oversize}");

    let lease_key = format!("resident-test-{}", uuid::Uuid::new_v4());
    reserve_resident_lease(&lease_key, "context.txt").expect("first resident owner reserves");
    let duplicate = reserve_resident_lease(&lease_key, "context.txt")
        .expect_err("a second resident owner must be rejected")
        .to_string();
    assert!(duplicate.contains("already leased"), "{duplicate}");
    rollback_pending_resident_lease(&lease_key);
    reserve_resident_lease(&lease_key, "context.txt").expect("rollback releases pending lease");
    rollback_pending_resident_lease(&lease_key);

    let other_workspace = tempdir().expect("second workspace");
    std::fs::write(other_workspace.path().join("context.txt"), "other context").unwrap();
    let other =
        read_bounded_resident_context(&ToolContext::new(other_workspace.path()), "context.txt")
            .expect("same relative path in another workspace");
    assert_ne!(resident.lease_key, other.lease_key);
    reserve_resident_lease(&resident.lease_key, &resident.display_path)
        .expect("first workspace lease");
    reserve_resident_lease(&other.lease_key, &other.display_path)
        .expect("unrelated workspace must not falsely collide");
    rollback_pending_resident_lease(&resident.lease_key);
    rollback_pending_resident_lease(&other.lease_key);
}

#[cfg(unix)]
#[test]
fn resident_file_context_rejects_symlink_aliases() {
    use std::os::unix::fs::symlink;

    let tmp = tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("target.txt"), "secret alias").unwrap();
    symlink("target.txt", tmp.path().join("alias.txt")).unwrap();
    let error = read_bounded_resident_context(&ToolContext::new(tmp.path()), "alias.txt")
        .expect_err("resident context must not traverse symlinks")
        .to_string();
    assert!(error.contains("must not traverse symlinks"), "{error}");
}

#[test]
fn new_session_tools_use_single_agent_name() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    assert_eq!(AgentTool::new(manager, stub_runtime()).name(), "agent");
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
    assert_eq!(parsed.fork_context, Some(true));

    let input = json!({
        "prompt": "continue from here",
        "inherit_context": true
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.fork_context, Some(true));

    // Omitted entirely: deferred to the spawn-time auto policy.
    let input = json!({ "prompt": "continue from here" });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.fork_context, None);
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
fn explore_subagent_inherits_active_model_by_default() {
    // Role names never silently change the model. A Fleet without custom
    // routing should behave exactly like the active session.
    let input = json!({
        "prompt": "find every caller of normalize_model_name",
        "type": "explore"
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.agent_type, SubAgentType::Explore);
    assert_eq!(parsed.model_strength, SubAgentModelStrength::Same);

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
    // any strength hint, so model_strength remains Same.
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
    assert_eq!(parsed.fork_context, Some(true));
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

fn fleet_roster_with(id: &str, profile: codewhale_config::FleetProfile) -> FleetRoster {
    let tmp = tempdir().expect("tempdir");
    let config = codewhale_config::FleetConfigToml {
        profiles: std::collections::BTreeMap::from([(id.to_string(), profile)]),
        ..Default::default()
    };
    FleetRoster::load(&config, tmp.path())
}

fn custom_fleet_profile(role: &str) -> codewhale_config::FleetProfile {
    codewhale_config::FleetProfile {
        slot: codewhale_config::FleetSlot::from_name(role),
        role: codewhale_config::FleetRole {
            name: role.to_string(),
            description: None,
            instructions: None,
        },
        ..Default::default()
    }
}

#[test]
fn test_parse_spawn_request_accepts_profile_and_normalizes() {
    let input = json!({
        "prompt": "review the diff",
        "profile": "  Reviewer  "
    });
    let parsed = parse_spawn_request(&input).expect("spawn request should parse");
    assert_eq!(parsed.profile.as_deref(), Some("reviewer"));
    assert!(!parsed.agent_type_explicit);
    assert!(!parsed.model_strength_explicit);

    let parsed = parse_spawn_request(&json!({"prompt": "x", "fleet_profile": "Scout"}))
        .expect("fleet_profile alias should parse");
    assert_eq!(parsed.profile.as_deref(), Some("scout"));

    let parsed = parse_spawn_request(&json!({"prompt": "x", "roster_profile": "BUILDER"}))
        .expect("roster_profile alias should parse");
    assert_eq!(parsed.profile.as_deref(), Some("builder"));
}

#[test]
fn test_parse_spawn_request_rejects_invalid_profile_token() {
    for bad in [
        "rev iewer",
        "rev\"iewer",
        "rev'iewer",
        "rev`iewer",
        "rev=er",
    ] {
        let err = parse_spawn_request(&json!({"prompt": "x", "profile": bad}))
            .expect_err("invalid profile token should fail");
        assert!(
            err.to_string()
                .contains("profile must be a bare roster member id"),
            "{bad}: {err}"
        );
    }
}

#[test]
fn test_apply_spawn_profile_unknown_lists_available_members() {
    let roster = FleetRoster::built_ins_only();
    let mut request =
        parse_spawn_request(&json!({"prompt": "x", "profile": "warlock"})).expect("parse");
    let err = apply_spawn_profile(&mut request, &roster).expect_err("unknown profile should fail");
    let message = err.to_string();
    assert!(
        message.contains("Unknown fleet role/profile 'warlock'"),
        "{message}"
    );
    for member in [
        "manager",
        "scout",
        "builder",
        "reviewer",
        "verifier",
        "synthesizer",
        "general",
    ] {
        assert!(message.contains(member), "missing {member}: {message}");
    }
}

#[test]
fn test_apply_spawn_profile_rejects_conflicting_explicit_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "reviewer",
        "type": "implementer"
    }))
    .expect("parse");
    let err = apply_spawn_profile(&mut request, &roster).expect_err("type conflict should fail");
    let message = err.to_string();
    assert!(
        message.contains("profile 'reviewer' implies type review"),
        "{message}"
    );
    assert!(
        message.contains("conflicting explicit type 'implementer'"),
        "{message}"
    );
}

#[test]
fn test_apply_spawn_profile_accepts_agreeing_explicit_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({
        "prompt": "x",
        "profile": "reviewer",
        "type": "review"
    }))
    .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("agreeing type should pass")
        .expect("member resolved");
    assert_eq!(member.id, "reviewer");
    assert_eq!(request.agent_type, SubAgentType::Review);
    assert_eq!(request.assignment.role.as_deref(), Some("reviewer"));
}

#[test]
fn test_apply_spawn_profile_scout_yields_explore_type_and_inherits_route() {
    let roster = FleetRoster::built_ins_only();
    let mut request = parse_spawn_request(&json!({"prompt": "map the parser", "profile": "scout"}))
        .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("scout should resolve")
        .expect("member resolved");
    assert_eq!(request.agent_type, SubAgentType::Explore);
    let selected = resolve_spawn_model_selection(&stub_runtime(), &request, Some(&member))
        .expect("scout model selection");
    assert_eq!(
        selected.model_route,
        ModelRoute::Inherit,
        "without Fleet setup the scout inherits the active session model"
    );
    assert_eq!(selected.source, SpawnRouteSource::RunModel);
}

#[test]
fn test_apply_spawn_profile_synthesizer_yields_plan_type() {
    let roster = FleetRoster::built_ins_only();
    let mut request =
        parse_spawn_request(&json!({"prompt": "merge findings", "profile": "synthesizer"}))
            .expect("parse");
    apply_spawn_profile(&mut request, &roster).expect("synthesizer should resolve");
    assert_eq!(request.agent_type, SubAgentType::Plan);
}

#[test]
fn spawn_model_selection_has_stable_four_tier_precedence_and_source() {
    let mut runtime = stub_runtime();
    runtime.model = "deepseek-v4-flash".to_string();
    runtime
        .role_models
        .insert("reviewer".to_string(), "deepseek-v4-flash".to_string());

    let mut profile = custom_fleet_profile("reviewer");
    profile.model = Some("deepseek-v4-pro".to_string());
    let roster = fleet_roster_with("auditor", profile);
    let member = roster.get("auditor").expect("auditor profile");

    let request = parse_spawn_request(&json!({
        "prompt": "x",
        "role": "review",
        "model": "deepseek-v4-flash"
    }))
    .expect("task model request");
    let selected = resolve_spawn_model_selection(&runtime, &request, Some(member))
        .expect("task model selection");
    assert_eq!(
        selected,
        SpawnModelSelection {
            model_route: ModelRoute::Fixed("deepseek-v4-flash".to_string()),
            source: SpawnRouteSource::TaskModel,
        }
    );

    let request = parse_spawn_request(&json!({
        "prompt": "x",
        "role": "review",
        "model_strength": "faster"
    }))
    .expect("task strength request");
    let selected = resolve_spawn_model_selection(&runtime, &request, Some(member))
        .expect("task strength selection");
    assert_eq!(selected.model_route, ModelRoute::Faster);
    assert_eq!(selected.source, SpawnRouteSource::TaskModelStrength);

    let request =
        parse_spawn_request(&json!({"prompt": "x", "role": "review"})).expect("profile request");
    let selected =
        resolve_spawn_model_selection(&runtime, &request, Some(member)).expect("profile selection");
    assert_eq!(
        selected.model_route,
        ModelRoute::Fixed("deepseek-v4-pro".to_string()),
        "saved AgentProfile model must beat the configured role default"
    );
    assert_eq!(selected.source, SpawnRouteSource::AgentProfileModel);

    let mut strong_profile = custom_fleet_profile("reviewer");
    strong_profile.loadout = codewhale_config::FleetLoadout::Custom("strong".to_string());
    let strong_roster = fleet_roster_with("architect", strong_profile);
    let selected =
        resolve_spawn_model_selection(&runtime, &request, strong_roster.get("architect"))
            .expect("custom profile selection");
    assert_eq!(selected.model_route, ModelRoute::Inherit);
    assert_eq!(selected.source, SpawnRouteSource::RunModel);

    let mut fast_profile = custom_fleet_profile("reviewer");
    fast_profile.loadout = codewhale_config::FleetLoadout::Fast;
    let fast_roster = fleet_roster_with("fast-reviewer", fast_profile);
    let selected =
        resolve_spawn_model_selection(&runtime, &request, fast_roster.get("fast-reviewer"))
            .expect("fast profile selection");
    assert_eq!(selected.model_route, ModelRoute::Faster);
    assert_eq!(selected.source, SpawnRouteSource::AgentProfileLoadout);

    let selected =
        resolve_spawn_model_selection(&runtime, &request, None).expect("role default selection");
    assert_eq!(
        selected.model_route,
        ModelRoute::Fixed("deepseek-v4-flash".to_string())
    );
    assert_eq!(selected.source, SpawnRouteSource::RoleDefault);

    runtime.role_models.clear();
    let selected =
        resolve_spawn_model_selection(&runtime, &request, None).expect("run model selection");
    assert_eq!(selected.model_route, ModelRoute::Inherit);
    assert_eq!(selected.source, SpawnRouteSource::RunModel);
}

#[test]
fn test_child_max_spawn_depth_profile_hint_only_narrows() {
    // Profile hint narrows the inherited budget...
    assert_eq!(child_max_spawn_depth_for_spawn(3, 1, None, Some(1)), 2);
    // ...but never widens it.
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, None, Some(6)), 2);
    // Explicit request takes the min with the hint.
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, Some(3), Some(1)), 1);
    // Explicit request alone keeps its existing widen-up-to-ceiling semantics.
    assert_eq!(child_max_spawn_depth_for_spawn(2, 0, Some(3), None), 3);
    assert_eq!(
        child_max_spawn_depth_for_spawn(
            2,
            0,
            Some(codewhale_config::MAX_SPAWN_DEPTH_CEILING),
            None
        ),
        codewhale_config::MAX_SPAWN_DEPTH_CEILING
    );
    // Neither request nor hint: inherit unchanged.
    assert_eq!(child_max_spawn_depth_for_spawn(5, 2, None, None), 5);
}

#[test]
fn test_apply_spawn_profile_depth_hint_flows_from_member() {
    let mut profile = custom_fleet_profile("scout");
    profile.delegation.max_spawn_depth = Some(1);
    let roster = fleet_roster_with("recon", profile);
    let mut request =
        parse_spawn_request(&json!({"prompt": "x", "profile": "recon", "max_depth": 3}))
            .expect("parse");
    let member = apply_spawn_profile(&mut request, &roster)
        .expect("resolve")
        .expect("member resolved");
    let effective = child_max_spawn_depth_for_spawn(
        DEFAULT_MAX_SPAWN_DEPTH,
        1,
        request.max_depth,
        member.profile.delegation.max_spawn_depth,
    );
    assert_eq!(
        effective, 2,
        "hint 1 caps the requested 3 at spawn_depth 1 + 1"
    );
}

#[test]
fn test_apply_spawn_profile_appends_instruction_overlay() {
    let mut profile = custom_fleet_profile("reviewer");
    profile.role.description = Some("Security-focused reviewer.".to_string());
    profile.role.instructions = Some("Check unsafe blocks first.".to_string());
    let roster = fleet_roster_with("auditor", profile);
    let mut request =
        parse_spawn_request(&json!({"prompt": "audit the crate", "profile": "auditor"}))
            .expect("parse");
    apply_spawn_profile(&mut request, &roster).expect("resolve");
    assert!(
        request.prompt.starts_with("audit the crate"),
        "{}",
        request.prompt
    );
    assert!(
        request.prompt.contains("Fleet profile: auditor"),
        "{}",
        request.prompt
    );
    assert!(
        request
            .prompt
            .contains("Profile description:\nSecurity-focused reviewer."),
        "{}",
        request.prompt
    );
    assert!(
        request
            .prompt
            .contains("Profile instructions:\nCheck unsafe blocks first."),
        "{}",
        request.prompt
    );
    // Ledger objective keeps the original task; the overlay is prompt-only.
    assert_eq!(request.assignment.objective, "audit the crate");
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
    assert_eq!(parsed.fork_context, Some(true));

    let input = with_default_fork_context(
        json!({ "prompt": "fresh exploration", "fork_context": false }),
        true,
    );
    let parsed = parse_spawn_request(&input).expect("delegate override should parse");
    assert_eq!(parsed.fork_context, Some(false));
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
    let parent_message = Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "parent turn".to_string(),
            cache_control: None,
        }],
    };
    let fork_context = SubAgentForkContext {
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
        subagent_request_system_prompt("child system"),
        SystemPrompt::Text("child system".to_string())
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
        "role": "unknown role"
    });
    let err = parse_spawn_request(&input).expect_err("invalid role should fail");
    assert!(
        err.to_string()
            .contains("role must be a bare roster member id"),
        "{err}"
    );
}

#[test]
fn test_parse_spawn_request_accepts_fleet_role_token_for_runtime_resolution() {
    let input = json!({
        "prompt": "do work",
        "role": "release_lead"
    });
    let parsed = parse_spawn_request(&input).expect("fleet role token should parse");
    assert_eq!(parsed.agent_type, SubAgentType::General);
    assert!(!parsed.agent_type_explicit);
    assert_eq!(parsed.assignment.role.as_deref(), Some("release_lead"));
    assert_eq!(parsed.profile.as_deref(), Some("release_lead"));

    let roster = FleetRoster::built_ins_only();
    let mut parsed = parsed;
    let member = apply_spawn_profile(&mut parsed, &roster)
        .expect("release_lead should resolve")
        .expect("release_lead should select a roster member");
    assert_eq!(member.id, "manager");
    assert_eq!(parsed.profile.as_deref(), Some("manager"));

    let mut scout =
        parse_spawn_request(&json!({"prompt": "map it", "role": "scout"})).expect("scout");
    let member = apply_spawn_profile(&mut scout, &roster)
        .expect("scout should resolve")
        .expect("scout should select a roster member");
    assert_eq!(member.id, "scout");
    assert_eq!(scout.agent_type, SubAgentType::Explore);
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
        let mut parsed = parse_spawn_request(&input)
            .unwrap_or_else(|e| panic!("role {role:?} should parse, got {e}"));
        assert_eq!(parsed.agent_type, expected_type, "type for role {role:?}");
        assert_eq!(
            parsed.assignment.role.as_deref(),
            Some(expected_role),
            "canonical role for {role:?}"
        );
        assert!(
            parsed.profile.is_none(),
            "descriptive role alias {role:?} must not become a roster profile"
        );
        assert!(
            apply_spawn_profile(&mut parsed, &FleetRoster::built_ins_only())
                .unwrap_or_else(|e| panic!("role {role:?} should apply without a profile: {e}"))
                .is_none(),
            "descriptive role alias {role:?} should not require roster resolution"
        );
    }
}

#[test]
fn test_invalid_role_error_lists_real_aliases() {
    // Well-formed fleet role tokens parse and then fail clearly at roster
    // resolution time with both real roster members and type aliases (#4177).
    let roster = FleetRoster::built_ins_only();
    let input = json!({ "prompt": "do work", "role": "nonsense" });
    let mut request = parse_spawn_request(&input).expect("fleet role token should parse");
    let err = apply_spawn_profile(&mut request, &roster)
        .expect_err("unknown fleet role should fail at runtime resolution")
        .to_string();
    assert!(
        err.contains("Unknown fleet role/profile 'nonsense'"),
        "{err}"
    );
    assert!(err.contains("scout"), "hint should list scout: {err}");
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
        model_strength.contains("inherit the active model")
            && model_strength.contains("Choose faster explicitly"),
        "model_strength description should teach predictable default routing: {model_strength}"
    );
    let thinking = schema_property_description(&agent_schema, "thinking");
    assert!(
        thinking.contains("inherit") && thinking.contains("model_strength=faster"),
        "thinking description should teach child thinking control: {thinking}"
    );
    assert!(agent_schema["properties"].get("model").is_some());
    assert!(
        agent_schema["properties"].get("token_budget").is_none(),
        "ad-hoc children should inherit the generous runtime budget; exposing an optional cap invites accidental micromanagement"
    );
    let worktree = schema_property_description(&agent_schema, "worktree");
    assert!(
        worktree.contains("git worktree") && worktree.contains("parallel edit"),
        "worktree description should teach isolated parallel edits: {worktree}"
    );
    assert!(agent_schema["properties"].get("worktree_branch").is_some());
    assert!(agent_schema["properties"].get("worktree_path").is_some());
}

#[test]
fn agent_tool_prompt_schema_keeps_ordinary_starts_message_first() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 1);
    let agent_schema = AgentTool::new(manager, stub_runtime()).input_schema();
    let prompt = schema_property_description(&agent_schema, "prompt");
    assert!(prompt.contains("focused task"));
    assert!(prompt.contains("only field needed"));
    for ceremony in [
        "Subagent Brief",
        "QUESTION",
        "STOP_CONDITION",
        "ALREADY_KNOWN",
    ] {
        assert!(
            !prompt.contains(ceremony),
            "ordinary worker starts should not require structured brief ceremony {ceremony:?}: {prompt}"
        );
    }
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
        manager_guard.record_worker_event(
            &agent_id,
            AgentWorkerStatus::ModelWait,
            Some("step 1: requesting model response".to_string()),
            Some(1),
            None,
        );
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

    let second = tool
        .execute(
            json!({"action": "cancel", "agent_id": "agent_cancel_probe"}),
            &context,
        )
        .await
        .expect("repeated cancel stays idempotent");
    assert_eq!(second.metadata.as_ref().unwrap()["action"], json!("cancel"));
    let record = manager
        .read()
        .await
        .get_worker_record("agent_cancel_probe")
        .expect("worker record remains inspectable");
    assert_eq!(
        record
            .events
            .iter()
            .filter(|event| event.status == AgentWorkerStatus::Cancelled)
            .count(),
        1,
        "repeated stop must not append a second terminal outcome"
    );
}

#[tokio::test]
async fn model_wait_cancel_fans_in_once_and_preserves_checkpoint() {
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    let agent_id = "agent_model_wait_cancel".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "cancel while waiting on provider".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.current_session_boot_id.clone(),
    );
    agent.checkpoint = Some(make_checkpoint(
        &agent_id,
        1,
        vec![text_message("user", "request in flight")],
    ));
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.mailbox = Some(mailbox);
    runtime.event_tx = Some(event_tx);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
    manager.agents.insert(agent_id.clone(), agent);
    manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    manager.record_worker_event(
        &agent_id,
        AgentWorkerStatus::ModelWait,
        Some(SUBAGENT_MODEL_WAIT_REASON.to_string()),
        Some(1),
        None,
    );

    let first = manager.cancel_agent(&agent_id).expect("first Stop");
    let second = manager.cancel_agent(&agent_id).expect("repeated Stop");
    assert_eq!(first.status, SubAgentStatus::Cancelled);
    assert_eq!(second.status, SubAgentStatus::Cancelled);
    assert_eq!(
        first
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.reason.as_str()),
        Some("test_checkpoint")
    );

    let completion = completion_rx
        .try_recv()
        .expect("parent cancellation fan-in");
    assert!(completion.payload.contains(r#""status":"cancelled""#));
    assert!(completion_rx.try_recv().is_err());

    let terminal_mail = mailbox_rx
        .drain()
        .into_iter()
        .filter(|envelope| {
            matches!(
                envelope.message,
                MailboxMessage::Completed { .. }
                    | MailboxMessage::Failed { .. }
                    | MailboxMessage::Interrupted { .. }
                    | MailboxMessage::Cancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_mail.len(), 1);
    assert!(matches!(
        terminal_mail[0].message,
        MailboxMessage::Cancelled { ref agent_id } if agent_id == "agent_model_wait_cancel"
    ));

    let complete_events = std::iter::from_fn(|| event_rx.try_recv().ok())
        .filter(|event| matches!(event, Event::AgentComplete { .. }))
        .count();
    assert_eq!(complete_events, 1);
    let worker = manager.get_worker_record(&agent_id).expect("worker record");
    assert_eq!(worker.status, AgentWorkerStatus::Cancelled);
    assert_eq!(
        worker
            .events
            .iter()
            .filter(|event| event.status.is_terminal())
            .count(),
        1
    );
}

#[tokio::test]
async fn coordination_interrupt_fans_in_once_and_preserves_checkpoint() {
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    let agent_id = "agent_coordination_interrupt".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "interrupt with a recoverable checkpoint".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.current_session_boot_id.clone(),
    );
    agent.checkpoint = Some(make_checkpoint(
        &agent_id,
        2,
        vec![text_message("user", "resume this coordinated task")],
    ));
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.mailbox = Some(mailbox);
    runtime.event_tx = Some(event_tx);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
    manager.agents.insert(agent_id.clone(), agent);
    manager.register_worker(make_worker_spec("agent_parent", tmp.path().to_path_buf()));
    let mut child_spec = make_worker_spec(&agent_id, tmp.path().to_path_buf());
    child_spec.parent_run_id = Some("agent_parent".to_string());
    manager.register_worker(child_spec);
    manager.record_worker_event(
        &agent_id,
        AgentWorkerStatus::RunningTool,
        Some("step 2/8: running tool 'read_file'".to_string()),
        Some(2),
        Some("read_file".to_string()),
    );

    let reason = "parent rerouted this lane".to_string();
    let (prior, first) = manager
        .interrupt_child(&agent_id, Some("agent_parent"), reason.clone())
        .expect("first coordination interrupt");
    let (_, second) = manager
        .interrupt_child(&agent_id, Some("agent_parent"), reason.clone())
        .expect("repeated coordination interrupt");
    assert_eq!(prior.status, SubAgentStatus::Running);
    assert!(matches!(
        first.status,
        SubAgentStatus::Interrupted(ref actual) if actual == &reason
    ));
    assert_eq!(second.status, first.status);
    assert_eq!(
        first
            .checkpoint
            .as_ref()
            .map(|checkpoint| (checkpoint.reason.as_str(), checkpoint.steps_taken)),
        Some(("test_checkpoint", 2))
    );

    let completion = completion_rx
        .try_recv()
        .expect("parent interruption fan-in");
    assert!(completion.payload.contains(r#""status":"interrupted""#));
    assert!(completion.payload.contains(&reason));
    assert!(completion_rx.try_recv().is_err());

    let terminal_mail = mailbox_rx
        .drain()
        .into_iter()
        .filter(|envelope| {
            matches!(
                envelope.message,
                MailboxMessage::Completed { .. }
                    | MailboxMessage::Failed { .. }
                    | MailboxMessage::Interrupted { .. }
                    | MailboxMessage::Cancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_mail.len(), 1);
    assert!(matches!(
        terminal_mail[0].message,
        MailboxMessage::Interrupted {
            ref agent_id,
            ref reason
        } if agent_id == "agent_coordination_interrupt" && reason == "parent rerouted this lane"
    ));

    let complete_events = std::iter::from_fn(|| event_rx.try_recv().ok())
        .filter(|event| matches!(event, Event::AgentComplete { .. }))
        .count();
    assert_eq!(complete_events, 1);
    let worker = manager.get_worker_record(&agent_id).expect("worker record");
    assert_eq!(worker.status, AgentWorkerStatus::WaitingForUser);
    assert_eq!(
        worker
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.status,
                    AgentWorkerStatus::WaitingForUser | AgentWorkerStatus::Interrupted
                )
            })
            .count(),
        1,
        "repeated interrupt must not append a second terminal or parked outcome"
    );
}

#[tokio::test]
async fn late_completion_does_not_overwrite_cancelled_outcome() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    let agent_id = "agent_cancel_completion_race".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "race".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.current_session_boot_id.clone(),
    );
    manager.agents.insert(agent_id.clone(), agent);
    manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));

    manager.cancel_agent(&agent_id).expect("cancel wins race");
    let mut late = manager
        .get_result(&agent_id)
        .expect("cancelled snapshot exists");
    late.status = SubAgentStatus::Completed;
    late.result = Some("late success".to_string());
    assert!(
        !manager.update_from_result(&agent_id, late),
        "late completion must lose the terminal transition"
    );

    let snapshot = manager
        .get_result(&agent_id)
        .expect("terminal snapshot remains");
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
    assert_eq!(
        snapshot.result.as_deref(),
        Some("Cancelled by parent request.")
    );
    let record = manager
        .get_worker_record(&agent_id)
        .expect("worker record remains");
    let terminal = record
        .events
        .iter()
        .filter(|event| event.status.is_terminal())
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].status, AgentWorkerStatus::Cancelled);
}

#[tokio::test]
async fn completion_claim_preserves_running_gate_and_excludes_late_cancel() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    let agent_id = "agent_completion_claim".to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "claim".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        tmp.path().to_path_buf(),
        manager.current_session_boot_id.clone(),
    );
    agent.task_handle = Some(tokio::spawn(async {}));
    manager.agents.insert(agent_id.clone(), agent);
    manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));

    assert!(manager.claim_terminal_delivery(&agent_id));
    assert_eq!(manager.running_count(), 1);
    assert_eq!(
        manager.get_result(&agent_id).unwrap().status,
        SubAgentStatus::Running,
        "claimed completion must keep the running-child gate open until delivery"
    );
    assert_eq!(
        manager.cancel_agent(&agent_id).unwrap().status,
        SubAgentStatus::Running,
        "cancellation after the claim must not steal terminal ownership"
    );

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let runtime = runtime_with_depth(1, Some(completion_tx));
    assert!(emit_parent_completion(
        &runtime,
        &agent_id,
        "summary\n<sentinel/>"
    ));
    assert_eq!(
        completion_rx.try_recv().unwrap().agent_id,
        agent_id,
        "parent completion must be queued before closing Running"
    );
    assert_eq!(
        manager.get_result(&agent_id).unwrap().status,
        SubAgentStatus::Running
    );
    assert_eq!(
        manager.running_count(),
        1,
        "child remains counted until parent delivery is queued"
    );

    let mut result = manager.get_result(&agent_id).unwrap();
    result.status = SubAgentStatus::Completed;
    result.result = Some("done".to_string());
    assert!(manager.update_from_result(&agent_id, result));
    assert_eq!(
        manager.get_result(&agent_id).unwrap().status,
        SubAgentStatus::Completed
    );
    assert_eq!(manager.running_count(), 0);
    let terminal = manager
        .get_worker_record(&agent_id)
        .unwrap()
        .events
        .iter()
        .filter(|event| event.status.is_terminal())
        .count();
    assert_eq!(terminal, 1, "exactly one terminal outcome is recorded");
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
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = false;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Explore,
        Some(vec![
            "read_file".to_string(),
            "update_goal".to_string(),
            "missing_tool".to_string(),
        ]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    assert_eq!(
        registry.unavailable_allowed_tools(),
        vec!["update_goal".to_string(), "missing_tool".to_string()]
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

/// Return the exact model-visible catalog built for a default General child.
///
/// Request-boundary tests use this fixture so Moonshot compatibility coverage
/// cannot drift into a hand-maintained approximation of the child surface.
pub(crate) fn kimi_general_child_request_tools_fixture() -> Vec<Tool> {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );
    let tools = registry.tools_for_model(&SubAgentType::General);
    let names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();

    assert!(names.contains("get_goal"));
    assert!(!names.contains("create_goal"));
    assert!(!names.contains("update_goal"));
    tools
}

fn disabled_feature_agent_surface_options() -> AgentToolSurfaceOptions {
    let mut options = AgentToolSurfaceOptions::new(ShellPolicy::Full);
    options.goal_state = Some(crate::tools::goal::new_shared_goal_state());
    options
}

#[test]
fn subagent_general_catalog_keeps_parent_surface_except_root_goal_mutators() {
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
    let expected_child_names = parent_names
        .iter()
        .filter(|name| !matches!(name.as_str(), "create_goal" | "update_goal"))
        .cloned()
        .collect::<HashSet<_>>();

    assert!(parent_names.contains("create_goal"));
    assert!(parent_names.contains("update_goal"));
    assert!(child_names.contains("get_goal"));
    assert!(!child_names.contains("create_goal"));
    assert!(!child_names.contains("update_goal"));
    assert_eq!(
        child_names, expected_child_names,
        "default General sub-agent catalog must match the parent Agent surface except for root-owned goal mutators"
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

    let tools = registry.tools_for_model(&SubAgentType::Explore);
    let names = tool_names(tools.clone());
    for name in ["File", "Git", "Web", "web.run"] {
        assert!(names.contains(name), "Explore should inherit {name}");
    }
    for name in [
        "Bash",
        "Run",
        "write_file",
        "edit_file",
        "apply_patch",
        "fim_edit",
        "exec_shell",
        "task_shell_start",
    ] {
        assert!(!names.contains(name), "Explore must hide {name}");
    }
    let file = tools.iter().find(|tool| tool.name == "File").unwrap();
    assert_eq!(
        file.input_schema["properties"]["action"]["enum"],
        json!(["read", "list", "search_name", "search_content"])
    );
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

    let tools = registry.tools_for_model(&SubAgentType::Implementer);
    let names = tool_names(tools.clone());
    for name in ["File", "fim_edit"] {
        assert!(
            names.contains(name),
            "Implementer should inherit write-capable tool {name}"
        );
    }
    let file = tools.iter().find(|tool| tool.name == "File").unwrap();
    let actions = file.input_schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap();
    for action in ["write", "edit", "patch"] {
        assert!(actions.iter().any(|value| value.as_str() == Some(action)));
    }
}

#[tokio::test]
async fn plan_parent_profile_narrows_even_implementer_child_to_read_only() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime =
        stub_runtime().with_agent_tool_surface_options(enabled_agent_surface_options());
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = true;
    runtime.allow_shell = false;
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Plan);
    runtime.agent_tool_surface_options.shell_policy = ShellPolicy::None;

    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::Implementer,
        None,
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );

    let names = tool_names(registry.tools_for_model(&SubAgentType::Implementer));
    assert!(names.contains("agent"), "Plan children may still delegate");
    for name in [
        "write_file",
        "edit_file",
        "apply_patch",
        "fim_edit",
        "exec_shell",
        "task_shell_start",
    ] {
        assert!(
            !names.contains(name),
            "Plan parent profile must hide child capability {name}"
        );
    }

    let err = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "plan-parent-write.txt", "content": "denied"}),
        )
        .await
        .expect_err("Plan parent profile must block writes even for implementer children");
    assert!(
        err.to_string().contains("not permitted"),
        "expected posture rejection, got: {err}"
    );
    assert!(!workspace.join("plan-parent-write.txt").exists());
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
        wall_time: DEFAULT_CHILD_WALL_TIME,
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

#[test]
fn transient_provider_classifier_matches_structured_rate_limit() {
    let err = anyhow::Error::new(crate::llm_client::LlmError::RateLimited {
        message: "please slow down".to_string(),
        retry_after: Some(Duration::from_secs(2)),
    })
    .context("Responses API request failed");

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
        wall_time: DEFAULT_CHILD_WALL_TIME,
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
async fn subagent_rate_limit_exhaustion_interrupts_with_checkpoint() {
    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let agent_id = "agent_rate_limited_checkpoint".to_string();
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "Inspect rate-limit recovery".to_string(),
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

    let (client, calls) = always_rate_limited_chat_client().await;
    let mut runtime = stub_runtime().with_step_api_timeout(Duration::from_secs(5));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());

    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Inspect rate-limit recovery".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 3,
        token_budget: None,
        wall_time: DEFAULT_CHILD_WALL_TIME,
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
        SUBAGENT_TRANSIENT_PROVIDER_MAX_RETRIES.saturating_add(1) as usize,
        "rate-limit retries should be owned by the sub-agent retry loop"
    );
    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("agent should stay registered")
    };
    let SubAgentStatus::Interrupted(reason) = &snapshot.status else {
        panic!("expected interrupted sub-agent, got {:?}", snapshot.status);
    };
    assert!(
        reason.contains("rate-limited provider response"),
        "reason should name the provider rate limit: {reason}"
    );
    let checkpoint = snapshot
        .checkpoint
        .as_ref()
        .expect("rate-limit interruption should preserve checkpoint");
    assert_eq!(checkpoint.reason, "api_rate_limited");
    assert!(checkpoint.continuable);
    assert!(snapshot.needs_input.is_some());
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
async fn shared_write_claim_is_registered_before_parallel_launch_and_manifested() {
    let tmp = tempdir().expect("tempdir");
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
    let mut runtime = stub_runtime();
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    let options = SubAgentSpawnOptions {
        name: Some("writer-a".into()),
        write_claim: Some(WriteScopeClaim {
            owner: String::new(),
            roots: vec!["src".into()],
            exact_files: vec![],
            contracts: vec!["public-api".into()],
        }),
        expected_artifact: Some("tested patch".into()),
        ..Default::default()
    };
    let (first_id, contention) = {
        let mut guard = manager.write().await;
        let first = guard
            .spawn_background_with_assignment_options(
                Arc::clone(&manager),
                runtime.clone(),
                SubAgentType::Implementer,
                "edit src".into(),
                make_assignment(),
                Some(vec![]),
                options,
            )
            .expect("first writer admitted");
        let second = guard
            .spawn_background_with_assignment_options(
                Arc::clone(&manager),
                runtime,
                SubAgentType::Implementer,
                "edit same contract".into(),
                make_assignment(),
                Some(vec![]),
                SubAgentSpawnOptions {
                    name: Some("writer-b".into()),
                    write_claim: Some(WriteScopeClaim {
                        owner: String::new(),
                        roots: vec!["docs".into()],
                        exact_files: vec![],
                        contracts: vec!["public-api".into()],
                    }),
                    ..Default::default()
                },
            )
            .expect_err("overlapping live contract must contend");
        (first.agent_id, second.to_string())
    };
    assert!(
        contention.contains("contention") && contention.contains(&first_id),
        "{contention}"
    );
    let guard = manager.read().await;
    let record = guard.get_worker_record(&first_id).expect("worker record");
    let manifest = record
        .spec
        .launch_manifest
        .as_ref()
        .expect("launch manifest");
    assert_eq!(manifest.child_id, first_id);
    assert_eq!(manifest.writable_roots, vec!["src"]);
    assert_eq!(manifest.coordination_contracts, vec!["public-api"]);
    assert_eq!(manifest.expected_artifact.as_deref(), Some("tested patch"));
}

#[tokio::test]
async fn write_capable_agent_does_not_launch_when_durable_registration_fails() {
    let tmp = tempdir().expect("tempdir");
    let blocked_state_path = tmp.path().join("blocked-state.json");
    std::fs::create_dir(&blocked_state_path).expect("directory blocks atomic state rename");
    let manager = Arc::new(RwLock::new(
        SubAgentManager::new(tmp.path().to_path_buf(), 4)
            .with_state_path(blocked_state_path.clone()),
    ));
    let mut runtime = stub_runtime();
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());

    let error = manager
        .write()
        .await
        .spawn_background_with_assignment_options(
            Arc::clone(&manager),
            runtime,
            SubAgentType::Implementer,
            "must never execute".into(),
            make_assignment(),
            Some(vec![]),
            SubAgentSpawnOptions {
                name: Some("durable-writer".into()),
                write_claim: Some(WriteScopeClaim {
                    owner: String::new(),
                    roots: vec!["src".into()],
                    exact_files: Vec::new(),
                    contracts: Vec::new(),
                }),
                ..Default::default()
            },
        )
        .expect_err("writer must fail before spawn when its durable claim cannot commit")
        .to_string();
    assert!(error.contains("durably register"), "{error}");

    let guard = manager.read().await;
    assert!(guard.agents.is_empty(), "no child task was admitted");
    assert!(
        guard.list_worker_records().is_empty(),
        "failed durable registration rolls back worker identity"
    );
    assert!(
        guard.coordination_snapshot().write_claims.is_empty(),
        "failed durable registration rolls back write ownership"
    );
    assert!(blocked_state_path.is_dir());
}

#[tokio::test]
async fn write_scope_contention_covers_regular_agent_and_active_fleet_writer() {
    let tmp = tempdir().expect("tempdir");
    let mut inner = SubAgentManager::new(tmp.path().to_path_buf(), 4);
    inner
        .register_worker_with_coordination(make_write_worker_spec(
            "fleet-writer",
            tmp.path().to_path_buf(),
            "src/shared",
        ))
        .expect("active Fleet writer claim");

    let regular_id = inner.insert_test_running_agent("regular", tmp.path());
    inner
        .coordination
        .register_claim(
            WriteScopeClaim {
                owner: regular_id.clone(),
                roots: vec!["docs".into()],
                exact_files: Vec::new(),
                contracts: Vec::new(),
            },
            false,
            |_| false,
        )
        .expect("regular agent initial claim");
    let expansion = inner
        .expand_write_claim(
            &regular_id,
            vec!["src/shared/api".into()],
            Vec::new(),
            Vec::new(),
        )
        .expect_err("regular-agent scope expansion must see active Fleet ownership");
    assert!(
        expansion.contains("fleet-writer") && expansion.contains("contention"),
        "{expansion}"
    );

    let manager = Arc::new(RwLock::new(inner));
    let mut runtime = stub_runtime();
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    let launch = manager
        .write()
        .await
        .spawn_background_with_assignment_options(
            Arc::clone(&manager),
            runtime,
            SubAgentType::Implementer,
            "edit Fleet-owned scope".into(),
            make_assignment(),
            Some(vec![]),
            SubAgentSpawnOptions {
                name: Some("regular-writer".into()),
                write_claim: Some(WriteScopeClaim {
                    owner: String::new(),
                    roots: vec!["src/shared".into()],
                    exact_files: Vec::new(),
                    contracts: Vec::new(),
                }),
                ..Default::default()
            },
        )
        .expect_err("regular-agent launch must see active Fleet ownership");
    let launch = launch.to_string();
    assert!(
        launch.contains("fleet-writer") && launch.contains("contention"),
        "{launch}"
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
    use tokio_util::sync::CancellationToken;

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
    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.mailbox = Some(mailbox);
    runtime.event_tx = Some(event_tx);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
    manager.agents.insert(agent_id.clone(), agent);
    manager.register_worker(make_worker_spec(&agent_id, PathBuf::from(".")));
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
    let completion = completion_rx
        .try_recv()
        .expect("stale cleanup should wake the immediate parent");
    assert_eq!(completion.agent_id, agent_id);
    assert!(completion.payload.contains(r#""status":"cancelled""#));
    assert!(completion_rx.try_recv().is_err());
    assert!(matches!(
        mailbox_rx.drain().as_slice(),
        [MailboxEnvelope {
            message: MailboxMessage::Cancelled { agent_id: id },
            ..
        }] if id == &agent_id
    ));
    assert!(matches!(
        event_rx.try_recv(),
        Ok(Event::AgentComplete { id, result })
            if id == agent_id && result.contains(r#""status":"cancelled""#)
    ));
    assert_eq!(
        manager.get_worker_record(&agent_id).unwrap().status,
        AgentWorkerStatus::Cancelled
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
    let result =
        inspect_agent_from_input(&json!({"action": "status"}), manager, &context, false, None)
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
fn generated_whale_name_is_not_persisted_or_replayed_on_load() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");
    let mut manager =
        SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path.clone());
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_locale_neutral";
    let generated = whale_name_for_id_in_locale(agent_id, "ja");
    let mut agent = SubAgent::new(
        agent_id.to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some(generated.clone()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    agent.session_name = "docs-worker".to_string();
    manager.agents.insert(agent.id.clone(), agent);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let mut persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read persisted state"))
            .expect("parse persisted state");
    assert!(
        persisted["agents"][0].get("nickname").is_none(),
        "generated locale text is not durable identity"
    );

    // Recreate a pre-fix state file whose generated display came from a
    // Japanese session. Loading under a later session must discard it.
    persisted["agents"][0]["nickname"] = json!(generated);
    std::fs::write(
        &state_path,
        serde_json::to_string_pretty(&persisted).expect("serialize legacy state"),
    )
    .expect("write legacy state");

    let mut reloaded = SubAgentManager::new(workspace, 2).with_state_path(state_path);
    reloaded.load_state().expect("load legacy state");
    let snapshot = reloaded
        .get_result(agent_id)
        .expect("neutral id survives load");
    assert_eq!(snapshot.agent_id, "agent_locale_neutral");
    assert_eq!(snapshot.name, "docs-worker");
    assert_eq!(snapshot.nickname, None);
}

#[test]
fn explicit_nonmatching_whale_word_is_persisted_and_loaded() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");
    let agent_id = "agent_explicit_whale_word";
    let explicit_whale = built_in_whale_name_that_cannot_be_generated_for(agent_id);
    assert!(generated_whale_name_base(agent_id, explicit_whale).is_none());

    let mut manager =
        SubAgentManager::new(workspace.clone(), 2).with_state_path(state_path.clone());
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let agent = SubAgent::new(
        agent_id.to_string(),
        SubAgentType::General,
        "work".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        Some(explicit_whale.to_string()),
        Some(vec!["read_file".to_string()]),
        input_tx,
        PathBuf::from("."),
        "boot_test".to_string(),
    );
    manager.agents.insert(agent.id.clone(), agent);
    manager
        .persist_state()
        .expect("persist state")
        .join()
        .expect("persist thread");

    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read persisted state"))
            .expect("parse persisted state");
    assert_eq!(
        persisted["agents"][0]["nickname"],
        json!(explicit_whale),
        "the explicit whale-word nickname remains durable"
    );

    let mut reloaded = SubAgentManager::new(workspace, 2).with_state_path(state_path);
    reloaded.load_state().expect("load state");
    let snapshot = reloaded.get_result(agent_id).expect("agent survives load");
    assert_eq!(snapshot.nickname.as_deref(), Some(explicit_whale));
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

#[test]
fn restart_reconciles_every_orphan_execution_status_once_and_preserves_receipts() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let state_path = default_state_path(tmp.path()).expect("default state path");
    let mut manager =
        SubAgentManager::new(workspace.clone(), 8).with_state_path(state_path.clone());

    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut running = SubAgent::new(
        "agent_restart_model_wait".to_string(),
        SubAgentType::General,
        "resume after restart".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        workspace.clone(),
        "boot_before_restart".to_string(),
    );
    running.checkpoint = Some(make_checkpoint(
        &running.id,
        3,
        vec![
            text_message("user", "original assignment"),
            text_message("assistant", "partial checkpoint"),
        ],
    ));
    manager.agents.insert(running.id.clone(), running);

    let orphan_statuses = [
        ("agent_restart_queued", AgentWorkerStatus::Queued),
        ("agent_restart_starting", AgentWorkerStatus::Starting),
        ("agent_restart_running", AgentWorkerStatus::Running),
        ("agent_restart_model_wait", AgentWorkerStatus::ModelWait),
        ("agent_restart_running_tool", AgentWorkerStatus::RunningTool),
    ];
    for (worker_id, status) in orphan_statuses {
        manager.register_worker(make_worker_spec(worker_id, workspace.clone()));
        if status != AgentWorkerStatus::Starting {
            manager.record_worker_event(
                worker_id,
                status,
                Some(agent_worker_status_name(status).to_string()),
                Some(3),
                None,
            );
        }
    }

    manager.register_worker(make_worker_spec("agent_restart_waiting", workspace.clone()));
    manager.record_worker_event(
        "agent_restart_waiting",
        AgentWorkerStatus::WaitingForUser,
        Some("waiting for user follow-up".to_string()),
        Some(2),
        None,
    );
    manager.register_worker(make_worker_spec(
        "agent_restart_completed",
        workspace.clone(),
    ));
    let mut completed = make_snapshot(SubAgentStatus::Completed);
    completed.agent_id = "agent_restart_completed".to_string();
    completed.name = completed.agent_id.clone();
    completed.result = Some("durable terminal receipt".to_string());
    manager.complete_worker_from_result(&completed.agent_id, &completed);
    let waiting_events = manager
        .get_worker_record("agent_restart_waiting")
        .unwrap()
        .events;
    let completed_events = manager
        .get_worker_record("agent_restart_completed")
        .unwrap()
        .events;

    manager
        .persist_state()
        .expect("persist restart fixture")
        .join()
        .expect("persist thread");

    let mut reloaded =
        SubAgentManager::new(workspace.clone(), 8).with_state_path(state_path.clone());
    reloaded.load_state().expect("load restart fixture");

    let restored = reloaded
        .get_result("agent_restart_model_wait")
        .expect("restored agent");
    assert!(matches!(
        restored.status,
        SubAgentStatus::Interrupted(ref reason) if reason == SUBAGENT_RESTART_REASON
    ));
    let checkpoint = restored.checkpoint.expect("checkpoint survives restart");
    assert_eq!(checkpoint.steps_taken, 3);
    assert_eq!(message_text(&checkpoint.messages[1]), "partial checkpoint");

    for (worker_id, _) in orphan_statuses {
        let worker = reloaded
            .get_worker_record(worker_id)
            .expect("orphan worker");
        assert_eq!(worker.status, AgentWorkerStatus::Interrupted, "{worker_id}");
        assert_eq!(
            worker
                .events
                .iter()
                .filter(|event| event.status == AgentWorkerStatus::Interrupted)
                .count(),
            1,
            "{worker_id} gets one restart terminal receipt"
        );
    }
    assert_eq!(
        reloaded
            .get_worker_record("agent_restart_waiting")
            .unwrap()
            .events,
        waiting_events,
        "waiting-for-user is not an orphan execution state"
    );
    assert_eq!(
        reloaded
            .get_worker_record("agent_restart_completed")
            .unwrap()
            .events,
        completed_events,
        "terminal receipts remain byte-for-byte intact"
    );

    let event_counts = orphan_statuses.map(|(worker_id, _)| {
        reloaded
            .get_worker_record(worker_id)
            .expect("reconciled worker")
            .events
            .len()
    });
    assert_eq!(
        reloaded.reconcile_orphaned_workers_after_restart(),
        0,
        "repeat reconciliation is idempotent"
    );
    assert_eq!(
        orphan_statuses.map(|(worker_id, _)| {
            reloaded
                .get_worker_record(worker_id)
                .expect("reconciled worker")
                .events
                .len()
        }),
        event_counts
    );

    reloaded
        .persist_state()
        .expect("persist reconciled state")
        .join()
        .expect("persist thread");
    let mut loaded_again = SubAgentManager::new(workspace, 8).with_state_path(state_path);
    loaded_again.load_state().expect("load reconciled state");
    assert_eq!(
        orphan_statuses.map(|(worker_id, _)| {
            loaded_again
                .get_worker_record(worker_id)
                .expect("persisted reconciled worker")
                .events
                .len()
        }),
        event_counts,
        "a later restart does not append duplicate interrupted receipts"
    );
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
fn parse_spawn_request_accepts_cwd_with_worktree_isolation() {
    let input = json!({
        "prompt": "build feature A",
        "cwd": ".worktrees/manual",
        "worktree": true
    });
    let parsed = parse_spawn_request(&input).expect("cwd and worktree may be combined");
    assert!(parsed.worktree.is_some());
    assert!(parsed.cwd.is_some());
}

#[test]
fn git_repo_root_finds_repo_from_direct_cwd() {
    let repo = init_subagent_git_repo();
    let root = git_repo_root(repo.path()).expect("direct repo cwd should resolve");
    assert_eq!(
        root.canonicalize().expect("canonical root"),
        repo.path().canonicalize().expect("canonical repo")
    );
}

#[test]
fn git_repo_root_discovers_one_level_nested_repo_from_harness() {
    let repo = init_subagent_git_repo();
    let harness = tempdir().expect("harness dir");
    let nested = harness.path().join("CodeWhale");
    Command::new("git")
        .args([
            "clone",
            repo.path().to_str().unwrap(),
            nested.to_str().unwrap(),
        ])
        .output()
        .expect("clone nested repo");
    let root = git_repo_root(harness.path()).expect("harness cwd should discover nested repo");
    assert_eq!(
        root.canonicalize().expect("canonical root"),
        nested.canonicalize().expect("canonical nested")
    );
}

#[test]
fn git_repo_root_reports_attempted_paths_when_no_repo_found() {
    let repo_root = git_repo_root(&std::env::current_dir().expect("current dir"))
        .expect("test should run inside the checkout");
    let harness = TempDirBuilder::new()
        .prefix(".codewhale-no-repo-")
        .tempdir_in(repo_root.parent().expect("repo parent"))
        .expect("empty harness outside checkout");
    let empty = harness
        .path()
        .join("isolated")
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("empty");
    std::fs::create_dir_all(&empty).expect("empty nested dir");
    let expected = empty.canonicalize().expect("canonical empty dir");
    let err = git_repo_root(&empty).expect_err("missing repo should fail cleanly");
    let message = err.to_string();
    assert!(
        message.contains("Tried:") && message.contains(expected.to_string_lossy().as_ref()),
        "expected friendly attempted-path error, got: {message}"
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

fn init_git_repo_at(path: &std::path::Path) {
    let init = Command::new("git")
        .arg("init")
        .current_dir(path)
        .output()
        .expect("git init should run");
    assert!(init.status.success(), "git init failed");
    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ])
        .current_dir(path)
        .output()
        .expect("git commit should run");
    assert!(commit.status.success(), "git commit failed");
}

#[test]
fn create_isolated_worktree_discovers_nested_repo_from_harness_parent() {
    let harness = tempdir().expect("harness");
    let nested = harness.path().join("CodeWhale");
    std::fs::create_dir_all(&nested).expect("nested checkout dir");
    init_git_repo_at(&nested);
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-harness-nested".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let path = create_isolated_worktree(
        harness.path(),
        &request,
        Some("harness-nested"),
        &SubAgentType::Explore,
    )
    .expect("harness parent should discover nested repo");

    assert!(path.exists(), "worktree path should exist");
    assert_eq!(
        current_git_branch(&path).as_deref(),
        Some("codex/agent-harness-nested")
    );
}

#[test]
fn create_isolated_worktree_reports_friendly_error_when_no_repo_found() {
    let harness = tempdir().expect("harness");
    // Keep the probe more than `git_repo_root`'s parent-search limit below
    // the temporary root. Containerized CI commonly checks the repository out
    // at `/workspace`; a shallow `/tmp` fixture can otherwise reach `/` and
    // correctly discover that sibling checkout instead of exercising the
    // no-repository path.
    let no_repo = harness
        .path()
        .join("not-a-repo")
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("empty");
    std::fs::create_dir_all(&no_repo).expect("mkdir");
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-missing".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(&no_repo, &request, None, &SubAgentType::General)
        .expect_err("missing repo should fail with friendly error");

    let message = err.to_string();
    assert!(
        message.contains("requires a git repository") && message.contains("Tried:"),
        "expected actionable discovery error, got: {message}"
    );
}

#[test]
fn create_isolated_worktree_rejects_ambiguous_nested_repos() {
    let harness = tempdir().expect("harness");
    for name in ["RepoA", "RepoB"] {
        let nested = harness.path().join(name);
        std::fs::create_dir_all(&nested).expect("nested dir");
        init_git_repo_at(&nested);
    }
    let worktree_home = tempdir().expect("worktree home");
    let request = SubAgentWorktreeRequest {
        branch: Some("codex/agent-ambiguous".to_string()),
        path: Some(worktree_home.path().join("isolated")),
        base_ref: None,
    };

    let err = create_isolated_worktree(harness.path(), &request, None, &SubAgentType::General)
        .expect_err("multiple nested repos should fail deterministically");

    let message = err.to_string();
    assert!(
        message.contains("Multiple git repositories found"),
        "expected ambiguity diagnostic, got: {message}"
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
fn fresh_forked_and_nested_subagents_share_authority_bound_skill_catalogs() {
    let _env = crate::test_support::lock_test_env();
    let tmp = tempdir().expect("tempdir");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path().join("home"));
    let workspace = tmp.path().join("workspace");
    let native_skill = workspace.join(".agents/skills/native-review");
    let plugin_root = workspace.join(".codewhale/plugins/demo");
    std::fs::create_dir_all(&native_skill).expect("native Skill dir");
    std::fs::create_dir_all(plugin_root.join("skills/review")).expect("plugin Skill dir");
    std::fs::write(
        native_skill.join("SKILL.md"),
        "---\nname: native-review\ndescription: native workspace review\n---\nbody\n",
    )
    .expect("native Skill");
    std::fs::write(
        plugin_root.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n[skills]\npath = \"skills\"\n",
    )
    .expect("plugin manifest");
    std::fs::write(
        plugin_root.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: reviewed plugin review\n---\nbody\n",
    )
    .expect("plugin Skill");
    let config = crate::plugins::discovery::DiscoveryConfig {
        workspace: workspace.clone(),
        user_plugins_dir: tmp.path().join("user-plugins"),
        workspace_plugins_dir: workspace.join(".codewhale/plugins"),
        builtin_plugin_dirs: Vec::new(),
        state_path: tmp.path().join("plugin-state/state.json"),
    };
    let mut plugins = crate::plugins::discovery::discover_with_config(&config);
    plugins.trust("demo").expect("trust plugin");
    plugins.enable("demo").expect("enable plugin");
    let context = ToolContext::new(&workspace).with_plugin_registry(Arc::new(plugins));
    let assignment = SubAgentAssignment::new("review".to_string(), None);
    let system =
        build_subagent_system_prompt_with_skills(&SubAgentType::Review, &assignment, &context);

    assert!(system.contains("`native-review`"), "{system}");
    assert!(system.contains("`demo:review`"), "{system}");
    assert!(system.contains("reviewed plugin demo id="), "{system}");
    assert!(system.contains("generation="), "{system}");
    assert!(
        !system.contains(&plugin_root.display().to_string()),
        "{system}"
    );
    assert_eq!(
        subagent_request_system_prompt(&system),
        SystemPrompt::Text(system.clone()),
        "fresh children receive the catalog at system precedence"
    );

    let fork_context = SubAgentForkContext {
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "parent".to_string(),
                cache_control: None,
            }],
        }],
        structured_state_block: None,
    };
    let forked = build_initial_subagent_messages_with_system(
        "review",
        &assignment,
        &SubAgentType::Review,
        &system,
        Some(&fork_context),
    );
    assert!(
        forked
            .iter()
            .filter(|message| message.role == "system")
            .any(|message| message_text(message).contains("`demo:review`")),
        "forked children must receive the same resolved catalog"
    );

    let mut direct_child = runtime_with_depth(1, None);
    direct_child.context = context.clone();
    let (nested_runtime, _nested_rx) = runtime_for_nested_agent_tools(
        &direct_child,
        "agent_parent",
        SubAgentForkContext {
            messages: Vec::new(),
            structured_state_block: None,
        },
    );
    let nested_system = build_subagent_system_prompt_with_skills(
        &SubAgentType::Review,
        &assignment,
        &nested_runtime.context,
    );
    assert!(nested_system.contains("`demo:review`"), "{nested_system}");

    let isolated_workspace = tmp.path().join("isolated-worktree");
    std::fs::create_dir_all(&isolated_workspace).expect("isolated worktree");
    let isolated_plugins = context
        .plugin_registry
        .as_ref()
        .expect("plugin registry")
        .rediscover_for_workspace(&isolated_workspace);
    let isolated = ToolContext::new(&isolated_workspace).with_plugin_registry(isolated_plugins);
    let isolated_system =
        build_subagent_system_prompt_with_skills(&SubAgentType::Review, &assignment, &isolated);
    assert!(
        !isolated_system.contains("`demo:review`"),
        "workspace plugin authority must not leak into another worktree: {isolated_system}"
    );
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

    let provider = crate::config::ApiProvider::OpenaiCodex;
    let route = ModelRoute::Fixed("gpt-5.5-codex".to_string());
    let annotated = annotate_child_model_error(
        &subagent_failure_message(&err),
        "gpt-5.5-codex",
        provider,
        &route,
    );

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
    // #4049: the failure now names the provider and the route source.
    assert!(annotated.contains(provider.display_name()), "{annotated}");
    assert!(annotated.contains("route:"), "{annotated}");
    assert!(annotated.contains("explicit model id"), "{annotated}");
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
    let provider = crate::config::ApiProvider::Moonshot;
    let inherit = ModelRoute::Inherit;
    let auth = annotate_child_model_error("403 Forbidden", "kimi-k2", provider, &inherit);
    assert!(auth.contains("kimi-k2"), "names the model: {auth}");
    assert!(
        auth.contains("child model override"),
        "names the recovery path: {auth}"
    );
    assert!(
        auth.contains("403 Forbidden"),
        "preserves the original: {auth}"
    );
    // #4049: provider + route source are named in the hint.
    assert!(auth.contains(provider.display_name()), "{auth}");
    assert!(auth.contains("inherited from the parent"), "{auth}");

    // Unrelated errors still pass through completely unchanged (no provider
    // /route noise on a network failure).
    let unrelated =
        annotate_child_model_error("connection reset by peer", "kimi-k2", provider, &inherit);
    assert_eq!(unrelated, "connection reset by peer");

    // #3020: provider rejections that classify as Internal (not
    // Authorization/State) still get the hint via raw-text matching.
    let not_exist = annotate_child_model_error("Model Not Exist", "kimi-k2", provider, &inherit);
    assert!(
        not_exist.contains("child-agent model config"),
        "DeepSeek-style rejection gets the hint: {not_exist}"
    );

    let openai_style = annotate_child_model_error(
        "The model `gpt-5.5-nano` does not exist or you do not have access to it.",
        "gpt-5.5-nano",
        crate::config::ApiProvider::OpenaiCodex,
        &ModelRoute::Fixed("gpt-5.5-nano".to_string()),
    );
    assert!(
        openai_style.contains("child-agent model config"),
        "OpenAI-style rejection gets the hint: {openai_style}"
    );
}

#[test]
fn child_launch_error_names_provider_model_and_route_source() {
    // #4049: a model-not-found child launch failure must name the provider
    // that was used, the model that was requested, and the route that produced
    // it, so the parent (and user) can tell whether the provider context was
    // lost, the wrong model was requested, or an override needs adjusting.
    let err = anyhow::Error::new(crate::llm_client::LlmError::ModelError(
        "Model \"deepseek-v4-pro\" not found".to_string(),
    ));
    let provider = crate::config::ApiProvider::Deepseek;
    let route = ModelRoute::Fixed("deepseek-v4-pro".to_string());
    let annotated = annotate_child_model_error(
        &subagent_failure_message(&err),
        "deepseek-v4-pro",
        provider,
        &route,
    );
    assert!(
        annotated.contains(provider.display_name()),
        "provider: {annotated}"
    );
    assert!(annotated.contains("deepseek-v4-pro"), "model: {annotated}");
    assert!(
        annotated.contains("route:"),
        "route label present: {annotated}"
    );
    assert!(
        annotated.contains("explicit model id"),
        "route source: {annotated}"
    );

    // The route label reflects an inherited route distinctly from a fixed one.
    let inherited = annotate_child_model_error(
        &subagent_failure_message(&err),
        "deepseek-v4-pro",
        provider,
        &ModelRoute::Inherit,
    );
    assert!(
        inherited.contains("inherited from the parent"),
        "inherit route source: {inherited}"
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
    // Drop-clear the window even if an assertion below panics: this state is
    // process-global, and a leaked 30s pause strands every concurrently
    // running test whose worker issues a model request.
    let _clear = ClearRateLimitOnDrop;
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

const MCP_ACTION_TOOL: &str = "mcp_github_create_pull_request";

fn subagent_registry_with_mcp_action(auto_approve: bool) -> SubAgentToolRegistry {
    let mut runtime = stub_runtime();
    runtime.context.auto_approve = auto_approve;
    let mut registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        Some(vec![MCP_ACTION_TOOL.to_string()]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    registry
        .registry
        .register(crate::tools::registry::mcp_tool_adapter_for_test(
            MCP_ACTION_TOOL,
        ));
    registry
}

#[tokio::test]
async fn child_write_tool_fails_closed_outside_registered_scope() {
    let tmp = tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("outside")).unwrap();
    let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
    {
        let mut guard = manager.write().await;
        guard
            .coordination
            .register_claim(
                WriteScopeClaim {
                    owner: "agent_scoped".into(),
                    roots: vec!["src".into()],
                    exact_files: vec![],
                    contracts: vec![],
                },
                false,
                |_| false,
            )
            .unwrap();
        guard
            .register_worker_with_coordination(make_write_worker_spec(
                "other-writer",
                tmp.path().to_path_buf(),
                "conflict",
            ))
            .expect("active conflicting writer");
    }
    let mut runtime = stub_runtime();
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    runtime.context.auto_approve = true;
    runtime.worker_profile = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
    let registry = SubAgentToolRegistry::new_with_owner(
        runtime,
        SubAgentType::Implementer,
        "agent_scoped".into(),
        "implementer".into(),
        Some(vec![
            "write_file".into(),
            "exec_shell".into(),
            "Bash".into(),
            "Run".into(),
            "agents/coordinate".into(),
            "work_update".into(),
            "agent".into(),
        ]),
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );
    registry
        .execute(
            "agent_scoped",
            "write_file",
            json!({"path": "src/ok.txt", "content": "ok"}),
        )
        .await
        .expect("in-scope write");
    registry
        .execute(
            "agent_scoped",
            "agents/coordinate",
            json!({"action": "claim", "roots": ["docs/guides"]}),
        )
        .await
        .expect("coordination-only non-overlapping scope expansion stays available");
    let claim_collision = registry
        .execute(
            "agent_scoped",
            "agents/coordinate",
            json!({"action": "claim", "roots": ["conflict/nested"]}),
        )
        .await
        .expect_err("coordination scope expansion still rejects live overlap")
        .to_string();
    assert!(
        claim_collision.contains("contention") && claim_collision.contains("other-writer"),
        "{claim_collision}"
    );
    registry
        .execute(
            "agent_scoped",
            "work_update",
            json!({
                "todos": [{"content": "bounded edit", "status": "in_progress"}]
            }),
        )
        .await
        .expect("shared writers can still publish internal Work progress");
    let child_collision = registry
        .execute(
            "agent_scoped",
            "agent",
            json!({
                "prompt": "edit the same scope",
                "type": "implementer",
                "workspace_policy": "shared",
                "write_authority": "workspace_write",
                "write_roots": ["conflict"],
                "expected_artifact": "tested patch"
            }),
        )
        .await
        .expect_err("agent exemption still subjects child writers to coordination")
        .to_string();
    assert!(
        child_collision.contains("contention") && child_collision.contains("other-writer"),
        "{child_collision}"
    );
    registry
        .execute(
            "agent_scoped",
            "agent",
            json!({
                "prompt": "inspect without mutation",
                "write_authority": "read_only"
            }),
        )
        .await
        .expect("shared writers may still delegate a read-only child");
    let spawned_id = manager
        .read()
        .await
        .agents
        .keys()
        .next()
        .cloned()
        .expect("read-only child registered");
    manager
        .write()
        .await
        .cancel_agent(&spawned_id)
        .expect("stop test child");
    let err = registry
        .execute(
            "agent_scoped",
            "write_file",
            json!({"path": "docs/no.txt", "content": "no"}),
        )
        .await
        .expect_err("out-of-scope write must fail")
        .to_string();
    assert!(
        err.contains("outside") && err.contains("agents/coordinate"),
        "{err}"
    );
    assert!(!tmp.path().join("docs/no.txt").exists());
    for (tool_name, input, target) in [
        (
            "exec_shell",
            json!({"command": "touch outside/compat.txt"}),
            "outside/compat.txt",
        ),
        (
            "Bash",
            json!({"action": "run", "command": "touch outside/canonical.txt"}),
            "outside/canonical.txt",
        ),
    ] {
        let shell_err = registry
            .execute("agent_scoped", tool_name, input)
            .await
            .expect_err("unbounded shared-workspace shell must fail")
            .to_string();
        assert!(
            shell_err.contains("cannot prove a bounded file target"),
            "{tool_name}: {shell_err}"
        );
        assert!(
            !tmp.path().join(target).exists(),
            "{tool_name} created {target} outside its registered claim"
        );
    }
    let run_err = registry
        .execute(
            "agent_scoped",
            "Run",
            json!({
                "action": "verifiers",
                "commands": [{
                    "name": "escape",
                    "program": "/bin/sh",
                    "args": ["-c", "touch docs/run-escape.txt"]
                }]
            }),
        )
        .await
        .expect_err("custom verifier commands cannot bypass shared write ownership")
        .to_string();
    assert!(
        run_err.contains("cannot prove a bounded file target"),
        "{run_err}"
    );
    assert!(!tmp.path().join("docs/run-escape.txt").exists());
}

#[test]
fn shared_claim_shell_gate_normalizes_only_the_run_action() {
    assert!(is_unbounded_shell_run(
        "exec_shell",
        &json!({"command": "true"})
    ));
    assert!(is_unbounded_shell_run(
        "Bash",
        &json!({"action": "run", "command": "true"})
    ));
    assert!(is_unbounded_shell_run("Bash", &json!({"command": "true"})));
    for action in ["wait", "interact", "cancel"] {
        assert!(
            !is_unbounded_shell_run("Bash", &json!({"action": action})),
            "Bash.{action} must retain its existing non-run claim behavior"
        );
    }
}

#[tokio::test]
async fn subagent_blocks_mcp_action_without_parent_auto_approve() {
    let registry = subagent_registry_with_mcp_action(false);

    let err = registry
        .execute("agent_test", MCP_ACTION_TOOL, json!({}))
        .await
        .expect_err("non-read MCP actions must require parent auto approval");

    assert!(
        err.to_string().contains(
            "requires approval and cannot run inside this sub-agent unless the parent session is auto-approved"
        ),
        "unexpected MCP approval error: {err}"
    );
}

#[tokio::test]
async fn auto_approved_subagent_passes_mcp_action_approval_gate() {
    let registry = subagent_registry_with_mcp_action(true);

    let err = registry
        .execute("agent_test", MCP_ACTION_TOOL, json!({}))
        .await
        .expect_err("the empty test MCP pool should reject execution after the approval gate");
    let message = err.to_string();
    assert!(
        message.contains("MCP tool failed"),
        "auto approval should reach the MCP adapter, got: {message}"
    );
    assert!(
        !message.contains("requires approval"),
        "auto-approved MCP actions must not stop at the approval gate: {message}"
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
async fn workflow_accept_edits_allows_general_file_write_without_parent_auto_approve() {
    // Workflow-spawned children accept Suggest-level file edits for write-capable
    // postures (including general) while shell tools still require parent auto-approve.
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(workspace.clone());
    runtime.context.auto_approve = false;
    runtime.accept_edits = true;
    let registry = SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    let result = registry
        .execute(
            "agent_test",
            "write_file",
            json!({"path": "workflow_edit.txt", "content": "from workflow"}),
        )
        .await
        .expect("workflow accept_edits should allow general write");
    let written =
        std::fs::read_to_string(workspace.join("workflow_edit.txt")).expect("file should exist");
    assert_eq!(written, "from workflow");
    assert!(!result.contains("requires approval"), "{result}");

    let err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await
        .expect_err("shell must still require parent auto-approve");
    assert!(
        err.to_string().contains("requires approval"),
        "unexpected: {err}"
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

    std::fs::write(tmp.path().join("allowed.txt"), "visible").unwrap();
    let read = registry
        .execute(
            "agent_test",
            "File",
            json!({"action": "read", "path": "allowed.txt"}),
        )
        .await
        .expect("Explore should retain canonical read access");
    assert!(read.contains("visible"));

    let err = registry
        .execute(
            "agent_test",
            "File",
            json!({"action": "write", "path": "nope.txt", "content": "denied"}),
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
fn detached_background_children_survive_parent_cancellation() {
    let parent = stub_runtime();
    let first = parent.background_runtime();
    let second = parent.background_runtime();
    parent.cancel_token.cancel();

    assert!(parent.cancel_token.is_cancelled());
    assert!(
        !first.cancel_token.is_cancelled() && !second.cancel_token.is_cancelled(),
        "parent stop must leave every detached child running until explicitly cancelled"
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
        api_config: None,
        model: "deepseek-v4-flash".to_string(),
        locale_tag: "en".to_string(),
        auto_model: false,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        role_models: std::collections::HashMap::new(),
        fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::built_ins_only()),
        context,
        allow_shell: true,
        accept_edits: false,
        accept_verification: false,
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
        parent_mode: crate::tui::app::AppMode::Agent,
        mcp_pool: None,
        step_api_timeout: DEFAULT_STEP_API_TIMEOUT,
        tool_timeout: DEFAULT_TOOL_TIMEOUT,
        speech_output_dir: None,
        todos: crate::tools::todo::new_shared_todo_list(),
    }
}

#[test]
fn root_operate_dispatch_delegates_file_edits_without_bypassing_required_tools() {
    let mut runtime = stub_runtime();
    runtime.parent_mode = crate::tui::app::AppMode::Operate;
    assert!(!runtime.accept_edits);
    assert!(!runtime.accept_verification);
    assert!(!runtime.context.auto_approve);

    apply_session_spawn_defaults(&mut runtime);

    assert!(runtime.accept_edits);
    assert!(runtime.accept_verification);
    assert!(
        !runtime.context.auto_approve,
        "Operate dispatch must not silently grant Required tools such as shell"
    );
}

#[tokio::test]
async fn root_operate_dispatch_delegates_builtin_verification_but_not_shell() {
    let tmp = tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"operate-verification-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("manifest");
    std::fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn ready() -> bool { true }\n",
    )
    .expect("source");

    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.context.auto_approve = false;
    runtime.parent_mode = crate::tui::app::AppMode::Operate;
    apply_session_spawn_defaults(&mut runtime);
    let registry = SubAgentToolRegistry::new(
        runtime.clone(),
        SubAgentType::General,
        None,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    );

    registry
        .execute("agent_test", "run_tests", json!({}))
        .await
        .expect("parent-approved Operate worker should run built-in tests");

    let targeted_err = registry
        .execute(
            "agent_test",
            "run_tests",
            json!({"args": "--manifest-path ../outside/Cargo.toml"}),
        )
        .await
        .expect_err("raw Cargo argv must stay approval-gated");
    assert!(targeted_err.to_string().contains("requires approval"));

    let shell_err = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo nope"}))
        .await
        .expect_err("Operate verification delegation must not grant raw shell");
    assert!(shell_err.to_string().contains("requires approval"));

    let custom_err = registry
        .execute(
            "agent_test",
            "run_verifiers",
            json!({"commands": [{"name": "custom", "program": "echo", "args": ["nope"]}]}),
        )
        .await
        .expect_err("Operate verification delegation must not grant custom commands");
    assert!(custom_err.to_string().contains("requires approval"));

    let direct_child = runtime.child_runtime();
    assert!(direct_child.accept_verification);
    let grandchild = direct_child.child_runtime();
    assert!(
        !grandchild.accept_verification,
        "Operate verification delegation must not propagate past the direct worker"
    );
}

#[test]
fn worker_lifecycle_records_direct_operate_approval_without_delegating_authority() {
    let todos = crate::tools::todo::new_shared_todo_list();
    let plan = crate::tools::plan::new_shared_plan_state();
    let work = crate::work_graph::new_shared_work_runtime(todos, plan);

    let mut direct = stub_runtime();
    direct.context.state_namespace = "worker-lifecycle".to_string();
    direct.context.runtime.work = Some(work.clone());
    direct.accept_verification = true;
    direct.spawn_depth = 1;
    let lifecycle =
        SubAgentWorkLifecycle::register(&direct, "agent_01234567", "verify installed acceptance")
            .expect("direct worker registration")
            .expect("work runtime attached");
    lifecycle
        .reconcile_state(OwnerState::Running, 2, None)
        .expect("running owner report");
    let receipt = EvidenceRef::new(
        EvidenceKind::Receipt {
            owner: "worker".to_string(),
        },
        "worker:agent_01234567:result",
        Some(512),
        false,
    )
    .expect("safe worker receipt");
    lifecycle
        .reconcile_state(OwnerState::Completed, 3, Some(receipt))
        .expect("terminal owner report");

    let direct_graph = work
        .capture(Some("worker-lifecycle"))
        .expect("capture direct worker")
        .expect("graph")
        .graph;
    assert_eq!(
        direct_graph
            .nodes
            .iter()
            .filter(|node| node.kind == crate::work_graph::NodeKind::Approval)
            .count(),
        1,
        "direct Operate verification must leave one provenance record"
    );
    assert!(direct_graph.edges.iter().any(|edge| {
        edge.kind == crate::work_graph::EdgeKind::RequiresApproval
            && direct_graph
                .node(&edge.from)
                .is_some_and(|node| node.title == "verify installed acceptance")
            && direct_graph
                .node(&edge.to)
                .is_some_and(|node| node.kind == crate::work_graph::NodeKind::Approval)
    }));
    let direct_operation = direct_graph
        .nodes
        .iter()
        .find(|node| {
            node.binding
                .as_ref()
                .is_some_and(|binding| binding.external == "worker:agent_01234567")
        })
        .expect("bound direct worker operation");
    assert_eq!(
        direct_operation.state,
        crate::work_graph::NodeState::Completed
    );
    assert_eq!(
        direct_operation
            .binding
            .as_ref()
            .and_then(|binding| binding.last_observation.as_ref())
            .and_then(|observation| observation.output.as_ref())
            .and_then(EvidenceRef::raw_bytes),
        Some(512)
    );

    let mut nested = direct.child_runtime();
    nested.accept_verification = true;
    nested.spawn_depth = 2;
    SubAgentWorkLifecycle::register(&nested, "agent_89abcdef", "nested verification")
        .expect("nested worker registration")
        .expect("work runtime attached");
    let nested_graph = work
        .capture(Some("worker-lifecycle"))
        .expect("capture nested worker")
        .expect("graph")
        .graph;
    assert_eq!(
        nested_graph
            .nodes
            .iter()
            .filter(|node| node.kind == crate::work_graph::NodeKind::Approval)
            .count(),
        1,
        "nested workers must not inherit Operate approval authority"
    );
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
        "sakana" => {
            providers.sakana = crate::config::ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            };
        }
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

// ---- #4193: interactive-TUI in-process spawn honors a profile's pinned provider ----

/// A `Config` with two fully-configured providers, each on a DISTINCT host so a
/// test can prove a child client actually re-pointed: `deepseek` is the session
/// route, `zai` is a pinned route. Provider-scoped keys/base URLs are used (root
/// `api_key` intentionally unset) so `deepseek_api_key`/`deepseek_base_url`
/// resolve each provider independently.
fn cross_provider_config() -> crate::config::Config {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut custom = std::collections::HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            api_key: Some("lm-studio-key".to_string()),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            model: Some("qwen-2.5-7b".to_string()),
            ..Default::default()
        },
    );
    for (name, base_url, model) in [
        ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
        ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
        ("CUSTOM", "http://127.0.0.1:18183/v1", "model-upper"),
        ("custom", "http://127.0.0.1:18184/v1", "model-literal"),
        ("OPENAI", "http://127.0.0.1:18185/v1", "model-openai"),
    ] {
        custom.insert(
            name.to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                api_key: Some("local-test-key".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                ..Default::default()
            },
        );
    }
    let providers = crate::config::ProvidersConfig {
        deepseek: crate::config::ProviderConfig {
            api_key: Some("session-key".to_string()),
            base_url: Some("https://session-provider.example.com/v1".to_string()),
            ..Default::default()
        },
        zai: crate::config::ProviderConfig {
            api_key: Some("pinned-key".to_string()),
            base_url: Some("https://pinned-provider.example.com/v1".to_string()),
            ..Default::default()
        },
        custom,
        ..crate::config::ProvidersConfig::default()
    };
    crate::config::Config {
        provider: Some("deepseek".to_string()),
        providers: Some(providers),
        ..crate::config::Config::default()
    }
}

/// A session runtime on `deepseek` with the cross-provider `Config` threaded in,
/// exactly as the engine wires it via `with_api_config`.
fn cross_provider_runtime() -> SubAgentRuntime {
    let config = cross_provider_config();
    let client = DeepSeekClient::new(&config).expect("session client builds");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;
    runtime
}

/// A roster member whose profile explicitly pins `provider` (+ an arbitrary
/// `model`), mirroring the on-disk `[fleet]` profile shape.
fn member_pinning_provider(provider: &str, model: &str) -> crate::fleet::profile::AgentProfile {
    let mut profile = custom_fleet_profile("worker");
    profile.provider = Some(provider.to_string());
    profile.model = Some(model.to_string());
    crate::fleet::profile::AgentProfile {
        id: format!("{provider}-worker"),
        display_name: Some(format!("{provider} worker")),
        description: None,
        profile,
        source: std::path::PathBuf::from(format!("{provider}-worker.toml")),
        origin: crate::fleet::roster::ProfileOrigin::Workspace,
    }
}

#[test]
fn spawn_child_client_targets_profile_pinned_provider() {
    // Session runs on DeepSeek; the roster member pins Z.ai. The in-process
    // child must issue its request to a Z.ai client (Z.ai base URL + creds),
    // not the shared session DeepSeek client (#4193 acceptance criterion).
    let runtime = cross_provider_runtime();
    assert_eq!(
        runtime.client.api_provider(),
        crate::config::ApiProvider::Deepseek,
        "precondition: session is on DeepSeek"
    );

    let member = member_pinning_provider("zai", "glm-4.6");
    let child_client = child_client_for_member(&runtime, Some(&member))
        .expect("pinned-provider client builds when its creds are configured");

    assert_eq!(
        child_client.api_provider(),
        crate::config::ApiProvider::Zai,
        "child client must target the profile-pinned provider (#4193)"
    );
    assert!(
        child_client
            .base_url()
            .contains("pinned-provider.example.com"),
        "child must talk to the pinned provider's endpoint, got {}",
        child_client.base_url()
    );
    assert!(
        !child_client
            .base_url()
            .contains("session-provider.example.com"),
        "child must NOT reuse the session provider's endpoint (the #4093 misroute)"
    );
}

#[test]
fn spawn_child_client_targets_custom_profile_provider() {
    // #3965: LM Studio and other user-named OpenAI-compatible providers live in
    // `[providers.<name>]` tables. A profile pin must preserve that name so the
    // child client resolves the custom table instead of rejecting it or
    // silently inheriting the DeepSeek session client.
    let runtime = cross_provider_runtime();
    assert_eq!(
        runtime.client.api_provider(),
        crate::config::ApiProvider::Deepseek,
        "precondition: session is on DeepSeek"
    );

    let member = member_pinning_provider("lm-studio", "qwen-2.5-7b");
    let child_client = child_client_for_member(&runtime, Some(&member))
        .expect("custom provider client builds from the named provider table");

    assert_eq!(
        child_client.api_provider(),
        crate::config::ApiProvider::Custom
    );
    assert_eq!(child_client.base_url(), "http://127.0.0.1:1234/v1");
}

#[test]
fn spawn_child_client_switches_between_exact_named_custom_endpoints() {
    let mut config = cross_provider_config();
    config.provider = Some("custom-a".to_string());
    let client = DeepSeekClient::new(&config).expect("custom A session client");
    assert_eq!(client.base_url(), "http://127.0.0.1:18181/v1");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;

    let member = member_pinning_provider("custom-b", "model-b");
    let child_client =
        child_client_for_member(&runtime, Some(&member)).expect("custom B child client builds");

    assert_eq!(
        child_client.api_provider(),
        crate::config::ApiProvider::Custom
    );
    assert_eq!(child_client.base_url(), "http://127.0.0.1:18182/v1");
}

#[test]
fn cross_custom_child_rebinds_config_receipts_and_grandchild_route_atomically() {
    let mut config = cross_provider_config();
    config.provider = Some("custom-a".to_string());
    let client = DeepSeekClient::new(&config).expect("custom A session client");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;

    let member_b = member_pinning_provider("custom-b", "model-b");
    let binding_b =
        child_provider_binding(&runtime, Some(&member_b)).expect("custom B child provider binding");
    let mut child_runtime = runtime.background_runtime();
    child_runtime.client = binding_b.client;
    child_runtime.api_config = binding_b.api_config;

    assert_eq!(child_runtime.client.base_url(), "http://127.0.0.1:18182/v1");
    assert_eq!(
        child_runtime
            .api_config
            .as_ref()
            .and_then(|config| config.provider.as_deref()),
        Some("custom-b")
    );
    let worker_profile = worker_profile_for_spawn(
        &child_runtime,
        &SubAgentType::Implementer,
        &AgentWorkerToolProfile::Inherited,
        "model-b",
        None,
    );
    assert_eq!(worker_profile.provider.as_deref(), Some("custom-b"));

    assert!(!provider_pin_matches_session(&child_runtime, "custom-a"));
    let member_a = member_pinning_provider("custom-a", "model-a");
    let binding_a = child_provider_binding(&child_runtime, Some(&member_a))
        .expect("grandchild rebinds to custom A");
    assert_eq!(binding_a.client.base_url(), "http://127.0.0.1:18181/v1");
    assert_eq!(
        binding_a
            .api_config
            .as_ref()
            .and_then(|config| config.provider.as_deref()),
        Some("custom-a")
    );
}

#[test]
fn spawn_child_client_does_not_collapse_case_colliding_custom_pins() {
    let mut config = cross_provider_config();
    config.provider = Some("custom-a".to_string());
    let client = DeepSeekClient::new(&config).expect("custom A session client");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;

    for (provider_id, model, endpoint) in [
        ("CUSTOM", "model-upper", "http://127.0.0.1:18183/v1"),
        ("custom", "model-literal", "http://127.0.0.1:18184/v1"),
        ("OPENAI", "model-openai", "http://127.0.0.1:18185/v1"),
    ] {
        assert!(!provider_pin_matches_session(&runtime, provider_id));
        let member = member_pinning_provider(provider_id, model);
        let child = child_client_for_member(&runtime, Some(&member))
            .expect("case-colliding custom client builds from exact table");
        assert_eq!(child.api_provider(), crate::config::ApiProvider::Custom);
        assert_eq!(child.base_url(), endpoint);
    }
}

#[test]
fn removed_case_colliding_custom_pin_fails_closed() {
    let mut config = cross_provider_config();
    config.provider = Some("custom-a".to_string());
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .remove("CUSTOM");
    let client = DeepSeekClient::new(&config).expect("custom A session client");
    let mut runtime = stub_runtime().with_api_config(config);
    runtime.client = client;

    assert!(!provider_pin_matches_session(&runtime, "CUSTOM"));
    let member = member_pinning_provider("CUSTOM", "model-upper");
    let err = match child_client_for_member(&runtime, Some(&member)) {
        Ok(_) => panic!("removed custom pin must not inherit active custom client"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("CUSTOM"), "{err}");
}

#[test]
fn spawn_child_client_inherits_session_provider_without_pin() {
    // Regression: profile-less members and members that pin no provider (or the
    // session's own provider) keep the session client. No cross-provider build,
    // no misroute, no behavior change from before #4193.
    let runtime = cross_provider_runtime();

    let inherited = child_client_for_member(&runtime, None)
        .expect("profile-less spawn reuses the session client");
    assert_eq!(
        inherited.api_provider(),
        crate::config::ApiProvider::Deepseek
    );
    assert!(
        inherited
            .base_url()
            .contains("session-provider.example.com"),
        "profile-less child stays on the session endpoint, got {}",
        inherited.base_url()
    );

    // A member that pins the SAME provider as the session also stays put.
    let same = member_pinning_provider("deepseek", "deepseek-v4-flash");
    let same_client = child_client_for_member(&runtime, Some(&same))
        .expect("same-provider pin reuses the session client");
    assert_eq!(
        same_client.api_provider(),
        crate::config::ApiProvider::Deepseek
    );
    assert!(
        same_client
            .base_url()
            .contains("session-provider.example.com")
    );
}

#[test]
fn spawn_child_client_fails_closed_when_pinned_provider_unavailable() {
    // Defense in depth (#4093): if the pinned provider's client cannot be built
    // (here: no session Config threaded in), fail the spawn instead of silently
    // sending the pinned model id to the session provider's endpoint.
    let mut runtime = cross_provider_runtime();
    runtime.api_config = None; // simulate a legacy/untethered runtime

    let member = member_pinning_provider("zai", "glm-4.6");
    // `DeepSeekClient` is not `Debug`, so match instead of `expect_err`.
    let err = match child_client_for_member(&runtime, Some(&member)) {
        Ok(_) => panic!("must fail closed when the pinned client cannot be built"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("zai"),
        "error must name the pinned provider so the failure is actionable: {msg}"
    );
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
async fn run_subagent_task_claims_before_delivery_and_then_finalizes() {
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

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.manager = Arc::clone(&manager);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
    manager.write().await.agents.insert(agent_id.clone(), agent);

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
        wall_time: DEFAULT_CHILD_WALL_TIME,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    let manager_lock = manager.write().await;
    let task_handle = tokio::spawn(run_subagent_task(task));

    // External delivery must wait for the terminal claim. Holding the manager
    // lock keeps that claim pending and therefore keeps the parent-completion
    // inbox empty.
    let premature = tokio::time::timeout(Duration::from_millis(100), completion_rx.recv()).await;
    assert!(
        premature.is_err(),
        "completion escaped before the manager terminal claim"
    );
    drop(manager_lock);

    let completion = tokio::time::timeout(Duration::from_secs(1), completion_rx.recv())
        .await
        .expect("completion should follow the successful terminal claim");
    let completion = completion.expect("completion channel should remain open");
    assert_eq!(completion.agent_id, agent_id);

    task_handle
        .await
        .expect("run_subagent_task should complete after lock release");

    let snapshot = manager
        .read()
        .await
        .get_result(&agent_id)
        .expect("completed agent should be present");
    assert!(
        matches!(snapshot.status, SubAgentStatus::Failed(_)),
        "0 max_steps cannot produce a final summary, so the child must fail: {:?}",
        snapshot.status
    );
}

#[tokio::test]
async fn cancellation_wins_task_race_but_still_fans_in_exactly_once() {
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_cancelled_at_epilogue".to_string();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "noop".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );
    agent.status = SubAgentStatus::Running;

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.manager = Arc::clone(&manager);
    runtime.mailbox = Some(mailbox);
    runtime.event_tx = Some(event_tx);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));

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
        wall_time: DEFAULT_CHILD_WALL_TIME,
        input_rx: task_input_rx,
        launch_gate: None,
    };

    let mut manager_lock = manager.write().await;
    manager_lock.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    manager_lock.agents.insert(agent_id.clone(), agent);
    let task_handle = tokio::spawn(run_subagent_task(task));

    // max_steps=0 reaches the task epilogue without provider I/O. Keep the
    // terminal lock occupied long enough for that epilogue to queue behind us,
    // then let cancellation win the same transition point deterministically.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let cancelled = manager_lock
        .cancel_agent(&agent_id)
        .expect("cancellation should win");
    assert_eq!(cancelled.status, SubAgentStatus::Cancelled);
    drop(manager_lock);

    task_handle
        .await
        .expect("late task epilogue should exit cleanly");

    let snapshot = {
        let manager = manager.read().await;
        manager
            .get_result(&agent_id)
            .expect("cancelled agent should remain present")
    };
    assert_eq!(snapshot.status, SubAgentStatus::Cancelled);
    assert_eq!(
        snapshot.result.as_deref(),
        Some("Cancelled by parent request.")
    );

    let completion = completion_rx
        .try_recv()
        .expect("winning cancellation must wake the immediate parent");
    assert_eq!(completion.agent_id, agent_id);
    assert!(completion.payload.contains(r#""status":"cancelled""#));
    assert!(
        completion_rx.try_recv().is_err(),
        "late task output must not publish a second parent completion"
    );

    let terminal_mail = mailbox_rx
        .drain()
        .into_iter()
        .filter(|envelope| {
            matches!(
                envelope.message,
                MailboxMessage::Completed { .. }
                    | MailboxMessage::Failed { .. }
                    | MailboxMessage::Interrupted { .. }
                    | MailboxMessage::Cancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_mail.len(), 1);
    assert!(matches!(
        terminal_mail[0].message,
        MailboxMessage::Cancelled { ref agent_id } if agent_id == &snapshot.agent_id
    ));

    let terminal_events = std::iter::from_fn(|| event_rx.try_recv().ok())
        .filter(|event| matches!(event, Event::AgentComplete { .. }))
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert!(matches!(
        &terminal_events[0],
        Event::AgentComplete { id, result }
            if id == &snapshot.agent_id && result.contains(r#""status":"cancelled""#)
    ));
}

#[tokio::test]
async fn non_retryable_provider_failure_fans_in_to_every_terminal_sink() {
    use tokio_util::sync::CancellationToken;

    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let (task_input_tx, task_input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_fatal_provider_failure".to_string();
    let mut agent = SubAgent::new(
        agent_id.clone(),
        SubAgentType::General,
        "noop".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        task_input_tx,
        tmp.path().to_path_buf(),
        "boot_test".to_string(),
    );

    let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<SubAgentCompletion>();
    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let (client, calls) = always_invalid_request_chat_client().await;
    let mut runtime = runtime_with_depth(1, Some(completion_tx));
    runtime.client = client;
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    runtime.mailbox = Some(mailbox);
    runtime.event_tx = Some(event_tx);
    agent.terminal_delivery = Some(SubAgentTerminalDeliveryContext::from_runtime(&runtime));
    {
        let mut manager = manager.write().await;
        manager.agents.insert(agent_id.clone(), agent);
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
    }

    run_subagent_task(SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Request a model response".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(Vec::new()),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 1,
        token_budget: None,
        wall_time: DEFAULT_CHILD_WALL_TIME,
        input_rx: task_input_rx,
        launch_gate: None,
    })
    .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "invalid requests are fatal and must not retry"
    );
    let completion = completion_rx.try_recv().expect("parent failure fan-in");
    assert_eq!(completion.agent_id, agent_id);
    assert!(completion.payload.contains(r#""status":"failed""#));
    assert!(completion_rx.try_recv().is_err());
    let terminal_mail = mailbox_rx
        .drain()
        .into_iter()
        .filter(|envelope| {
            matches!(
                envelope.message,
                MailboxMessage::Completed { .. }
                    | MailboxMessage::Failed { .. }
                    | MailboxMessage::Interrupted { .. }
                    | MailboxMessage::Cancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        terminal_mail.as_slice(),
        [MailboxEnvelope {
            message: MailboxMessage::Failed { agent_id: id, .. },
            ..
        }] if id == &agent_id
    ));
    let complete_events = std::iter::from_fn(|| event_rx.try_recv().ok())
        .filter_map(|event| match event {
            Event::AgentComplete { id, result } => Some((id, result)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        complete_events.as_slice(),
        [(id, result)] if id == &agent_id && result.contains(r#""status":"failed""#)
    ));

    let manager = manager.read().await;
    let snapshot = manager.get_result(&agent_id).expect("failed snapshot");
    assert!(matches!(snapshot.status, SubAgentStatus::Failed(_)));
    assert_eq!(
        snapshot.checkpoint.as_ref().map(|cp| cp.steps_taken),
        Some(1)
    );
    assert_eq!(
        manager.get_worker_record(&agent_id).unwrap().status,
        AgentWorkerStatus::Failed
    );
}

#[test]
fn summarize_subagent_result_diagnoses_missing_completed_payload() {
    let snap = make_snapshot(SubAgentStatus::Completed);
    let summary = summarize_subagent_result(&snap);
    assert!(
        summary.contains("no final summary"),
        "Completed without payload must not read as silent success: {summary}"
    );
}

#[test]
fn summarize_subagent_result_budget_exhaustion_is_actionable_not_raw_done() {
    let mut snap = make_snapshot(SubAgentStatus::BudgetExhausted);
    snap.result = Some("partial findings from step 1".to_string());
    let summary = summarize_subagent_result(&snap);
    assert!(summary.contains("partial output preserved"), "{summary}");
    assert!(!summary.eq("Token budget exhausted"), "{summary}");

    let empty = make_snapshot(SubAgentStatus::BudgetExhausted);
    let summary = summarize_subagent_result(&empty);
    assert!(
        summary.contains("retry with a smaller scoped task"),
        "{summary}"
    );
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
fn subagent_completion_from_result_surfaces_step_limit_not_silent_success() {
    let snap = make_snapshot(SubAgentStatus::Failed(
        "child step budget exhausted (limit: 12 steps; used: 12); raise it with max_steps or split the work into smaller independent tasks".to_string(),
    ));
    let completion = subagent_completion_from_result(&snap);
    assert!(
        completion.payload.contains("step budget exhausted"),
        "{completion:?}"
    );
    assert!(completion.payload.contains("max_steps"), "{completion:?}");
    assert!(!completion.payload.contains("Completed (no output)"));
}

#[test]
fn subagent_completion_from_result_preserves_missing_final_summary_diagnostic() {
    let snap = make_snapshot(SubAgentStatus::Completed);
    let completion = subagent_completion_from_result(&snap);
    assert!(
        completion.payload.contains("no final summary"),
        "{completion:?}"
    );
}

#[test]
fn subagent_budget_exhaustion_completion_carries_budget_exhausted_sentinel() {
    let mut snap = make_snapshot(SubAgentStatus::BudgetExhausted);
    snap.result = Some("partial findings from step 2".to_string());
    let completion = subagent_completion_from_result(&snap);
    assert!(
        completion.payload.contains("partial output preserved"),
        "{completion:?}"
    );
    let inner = completion
        .payload
        .split("<codewhale:subagent.done>")
        .nth(1)
        .and_then(|chunk| chunk.split("</codewhale:subagent.done>").next())
        .expect("sentinel json");
    let parsed: serde_json::Value = serde_json::from_str(inner).expect("sentinel parses");
    assert_eq!(parsed["status"], "budget_exhausted");
    assert_eq!(parsed["summary_location"], "previous_line");
}

#[test]
fn subagent_completion_inlines_evidence_before_sentinel() {
    let mut snap = make_snapshot(SubAgentStatus::Completed);
    snap.result =
        Some("VERDICT: pass\n### EVIDENCE\n- src/lib.rs:1-3 — init ok\n### GAPS\nnone".to_string());
    let completion = subagent_completion_from_result(&snap);
    let evidence_pos = completion
        .payload
        .find("### EVIDENCE")
        .expect("evidence block");
    let sentinel_pos = completion
        .payload
        .find("<codewhale:subagent.done>")
        .expect("sentinel");
    assert!(evidence_pos < sentinel_pos, "evidence before sentinel");
    assert!(completion.payload.contains("src/lib.rs:1-3"));
    assert!(
        completion.payload.find("VERDICT: pass").unwrap_or(0) < evidence_pos,
        "summary before evidence"
    );
}

#[test]
fn subagent_completion_skips_empty_evidence_on_failed_child() {
    let mut snap = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    snap.result = Some("### EVIDENCE\n- should-not-appear".to_string());
    let completion = subagent_completion_from_result(&snap);
    assert!(!completion.payload.contains("### EVIDENCE"));
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
    // prompts/text.rs (SUBAGENT_OUTPUT_FORMAT): human summary on line 1,
    // `<codewhale:subagent.done>` sentinel on line 2. This test pins the
    // format so future refactors don't silently break the model's parsing
    // contract.
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
fn inherit_route_remaps_stale_deepseek_model_for_sakana_provider() {
    let mut runtime = stub_runtime_for_provider("sakana");
    runtime.model = "deepseek-v4-flash".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Inherit,
        SubAgentThinking::Inherit,
        "summarize the repo layout",
    );
    assert_eq!(route.model, "deepseek-v4-flash");

    let validated = ensure_subagent_model_for_provider(&runtime, &route.model_route, route.model)
        .expect("inherit should remap to operator route");
    assert_eq!(validated, crate::config::DEFAULT_SAKANA_MODEL);
    assert!(
        !validated.contains("deepseek"),
        "Sakana inherit must not keep DeepSeek ids: {validated}"
    );
}

#[test]
fn faster_route_remaps_stale_deepseek_model_for_sakana_provider() {
    let mut runtime = stub_runtime_for_provider("sakana");
    runtime.model = "deepseek-v4-flash".to_string();

    let route = fallback_subagent_assignment_route(
        &runtime,
        None,
        ModelRoute::Faster,
        SubAgentThinking::Inherit,
        "quick scan",
    );
    let validated = ensure_subagent_model_for_provider(&runtime, &route.model_route, route.model)
        .expect("faster should remap to operator route");
    assert_eq!(validated, crate::config::DEFAULT_SAKANA_MODEL);
}

#[test]
fn fixed_route_rejects_deepseek_model_for_sakana_provider() {
    let runtime = stub_runtime_for_provider("sakana");
    let err = ensure_subagent_model_for_provider(
        &runtime,
        &ModelRoute::Fixed("deepseek-v4-flash".to_string()),
        "deepseek-v4-flash".to_string(),
    )
    .expect_err("explicit DeepSeek pin must fail before spawn");
    assert!(
        err.to_string().contains("deepseek-v4-flash"),
        "error should name the model: {err}"
    );
}

#[test]
fn normalize_requested_subagent_model_rejects_cross_namespace_for_sakana() {
    let err = normalize_requested_subagent_model(
        "deepseek-v4-flash",
        "model",
        crate::config::ApiProvider::Sakana,
    )
    .expect_err("Sakana must reject DeepSeek-only model ids at spawn");
    assert!(
        err.to_string().contains("deepseek-v4-flash"),
        "error should name the model: {err}"
    );
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
fn operator_model_for_subagent_enumerates_from_catalog_facade() {
    // #4116: the operator-route fallback must source its model from the
    // catalog-backed ProviderLake facade, not the raw legacy table. On the
    // strict official DeepSeek API an invalid id is rejected, forcing the
    // enumeration branch; the chosen model must be exactly the facade's first
    // entry (proving the consumer was migrated off the raw legacy path), never
    // an invented id.
    crate::provider_lake::clear_live_snapshot();
    let mut runtime = stub_runtime(); // official DeepSeek API (strict validation)
    runtime.model = "definitely-not-a-real-model".to_string();

    let provider = runtime.client.api_provider();
    assert_eq!(provider, crate::config::ApiProvider::Deepseek);
    // Sanity: the strict provider really does reject the invalid id, so
    // operator_model_for_subagent must take the enumeration branch.
    assert!(crate::config::validate_route(provider, &runtime.model).is_err());

    let facade = crate::provider_lake::all_catalog_models_for_provider(provider);
    assert!(
        !facade.is_empty(),
        "expected the catalog facade to enumerate DeepSeek models"
    );

    let chosen = operator_model_for_subagent(&runtime);
    assert_eq!(
        chosen, facade[0],
        "operator model must come from the catalog-backed facade"
    );
    assert_ne!(
        chosen, "definitely-not-a-real-model",
        "operator model must not echo an invalid id"
    );
    // No-regression guard: DeepSeek's catalog view still enumerates every legacy
    // id it accepted before the migration (facade ⊇ legacy for this provider).
    let facade_lower: std::collections::BTreeSet<String> =
        facade.iter().map(|m| m.to_ascii_lowercase()).collect();
    for legacy in crate::config::model_completion_names_for_provider(provider) {
        assert!(
            facade_lower.contains(&legacy.to_ascii_lowercase()),
            "catalog facade dropped legacy model {legacy:?} for {provider:?}"
        );
    }
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
    // Concrete role defaults keep progress truthful.
    assert_eq!(format_step_counter(16, 60), "step 16/60");
}

#[test]
fn format_step_counter_keeps_concrete_budgets() {
    assert_eq!(format_step_counter(3, 25), "step 3/25");
    assert_eq!(format_step_counter(0, 1), "step 0/1");
}

#[test]
fn child_step_override_wins_and_clamps_to_hard_ceiling() {
    assert_eq!(resolve_max_steps(SubAgentType::Explore, None, None), 60);
    assert_eq!(
        resolve_max_steps(SubAgentType::Implementer, Some(7), None),
        7
    );
    assert_eq!(
        resolve_max_steps(SubAgentType::General, Some(u32::MAX), None),
        MAX_SUBAGENT_STEPS
    );
}

#[test]
fn child_wall_timeout_reason_is_typed_and_actionable() {
    let reason = child_wall_time_exhausted_reason(Duration::from_millis(1));
    assert!(reason.contains("wall-time budget exhausted"), "{reason}");
    assert!(reason.contains("limit: 0s"), "{reason}");
    assert!(reason.contains("wall_time_secs"), "{reason}");
    assert!(reason.contains("smaller independent tasks"), "{reason}");
    assert!(!reason.contains("token_budget"), "{reason}");
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
            wall_time: DEFAULT_CHILD_WALL_TIME,
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

#[tokio::test]
async fn launch_gate_wait_counts_against_child_wall_timeout() {
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    const WALL_TIME: Duration = Duration::from_millis(150);

    let tmp = tempdir().expect("tempdir");
    let manager = Arc::new(RwLock::new(SubAgentManager::new(
        tmp.path().to_path_buf(),
        2,
    )));
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let agent_id = "agent_gate_wall_timeout".to_string();
    let mut agent = SubAgent::new(
        agent_id.clone(),
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
    agent.status = SubAgentStatus::Running;

    let (mailbox, mut mailbox_rx) = Mailbox::new(CancellationToken::new());
    let mut runtime = stub_runtime();
    runtime.manager = Arc::clone(&manager);
    runtime.context = ToolContext::new(tmp.path());
    runtime.mailbox = Some(mailbox);

    let gate = Arc::new(Semaphore::new(1));
    let held_launch_permit = Arc::clone(&gate)
        .acquire_owned()
        .await
        .expect("test holds the single launch permit past the wall timeout");
    let task = SubAgentTask {
        manager_handle: Arc::clone(&manager),
        runtime,
        agent_id: agent_id.clone(),
        agent_type: SubAgentType::General,
        prompt: "Answer".to_string(),
        assignment: make_assignment(),
        allowed_tools: Some(vec![]),
        fork_context: false,
        started_at: Instant::now(),
        max_steps: 1,
        token_budget: None,
        wall_time: WALL_TIME,
        input_rx,
        launch_gate: Some(Arc::clone(&gate)),
    };
    {
        let mut manager = manager.write().await;
        manager.register_worker(make_worker_spec(&agent_id, tmp.path().to_path_buf()));
        manager.agents.insert(agent_id.clone(), agent);
    }

    let mut task_handle = tokio::spawn(run_subagent_task(task));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let envelope = mailbox_rx
                .recv()
                .await
                .expect("queued progress mailbox remains open");
            if matches!(
                envelope.message,
                MailboxMessage::Progress { ref agent_id, ref status }
                    if agent_id == "agent_gate_wall_timeout" && status.contains("queued")
            ) {
                break;
            }
        }
    })
    .await
    .expect("child publishes queued progress before its wall timeout");

    match tokio::time::timeout(Duration::from_secs(1), &mut task_handle).await {
        Ok(joined) => joined.expect("wall-timed-out child task exits cleanly"),
        Err(_) => {
            task_handle.abort();
            panic!("launch-permit wait escaped the authored child wall timeout");
        }
    }
    assert_eq!(
        gate.available_permits(),
        0,
        "the task must time out while the test still holds the launch permit"
    );

    let manager = manager.read().await;
    let snapshot = manager
        .get_result(&agent_id)
        .expect("timed-out child remains inspectable");
    let SubAgentStatus::Failed(error) = &snapshot.status else {
        panic!("wall timeout must be a typed child failure: {snapshot:?}");
    };
    assert!(
        error.contains("child wall-time budget exhausted"),
        "{error}"
    );

    let worker = manager
        .get_worker_record(&agent_id)
        .expect("timed-out durable worker remains inspectable");
    assert_eq!(worker.status, AgentWorkerStatus::Failed);
    assert_eq!(worker.error.as_deref(), Some(error.as_str()));
    assert!(
        worker
            .events
            .iter()
            .any(|event| event.status == AgentWorkerStatus::Queued),
        "worker receipt must retain the launch-queue phase: {worker:?}"
    );
    assert_eq!(
        worker.events.back().map(|event| event.status),
        Some(AgentWorkerStatus::Failed),
        "worker receipt must close with a typed failure: {worker:?}"
    );

    drop(manager);
    drop(held_launch_permit);
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
    wall_time: Duration,
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
        wall_time,
        input_rx: task_input_rx,
        launch_gate: None,
    };
    let task_handle = tokio::spawn(run_subagent_task(task));
    (manager, agent_id, calls, task_handle)
}

#[tokio::test]
async fn worker_stops_with_typed_wall_time_reason() {
    let tmp = tempdir().expect("tempdir");
    let (manager, agent_id, _calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, None, 120, Duration::from_millis(1)).await;

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("wall-time-capped worker must terminate")
        .expect("task should finish");

    let result = manager
        .read()
        .await
        .get_result(&agent_id)
        .expect("agent registered");
    match result.status {
        SubAgentStatus::Failed(reason) => {
            assert!(reason.contains("wall-time budget exhausted"), "{reason}");
            assert!(reason.contains("limit:"), "{reason}");
            assert!(reason.contains("wall_time_secs"), "{reason}");
        }
        other => panic!("expected typed wall-time failure, got {other:?}"),
    }
}

#[tokio::test]
async fn worker_stops_when_per_worker_token_budget_exceeded() {
    let tmp = tempdir().expect("tempdir");
    // 100 tokens/turn (60 in + 40 out) vs a 50-token cap: the worker must
    // stop with `BudgetExhausted` after its very first model turn instead of
    // running on to `max_steps`.
    let (manager, agent_id, calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4, DEFAULT_CHILD_WALL_TIME).await;

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
        spawn_budget_capped_worker(tmp.path(), 60, 40, None, 4, DEFAULT_CHILD_WALL_TIME).await;

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
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4, DEFAULT_CHILD_WALL_TIME).await;

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

/// Clears the process-wide rate-limit window on drop so a panicking test
/// body cannot leak a live pause into concurrently running tests.
struct ClearRateLimitOnDrop;

impl Drop for ClearRateLimitOnDrop {
    fn drop(&mut self) {
        crate::retry_status::clear_rate_limit();
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn worker_is_not_stranded_by_transient_global_rate_limit_window() {
    // Regression for a parallel-suite flake: `rate_limit_pause_blocks_subagent_spawn`
    // opens a 30s process-wide rate-limit window and closes it milliseconds
    // later. A worker whose request reached `send_with_retry` inside that
    // window used to commit to sleeping the FULL remaining window without
    // re-checking, blowing the 5s timeouts in the budget tests above. The
    // pause must be re-polled so an already-cleared window releases
    // in-flight requests promptly.
    let _guard = crate::retry_status::test_guard();
    let _clear = ClearRateLimitOnDrop;
    crate::retry_status::note_rate_limit(Duration::from_secs(30));

    let tmp = tempdir().expect("tempdir");
    let (manager, agent_id, _calls, task_handle) =
        spawn_budget_capped_worker(tmp.path(), 60, 40, Some(50), 4, DEFAULT_CHILD_WALL_TIME).await;

    // Simulate the concurrent test finishing: the window closes shortly
    // after the worker's first request has already observed it.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        crate::retry_status::clear_rate_limit();
    });

    tokio::time::timeout(Duration::from_secs(5), task_handle)
        .await
        .expect("worker must not be stranded by an already-cleared rate-limit window")
        .expect("task should finish");

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

/// #4217: terminal worker records must age out of the persisted ledger so
/// long-lived sessions do not rewrite multi-MB `subagents.v1.json` forever.
#[test]
fn cleanup_evicts_stale_terminal_worker_records_and_keeps_live_ones() {
    let tmp = tempdir().expect("tempdir");
    let state_path = tmp.path().join("subagents.v1.json");
    let mut manager =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path.clone());

    manager.register_worker(make_worker_spec("agent_old_done", tmp.path().to_path_buf()));
    manager.register_worker(make_worker_spec(
        "agent_recent_done",
        tmp.path().to_path_buf(),
    ));
    manager.register_worker(make_worker_spec(
        "agent_still_running",
        tmp.path().to_path_buf(),
    ));

    let mut old_done = make_snapshot(SubAgentStatus::Completed);
    old_done.agent_id = "agent_old_done".to_string();
    old_done.name = "agent_old_done".to_string();
    manager.complete_worker_from_result("agent_old_done", &old_done);

    let mut recent_done = make_snapshot(SubAgentStatus::Failed("boom".to_string()));
    recent_done.agent_id = "agent_recent_done".to_string();
    recent_done.name = "agent_recent_done".to_string();
    manager.complete_worker_from_result("agent_recent_done", &recent_done);

    manager.record_worker_event(
        "agent_still_running",
        AgentWorkerStatus::Running,
        Some("working".to_string()),
        Some(1),
        None,
    );

    let now_ms = epoch_millis_now();
    let two_hours_ago = now_ms.saturating_sub(2 * 60 * 60 * 1000);
    {
        let old = manager
            .worker_records
            .get_mut("agent_old_done")
            .expect("old terminal worker");
        old.completed_at_ms = Some(two_hours_ago);
        old.updated_at_ms = two_hours_ago;
    }

    // One-hour retention matches COMPLETED_AGENT_RETENTION used by cleanup callers.
    let auto_cancelled = manager.cleanup(Duration::from_secs(60 * 60));
    assert_eq!(auto_cancelled, 0);

    assert!(
        manager.get_worker_record("agent_old_done").is_none(),
        "terminal worker older than retention must be evicted"
    );
    assert!(
        manager.get_worker_record("agent_recent_done").is_some(),
        "recent terminal worker must be retained"
    );
    let running = manager
        .get_worker_record("agent_still_running")
        .expect("running worker");
    assert_eq!(running.status, AgentWorkerStatus::Running);

    // Persist the pruned ledger and confirm eviction survives reload.
    manager
        .persist_state()
        .expect("persist after cleanup")
        .join()
        .expect("persist thread");
    let mut reloaded =
        SubAgentManager::new(tmp.path().to_path_buf(), 4).with_state_path(state_path);
    reloaded.load_state().expect("load pruned state");
    assert!(
        reloaded.get_worker_record("agent_old_done").is_none(),
        "eviction must survive reload of subagents.v1.json"
    );
    assert!(reloaded.get_worker_record("agent_recent_done").is_some());
    assert!(reloaded.get_worker_record("agent_still_running").is_some());
}

#[test]
fn cleanup_removes_complete_transcript_after_worker_retention_expires() {
    let tmp = tempdir().expect("tempdir");
    let agent_id = "agent_expired_transcript";
    let mut manager = SubAgentManager::new(tmp.path().to_path_buf(), 2);
    manager.register_worker(make_worker_spec(agent_id, tmp.path().to_path_buf()));
    let record = manager
        .worker_records
        .get_mut(agent_id)
        .expect("worker record");
    record.status = AgentWorkerStatus::Completed;
    let expired = epoch_millis_now().saturating_sub(2 * 60 * 60 * 1000);
    record.completed_at_ms = Some(expired);
    record.updated_at_ms = expired;

    let messages = vec![text_message("user", "retained until ledger cleanup")];
    let artifact = write_subagent_transcript_artifact_for_test(tmp.path(), agent_id, &messages)
        .expect("write transcript artifact");
    assert!(artifact.exists());

    manager.cleanup(Duration::from_secs(60 * 60));

    assert!(manager.get_worker_record(agent_id).is_none());
    assert!(
        !artifact.exists(),
        "artifact must share the terminal worker retention lifecycle"
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
    let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let prior_artifacts =
        crate::artifacts::set_test_artifact_sessions_root(Some(root.join("sessions")));
    struct Restore(Option<std::path::PathBuf>, Option<std::path::PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.0.take());
            crate::artifacts::set_test_artifact_sessions_root(self.1.take());
        }
    }
    let _restore = Restore(prior, prior_artifacts);
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

#[tokio::test]
async fn complete_transcript_artifact_survives_resident_handle_compaction() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime();
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
    let agent_id = "agent_complete_transcript";
    let early = format!("EARLY-TURN-MARKER\n{}", "x".repeat(1_100_000));
    let messages = vec![
        text_message("user", &early),
        text_message("assistant", "LAST-TURN-MARKER"),
    ];
    let mut artifact = SubAgentTranscriptArtifactWriter::for_runtime(&runtime, agent_id)
        .await
        .expect("create private transcript artifact");
    let artifact_path = artifact.path.clone();

    let handle = insert_subagent_full_transcript_handle(
        &runtime,
        agent_id,
        &SubAgentType::General,
        &make_assignment(),
        &SubAgentStatus::Completed,
        Some(&"LAST-TURN-MARKER".to_string()),
        None,
        Some(&mut artifact),
        &messages,
        1,
        10,
        false,
    )
    .await;

    let store = runtime.context.runtime.handle_store.lock().await;
    let record = store.get(&handle).expect("resident transcript handle");
    let crate::tools::handle::HandleValue::Json(payload) = &record.value else {
        panic!("sub-agent transcript handle must remain JSON");
    };
    assert_eq!(payload["omitted_messages"], json!(1));
    assert_eq!(payload["messages_complete"], json!(false));
    assert_eq!(
        payload["complete_transcript_artifact"]["complete"],
        json!(true)
    );
    assert!(
        !payload.to_string().contains("EARLY-TURN-MARKER"),
        "the >1 MiB early turn must not remain resident in the bounded handle"
    );
    drop(store);

    let restored = load_subagent_transcript_artifact(tmp.path(), agent_id)
        .expect("load complete transcript artifact");
    assert_eq!(restored.len(), messages.len());
    assert!(message_text(&restored[0]).starts_with("EARLY-TURN-MARKER"));
    assert_eq!(message_text(&restored[1]), "LAST-TURN-MARKER");
    assert!(artifact_path.starts_with(tmp.path().canonicalize().unwrap()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&artifact_path)
                .expect("artifact metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "worker chats may contain credentials and must stay private"
        );
    }
}

#[test]
fn malformed_transcript_artifact_fails_closed_instead_of_showing_partial_chat() {
    let tmp = tempdir().expect("tempdir");
    let agent_id = "agent_malformed_transcript";
    let artifact = write_subagent_transcript_artifact_for_test(
        tmp.path(),
        agent_id,
        &[text_message("user", "valid first turn")],
    )
    .expect("write transcript artifact");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&artifact)
        .expect("open artifact")
        .write_all(b"{not valid json}\n")
        .expect("append malformed record");

    let error = load_subagent_transcript_artifact(tmp.path(), agent_id)
        .expect_err("a malformed stream must not masquerade as a complete chat");
    assert!(error.to_string().contains("line"), "{error:#}");
}

#[cfg(unix)]
#[test]
fn transcript_artifact_reader_rejects_symlink_replacement() {
    use std::os::unix::fs::symlink;

    let tmp = tempdir().expect("tempdir");
    let agent_id = "agent_symlink_transcript";
    let artifact = write_subagent_transcript_artifact_for_test(
        tmp.path(),
        agent_id,
        &[text_message("user", "private worker chat")],
    )
    .expect("write transcript artifact");
    let outside = tmp.path().join("outside.jsonl");
    std::fs::write(&outside, "not a transcript").expect("outside file");
    std::fs::remove_file(&artifact).expect("remove artifact");
    symlink(&outside, &artifact).expect("replace with symlink");

    let error = load_subagent_transcript_artifact(tmp.path(), agent_id)
        .expect_err("transcript reader must reject symlink replacement");
    assert!(error.to_string().contains("must not traverse symlinks"));
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

        let (inline, spilled) = bound_subagent_tool_result(
            "fleet-worker-1",
            "call-42",
            "exec_shell",
            "session-test",
            true,
            raw.clone(),
        );

        let path = spilled.expect("multi-MB output must spill");
        // Model-visible content is bounded to head + footer.
        assert!(inline.len() <= 4 * 1024);
        assert!(inline.contains("Exact evidence retained"));
        assert!(!inline.contains(&path.display().to_string()));
        assert!(inline.contains("retrieve_tool_result"));
        // Full output remains recoverable from disk.
        let on_disk = std::fs::read_to_string(&path).expect("spill file readable");
        assert_eq!(on_disk.len(), raw_len);

        // Small outputs pass through untouched, no spill file.
        let (small, spilled) = bound_subagent_tool_result(
            "fleet-worker-1",
            "call-43",
            "read_file",
            "session-test",
            true,
            "ok".to_string(),
        );
        assert_eq!(small, "ok");
        assert!(spilled.is_none());

        // Oversized error output is bounded too: sub-agent errors are
        // routinely full build logs, unlike the root loop's short errors.
        let (bounded_err, spilled) = bound_subagent_tool_result(
            "fleet-worker-1",
            "call-44",
            "exec_shell",
            "session-test",
            false,
            format!("Error: {raw}"),
        );
        assert!(spilled.is_some());
        assert!(bounded_err.len() <= 4 * 1024);
        assert!(bounded_err.contains("Exact evidence retained"));
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
                let (inline, spilled) = bound_subagent_tool_result(
                    &agent_id,
                    &format!("call-{call}"),
                    "exec_shell",
                    "session-test",
                    true,
                    huge.clone(),
                );
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
            let payload = PersistedSubAgentState {
                snapshot_sequence: i + 1,
                ..PersistedSubAgentState::default()
            };
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
    assert!(parsed.get("snapshot_sequence").is_some());
    // No stray temp files left behind.
    let leftover: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .expect("read subagents dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftover.is_empty(), "temp files leaked: {leftover:?}");
}

#[test]
fn coordination_process_lock_rejects_second_process() {
    const ROLE_ENV: &str = "CODEWHALE_TEST_COORDINATION_LOCK_ROLE";
    const WORKSPACE_ENV: &str = "CODEWHALE_TEST_COORDINATION_LOCK_WORKSPACE";
    const TEST_NAME: &str =
        "tools::subagent::tests::coordination_process_lock_rejects_second_process";

    if let Some(role) = std::env::var_os(ROLE_ENV) {
        let workspace = PathBuf::from(std::env::var_os(WORKSPACE_ENV).expect("workspace env"));
        let manager = new_shared_subagent_manager_with_timeout(
            workspace.clone(),
            4,
            4,
            Duration::from_secs(30),
            4,
            None,
        );
        if role == "holder" {
            manager
                .try_read()
                .unwrap()
                .ensure_coordination_process_lock()
                .expect("holder owns lock");
            std::fs::write(workspace.join("holder.ready"), b"ready").unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !workspace.join("holder.release").exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(workspace.join("holder.release").exists(), "release timeout");
        } else {
            let error = manager
                .try_read()
                .unwrap()
                .ensure_coordination_process_lock()
                .expect_err("second process must fail closed");
            assert!(error.contains("another Codewhale process"), "{error}");
        }
        return;
    }

    let dir = tempdir().expect("tempdir");
    let workspace = dir.path().canonicalize().expect("canonical workspace");
    let test_binary = std::env::current_exe().expect("test binary");
    let mut holder = std::process::Command::new(&test_binary)
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(ROLE_ENV, "holder")
        .env(WORKSPACE_ENV, &workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn lock holder");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !workspace.join("holder.ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !workspace.join("holder.ready").exists() {
        let _ = holder.kill();
        let output = holder.wait_with_output().expect("holder output");
        panic!(
            "holder never acquired lock:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let contender = std::process::Command::new(&test_binary)
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(ROLE_ENV, "contender")
        .env(WORKSPACE_ENV, &workspace)
        .output()
        .expect("spawn lock contender");
    assert!(
        contender.status.success(),
        "contender failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&contender.stdout),
        String::from_utf8_lossy(&contender.stderr)
    );

    std::fs::write(workspace.join("holder.release"), b"release").unwrap();
    let output = holder.wait_with_output().expect("holder output");
    assert!(
        output.status.success(),
        "holder failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// === agent(action="wait") + peek throttling (#4097) ===

fn insert_running_agent(inner: &mut SubAgentManager, name: &str) -> String {
    let current_boot = inner.session_boot_id().to_string();
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let mut agent = SubAgent::new(
        name.to_string(),
        SubAgentType::Explore,
        "prompt".to_string(),
        make_assignment(),
        "deepseek-v4-flash".to_string(),
        None,
        None,
        input_tx,
        PathBuf::from("."),
        current_boot,
    );
    agent.task_handle = Some(tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }));
    let agent_id = agent.id.clone();
    inner.agents.insert(agent_id.clone(), agent);
    agent_id
}

#[tokio::test]
async fn agent_wait_returns_immediately_with_no_children() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    let context = ToolContext::new(".");
    let result = wait_for_subagents_from_input(&json!({"action": "wait"}), manager, &context)
        .await
        .expect("wait with no children should succeed");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    assert_eq!(payload["running"], json!(0));
    assert!(
        payload["settled"]
            .as_array()
            .expect("settled array")
            .is_empty()
    );
}

#[tokio::test]
async fn agent_wait_wakes_when_child_settles() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let agent_id = insert_running_agent(&mut inner, "test_agent_wait_settles");
    let manager = Arc::new(RwLock::new(inner));

    let flip = manager.clone();
    let flip_id = agent_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut manager = flip.write().await;
        if let Some(agent) = manager.agents.get_mut(&flip_id) {
            agent.status = SubAgentStatus::Completed;
        }
    });

    let context = ToolContext::new(".");
    let started = Instant::now();
    let result = wait_for_subagents_from_input(
        &json!({"action": "wait", "timeout_secs": 30}),
        manager,
        &context,
    )
    .await
    .expect("wait should succeed");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wait must wake on settle, not run out the 30s timeout"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    let settled = payload["settled"].as_array().expect("settled array");
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0]["agent_id"], json!(agent_id));
    assert_eq!(settled[0]["status"], json!("completed"));
    assert_eq!(payload["timed_out"], json!(false));
}

#[tokio::test]
async fn agent_wait_times_out_and_reports_running_child() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let _agent_id = insert_running_agent(&mut inner, "test_agent_wait_timeout");
    let manager = Arc::new(RwLock::new(inner));

    let context = ToolContext::new(".");
    let result = wait_for_subagents_from_input(
        &json!({"action": "wait", "timeout_secs": 1}),
        manager,
        &context,
    )
    .await
    .expect("wait timeout should return a snapshot, not an error");
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("wait payload should be json");
    assert_eq!(payload["timed_out"], json!(true));
    assert_eq!(payload["running"], json!(1));
    assert!(
        payload["settled"]
            .as_array()
            .expect("settled array")
            .is_empty()
    );
}

#[tokio::test]
async fn agent_wait_rejects_unknown_agent_ref() {
    let manager = Arc::new(RwLock::new(SubAgentManager::new(PathBuf::from("."), 1)));
    let context = ToolContext::new(".");
    let err = wait_for_subagents_from_input(
        &json!({"action": "wait", "agent_id": "agent_missing"}),
        manager,
        &context,
    )
    .await
    .expect_err("unknown agent ref must fail fast instead of blocking");
    assert!(matches!(err, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn agent_peek_unchanged_within_window_returns_compact_nudge() {
    let mut inner = SubAgentManager::new(PathBuf::from("."), 1);
    let agent_id = insert_running_agent(&mut inner, "test_agent_peek_throttle");
    let manager = Arc::new(RwLock::new(inner));
    let memo: Arc<std::sync::Mutex<HashMap<String, PeekMemo>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let context = ToolContext::new(".");
    let input = json!({"action": "peek", "agent_id": agent_id});

    let first = inspect_agent_from_input(&input, manager.clone(), &context, true, Some(&memo))
        .await
        .expect("first peek should succeed");
    let first_payload: serde_json::Value =
        serde_json::from_str(&first.content).expect("first peek payload should be json");
    assert!(
        first_payload.get("unchanged").is_none(),
        "first peek must return the full projection"
    );

    let second = inspect_agent_from_input(&input, manager, &context, true, Some(&memo))
        .await
        .expect("second peek should succeed");
    let second_payload: serde_json::Value =
        serde_json::from_str(&second.content).expect("second peek payload should be json");
    assert_eq!(second_payload["unchanged"], json!(true));
    assert!(
        second_payload["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("wait"),
        "nudge should point at agent(action=wait)"
    );
}

#[test]
fn agent_action_parses_wait_aliases() {
    for alias in ["wait", "join", "await", "block"] {
        assert_eq!(
            parse_agent_tool_action(&json!({"action": alias})).expect("alias should parse"),
            AgentToolAction::Wait,
        );
    }
}

// ===========================================================================
// #4042 — sub-agent tool restriction inheritance (Phase 1, harvested from
// PR #4096 by @JayBeest).
//
// These tests verify that the parent session's `--disallowed-tools` flows into
// spawned sub-agents through `SubAgentRuntime` → `SubAgentToolRegistry`. The
// deny-list is stamped onto `worker_profile.denied_tools` by the engine and
// cloned through `child_runtime()`/`background_runtime()`, so a registry built
// from a child runtime enforces it in `is_tool_allowed()`, `tools_for_model()`,
// and `execute()`.
//
// Deny always wins over allow. Wildcards (`prefix*`) and case-insensitive
// matching mirror the session-side `command_denies_tool()`.
// ===========================================================================

/// Build a stub runtime with the parent's `disallowed_tools` set on the
/// `WorkerRuntimeProfile`. The registry reads deny lists from the profile at
/// construction, and `child_runtime()` clones the profile so the list
/// propagates across generations.
fn stub_runtime_with_disallowed(disallowed: Vec<String>) -> SubAgentRuntime {
    let mut rt = stub_runtime();
    rt.worker_profile.denied_tools = disallowed;
    rt
}

/// Build a `SubAgentToolRegistry` wired with `disallowed_tools`. Passes the
/// runtime through `SubAgentToolRegistry::new()` so the constructor picks up
/// `worker_profile.denied_tools`. `allowed_tools` is forwarded directly.
fn new_registry_with_disallowed(
    runtime: SubAgentRuntime,
    allowed_tools: Option<Vec<String>>,
) -> SubAgentToolRegistry {
    SubAgentToolRegistry::new(
        runtime,
        SubAgentType::General,
        allowed_tools,
        Arc::new(Mutex::new(TodoList::new())),
        Arc::new(Mutex::new(PlanState::default())),
    )
}

#[test]
fn test_disallowed_tools_inheritance_denies_tool() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "exec_shell should be denied"
    );
    assert!(
        !registry.is_tool_allowed("write_file"),
        "write_file should be denied"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "read_file should still be allowed"
    );
    assert!(
        registry.is_tool_allowed("grep_files"),
        "unrelated tools should be allowed"
    );

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.iter().map(|t| t.name.clone()).collect();
    assert!(!names.contains("exec_shell"), "catalog excludes exec_shell");
    assert!(!names.contains("write_file"), "catalog excludes write_file");
    assert!(names.contains("File"), "catalog includes canonical File");
    let file = tools.into_iter().find(|tool| tool.name == "File").unwrap();
    let actions = file.input_schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap();
    assert!(
        !actions
            .iter()
            .any(|action| action.as_str() == Some("write"))
    );
}

#[test]
fn test_disallowed_tools_deny_wins_over_allow() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    // exec_shell is in BOTH the allowlist AND the deny list — deny must win.
    let registry = new_registry_with_disallowed(
        runtime,
        Some(vec!["exec_shell".to_string(), "read_file".to_string()]),
    );

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "deny must win over allow"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "read_file is allowed and not denied"
    );

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.iter().map(|t| t.name.clone()).collect();
    assert!(
        !names.contains("exec_shell"),
        "catalog must exclude denied tool even when allowlisted"
    );
    assert!(
        names.contains("File"),
        "legacy read allow exposes canonical File"
    );
    assert!(!names.contains("Bash"), "denied shell alias removes Bash");
}

#[test]
fn test_disallowed_tools_wildcard_matching() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["mcp_*".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("mcp_github_list_prs"),
        "mcp_* wildcard should deny all MCP tools"
    );
    assert!(
        !registry.is_tool_allowed("mcp_database_query"),
        "mcp_* wildcard denies any server prefix"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "non-MCP tools are unaffected by mcp_* deny"
    );
}

#[test]
fn test_disallowed_tools_case_insensitive_match() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["Exec_Shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("exec_shell"),
        "case-insensitive: Exec_Shell denies exec_shell"
    );
    assert!(
        !registry.is_tool_allowed("EXEC_SHELL"),
        "case-insensitive: Exec_Shell denies EXEC_SHELL"
    );
    assert!(
        registry.is_tool_allowed("read_file"),
        "unrelated tool unaffected"
    );
}

#[test]
fn test_disallowed_tools_specific_server_wildcard() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["mcp_dangerous_*".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    assert!(
        !registry.is_tool_allowed("mcp_dangerous_read"),
        "specific server wildcard denies its tools"
    );
    assert!(
        registry.is_tool_allowed("mcp_safe_query"),
        "different server prefix is not denied"
    );
}

#[test]
fn test_disallowed_tools_tools_for_model_excludes_denied() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec![
        "exec_shell".to_string(),
        "write_file".to_string(),
        "apply_patch".to_string(),
    ]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    let registry = new_registry_with_disallowed(runtime, None);

    let tools = registry.tools_for_model(&SubAgentType::General);
    let names: HashSet<_> = tools.iter().map(|t| t.name.clone()).collect();

    assert!(!names.contains("exec_shell"), "catalog excludes exec_shell");
    assert!(!names.contains("write_file"), "catalog excludes write_file");
    assert!(
        !names.contains("apply_patch"),
        "catalog excludes apply_patch"
    );
    assert!(names.contains("File"), "catalog includes canonical File");
    let file = tools.into_iter().find(|tool| tool.name == "File").unwrap();
    let actions = file.input_schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap();
    for denied in ["write", "patch"] {
        assert!(!actions.iter().any(|action| action.as_str() == Some(denied)));
    }
}

#[tokio::test]
async fn test_disallowed_tools_execute_rejects_denied_tool() {
    let tmp = tempdir().expect("tempdir");
    let mut runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    runtime.context = ToolContext::new(tmp.path().to_path_buf());
    runtime.allow_shell = true; // remove posture as a confound
    let registry = new_registry_with_disallowed(runtime, None);

    let result = registry
        .execute("agent_test", "exec_shell", json!({"command": "echo hi"}))
        .await;
    assert!(
        result.is_err(),
        "execute must reject a tool denied by disallowed_tools"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not allowed") || err.contains("denied"),
        "error should mention denial: {err}"
    );
}

// === deny-list propagation through runtime cloning ===

#[test]
fn test_disallowed_tools_propagates_through_child_runtime() {
    let runtime = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    let child = runtime.child_runtime();
    assert_eq!(
        child.worker_profile.denied_tools,
        vec!["exec_shell".to_string()],
        "child_runtime() must preserve parent's denied_tools"
    );
}

#[test]
fn test_disallowed_tools_propagates_through_background_runtime() {
    let runtime = stub_runtime_with_disallowed(vec!["write_file".to_string()]);
    let bg = runtime.background_runtime();
    assert_eq!(
        bg.worker_profile.denied_tools,
        vec!["write_file".to_string()],
        "background_runtime() must preserve parent's denied_tools"
    );
}

#[test]
fn test_disallowed_tools_across_two_generations() {
    let tmp = tempdir().expect("tempdir");
    let mut parent = stub_runtime_with_disallowed(vec!["exec_shell".to_string()]);
    parent.context = ToolContext::new(tmp.path().to_path_buf());
    let parent_registry = new_registry_with_disallowed(parent.clone(), None);
    assert!(!parent_registry.is_tool_allowed("exec_shell"));

    // Child A inherits from parent.
    let child_a = parent.child_runtime();
    assert_eq!(
        child_a.worker_profile.denied_tools,
        vec!["exec_shell".to_string()]
    );

    // Child B inherits from child A — same deny list.
    let mut child_b = child_a.child_runtime();
    child_b.context = ToolContext::new(tmp.path().to_path_buf());
    let b_registry = new_registry_with_disallowed(child_b, None);
    assert!(
        !b_registry.is_tool_allowed("exec_shell"),
        "third-generation sub-agent still inherits deny list"
    );
    assert!(b_registry.is_tool_allowed("read_file"));
}

// === spawn-path opt-out simulation ===

#[test]
fn test_disallowed_tools_opt_out_clears_inherited_denies() {
    // Simulate the spawn-path merge: parent runtime has denies, child sets
    // inherit_disallowed_tools = false — the inherited denies are cleared.
    let tmp = tempdir().expect("tempdir");
    let runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    let mut child_runtime = runtime.child_runtime();
    child_runtime.context = ToolContext::new(tmp.path().to_path_buf());
    assert!(
        !child_runtime.worker_profile.denied_tools.is_empty(),
        "child starts with parent's denies"
    );

    // Simulate spawn merge: inherit_disallowed_tools = false, no caller deny.
    child_runtime.worker_profile.denied_tools.clear();

    let registry = new_registry_with_disallowed(child_runtime, None);
    assert!(
        registry.is_tool_allowed("exec_shell"),
        "exec_shell allowed after opt-out cleared parent denies"
    );
    assert!(
        registry.is_tool_allowed("write_file"),
        "write_file allowed after opt-out cleared parent denies"
    );
    assert!(registry.is_tool_allowed("read_file"));
}

#[test]
fn test_disallowed_tools_opt_out_keeps_explicit_caller_deny() {
    // Opt-out clears inherited denies, but explicit caller disallowed_tools
    // still apply (the union merge — caller deny always applies).
    let tmp = tempdir().expect("tempdir");
    let runtime =
        stub_runtime_with_disallowed(vec!["exec_shell".to_string(), "write_file".to_string()]);
    let mut child_runtime = runtime.child_runtime();
    child_runtime.context = ToolContext::new(tmp.path().to_path_buf());

    // Simulate spawn merge: inherit_disallowed_tools = false, then caller adds
    // ["write_file"].
    child_runtime.worker_profile.denied_tools.clear();
    child_runtime
        .worker_profile
        .denied_tools
        .push("write_file".to_string());

    let registry = new_registry_with_disallowed(child_runtime, None);
    // Parent denied exec_shell, but opt-out cleared it → allowed.
    assert!(
        registry.is_tool_allowed("exec_shell"),
        "exec_shell allowed (parent deny cleared by opt-out)"
    );
    // Caller explicitly denied write_file → still denied.
    assert!(
        !registry.is_tool_allowed("write_file"),
        "write_file denied by caller's explicit list"
    );
    assert!(registry.is_tool_allowed("read_file"));
}

// === parse_spawn_request disallowed_tools ===

#[test]
fn test_parse_spawn_request_reads_disallowed_tools() {
    let input = json!({
        "prompt": "do something",
        "disallowed_tools": ["exec_shell", "write_file"]
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert_eq!(
        req.disallowed_tools,
        Some(vec!["exec_shell".to_string(), "write_file".to_string()])
    );
}

#[test]
fn test_parse_spawn_request_disallowed_tools_dedupes_and_trims() {
    let input = json!({
        "prompt": "do something",
        "disallowed_tools": [" exec_shell ", "exec_shell", "", "  ", "write_file"]
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert_eq!(
        req.disallowed_tools,
        Some(vec!["exec_shell".to_string(), "write_file".to_string()]),
        "blanks and duplicates are dropped"
    );
}

#[test]
fn test_parse_spawn_request_disallowed_tools_defaults_to_none() {
    let input = json!({"prompt": "do something"});
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        req.disallowed_tools.is_none(),
        "disallowed_tools should be None when not provided"
    );
}

#[test]
fn test_parse_spawn_request_inherit_disallowed_tools_defaults_true() {
    let input = json!({"prompt": "do something"});
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        req.inherit_disallowed_tools,
        "inherit_disallowed_tools should default to true"
    );
}

#[test]
fn test_parse_spawn_request_inherit_disallowed_tools_explicit_false() {
    let input = json!({
        "prompt": "do something",
        "inherit_disallowed_tools": false
    });
    let req = parse_spawn_request(&input).expect("parse");
    assert!(
        !req.inherit_disallowed_tools,
        "inherit_disallowed_tools should parse an explicit false"
    );
}
