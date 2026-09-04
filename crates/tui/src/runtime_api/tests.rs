use super::*;
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use crate::core::ops::Op;
use crate::models::Role;
use crate::models::Usage;
use crate::runtime_threads::RuntimeEventRecord;
use crate::test_support::{EnvVarGuard, lock_test_env};
use anyhow::{Context, bail};
use futures_util::StreamExt;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::sleep;
use uuid::Uuid;

/// Scale a wait budget for shared CI runners.
///
/// These deadlines are tuned for a developer laptop running one test at a
/// time. CI runs the whole workspace suite on a shared runner, where the same
/// async progress can legitimately take several times longer. Every budget
/// guarded by this helper is a deadline on a poll or a oneshot that resolves
/// as soon as the runtime makes progress, so a larger budget does not slow a
/// passing run down — it only changes how long a genuinely stuck test waits
/// before it fails. Keeping the local value tight preserves fast feedback
/// while the tests stop failing for being unlucky about scheduling.
fn ci_scaled(base: Duration) -> Duration {
    if std::env::var_os("CI").is_some() {
        base * 4
    } else {
        base
    }
}

struct MockExecutor;

#[cfg(unix)]
#[test]
fn runtime_session_fallback_retains_non_unicode_explicit_home_boundary() {
    use std::os::unix::ffi::OsStringExt;

    let _lock = lock_test_env();
    let tmp = tempfile::tempdir().expect("temporary root");
    let home = tmp.path().join("home");
    let explicit = tmp.path().join(std::ffi::OsString::from_vec(
        b"codewhale-\xff-home".to_vec(),
    ));
    let _home = EnvVarGuard::set("HOME", &home);
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit);

    assert_eq!(fallback_sessions_dir(), explicit.join("sessions"));
}

#[test]
fn thread_route_credential_error_is_bad_request_not_not_found() {
    let credential = map_thread_err(anyhow::anyhow!("DeepSeek API key not found"));
    assert_eq!(credential.status, StatusCode::BAD_REQUEST);

    let missing = map_thread_err(anyhow::anyhow!("thread 'thr_missing' not found"));
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let operation_mismatch = map_thread_err(anyhow::anyhow!(
        "operation_key is already bound to a different turn request"
    ));
    assert_eq!(operation_mismatch.status, StatusCode::CONFLICT);
}

#[test]
fn web_launcher_failure_is_a_recoverable_manual_bootstrap_warning() {
    assert!(web_launcher_warning(Ok(())).is_none());
    let warning = web_launcher_warning(Err(anyhow::anyhow!("launcher unavailable")))
        .expect("launcher failure should be reported without failing Runtime startup");
    assert!(warning.contains("could not open the default browser"));
    assert!(warning.contains("open the bootstrap URL above manually"));
}

fn provider_default_model_cases() -> Vec<(&'static str, Config, &'static str)> {
    let deepseek = Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("deepseek-test-key".to_string()),
        base_url: Some("http://127.0.0.1:1/v1".to_string()),
        default_text_model: Some("deepseek-v4-flash".to_string()),
        ..Config::default()
    };

    let mut zai_providers = crate::config::ProvidersConfig::default();
    zai_providers.zai.api_key = Some("zai-test-key".to_string());
    let zai = Config {
        provider: Some("zai".to_string()),
        // A saved DeepSeek root default must not leak onto the active Z.ai route.
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        providers: Some(zai_providers),
        ..Config::default()
    };

    let mut custom_providers = crate::config::ProvidersConfig::default();
    custom_providers.custom.insert(
        "acme".to_string(),
        crate::config::ProviderConfig {
            api_key: Some("acme-test-key".to_string()),
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            model: Some("acme-coder".to_string()),
            kind: Some("openai-compatible".to_string()),
            ..crate::config::ProviderConfig::default()
        },
    );
    let custom = Config {
        provider: Some("acme".to_string()),
        providers: Some(custom_providers),
        ..Config::default()
    };

    vec![
        ("deepseek", deepseek, "deepseek-v4-flash"),
        ("zai", zai, crate::config::DEFAULT_ZAI_MODEL),
        ("custom", custom, "acme-coder"),
    ]
}

#[test]
fn runtime_request_model_uses_the_active_provider_default() {
    for (label, config, expected) in provider_default_model_cases() {
        assert_eq!(
            runtime_request_model(&config, None),
            expected,
            "{label} omitted-model resolution"
        );
        assert_eq!(
            runtime_request_model(&config, Some("explicit-model")),
            "explicit-model",
            "{label} explicit model"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_and_web_server_thread_manager_installs_configured_workshop_byte_budgets() -> Result<()>
{
    let _workshop_guard = crate::tools::large_output_router::active_workshop_test_guard();
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace)?;
    let config = Config {
        workshop: Some(crate::tools::large_output_router::WorkshopConfig {
            read_result_max_bytes: Some(73_728),
            tool_result_max_bytes: Some(65_536),
            ..crate::tools::large_output_router::WorkshopConfig::default()
        }),
        ..Config::default()
    };

    let (runtime_threads, startup_activation) = open_runtime_threads_for_server(
        &config,
        workspace.clone(),
        RuntimeThreadManagerConfig {
            data_dir: temp.path().join("runtime"),
            task_data_dir: temp.path().join("tasks"),
            max_active_threads: 2,
        },
        Arc::new(crate::plugins::PluginRegistry::empty(&workspace)),
    )?;

    assert_eq!(startup_activation.read_result_max_bytes, Some(73_728));
    assert_eq!(startup_activation.tool_result_max_bytes, Some(65_536));
    assert_eq!(
        crate::tools::large_output_router::WorkshopConfig::active_read_result_max_bytes(),
        Some(73_728)
    );
    assert_eq!(
        crate::tools::large_output_router::WorkshopConfig::active_tool_result_max_bytes(),
        Some(65_536)
    );

    let reload_activation = runtime_threads
        .reload_config(Config {
            workshop: Some(crate::tools::large_output_router::WorkshopConfig {
                read_result_max_bytes: Some(81_920),
                tool_result_max_bytes: Some(77_824),
                ..crate::tools::large_output_router::WorkshopConfig::default()
            }),
            ..Config::default()
        })
        .await?;
    assert_eq!(reload_activation.read_result_max_bytes, Some(81_920));
    assert_eq!(reload_activation.tool_result_max_bytes, Some(77_824));
    assert_eq!(
        crate::tools::large_output_router::WorkshopConfig::active_read_result_max_bytes(),
        Some(81_920)
    );
    assert_eq!(
        crate::tools::large_output_router::WorkshopConfig::active_tool_result_max_bytes(),
        Some(77_824)
    );
    crate::tools::large_output_router::WorkshopConfig::install_active(None);
    Ok(())
}

#[test]
fn runtime_tui_settings_reject_legacy_modes_and_do_not_save_env_overlays() -> Result<()> {
    let _lock = lock_test_env();
    let tmp = tempfile::tempdir()?;
    let settings_dir = tmp.path().join(".codewhale");
    fs::create_dir_all(&settings_dir)?;
    fs::write(
        settings_dir.join("settings.toml"),
        "default_mode = \"plan\"\nlow_motion = false\nfancy_animations = true\nauto_compact = true\n",
    )?;
    let _config_path = EnvVarGuard::set(
        "DEEPSEEK_CONFIG_PATH",
        settings_dir.join("config.toml").as_os_str(),
    );
    let _no_animations = EnvVarGuard::set("NO_ANIMATIONS", "1");

    // yolo remains a permission-migration alias, not a startup mode write.
    let error = persist_runtime_tui_setting("default_mode", "yolo")
        .expect_err("yolo must not be saved as a startup mode");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        crate::settings::Settings::load_persisted()?.default_mode,
        "plan",
        "a rejected write must leave the saved startup mode intact"
    );

    persist_runtime_tui_setting("default_mode", "operate")
        .expect("operate is a valid startup mode");
    assert_eq!(
        crate::settings::Settings::load_persisted()?.default_mode,
        "operate"
    );
    persist_runtime_tui_setting("default_mode", "agent")
        .expect("agent should be a valid startup mode");
    persist_runtime_tui_setting("auto_compact", "false").expect("strict boolean should persist");
    let saved = crate::settings::Settings::load_persisted()?;
    assert_eq!(saved.default_mode, "agent");
    assert!(!saved.auto_compact);
    assert!(!saved.low_motion, "NO_ANIMATIONS is runtime-only");
    assert!(saved.fancy_animations, "NO_ANIMATIONS is runtime-only");
    Ok(())
}

#[async_trait::async_trait]
impl crate::task_manager::TaskExecutor for MockExecutor {
    async fn execute(
        &self,
        _task: crate::task_manager::ExecutionTask,
        events: mpsc::Sender<crate::task_manager::TaskExecutionEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> crate::task_manager::TaskExecutionResult {
        let _ = events
            .send(crate::task_manager::TaskExecutionEvent::Status {
                message: "started".to_string(),
            })
            .await;
        sleep(Duration::from_millis(100)).await;
        if cancel.is_cancelled() {
            return crate::task_manager::TaskExecutionResult {
                status: crate::task_manager::TaskStatus::Canceled,
                result_text: None,
                error: None,
                terminal_reason: crate::task_manager::TaskTerminalReason::Canceled,
            };
        }
        crate::task_manager::TaskExecutionResult {
            status: crate::task_manager::TaskStatus::Completed,
            result_text: Some("ok".to_string()),
            error: None,
            terminal_reason: crate::task_manager::TaskTerminalReason::Completed,
        }
    }
}

fn saved_session_with_blocks(blocks: Vec<crate::models::ContentBlock>) -> SavedSession {
    SavedSession {
        schema_version: 1,
        metadata: SessionMetadata {
            id: "session-1".to_string(),
            title: "test session".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            message_count: 1,
            total_tokens: 0,
            model: "test-model".to_string(),
            model_provider: "deepseek".to_string(),
            model_provider_id: None,
            workspace: PathBuf::from("."),
            mode: None,
            cost: Default::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
            archived: false,
            spawn_depth: 0,
        },
        journal: None,
        leaf_id: None,
        messages: vec![crate::models::Message {
            role: Role::Assistant,
            content: blocks,
        }],
        system_prompt: None,
        context_references: Vec::new(),
        artifacts: Vec::new(),
        approval_receipts: Vec::new(),
        work_state: None,
        window_title: None,
        last_auto_route: None,
    }
}

fn run_test_git(workspace: &std::path::Path, args: &[&str]) -> Result<()> {
    let output = crate::dependencies::Git::output(args, workspace)
        .with_context(|| format!("git {args:?} failed to spawn"))?;
    if !output.status.success() {
        bail!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[test]
fn workspace_status_reports_head_and_dirty_counts() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo)?;
    run_test_git(&repo, &["init", "-b", "main"])?;
    run_test_git(&repo, &["config", "core.autocrlf", "false"])?;
    fs::write(repo.join("tracked.txt"), "clean\n")?;
    run_test_git(&repo, &["add", "tracked.txt"])?;
    run_test_git(
        &repo,
        &[
            "-c",
            "user.name=CodeWhale Test",
            "-c",
            "user.email=codewhale@example.invalid",
            "commit",
            "-m",
            "init",
        ],
    )?;

    let clean = collect_workspace_status(&repo);
    assert!(clean.git_repo);
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert!(clean.head.as_deref().is_some_and(|head| !head.is_empty()));
    assert!(!clean.dirty);

    fs::write(repo.join("tracked.txt"), "dirty\n")?;
    fs::write(repo.join("untracked.txt"), "new\n")?;

    let dirty = collect_workspace_status(&repo);
    assert!(dirty.dirty);
    assert_eq!(dirty.unstaged, 1);
    assert_eq!(dirty.untracked, 1);
    Ok(())
}

#[test]
fn session_detail_scenario() {
    // Scenario consolidation of: session_detail_tool_use_preserves_caller_metadata, session_detail_tool_result_keeps_fallback_content_with_blocks
    // from session_detail_tool_use_preserves_caller_metadata
    {
        let detail = session_to_detail(saved_session_with_blocks(vec![
            crate::models::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "task_shell_start".to_string(),
                input: json!({ "cmd": "cargo test" }),
                caller: Some(crate::models::ToolCaller {
                    caller_type: "subagent".to_string(),
                    tool_id: Some("parent-tool".to_string()),
                }),
                thought_signature: None,
            },
        ]));

        let block = &detail.messages[0]["content"][0];
        assert_eq!(block["type"].as_str(), Some("tool_use"));
        assert_eq!(block["caller"]["type"].as_str(), Some("subagent"));
        assert_eq!(block["caller"]["tool_id"].as_str(), Some("parent-tool"));
    }
    // from session_detail_tool_result_keeps_fallback_content_with_blocks
    {
        let detail = session_to_detail(saved_session_with_blocks(vec![
            crate::models::ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "fallback text".to_string(),
                is_error: Some(false),
                content_blocks: Some(vec![json!({
                    "type": "text",
                    "text": "structured text"
                })]),
            },
        ]));

        let block = &detail.messages[0]["content"][0];
        assert_eq!(block["type"].as_str(), Some("tool_result"));
        assert_eq!(block["content"].as_str(), Some("fallback text"));
        assert_eq!(
            block["content_blocks"][0]["text"].as_str(),
            Some("structured text")
        );
        assert_eq!(block["is_error"].as_bool(), Some(false));
    }
}

#[test]
fn messages_from_thread_detail_batches_tool_results() {
    let now = Utc::now();
    let turn_id = "turn_detail".to_string();
    let thread = ThreadRecord {
        schema_version: 2,
        id: "thr_detail".to_string(),
        created_at: now,
        updated_at: now,
        model: DEFAULT_TEXT_MODEL.to_string(),
        model_provider: None,
        model_provider_id: None,
        reasoning_effort: None,
        allowed_tools: None,
        workspace: PathBuf::from("."),
        mode: "agent".to_string(),
        permission_posture: Some("ask".to_string()),
        allow_shell: false,
        trust_mode: false,
        auto_approve: false,
        latest_turn_id: Some(turn_id.clone()),
        latest_response_bookmark: None,
        archived: false,
        system_prompt: None,
        task_id: None,
        title: None,
        session_id: None,
    };
    let turn = TurnRecord {
        schema_version: 2,
        id: turn_id.clone(),
        thread_id: thread.id.clone(),
        status: RuntimeTurnStatus::Completed,
        input_summary: "check".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: Some(0),
        usage: None,
        permission_posture: Some("ask".to_string()),
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
        item_ids: vec![
            "item_user".to_string(),
            "item_reasoning".to_string(),
            "item_tool_use".to_string(),
            "item_result_one".to_string(),
            "item_result_two".to_string(),
            "item_answer".to_string(),
        ],
        steer_count: 0,
        agent_mail_message_id: None,
    };
    let item = |id: &str,
                kind: TurnItemKind,
                summary: &str,
                detail: Option<&str>,
                metadata: Option<Value>| {
        crate::runtime_threads::TurnItemRecord {
            schema_version: 2,
            id: id.to_string(),
            turn_id: turn_id.clone(),
            kind,
            status: TurnItemLifecycleStatus::Completed,
            summary: summary.to_string(),
            detail: detail.map(str::to_string),
            metadata,
            artifact_refs: Vec::new(),
            started_at: Some(now),
            ended_at: Some(now),
        }
    };
    let detail = ThreadDetail {
        thread,
        turns: vec![turn],
        items: vec![
            item(
                "item_user",
                TurnItemKind::UserMessage,
                "check",
                Some("check"),
                None,
            ),
            item(
                "item_reasoning",
                TurnItemKind::AgentReasoning,
                "thinking",
                Some("thinking"),
                None,
            ),
            item(
                "item_tool_use",
                TurnItemKind::ToolCall,
                "shell",
                Some(r#"{"cmd":"pwd"}"#),
                Some(json!({
                    "tool_use_id": "tool-1",
                    "tool_name": "shell"
                })),
            ),
            item(
                "item_result_one",
                TurnItemKind::ToolCall,
                "one",
                Some("one"),
                Some(json!({
                    "tool_result_for": "tool-1",
                    "is_error": false,
                    "content_blocks": [{
                        "type": "text",
                        "text": "structured one"
                    }]
                })),
            ),
            item(
                "item_result_two",
                TurnItemKind::ToolCall,
                "two",
                Some("two"),
                Some(json!({
                    "tool_result_for": "tool-2",
                    "is_error": true
                })),
            ),
            item(
                "item_answer",
                TurnItemKind::AgentMessage,
                "done",
                Some("done"),
                None,
            ),
        ],
        latest_seq: 0,
        pending_approvals: Vec::new(),
        pending_user_inputs: Vec::new(),
        pending_dynamic_tool_calls: Vec::new(),
    };

    let messages = messages_from_thread_detail(&detail);
    let roles = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    assert_eq!(messages[2].content.len(), 2);
    match &messages[2].content[0] {
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
    match &messages[2].content[1] {
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
}

#[test]
fn legacy_exact_thread_export_normalizes_provider_kind_and_id() {
    let now = Utc::now();
    let detail = ThreadDetail {
        thread: ThreadRecord {
            schema_version: 2,
            id: "thr_legacy_custom".to_string(),
            created_at: now,
            updated_at: now,
            model: "local-model".to_string(),
            // Pre-additive records overloaded this legacy field with the exact id.
            model_provider: Some("lm-studio".to_string()),
            model_provider_id: None,
            reasoning_effort: None,
            allowed_tools: None,
            workspace: PathBuf::from("."),
            mode: "agent".to_string(),
            permission_posture: None,
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
        },
        turns: Vec::new(),
        items: Vec::new(),
        latest_seq: 0,
        pending_approvals: Vec::new(),
        pending_user_inputs: Vec::new(),
        pending_dynamic_tool_calls: Vec::new(),
    };
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(crate::config::ProvidersConfig {
            custom: std::collections::HashMap::from([(
                "lm-studio".to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("http://127.0.0.1:1234/v1".to_string()),
                    model: Some("local-model".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut session = crate::session_manager::create_saved_session_with_mode(
        &[],
        "local-model",
        std::path::Path::new("."),
        0,
        None,
        Some("agent"),
    );

    sessions::stamp_session_provider_from_thread(&config, &detail, &mut session.metadata)
        .expect("normalize legacy exact provider");

    assert_eq!(session.metadata.model_provider, "custom");
    assert_eq!(
        session.metadata.model_provider_id.as_deref(),
        Some("lm-studio")
    );
}

#[test]
fn runtime_auth_scenario() {
    // Scenario consolidation of: runtime_auth_generates_token_by_default, runtime_auth_status_does_not_render_generated_token, runtime_auth_requires_explicit_insecure_for_no_token, runtime_auth_prefers_cli_token_over_env_token, runtime_auth_ignores_blank_configured_tokens
    // from runtime_auth_generates_token_by_default
    {
        let auth = resolve_runtime_auth(None, None, false);
        assert!(auth.generated);
        let token = auth.token.expect("generated token");
        assert!(token.starts_with("cwrt_"));
        assert!(token.len() > 32);
    }
    // from runtime_auth_status_does_not_render_generated_token
    {
        let auth = ResolvedRuntimeAuth {
            token: Some("cwrt_super_secret_test_token".to_string()),
            generated: true,
        };
        let rendered = runtime_auth_status_lines(&auth).join("\n");

        assert!(!rendered.contains("cwrt_super_secret_test_token"));
        assert!(rendered.contains("not printed"));
    }
    // from runtime_auth_requires_explicit_insecure_for_no_token
    {
        let auth = resolve_runtime_auth(None, None, true);
        assert_eq!(
            auth,
            ResolvedRuntimeAuth {
                token: None,
                generated: false,
            }
        );
    }
    // from runtime_auth_prefers_cli_token_over_env_token
    {
        let auth = resolve_runtime_auth(
            Some(" cli-token ".to_string()),
            Some("env-token".to_string()),
            false,
        );
        assert_eq!(
            auth,
            ResolvedRuntimeAuth {
                token: Some("cli-token".to_string()),
                generated: false,
            }
        );
    }
    // from runtime_auth_ignores_blank_configured_tokens
    {
        let auth = resolve_runtime_auth(Some(" ".to_string()), Some("\t".to_string()), false);
        assert!(auth.generated);
        assert!(auth.token.is_some());
    }
}

#[test]
fn runtime_token_scenario() {
    // Scenario consolidation of: runtime_token_environment_prefers_the_codewhale_name, runtime_token_environment_falls_through_a_blank_primary_to_the_legacy_alias
    // from runtime_token_environment_prefers_the_codewhale_name
    {
        let environment = runtime_token_environment(&|name| match name {
            RUNTIME_TOKEN_ENV => Some(" canonical-token ".to_string()),
            LEGACY_RUNTIME_TOKEN_ENV => Some("legacy-token".to_string()),
            _ => None,
        });

        assert_eq!(environment.token.as_deref(), Some("canonical-token"));
        assert!(!environment.legacy_alias_used);
        assert!(runtime_token_alias_warning(None, &environment).is_none());
    }
    // from runtime_token_environment_falls_through_a_blank_primary_to_the_legacy_alias
    {
        let environment = runtime_token_environment(&|name| match name {
            RUNTIME_TOKEN_ENV => Some(" \t ".to_string()),
            LEGACY_RUNTIME_TOKEN_ENV => Some(" legacy-token ".to_string()),
            _ => None,
        });

        assert_eq!(environment.token.as_deref(), Some("legacy-token"));
        assert!(environment.legacy_alias_used);
    }
}

#[test]
fn mobile_listener_fails_closed_outside_loopback_without_verified_transport() {
    let non_loopback = RuntimeApiOptions {
        host: "0.0.0.0".to_string(),
        mobile: true,
        insecure_no_auth: true,
        ..RuntimeApiOptions::default()
    };
    let err = validate_runtime_listener_security(&non_loopback).unwrap_err();
    assert!(err.to_string().contains("mobile is loopback-only"));

    for host in ["127.0.0.1", "::1"] {
        let loopback = RuntimeApiOptions {
            host: host.to_string(),
            mobile: true,
            ..RuntimeApiOptions::default()
        };
        assert!(
            validate_runtime_listener_security(&loopback).is_ok(),
            "host={host}"
        );
    }
    assert_eq!(
        runtime_bind_address("::1", 7878)
            .expect("IPv6 loopback bind address")
            .to_string(),
        "[::1]:7878"
    );

    let ordinary_http = RuntimeApiOptions {
        host: "0.0.0.0".to_string(),
        mobile: false,
        ..RuntimeApiOptions::default()
    };
    assert!(validate_runtime_listener_security(&ordinary_http).is_ok());
}

#[test]
fn consumed_legacy_runtime_token_reports_one_value_free_deprecation_line() {
    let secret = "legacy-super-secret-token";
    let environment = runtime_token_environment(&|name| {
        (name == LEGACY_RUNTIME_TOKEN_ENV).then(|| secret.to_string())
    });
    let warning = runtime_token_alias_warning(None, &environment).expect("legacy warning");

    assert_eq!(warning.lines().count(), 1);
    assert!(warning.contains(LEGACY_RUNTIME_TOKEN_ENV));
    assert!(warning.contains(RUNTIME_TOKEN_ENV));
    assert!(warning.contains("0.10.0"));
    assert!(!warning.contains(secret));
}

#[test]
fn explicit_cli_runtime_token_does_not_warn_about_an_unused_legacy_alias() {
    let environment = runtime_token_environment(&|name| {
        (name == LEGACY_RUNTIME_TOKEN_ENV).then(|| "legacy-token".to_string())
    });

    assert!(runtime_token_alias_warning(Some("cli-token"), &environment).is_none());
    assert!(runtime_token_alias_warning(Some(" \t"), &environment).is_some());
}

async fn spawn_test_server_with_root(
    root: PathBuf,
    sessions_dir: PathBuf,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    spawn_test_server_with_root_and_token(root, sessions_dir, None).await
}

async fn spawn_test_server_with_root_and_token(
    root: PathBuf,
    sessions_dir: PathBuf,
    runtime_token: Option<String>,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    spawn_test_server_with_root_token_and_mobile(root, sessions_dir, runtime_token, false).await
}

async fn spawn_test_server_with_root_token_and_mobile(
    root: PathBuf,
    sessions_dir: PathBuf,
    runtime_token: Option<String>,
    mobile_enabled: bool,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    spawn_test_server_with_root_token_mobile_workspace(
        root,
        sessions_dir,
        runtime_token,
        mobile_enabled,
        PathBuf::from("."),
    )
    .await
}

async fn spawn_test_server_with_root_token_mobile_workspace(
    root: PathBuf,
    sessions_dir: PathBuf,
    runtime_token: Option<String>,
    mobile_enabled: bool,
    workspace: PathBuf,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    spawn_test_server_with_root_token_mobile_workspace_and_subagents(
        root,
        sessions_dir,
        runtime_token,
        mobile_enabled,
        workspace,
        None,
        None,
    )
    .await
}

#[derive(Default)]
struct TestServerOverrides {
    sub_agent_manager: Option<SharedSubAgentManager>,
    fleet_codewhale_binary: Option<String>,
    config: Option<Config>,
    config_path: Option<PathBuf>,
    config_profile: Option<String>,
    mobile: Option<mobile::RuntimeMobileState>,
    web: Option<web::RuntimeWebState>,
    compat_stream_test_hook: Option<mpsc::UnboundedSender<CompatStreamTestPoint>>,
    plugin_discovery: Option<Arc<crate::plugins::PluginDiscoveryContext>>,
}

async fn spawn_test_server_with_root_token_mobile_workspace_and_subagents(
    root: PathBuf,
    sessions_dir: PathBuf,
    runtime_token: Option<String>,
    mobile_enabled: bool,
    workspace: PathBuf,
    sub_agent_manager: Option<SharedSubAgentManager>,
    fleet_codewhale_binary: Option<String>,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    spawn_test_server_with_root_token_mobile_workspace_and_overrides(
        root,
        sessions_dir,
        runtime_token,
        mobile_enabled,
        workspace,
        TestServerOverrides {
            sub_agent_manager,
            fleet_codewhale_binary,
            ..TestServerOverrides::default()
        },
    )
    .await
}

async fn spawn_test_server_with_root_token_mobile_workspace_and_overrides(
    root: PathBuf,
    sessions_dir: PathBuf,
    runtime_token: Option<String>,
    mobile_enabled: bool,
    workspace: PathBuf,
    overrides: TestServerOverrides,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    fs::create_dir_all(&sessions_dir)?;
    fs::create_dir_all(&workspace)?;
    let mut config = if let Some(config) = overrides.config.clone() {
        config
    } else if let Some(path) = overrides.config_path.clone() {
        Config::load(Some(path), None)?
    } else {
        Config {
            api_key: Some("runtime-api-test-key".to_string()),
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            ..Config::default()
        }
    };
    config.mcp_config_path = Some(root.join("mcp.json").to_string_lossy().to_string());

    config.mcp_config_path = Some(root.join("mcp.json").to_string_lossy().to_string());
    let manager = TaskManager::start_with_executor(
        TaskManagerConfig {
            data_dir: root.join("tasks"),
            worker_count: 1,
            default_workspace: workspace.clone(),
            default_model: DEFAULT_TEXT_MODEL.to_string(),
            default_mode: "agent".to_string(),
            allow_shell: false,
            trust_mode: false,
            execution_limits: crate::task_manager::TaskExecutionLimits::default(),
        },
        Arc::new(MockExecutor),
    )
    .await?;
    let runtime_threads: SharedRuntimeThreadManager = Arc::new(RuntimeThreadManager::open(
        config.clone(),
        workspace.clone(),
        RuntimeThreadManagerConfig::from_task_data_dir(root.join("runtime")),
    )?);
    runtime_threads.attach_task_manager(manager.clone());
    let automations = Arc::new(Mutex::new(AutomationManager::open(
        root.join("automations"),
    )?));
    runtime_threads.attach_automation_manager(automations.clone());

    let auth_required = runtime_token.is_some();
    let sub_agent_manager = overrides
        .sub_agent_manager
        .unwrap_or_else(|| runtime_api_sub_agent_manager(&workspace, 2));
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let addr = listener.local_addr()?;
    let mobile = if mobile_enabled && runtime_token.is_some() {
        Some(
            overrides
                .mobile
                .unwrap_or_else(|| mobile::RuntimeMobileState::new().0),
        )
    } else {
        None
    };
    let state = RuntimeApiState {
        config: Arc::new(parking_lot::RwLock::new(config)),
        workspace,
        plugin_discovery: overrides
            .plugin_discovery
            .unwrap_or_else(crate::plugins::PluginDiscoveryContext::capture_pre_dotenv),
        task_manager: manager,
        runtime_threads: runtime_threads.clone(),
        cors_origins: Vec::new(),
        sessions_dir,
        config_path: overrides.config_path.clone(),
        config_profile: overrides.config_profile,
        mcp_pool: Arc::new(Mutex::new(None)),
        automations,
        sub_agent_manager,
        runtime_token,
        skill_state: Arc::new(Mutex::new(
            SkillStateStore::load_from(root.join("skills_state.toml")).unwrap(),
        )),
        auth_required,
        bind_host: "127.0.0.1".to_string(),
        bind_port: addr.port(),
        mobile_enabled,
        mobile,
        web: overrides.web,
        fleet_codewhale_binary: overrides
            .fleet_codewhale_binary
            .unwrap_or_else(configured_codewhale_binary),
        compat_stream_test_hook: overrides.compat_stream_test_hook,
    };
    let app = build_router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Ok(Some((addr, runtime_threads, handle)))
}

async fn spawn_test_server() -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    let root = std::env::temp_dir().join(format!("deepseek-runtime-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    spawn_test_server_with_root(root, sessions_dir).await
}

async fn spawn_test_server_with_config_path(
    config_path: PathBuf,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    let root = std::env::temp_dir().join(format!("codewhale-config-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    fs::create_dir_all(&root)?;
    spawn_test_server_with_root_token_mobile_workspace_and_overrides(
        root,
        sessions_dir,
        None,
        false,
        workspace,
        TestServerOverrides {
            config_path: Some(config_path),
            ..TestServerOverrides::default()
        },
    )
    .await
}

async fn spawn_test_server_with_config_path_and_profile(
    config_path: PathBuf,
    config_profile: String,
) -> Result<
    Option<(
        SocketAddr,
        SharedRuntimeThreadManager,
        tokio::task::JoinHandle<()>,
    )>,
> {
    let root = std::env::temp_dir().join(format!("codewhale-config-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    fs::create_dir_all(&root)?;
    spawn_test_server_with_root_token_mobile_workspace_and_overrides(
        root,
        sessions_dir,
        None,
        false,
        workspace,
        TestServerOverrides {
            config_path: Some(config_path),
            config_profile: Some(config_profile),
            ..TestServerOverrides::default()
        },
    )
    .await
}

async fn read_first_sse_frame(resp: reqwest::Response) -> Result<String> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    loop {
        let next = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), stream.next())
            .await
            .context("timed out waiting for SSE frame")?
            .context("SSE stream ended unexpectedly")??;
        buf.extend_from_slice(&next);

        let text = String::from_utf8_lossy(&buf);
        if let Some(idx) = text.find("\n\n").or_else(|| text.find("\r\n\r\n")) {
            return Ok(text[..idx].to_string());
        }

        if buf.len() > 64 * 1024 {
            bail!("SSE frame exceeded 64KB without delimiter");
        }
    }
}

fn take_complete_sse_frame(buffer: &mut Vec<u8>) -> Result<Option<String>> {
    let text = String::from_utf8_lossy(buffer);
    let lf = text.find("\n\n").map(|index| (index, 2));
    let crlf = text.find("\r\n\r\n").map(|index| (index, 4));
    let delimiter = match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    };
    let Some((index, delimiter_len)) = delimiter else {
        return Ok(None);
    };
    let frame = String::from_utf8(buffer[..index].to_vec())?;
    buffer.drain(..index + delimiter_len);
    Ok(Some(frame))
}

async fn collect_sse_frames(
    response: reqwest::Response,
    frame_tx: mpsc::UnboundedSender<(String, serde_json::Value)>,
) -> Result<Vec<(String, serde_json::Value)>> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut frames = Vec::new();
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);
        while let Some(raw) = take_complete_sse_frame(&mut buffer)? {
            if raw.trim().is_empty() || raw.trim_start().starts_with(':') {
                continue;
            }
            let frame = parse_sse_frame(&raw)?;
            frame_tx
                .send(frame.clone())
                .map_err(|_| anyhow::anyhow!("SSE frame observer closed"))?;
            frames.push(frame);
        }
        if buffer.len() > 64 * 1024 {
            bail!("SSE frame exceeded 64KB without delimiter");
        }
    }
    Ok(frames)
}

#[cfg(unix)]
fn write_fake_fleet_binary(root: &Path, marker: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let binary = root.join("fake-codewhale");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\ntouch '{}'\nprintf '{{\"type\":\"content\",\"content\":\"restarted through Runtime API\"}}\\n'\nexit 0\n",
            marker.display()
        ),
    )?;
    let mut permissions = fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions)?;
    Ok(binary)
}

#[cfg(windows)]
fn write_fake_fleet_binary(root: &Path, marker: &Path) -> Result<PathBuf> {
    // Exercise the same executable/Job Object path as a released Windows
    // Codewhale binary. A `.cmd` fake introduces an extra `cmd.exe` wrapper
    // whose lifetime can end before the Fleet host attaches its Job Object,
    // making the test race a process topology production does not use.
    let source = root.join("fake-codewhale.rs");
    let binary = root.join("fake-codewhale.exe");
    let helper = format!(
        r##"fn main() {{
    std::fs::File::create({marker:?}).expect("create Fleet restart marker");
    println!("{{}}", r#"{{"type":"content","content":"restarted through Runtime API"}}"#);
    std::thread::sleep(std::time::Duration::from_millis(750));
}}
"##,
        marker = marker.to_string_lossy().as_ref(),
    );
    fs::write(&source, helper)?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-name=codewhale_fleet_test_helper")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .context("compile Windows Fleet restart helper")?;
    if !output.status.success() {
        bail!(
            "failed to compile Windows Fleet restart helper: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(binary)
}

fn parse_sse_frame(frame: &str) -> Result<(String, serde_json::Value)> {
    let mut event_name: Option<String> = None;
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }
    let event_name = event_name.context("missing SSE event field")?;
    let payload = if data_lines.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&data_lines.join("\n"))
            .with_context(|| format!("invalid SSE data payload: {}", data_lines.join("\n")))?
    };
    Ok((event_name, payload))
}

async fn wait_for_terminal_turn_status(
    client: &reqwest::Client,
    addr: SocketAddr,
    thread_id: &str,
    turn_id: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = tokio::time::Instant::now() + ci_scaled(timeout);
    loop {
        let detail: serde_json::Value = client
            .get(format!("http://{addr}/v1/threads/{thread_id}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let status = detail["turns"]
            .as_array()
            .and_then(|turns| turns.iter().find(|turn| turn["id"] == turn_id))
            .and_then(|turn| turn.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if matches!(
            status.as_str(),
            "completed" | "failed" | "interrupted" | "canceled"
        ) {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out waiting for terminal turn status for {turn_id}");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_in_progress_item(
    client: &reqwest::Client,
    addr: SocketAddr,
    thread_id: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + ci_scaled(timeout);
    loop {
        let detail: serde_json::Value = client
            .get(format!("http://{addr}/v1/threads/{thread_id}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if detail["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["status"] == "in_progress"))
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out waiting for in-progress item in thread {thread_id}");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn health_and_tasks_endpoints_work() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let health: serde_json::Value = client
        .get(format!("http://{addr}/health"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["service"], "codewhale-runtime-api");

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/tasks"))
        .json(&json!({ "prompt": "hello task" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"].as_str().expect("task id").to_string();

    let listed: serde_json::Value = client
        .get(format!("http://{addr}/v1/tasks"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        listed["tasks"]
            .as_array()
            .is_some_and(|tasks| !tasks.is_empty())
    );

    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/tasks/{id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["id"], id);

    let _cancelled: serde_json::Value = client
        .post(format!("http://{addr}/v1/tasks/{id}/cancel"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn omitted_runtime_models_use_the_active_provider_default() -> Result<()> {
    for (label, config, expected) in provider_default_model_cases() {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join(label);
        let sessions_dir = root.join("sessions");
        let workspace = root.join("workspace");
        let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
        let Some((addr, runtime_threads, handle)) =
            spawn_test_server_with_root_token_mobile_workspace_and_overrides(
                root,
                sessions_dir,
                None,
                false,
                workspace,
                TestServerOverrides {
                    config: Some(config),
                    compat_stream_test_hook: Some(hook_tx),
                    ..TestServerOverrides::default()
                },
            )
            .await?
        else {
            return Ok(());
        };

        let created: serde_json::Value = crate::tls::reqwest_client()
            .post(format!("http://{addr}/v1/tasks"))
            .json(&json!({ "prompt": format!("{label} omitted model") }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(created["model"], expected, "{label} durable task model");

        let stream_client = crate::tls::reqwest_client();
        let stream_task = tokio::spawn(async move {
            stream_client
                .post(format!("http://{addr}/v1/stream"))
                .json(&json!({ "prompt": format!("{label} omitted stream model") }))
                .send()
                .await
        });
        let created = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
            .await
            .with_context(|| format!("{label} compatibility stream did not create its thread"))?
            .with_context(|| format!("{label} compatibility stream test hook closed"))?;
        let (thread_id, _resume) = match created {
            CompatStreamTestPoint::ThreadCreated { thread_id, resume } => (thread_id, resume),
            CompatStreamTestPoint::SubscribedBeforeReplay { .. }
            | CompatStreamTestPoint::ReplayLoaded { .. } => {
                bail!("{label} compatibility stream advanced before thread inspection")
            }
        };
        let thread = runtime_threads.get_thread(&thread_id).await?;
        assert_eq!(thread.model, expected, "{label} compatibility stream model");
        stream_task.abort();
        handle.abort();
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn mcp_tools_endpoint_is_passive_until_connect_requested() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-mcp-tools-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;
    let sentinel = root.join("mcp-spawned");
    fs::write(
        root.join("mcp.json"),
        serde_json::json!({
            "servers": {
                "sentinel": {
                    "command": "sh",
                    "args": [
                        "-c",
                        "printf spawned > \"$1\"",
                        "sh",
                        sentinel
                    ]
                }
            }
        })
        .to_string(),
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let passive: serde_json::Value = client
        .get(format!("http://{addr}/v1/apps/mcp/tools"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(passive["tools"].as_array().map(Vec::len), Some(0));
    assert!(
        !sentinel.exists(),
        "passive MCP tool listing must not spawn stdio servers"
    );

    let _live: serde_json::Value = client
        .get(format!("http://{addr}/v1/apps/mcp/tools?connect=true"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    for _ in 0..20 {
        if sentinel.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        sentinel.exists(),
        "explicit MCP connect should spawn configured stdio servers"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn runtime_token_guard_protects_v1_routes() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-runtime-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let token = "local-test-token".to_string();
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_and_token(root, sessions_dir, Some(token.clone())).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let health = client
        .get(format!("http://{addr}/health"))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(health.status(), StatusCode::OK);

    let unauthorized = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .send()
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let bearer = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(bearer.status(), StatusCode::OK);

    let query_token = client
        .get(format!("http://{addr}/v1/threads/summary?token={token}"))
        .send()
        .await?;
    assert_eq!(query_token.status(), StatusCode::UNAUTHORIZED);

    let cookie_token = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .header(header::COOKIE, format!("codewhale_runtime_token={token}"))
        .send()
        .await?;
    assert_eq!(cookie_token.status(), StatusCode::UNAUTHORIZED);

    let codewhale_header = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .header("x-codewhale-runtime-token", &token)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(codewhale_header.status(), StatusCode::OK);

    let deepseek_header = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .header("x-deepseek-runtime-token", &token)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(deepseek_header.status(), StatusCode::OK);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn web_bootstrap_sets_strict_cookie_once_and_preserves_v1_auth() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-web-api-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    let token = "cwrt_runtime_secret_never_in_browser_storage".to_string();
    let (web, nonce) = web::RuntimeWebState::new();
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root,
            sessions_dir,
            Some(token.clone()),
            false,
            workspace,
            TestServerOverrides {
                web: Some(web),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let page = client.get(format!("http://{addr}/")).send().await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'"
        )
    );
    let page_body = page.text().await?;
    assert!(!page_body.contains(&token));
    assert!(!page_body.contains(&nonce));

    let icon = client
        .get(format!("http://{addr}/assets/codewhale-192.png"))
        .send()
        .await?;
    assert_eq!(icon.status(), StatusCode::OK);
    assert_eq!(
        icon.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    assert!(icon.bytes().await?.starts_with(b"\x89PNG\r\n\x1a\n"));

    let wrong = client
        .get(format!(
            "http://{addr}/__codewhale/bootstrap/cwwb_00000000000000000000000000000000"
        ))
        .send()
        .await?;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let exchange = client
        .get(format!("http://{addr}/__codewhale/bootstrap/{nonce}"))
        .send()
        .await?;
    assert_eq!(exchange.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        exchange
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/")
    );
    let set_cookie = exchange
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .context("missing bootstrap Set-Cookie")?
        .to_string();
    assert!(set_cookie.starts_with("codewhale_web_session=cwws_"));
    assert!(set_cookie.ends_with("; HttpOnly; SameSite=Strict; Path=/"));
    assert!(!set_cookie.contains(&token));

    let unauthorized = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .send()
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let cookie_pair = set_cookie
        .split(';')
        .next()
        .context("missing web session cookie pair")?;
    let authorized = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .header(header::COOKIE, cookie_pair)
        .send()
        .await?;
    assert_eq!(authorized.status(), StatusCode::OK);

    let same_origin_cookie_post = client
        .post(format!("http://{addr}/v1/threads"))
        .header(header::COOKIE, cookie_pair)
        .header(header::ORIGIN, format!("http://{addr}"))
        .header("sec-fetch-site", "same-origin")
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(same_origin_cookie_post.status(), StatusCode::CREATED);

    let cross_origin_cookie_post = client
        .post(format!("http://{addr}/v1/threads"))
        .header(header::COOKIE, cookie_pair)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-site")
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(cross_origin_cookie_post.status(), StatusCode::UNAUTHORIZED);

    let originless_cookie_post = client
        .post(format!("http://{addr}/v1/threads"))
        .header(header::COOKIE, cookie_pair)
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(originless_cookie_post.status(), StatusCode::UNAUTHORIZED);

    let bearer_post = client
        .post(format!("http://{addr}/v1/threads"))
        .bearer_auth(&token)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(bearer_post.status(), StatusCode::CREATED);

    let reused = client
        .get(format!("http://{addr}/__codewhale/bootstrap/{nonce}"))
        .send()
        .await?;
    assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);

    let mobile = client.get(format!("http://{addr}/mobile")).send().await?;
    assert_eq!(mobile.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn web_assets_are_absent_outside_web_mode() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    for path in [
        "/",
        "/assets/codewhale-web.css",
        "/assets/codewhale-web.js",
        "/assets/codewhale-192.png",
    ] {
        let response = client.get(format!("http://{addr}{path}")).send().await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "path={path}");
    }
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn thread_summary_includes_workspace_branch_metadata() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let sessions_dir = root.join("sessions");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo)?;
    run_test_git(&repo, &["init", "-b", "feature/agent"])?;
    run_test_git(&repo, &["config", "core.autocrlf", "false"])?;
    fs::write(repo.join("README.md"), "branch visibility\n")?;
    run_test_git(&repo, &["add", "README.md"])?;
    run_test_git(
        &repo,
        &[
            "-c",
            "user.name=CodeWhale Test",
            "-c",
            "user.email=codewhale@example.invalid",
            "commit",
            "-m",
            "init",
        ],
    )?;

    let non_git = tmp.path().join("non-git");
    fs::create_dir_all(&non_git)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let git_thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "title": "Git workspace",
            "workspace": repo,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let git_thread_id = git_thread["id"]
        .as_str()
        .context("missing git thread id")?
        .to_string();
    fs::write(
        repo.join("dirty.txt"),
        "worktree changed after thread spawn\n",
    )?;

    let plain_thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "title": "Plain workspace",
            "workspace": non_git,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let plain_thread_id = plain_thread["id"]
        .as_str()
        .context("missing plain thread id")?
        .to_string();

    let summary: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/summary?limit=100"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let summaries = summary.as_array().context("summary should be an array")?;
    let git_summary = summaries
        .iter()
        .find(|item| item["id"] == git_thread_id)
        .context("missing git workspace summary")?;
    assert_eq!(git_summary["branch"], "feature/agent");
    assert!(
        git_summary["head"]
            .as_str()
            .is_some_and(|head| !head.is_empty())
    );
    assert_eq!(git_summary["dirty"], true);
    assert_eq!(git_summary["workspace"], repo.to_string_lossy().as_ref());

    let plain_summary = summaries
        .iter()
        .find(|item| item["id"] == plain_thread_id)
        .context("missing plain workspace summary")?;
    assert_eq!(plain_summary["branch"], serde_json::Value::Null);
    assert_eq!(plain_summary["head"], serde_json::Value::Null);
    assert_eq!(plain_summary["dirty"], false);
    assert_eq!(
        plain_summary["workspace"],
        non_git.to_string_lossy().as_ref()
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn workspace_and_automation_endpoints_work() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let workspace: serde_json::Value = client
        .get(format!("http://{addr}/v1/workspace/status"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(workspace.get("workspace").is_some());

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/automations"))
        .json(&json!({
            "name": "Smoke automation",
            "prompt": "automation smoke test",
            "rrule": "FREQ=HOURLY;INTERVAL=2",
            "status": "active"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let automation_id = created["id"]
        .as_str()
        .context("missing automation id")?
        .to_string();

    let listed: serde_json::Value = client
        .get(format!("http://{addr}/v1/automations"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        listed
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == automation_id))
    );

    let run_now: serde_json::Value = client
        .post(format!("http://{addr}/v1/automations/{automation_id}/run"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(run_now["automation_id"], automation_id);

    let paused: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/automations/{automation_id}/pause"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(paused["status"], "paused");

    let resumed: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/automations/{automation_id}/resume"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(resumed["status"], "active");

    let updated: serde_json::Value = client
        .patch(format!("http://{addr}/v1/automations/{automation_id}"))
        .json(&json!({
            "name": "Smoke automation edited",
            "rrule": "FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=10;BYMINUTE=15"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(updated["name"], "Smoke automation edited");

    let runs: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/automations/{automation_id}/runs?limit=5"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        runs.as_array().is_some_and(|items| !items.is_empty()),
        "expected at least one run entry"
    );

    let _deleted: serde_json::Value = client
        .delete(format!("http://{addr}/v1/automations/{automation_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let missing_status = client
        .get(format!("http://{addr}/v1/automations/{automation_id}"))
        .send()
        .await?
        .status();
    assert_eq!(missing_status, StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

fn test_fleet_route_config() -> crate::config::Config {
    let mut providers = crate::config::ProvidersConfig::default();
    providers.deepseek.api_key = Some("test-key".to_string());
    providers.xai.api_key = Some("test-key".to_string());
    providers.zai.api_key = Some("test-key".to_string());
    crate::config::Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("test-key".to_string()),
        providers: Some(providers),
        ..crate::config::Config::default()
    }
}

#[tokio::test]
async fn fleet_status_runtime_api_exposes_state_and_actions() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-fleet-api-{}", Uuid::new_v4()));
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace)?;
    let sub_agent_manager = runtime_api_sub_agent_manager(&workspace, 2);
    let manager = FleetManager::open(&workspace)?
        .with_sub_agent_manager(sub_agent_manager.clone())
        .with_session_model(DEFAULT_TEXT_MODEL)
        .with_route_config(test_fleet_route_config());
    let task = codewhale_protocol::fleet::FleetTaskSpec {
        id: "task-a".to_string(),
        name: "Task A".to_string(),
        description: None,
        objective: Some("Inspect fleet status through Runtime API".to_string()),
        instructions: "Stay running for inspection.".to_string(),
        worker: Some(codewhale_protocol::fleet::FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: vec!["rg".to_string()],
            capabilities: vec!["fleet".to_string()],
        }),
        workspace: None,
        input_files: Vec::new(),
        context: Vec::new(),
        budget: None,
        tags: Vec::new(),
        expected_artifacts: vec![FleetArtifactKind::Log],
        scorer: None,
        retry_policy: None,
        alert_policy: None,
        timeout_seconds: None,
        metadata: std::collections::BTreeMap::new(),
    };
    let report = manager.create_run(
        crate::fleet::task_spec::FleetTaskSpecDocument {
            name: Some("api smoke".to_string()),
            labels: std::collections::BTreeMap::new(),
            security_policy: None,
            workers: Vec::new(),
            tasks: vec![task],
            usage_ceiling: None,
        },
        1,
    )?;
    let restarted_marker = root.join("restarted-worker-ran");
    let fake_codewhale = write_fake_fleet_binary(&root, &restarted_marker)?;
    let worker_id = report.worker_ids[0].clone();
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_subagents(
            root.clone(),
            sessions_dir,
            None,
            false,
            workspace,
            Some(sub_agent_manager),
            Some(fake_codewhale.display().to_string()),
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let runs: serde_json::Value = client
        .get(format!("http://{addr}/v1/fleet/runs"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(runs["status"]["running"], 1);
    assert_eq!(runs["runs"][0]["id"], report.run_id.0);

    let profiles: serde_json::Value = client
        .get(format!("http://{addr}/v1/fleet/profiles"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let profile_ids: Vec<&str> = profiles["profiles"]
        .as_array()
        .expect("profiles array")
        .iter()
        .map(|member| member["id"].as_str().expect("profile id"))
        .collect();
    assert!(
        profile_ids.contains(&"manager"),
        "built-in roster member 'manager' missing from /v1/fleet/profiles: {profile_ids:?}"
    );
    assert!(
        profiles["profiles"]
            .as_array()
            .expect("profiles array")
            .iter()
            .all(|member| member["origin"].is_string()),
        "every profile must carry its roster origin"
    );

    let worker: serde_json::Value = client
        .get(format!("http://{addr}/v1/fleet/workers/{worker_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        worker["objective"],
        "Inspect fleet status through Runtime API"
    );
    assert_eq!(worker["role"], "reviewer");
    assert_eq!(worker["host"], "local");
    assert_eq!(worker["artifacts"][0]["kind"], "log");
    assert_eq!(worker["runtime_state"]["agent_status"], "starting");
    assert_eq!(worker["runtime_state"]["steps_taken"], 0);
    assert_eq!(worker["runtime_state"]["has_session"], true);

    let interrupted: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/fleet/workers/{worker_id}/interrupt"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(interrupted["action"], "interrupt");
    assert_eq!(interrupted["worker"]["last_error"], "cancelled by operator");

    let restarted: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/fleet/workers/{worker_id}/restart"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(restarted["action"], "restart");
    assert_eq!(restarted["execution"], "scheduled");
    assert_eq!(restarted["worker"]["status"], "busy");

    let terminal_status = tokio::time::timeout(ci_scaled(Duration::from_secs(15)), async {
        loop {
            let status = manager.run_status(&report.run_id).unwrap();
            if status.queued == 0 && status.running == 0 {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("Runtime API restart never drove the replacement attempt to completion")?;
    assert_eq!(
        terminal_status.completed, 1,
        "replacement attempt did not complete successfully: {terminal_status:?}"
    );
    assert_eq!(
        terminal_status.failed, 0,
        "replacement attempt failed: {terminal_status:?}"
    );
    assert!(
        restarted_marker.is_file(),
        "Runtime API reported a restart without launching its Fleet worker"
    );
    let ledger_state = manager.rebuild_state()?;
    let restarted_task = ledger_state
        .tasks
        .values()
        .find(|task| task.entry.run_id == report.run_id && task.entry.task_id == "task-a")
        .context("missing restarted task")?;
    assert_eq!(restarted_task.entry.attempts, 2);
    assert_eq!(restarted_task.status, FleetTaskLedgerStatus::Completed);
    let receipt = ledger_state
        .receipts
        .values()
        .find(|receipt| receipt.run_id == report.run_id && receipt.task_id == "task-a")
        .context("missing restarted receipt")?;
    assert_eq!(receipt.attempt, Some(2));
    assert!(receipt.terminal_seq.is_some());

    let stopped: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/fleet/runs/{}/stop",
            report.run_id.0
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(stopped["action"], "stop");
    assert_eq!(stopped["stopped"], 0);
    assert_eq!(stopped["status"]["completed"], 1);

    handle.abort();
    Ok(())
}

/// A Parallel Workflow task may claim the whole workspace by normalizing its
/// write scope to `"."`. `managed_paths_overlap` compared normalized strings
/// only, so `"."` never matched a sibling task's `"crates/tui"` claim and the
/// admission gate let two workers write the same tree concurrently.
#[test]
fn workspace_root_write_claim_collides_with_a_subdirectory_claim() {
    fn task(id: &str, writable: &str) -> FleetTaskSpec {
        serde_json::from_value(json!({
            "id": id,
            "name": id,
            "instructions": "work",
            "workspace": { "writable_paths": [writable] },
        }))
        .expect("fleet task spec fixture")
    }

    let error = reject_parallel_write_collisions(&[task("root", "."), task("sub", "crates/tui")])
        .expect_err("a whole-workspace claim overlaps every subdirectory claim");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(
        error.message.contains("write scope collision"),
        "unexpected message: {}",
        error.message
    );

    // The reverse order is the same collision.
    assert!(
        reject_parallel_write_collisions(&[task("sub", "crates/tui"), task("root", ".")]).is_err()
    );
    // Disjoint subdirectories still admit.
    assert!(
        reject_parallel_write_collisions(&[task("a", "crates/tui"), task("b", "crates/cli")])
            .is_ok()
    );
}

#[test]
fn fleet_worker_json_includes_runtime_state_projection() {
    let inspection = FleetWorkerInspection {
        worker_id: "fleet-worker-1".to_string(),
        status: FleetWorkerStatus::Busy,
        current_run_id: Some(FleetRunId::from("fleet-run-1")),
        current_task_id: Some("task-a".to_string()),
        objective: Some("Inspect runtime projection".to_string()),
        role: Some("reviewer".to_string()),
        host: Some("local".to_string()),
        latest_heartbeat_at: None,
        latest_event: None,
        artifacts: Vec::new(),
        receipt_summary: None,
        last_error: None,
        alert_state: None,
        runtime_state: Some(FleetWorkerRuntimeProjection {
            agent_status: "running".to_string(),
            steps_taken: 3,
            latest_message: Some("reading files".to_string()),
            error: None,
            result_summary: None,
            has_session: true,
        }),
    };

    let worker = fleet_worker_json(&inspection);

    assert_eq!(worker["runtime_state"]["agent_status"], "running");
    assert_eq!(worker["runtime_state"]["steps_taken"], 3);
    assert_eq!(worker["runtime_state"]["latest_message"], "reading files");
    assert_eq!(worker["runtime_state"]["has_session"], true);
}

#[tokio::test]
async fn agent_runs_runtime_api_exposes_persisted_worker_receipts() -> Result<()> {
    use crate::tools::subagent::{
        AgentRunArtifactRef, AgentRunFollowUpTarget, AgentRunRecommendedAction,
        AgentRunTakeoverTarget, AgentRunUsage, AgentRunVerificationSummary, AgentWorkerEvent,
        AgentWorkerRecord, AgentWorkerSpec, AgentWorkerStatus, AgentWorkerToolProfile, FleetRole,
    };
    use crate::worker_profile::{ModelRoute, ToolScope, WorkerRuntimeProfile};
    use std::collections::VecDeque;

    let root = std::env::temp_dir().join(format!("codewhale-agent-runs-api-{}", Uuid::new_v4()));
    let workspace = root.join("workspace");
    fs::create_dir_all(workspace.join(".codewhale/state"))?;

    let record = AgentWorkerRecord {
        spec: AgentWorkerSpec {
            worker_id: "agent_receipt".to_string(),
            run_id: "run_receipt".to_string(),
            parent_run_id: Some("parent_run".to_string()),
            session_name: Some("receipt_lane".to_string()),
            objective: "Verify run receipt projection".to_string(),
            role: Some("verifier".to_string()),
            agent_type: FleetRole::Verifier,
            model: "deepseek-v4-flash".to_string(),
            workspace: workspace.clone(),
            git_branch: Some("codex/v0.8.60".to_string()),
            context_mode: "fresh".to_string(),
            fork_context: false,
            tool_profile: AgentWorkerToolProfile::Explicit(vec!["read_file".to_string()]),
            runtime_profile: {
                let mut profile = WorkerRuntimeProfile::for_role(FleetRole::Verifier);
                profile.tools = ToolScope::Explicit(vec!["read_file".to_string()]);
                profile.model = ModelRoute::Fixed("deepseek-v4-flash".to_string());
                profile.max_spawn_depth =
                    crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH.saturating_sub(1);
                profile
            },
            max_steps: 4,
            spawn_depth: 1,
            max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
            child_route: None,
            launch_manifest: None,
        },
        owner_session_id: "session-receipt".to_string(),
        actor_kind: "subagent".to_string(),
        parent_run_id: Some("parent_run".to_string()),
        follow_up: AgentRunFollowUpTarget {
            tool: "handle_read".to_string(),
            agent_id: "agent_receipt".to_string(),
            session_name: Some("receipt_lane".to_string()),
            accepted_statuses: vec!["running".to_string(), "interrupted_continuable".to_string()],
            latest_delivery: None,
        },
        takeover: AgentRunTakeoverTarget {
            kind: "local_subagent_session".to_string(),
            supported: true,
            agent_id: "agent_receipt".to_string(),
            session_name: Some("receipt_lane".to_string()),
            instructions: "Use handle_read on the transcript_handle for agent_receipt.".to_string(),
            unsupported_reason: None,
        },
        artifacts: vec![AgentRunArtifactRef {
            kind: "transcript".to_string(),
            name: "transcript_handle".to_string(),
            target: "agent:agent_receipt".to_string(),
            description: "Read with handle_read from a live projection.".to_string(),
        }],
        usage: AgentRunUsage {
            status: "unknown".to_string(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cost_microusd: None,
            token_budget: None,
            budget_spent_tokens: None,
            budget_remaining_tokens: None,
            budget_scope: None,
            note: "not reported".to_string(),
        },
        usage_source_fingerprints: Default::default(),
        verification: AgentRunVerificationSummary {
            status: "self_report_only".to_string(),
            summary: "no verified receipt attached".to_string(),
        },
        recommended_action: AgentRunRecommendedAction {
            action: "verify_self_report".to_string(),
            tool: Some("handle_read".to_string()),
            reason: "Worker agent_receipt completed; verify its self-report.".to_string(),
        },
        status: AgentWorkerStatus::Completed,
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        completed_at_ms: Some(2),
        latest_message: Some("completed".to_string()),
        result_summary: Some("receipt complete".to_string()),
        error: None,
        steps_taken: 2,
        events: VecDeque::from([AgentWorkerEvent {
            seq: 1,
            worker_id: "agent_receipt".to_string(),
            status: AgentWorkerStatus::Completed,
            timestamp_ms: 2,
            message: Some("completed".to_string()),
            step: Some(2),
            tool_name: None,
        }]),
    };
    let state_payload = json!({
        "schema_version": 1,
        "agents": [],
        "workers": [record],
    });
    fs::write(
        workspace.join(".codewhale/state/subagents.v1.json"),
        serde_json::to_vec_pretty(&state_payload)?,
    )?;

    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root.clone(),
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let runs: serde_json::Value = client
        .get(format!("http://{addr}/v1/agent-runs"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(runs["runs"][0]["spec"]["run_id"], "run_receipt");
    assert_eq!(runs["runs"][0]["follow_up"]["tool"], "handle_read");
    assert_eq!(
        runs["runs"][0]["verification"]["status"],
        "self_report_only"
    );

    let run: serde_json::Value = client
        .get(format!("http://{addr}/v1/agent-runs/run_receipt"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(run["spec"]["worker_id"], "agent_receipt");
    assert_eq!(run["takeover"]["supported"], true);
    assert_eq!(run["artifacts"][0]["kind"], "transcript");

    let missing = client
        .get(format!("http://{addr}/v1/agent-runs/missing"))
        .send()
        .await?
        .status();
    assert_eq!(missing, StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn stream_requires_prompt() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/stream"))
        .json(&json!({ "prompt": "" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn compatibility_stream_closes_losslessly_across_replay_live_handoff() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("server");
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root,
            sessions_dir,
            None,
            false,
            workspace,
            TestServerOverrides {
                compat_stream_test_hook: Some(hook_tx),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(());
    };

    let client = crate::tls::reqwest_client();
    let stream_client = client.clone();
    let stream_task = tokio::spawn(async move {
        let response = stream_client
            .post(format!("http://{addr}/v1/stream"))
            .json(&json!({ "prompt": "cross the replay handoff" }))
            .send()
            .await?
            .error_for_status()?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.text().await?;
        Ok::<_, anyhow::Error>((content_type, body))
    });

    let created = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility stream did not create its thread")?
        .context("compatibility stream test hook closed")?;
    let (thread_id, resume_created) = match created {
        CompatStreamTestPoint::ThreadCreated { thread_id, resume } => (thread_id, resume),
        CompatStreamTestPoint::SubscribedBeforeReplay { .. }
        | CompatStreamTestPoint::ReplayLoaded { .. } => {
            bail!("compatibility stream loaded replay before its thread was prepared")
        }
    };

    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    let (release_overlap, wait_for_overlap_release) = oneshot::channel();
    let (release_terminal, wait_for_terminal_release) = oneshot::channel();
    let engine_task = tokio::spawn(async move {
        if !matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            return;
        }
        let _ = wait_for_overlap_release.await;
        let _ = tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "mock_compat_handoff".to_string(),
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
                content: "handoff".to_string(),
            })
            .await;
        let _ = wait_for_terminal_release.await;
        let _ = tx_event
            .send(EngineEvent::MessageComplete { index: 0 })
            .await;
        let _ = tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 3,
                    output_tokens: 1,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    });
    resume_created
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility stream dropped thread-create handoff"))?;

    let subscribed = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility stream did not subscribe before replay")?
        .context("compatibility stream test hook closed")?;
    let (subscribed_thread_id, subscribed_turn_id, resume_subscribed) = match subscribed {
        CompatStreamTestPoint::SubscribedBeforeReplay {
            thread_id,
            turn_id,
            resume,
        } => (thread_id, turn_id, resume),
        CompatStreamTestPoint::ThreadCreated { .. }
        | CompatStreamTestPoint::ReplayLoaded { .. } => {
            bail!("compatibility stream did not expose its subscribe-before-replay boundary")
        }
    };
    assert_eq!(subscribed_thread_id, thread_id);

    release_overlap
        .send(())
        .map_err(|_| anyhow::anyhow!("mock engine dropped overlap release"))?;
    tokio::time::timeout(ci_scaled(Duration::from_secs(2)), async {
        loop {
            if runtime_threads
                .events_since(&thread_id, None)
                .is_ok_and(|events| {
                    events.iter().any(|event| {
                        event.turn_id.as_deref() == Some(&subscribed_turn_id)
                            && event.event == "item.delta"
                    })
                })
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("overlap event was not persisted before compatibility replay")?;
    resume_subscribed
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility stream dropped subscribe handoff"))?;

    let replay_loaded = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility stream did not reach its replay/live handoff")?
        .context("compatibility stream test hook closed")?;
    let (replay_thread_id, turn_id, resume_replay) = match replay_loaded {
        CompatStreamTestPoint::ReplayLoaded {
            thread_id,
            turn_id,
            resume,
        } => (thread_id, turn_id, resume),
        CompatStreamTestPoint::ThreadCreated { .. }
        | CompatStreamTestPoint::SubscribedBeforeReplay { .. } => {
            bail!("compatibility stream created more than one thread")
        }
    };
    assert_eq!(replay_thread_id, thread_id);
    assert_eq!(turn_id, subscribed_turn_id);

    release_terminal
        .send(())
        .map_err(|_| anyhow::anyhow!("mock engine dropped terminal release"))?;
    tokio::time::timeout(ci_scaled(Duration::from_secs(2)), async {
        loop {
            if runtime_threads
                .events_since(&thread_id, None)
                .is_ok_and(|events| {
                    events.iter().any(|event| {
                        event.turn_id.as_deref() == Some(&turn_id)
                            && event.event == "turn.completed"
                    })
                })
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("terminal event was not persisted during compatibility handoff")?;
    resume_replay
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility stream dropped replay handoff"))?;

    let (content_type, body) = tokio::time::timeout(ci_scaled(Duration::from_secs(3)), stream_task)
        .await
        .context("compatibility stream hung after its terminal event")?
        .context("compatibility stream request task panicked")??;
    engine_task.await.context("mock engine task panicked")?;

    assert!(content_type.starts_with("text/event-stream"));
    assert_eq!(body.matches("event: message.delta").count(), 1, "{body}");
    assert_eq!(body.matches("event: turn.completed").count(), 1, "{body}");
    assert_eq!(body.matches("event: done").count(), 1, "{body}");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn compatibility_stream_exposes_and_resolves_user_input_without_answer_echo() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("server");
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    let (hook_tx, mut hook_rx) = mpsc::unbounded_channel();
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root.clone(),
            sessions_dir,
            None,
            false,
            workspace,
            TestServerOverrides {
                compat_stream_test_hook: Some(hook_tx),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(());
    };

    let client = crate::tls::reqwest_client();
    let stream_client = client.clone();
    let request_task = tokio::spawn(async move {
        let response = stream_client
            .post(format!("http://{addr}/v1/stream"))
            .json(&json!({ "prompt": "ask before continuing" }))
            .send()
            .await?
            .error_for_status()?;
        Ok::<_, anyhow::Error>(response)
    });

    let created = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility stream did not create its interaction thread")?
        .context("compatibility stream interaction hook closed")?;
    let (thread_id, resume_created) = match created {
        CompatStreamTestPoint::ThreadCreated { thread_id, resume } => (thread_id, resume),
        CompatStreamTestPoint::SubscribedBeforeReplay { .. }
        | CompatStreamTestPoint::ReplayLoaded { .. } => {
            bail!("compatibility interaction stream advanced before engine installation")
        }
    };

    let mut harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let (submission_tx, submission_rx) = oneshot::channel();
    let (release_completion, wait_for_completion_release) = oneshot::channel();
    let engine_task = tokio::spawn(async move {
        if !matches!(harness.rx_op.recv().await, Some(Op::SendMessage { .. })) {
            bail!("compatibility interaction engine did not receive a prompt");
        }
        harness
            .tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "mock_compat_input".to_string(),
                created_at: chrono::Utc::now(),
                route: None,
            })
            .await?;
        let request = crate::tools::user_input::UserInputRequest {
            questions: vec![crate::tools::user_input::UserInputQuestion {
                header: "Continue".to_string(),
                id: "choice".to_string(),
                question: "Continue the compatibility turn?".to_string(),
                options: vec![
                    crate::tools::user_input::UserInputOption {
                        label: "Continue".to_string(),
                        description: "Finish the turn".to_string(),
                    },
                    crate::tools::user_input::UserInputOption {
                        label: "Stop".to_string(),
                        description: "Cancel the turn".to_string(),
                    },
                ],
                allow_free_text: false,
                multi_select: false,
            }],
        };
        harness
            .tx_event
            .send(EngineEvent::ToolCallStarted {
                id: "input_compat".to_string(),
                name: "request_user_input".to_string(),
                input: serde_json::to_value(&request)?,
            })
            .await?;
        harness
            .tx_event
            .send(EngineEvent::UserInputRequired {
                id: "input_compat".to_string(),
                request,
            })
            .await?;
        let submission = harness.recv_user_input_submission().await;
        let tool_result = submission
            .as_ref()
            .map(|(_, response)| crate::tools::spec::ToolResult::json(response))
            .transpose()?
            .context("compatibility user input was canceled before tool completion")?;
        let _ = submission_tx.send(submission);
        wait_for_completion_release
            .await
            .context("compatibility interaction test dropped completion release")?;
        harness
            .tx_event
            .send(EngineEvent::ToolCallComplete {
                id: "input_compat".to_string(),
                name: "request_user_input".to_string(),
                result: Ok(tool_result),
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
                content: "continued".to_string(),
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
        Ok::<_, anyhow::Error>(())
    });
    resume_created
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility interaction stream dropped create hook"))?;

    let subscribed = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility interaction stream did not subscribe")?
        .context("compatibility stream interaction hook closed")?;
    let (subscribed_thread_id, turn_id, resume_subscribed) = match subscribed {
        CompatStreamTestPoint::SubscribedBeforeReplay {
            thread_id,
            turn_id,
            resume,
        } => (thread_id, turn_id, resume),
        CompatStreamTestPoint::ThreadCreated { .. }
        | CompatStreamTestPoint::ReplayLoaded { .. } => {
            bail!("compatibility interaction stream missed subscribe-before-replay hook")
        }
    };
    assert_eq!(subscribed_thread_id, thread_id);
    resume_subscribed
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility interaction stream dropped subscribe hook"))?;

    let replay_loaded = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), hook_rx.recv())
        .await
        .context("compatibility interaction stream did not load replay")?
        .context("compatibility stream interaction hook closed")?;
    let (replay_thread_id, replay_turn_id, resume_replay) = match replay_loaded {
        CompatStreamTestPoint::ReplayLoaded {
            thread_id,
            turn_id,
            resume,
        } => (thread_id, turn_id, resume),
        CompatStreamTestPoint::ThreadCreated { .. }
        | CompatStreamTestPoint::SubscribedBeforeReplay { .. } => {
            bail!("compatibility interaction stream missed replay-loaded hook")
        }
    };
    assert_eq!(replay_thread_id, thread_id);
    assert_eq!(replay_turn_id, turn_id);
    resume_replay
        .send(())
        .map_err(|_| anyhow::anyhow!("compatibility interaction stream dropped replay hook"))?;

    let response = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), request_task)
        .await
        .context("compatibility interaction request did not return SSE headers")?
        .context("compatibility interaction request task panicked")??;
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel();
    let body_task = tokio::spawn(collect_sse_frames(response, frame_tx));

    let required_payload = tokio::time::timeout(ci_scaled(Duration::from_secs(2)), async {
        loop {
            let (event, payload) = frame_rx
                .recv()
                .await
                .context("compatibility interaction stream ended before user input")?;
            if event == "user_input.required" {
                break Ok::<_, anyhow::Error>(payload);
            }
        }
    })
    .await
    .context("compatibility stream did not expose required user input")??;
    assert_eq!(required_payload["id"], "input_compat");
    assert_eq!(required_payload["input_id"], "input_compat");
    assert_eq!(required_payload["thread_id"], thread_id);
    assert_eq!(required_payload["turn_id"], turn_id);
    assert_eq!(required_payload["status"], "required");
    assert_eq!(required_payload["request"]["questions"][0]["id"], "choice");
    assert!(required_payload.get("answers").is_none());

    const SECRET_ANSWER: &str = "compat-answer-must-not-be-echoed";
    let submitted: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/user-input/{thread_id}/input_compat"
        ))
        .json(&json!({
            "answers": [{
                "id": "choice",
                "label": "Continue",
                "value": SECRET_ANSWER,
            }],
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(submitted["delivered"], true);
    let (submitted_id, submitted_response) =
        tokio::time::timeout(ci_scaled(Duration::from_secs(2)), submission_rx)
            .await
            .context("mock engine did not receive compatibility user input")?
            .context("mock engine dropped compatibility user input")?
            .context("compatibility user input was canceled instead of submitted")?;
    assert_eq!(submitted_id, "input_compat");
    assert_eq!(submitted_response.answers[0].value, SECRET_ANSWER);
    release_completion
        .send(())
        .map_err(|_| anyhow::anyhow!("mock interaction engine dropped completion release"))?;

    let frames = tokio::time::timeout(ci_scaled(Duration::from_secs(3)), body_task)
        .await
        .context("compatibility interaction stream did not terminate")?
        .context("compatibility interaction body task panicked")??;
    engine_task
        .await
        .context("compatibility interaction engine task panicked")??;

    let answered = frames
        .iter()
        .find(|(event, _)| event == "user_input.answered")
        .context("compatibility stream omitted submitted user-input lifecycle")?;
    assert_eq!(answered.1["id"], "input_compat");
    assert_eq!(answered.1["status"], "submitted");
    assert!(answered.1.get("answers").is_none());
    assert_eq!(
        frames
            .iter()
            .filter(|(event, _)| event == "user_input.required")
            .count(),
        1
    );
    assert_eq!(
        frames
            .iter()
            .filter(|(event, _)| event == "user_input.answered")
            .count(),
        1
    );
    assert!(
        !frames
            .iter()
            .any(|(event, _)| event == "user_input.canceled")
    );
    assert!(frames.iter().any(|(event, _)| event == "turn.completed"));
    assert_eq!(
        frames.iter().filter(|(event, _)| event == "done").count(),
        1
    );
    assert!(
        !serde_json::to_string(&frames)?.contains(SECRET_ANSWER),
        "submitted answer leaked into compatibility SSE"
    );
    let detail = runtime_threads.get_thread_detail(&thread_id).await?;
    let serialized_detail = serde_json::to_string(&detail)?;
    assert!(
        !serialized_detail.contains(SECRET_ANSWER),
        "submitted answer leaked into the thread snapshot"
    );
    let redacted_item = detail
        .items
        .iter()
        .find(|item| {
            item.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("tool_name"))
                .and_then(Value::as_str)
                == Some("request_user_input")
        })
        .context("request_user_input Runtime receipt was not persisted")?;
    assert_eq!(
        redacted_item.detail.as_deref(),
        Some("User input submitted")
    );
    assert_eq!(
        redacted_item
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("response_redacted"))
            .and_then(Value::as_bool),
        Some(true)
    );
    let durable_events = runtime_threads.events_since(&thread_id, None)?;
    assert!(
        !serde_json::to_string(&durable_events)?.contains(SECRET_ANSWER),
        "submitted answer leaked into the durable Runtime event log"
    );
    let leaked_file = ignore::WalkBuilder::new(root.join("runtime"))
        .hidden(false)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .find_map(|entry| {
            fs::read_to_string(entry.path())
                .ok()
                .filter(|contents| contents.contains(SECRET_ANSWER))
                .map(|_| entry.path().to_path_buf())
        });
    assert!(
        leaked_file.is_none(),
        "submitted answer leaked into Runtime file {}",
        leaked_file
            .as_deref()
            .map(std::path::Path::display)
            .map(|path| path.to_string())
            .unwrap_or_default()
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn thread_endpoints_expose_lifecycle_contract() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let archived: serde_json::Value = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({ "archived": true }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(archived["id"], thread_id);
    assert_eq!(archived["archived"], true);

    let listed: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        listed
            .as_array()
            .is_some_and(|threads| threads.iter().all(|t| t["id"] != thread_id))
    );

    let listed_all: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads/summary?include_archived=true&limit=100"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        listed_all
            .as_array()
            .is_some_and(|threads| threads.iter().any(|t| t["id"] == thread_id))
    );

    let unarchived: serde_json::Value = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({ "archived": false }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(unarchived["archived"], false);

    let invalid_patch = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(invalid_patch.status(), StatusCode::BAD_REQUEST);

    let missing_patch = client
        .patch(format!("http://{addr}/v1/threads/thr_missing"))
        .json(&json!({ "archived": true }))
        .send()
        .await?;
    assert_eq!(missing_patch.status(), StatusCode::NOT_FOUND);

    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{thread_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["thread"]["id"], thread_id);

    let resumed: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/resume"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(resumed["id"], thread_id);

    let forked: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/fork"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let forked_id = forked["id"].as_str().context("missing forked id")?;
    assert_ne!(forked_id, thread_id);

    // Install a mock engine so the turn completes without calling the real API.
    // The mock handles both SendMessage and CompactContext ops so the
    // compact endpoint tested later also works.
    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        while let Some(op) = rx_op.recv().await {
            match op {
                Op::SendMessage { .. } => {
                    let _ = tx_event
                        .send(EngineEvent::TurnStarted {
                            turn_id: "mock_lifecycle".to_string(),
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
                            content: "mock reply".to_string(),
                        })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::MessageComplete { index: 0 })
                        .await;
                    let _ = tx_event
                        .send(EngineEvent::TurnComplete {
                            usage: Usage {
                                input_tokens: 10,
                                output_tokens: 5,
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
                    let _ = tx_event
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
                        .await;
                }
                _ => {}
            }
        }
    });

    let turn_start: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "prompt": "thread endpoint test" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let turn_id = turn_start["turn"]["id"]
        .as_str()
        .context("missing turn id")?
        .to_string();

    let _ =
        wait_for_terminal_turn_status(&client, addr, &thread_id, &turn_id, Duration::from_secs(2))
            .await?;

    let steer_resp = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/{turn_id}/steer"
        ))
        .json(&json!({ "prompt": "late steer" }))
        .send()
        .await?;
    assert_eq!(steer_resp.status(), StatusCode::CONFLICT);

    let interrupt_resp = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/{turn_id}/interrupt"
        ))
        .send()
        .await?;
    assert_eq!(interrupt_resp.status(), StatusCode::CONFLICT);

    let compact_start: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/compact"))
        .json(&json!({ "reason": "test manual compact" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(compact_start["thread"]["id"], thread_id);

    let events_resp = client
        .get(format!(
            "http://{addr}/v1/threads/{thread_id}/events?since_seq=0"
        ))
        .send()
        .await?
        .error_for_status()?;
    let content_type = events_resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/event-stream"));
    let chunk_text = read_first_sse_frame(events_resp).await?;
    assert!(
        chunk_text.contains("event:"),
        "expected SSE event chunk, got: {chunk_text}"
    );
    let (event_name, payload) = parse_sse_frame(&chunk_text)?;
    assert_eq!(event_name, "thread.started");
    assert!(
        event_name.starts_with("item.")
            || event_name.starts_with("turn.")
            || event_name.starts_with("thread.")
            || event_name == "turn.completed"
            || event_name == "turn.started"
            || event_name == "thread.started",
        "unexpected first event name: {event_name}"
    );
    assert_eq!(payload["event"], payload["kind"]);
    assert!(payload.get("turn_id").is_some());
    assert!(payload.get("item_id").is_some());
    assert!(payload["turn_id"].is_null());
    assert!(payload["item_id"].is_null());
    assert_eq!(payload["thread_id"], thread_id);
    assert!(
        payload["schema_version"]
            .as_u64()
            .is_some_and(|version| version >= 1)
    );
    assert!(payload.get("seq").and_then(Value::as_u64).is_some());
    assert!(payload["payload"].is_object() || payload["payload"].is_array());

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn turn_endpoint_operation_key_returns_original_and_conflicts_on_mismatch() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"].as_str().context("thread id")?.to_string();

    let mut harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let send_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = send_count.clone();
    let tx_event = harness.tx_event.clone();
    tokio::spawn(async move {
        while let Some(op) = harness.rx_op.recv().await {
            if !matches!(op, Op::SendMessage { .. }) {
                continue;
            }
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            sleep(Duration::from_millis(20)).await;
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

    let url = format!("http://{addr}/v1/threads/{thread_id}/turns");
    let request = json!({
        "prompt": "one HTTP operation",
        "operation_key": "cwc-http-operation-1",
        "reasoning_effort": "high",
        "allowed_tools": []
    });
    let first_response = client.post(&url).json(&request).send().await?;
    assert_eq!(first_response.status(), StatusCode::CREATED);
    let first: serde_json::Value = first_response.json().await?;
    let first_turn_id = first["turn"]["id"].as_str().context("turn id")?;
    assert!(!serde_json::to_string(&first)?.contains("cwc-http-operation-1"));

    let replay_response = client.post(&url).json(&request).send().await?;
    assert_eq!(replay_response.status(), StatusCode::CREATED);
    let replay: serde_json::Value = replay_response.json().await?;
    assert_eq!(replay["turn"]["id"], first_turn_id);

    let mismatch = client
        .post(&url)
        .json(&json!({
            "prompt": "changed HTTP operation",
            "operation_key": "cwc-http-operation-1",
            "reasoning_effort": "high",
            "allowed_tools": []
        }))
        .send()
        .await?;
    assert_eq!(mismatch.status(), StatusCode::CONFLICT);

    sleep(Duration::from_millis(40)).await;
    assert_eq!(
        send_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exact replay and mismatch must not emit another SendMessage"
    );
    assert_eq!(
        runtime_threads
            .test_store()
            .list_turns_for_thread(&thread_id)?
            .len(),
        1
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn events_endpoint_respects_since_seq_cursor() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    // Install a mock engine so the turn completes without calling the real API.
    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if !matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            return;
        }
        let _ = tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "mock_cursor".to_string(),
                created_at: chrono::Utc::now(),
                route: None,
            })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageStarted { index: 0 })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageComplete { index: 0 })
            .await;
        let _ = tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 3,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    });

    let started: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "prompt": "cursor replay test" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let turn_id = started["turn"]["id"]
        .as_str()
        .context("missing turn id")?
        .to_string();

    let _ =
        wait_for_terminal_turn_status(&client, addr, &thread_id, &turn_id, Duration::from_secs(2))
            .await?;

    let resp_a = client
        .get(format!(
            "http://{addr}/v1/threads/{thread_id}/events?since_seq=0"
        ))
        .send()
        .await?
        .error_for_status()?;
    let frame_a = read_first_sse_frame(resp_a).await?;
    let (event_a, payload_a) = parse_sse_frame(&frame_a)?;
    assert_eq!(event_a, "thread.started");
    assert!(payload_a.get("turn_id").is_some());
    assert!(payload_a.get("item_id").is_some());
    assert!(payload_a["turn_id"].is_null());
    assert!(payload_a["item_id"].is_null());
    assert!(payload_a.get("schema_version").is_some());
    assert_eq!(payload_a["event"], payload_a["kind"]);
    assert_eq!(payload_a["thread_id"], thread_id);
    let seq_a = payload_a
        .get("seq")
        .and_then(Value::as_u64)
        .context("missing seq in first replay frame")?;

    let resp_b = client
        .get(format!(
            "http://{addr}/v1/threads/{thread_id}/events?since_seq={seq_a}"
        ))
        .send()
        .await?
        .error_for_status()?;
    let frame_b = read_first_sse_frame(resp_b).await?;
    let (_event_b, payload_b) = parse_sse_frame(&frame_b)?;
    assert!(payload_b.get("schema_version").is_some());
    assert_eq!(payload_b["event"], payload_b["kind"]);
    assert_eq!(payload_b["thread_id"], thread_id);
    let seq_b = payload_b
        .get("seq")
        .and_then(Value::as_u64)
        .context("missing seq in second replay frame")?;
    assert!(
        seq_b > seq_a,
        "expected seq after cursor: {seq_b} <= {seq_a}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn event_handoff_replays_and_dedupes_interaction_prompts_without_a_gap() -> Result<()> {
    let Some((_addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let thread = runtime_threads
        .create_thread(CreateThreadRequest::default())
        .await?;
    let initial_seq = runtime_threads
        .events_since(&thread.id, None)?
        .last()
        .context("thread creation should emit an event")?
        .seq;

    // Deterministically place approval.required in the old vulnerable window:
    // the receiver exists, but durable replay has not been read yet.
    let live = runtime_threads.subscribe_events();
    let approval = runtime_threads
        .emit_event_for_test(
            &thread.id,
            None,
            "approval.required",
            json!({
                "approval_id": "approval-handoff",
                "tool_name": "exec_command",
                "description": "Run a local check",
            }),
        )
        .await?;
    let backlog = runtime_threads.events_since(&thread.id, Some(initial_seq))?;
    let (backlog_tx, backlog_rx) = mpsc::channel(1);
    backlog_tx
        .send(Ok(backlog))
        .await
        .map_err(|_| anyhow::anyhow!("failed to seed replay backlog"))?;
    drop(backlog_tx);

    // This request lands after the replay read and is therefore live-only.
    let input = runtime_threads
        .emit_event_for_test(
            &thread.id,
            None,
            "user_input.required",
            json!({
                "id": "input-handoff",
                "request": {
                    "questions": [{
                        "id": "choice",
                        "question": "Continue?",
                        "options": [],
                    }],
                },
            }),
        )
        .await?;

    let stream = replay_live_thread_events(
        runtime_threads.clone(),
        thread.id.clone(),
        initial_seq,
        backlog_rx,
        live,
    )
    .take(2);
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let rendered = String::from_utf8(body.to_vec())?;
    let frames = rendered
        .split("\n\n")
        .map(str::trim)
        .filter(|frame| !frame.is_empty())
        .map(parse_sse_frame)
        .collect::<Result<Vec<_>>>()?;

    assert_eq!(frames.len(), 2, "unexpected SSE frames: {rendered}");
    assert_eq!(frames[0].0, "approval.required");
    assert_eq!(frames[0].1["seq"], approval.seq);
    assert_eq!(frames[0].1["previous_seq"], initial_seq);
    assert_eq!(frames[1].0, "user_input.required");
    assert_eq!(frames[1].1["seq"], input.seq);
    assert_eq!(frames[1].1["previous_seq"], approval.seq);
    assert_eq!(rendered.matches("approval-handoff").count(), 1);
    assert_eq!(rendered.matches("input-handoff").count(), 1);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn steer_and_interrupt_endpoints_work_on_active_turn() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let mut rx_steer = harness.rx_steer;
    let tx_event = harness.tx_event;
    let cancel_token = harness.cancel_token;
    tokio::spawn(async move {
        if !matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            return;
        }
        let _ = tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "engine_turn_api".to_string(),
                created_at: chrono::Utc::now(),
                route: None,
            })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageStarted { index: 0 })
            .await;
        if let Some(steer_text) = rx_steer.recv().await {
            let _ = tx_event
                .send(EngineEvent::MessageDelta {
                    index: 0,
                    content: format!("steer:{steer_text}"),
                })
                .await;
        }
        cancel_token.cancelled().await;
        sleep(Duration::from_millis(60)).await;
        let _ = tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    });

    let turn_start: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "prompt": "active controls" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let turn_id = turn_start["turn"]["id"]
        .as_str()
        .context("missing turn id")?
        .to_string();

    let steer_resp: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/{turn_id}/steer"
        ))
        .json(&json!({ "prompt": "please steer" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(steer_resp["id"], turn_id);
    assert_eq!(steer_resp["steer_count"], 1);

    let interrupt_resp: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/{turn_id}/interrupt"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(interrupt_resp["id"], turn_id);

    let terminal =
        wait_for_terminal_turn_status(&client, addr, &thread_id, &turn_id, Duration::from_secs(3))
            .await?;
    assert_eq!(terminal, "interrupted");

    let events = runtime_threads.events_since(&thread_id, None)?;
    assert!(events.iter().any(|ev| ev.event == "turn.steered"));
    assert!(
        events
            .iter()
            .any(|ev| ev.event == "turn.interrupt_requested")
    );
    assert!(events.iter().any(|ev| {
        ev.event == "turn.completed"
            && ev
                .payload
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                == Some("interrupted")
    }));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn stream_compat_mapping_handles_expected_runtime_events() -> Result<()> {
    let agent_delta = RuntimeEventRecord {
        schema_version: 1,
        seq: 1,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: Some("item_test".to_string()),
        event: "item.delta".to_string(),
        payload: json!({
            "kind": "agent_message",
            "delta": "hello",
        }),
    };
    let mapped = map_compat_stream_event(&agent_delta).context("missing mapped SSE event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: message.delta"));
    assert!(text.contains("\"content\":\"hello\""));

    let tool_start = RuntimeEventRecord {
        schema_version: 1,
        seq: 2,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: Some("item_tool".to_string()),
        event: "item.started".to_string(),
        payload: json!({
            "tool": { "id": "tool_1", "name": "exec_shell", "input": { "cmd": "pwd" } }
        }),
    };
    let mapped = map_compat_stream_event(&tool_start).context("missing tool.started event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: tool.started"));

    let tool_done = RuntimeEventRecord {
        schema_version: 1,
        seq: 3,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: Some("item_tool".to_string()),
        event: "item.completed".to_string(),
        payload: json!({
            "item": {
                "id": "item_tool",
                "kind": "tool_call",
                "summary": "ok",
                "detail": "done"
            }
        }),
    };
    let mapped = map_compat_stream_event(&tool_done).context("missing tool.completed event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: tool.completed"));
    assert!(text.contains("\"success\":true"));

    let user_input_required = RuntimeEventRecord {
        schema_version: 1,
        seq: 4,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "user_input.required".to_string(),
        payload: json!({
            "id": "input_test",
            "request": {
                "questions": [{
                    "header": "Continue",
                    "id": "choice",
                    "question": "Continue?",
                    "options": [
                        { "label": "Yes", "description": "Continue" },
                        { "label": "No", "description": "Stop" }
                    ]
                }]
            },
            "internal_secret": "required-secret",
        }),
    };
    let mapped = map_compat_stream_event(&user_input_required)
        .context("missing user_input.required event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: user_input.required"));
    assert!(text.contains("\"input_id\":\"input_test\""));
    assert!(text.contains("\"status\":\"required\""));
    assert!(!text.contains("required-secret"));

    let user_input_answered = RuntimeEventRecord {
        schema_version: 1,
        seq: 5,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "user_input.answered".to_string(),
        payload: json!({
            "input_id": "input_test",
            "answers": [{ "id": "choice", "value": "answer-secret" }],
        }),
    };
    let mapped = map_compat_stream_event(&user_input_answered)
        .context("missing user_input.answered event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: user_input.answered"));
    assert!(text.contains("\"status\":\"submitted\""));
    assert!(!text.contains("answer-secret"));
    assert!(!text.contains("\"answers\""));

    let user_input_canceled = RuntimeEventRecord {
        schema_version: 1,
        seq: 6,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "user_input.canceled".to_string(),
        payload: json!({ "id": "input_test", "terminal": true }),
    };
    let mapped = map_compat_stream_event(&user_input_canceled)
        .context("missing user_input.canceled event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: user_input.canceled"));
    assert!(text.contains("\"status\":\"canceled\""));
    assert!(text.contains("\"terminal\":true"));

    let approval_required = RuntimeEventRecord {
        schema_version: 1,
        seq: 7,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "approval.required".to_string(),
        payload: json!({
            "approval_id": "approval_test",
            "tool_name": "exec_command",
            "description": "Run tests",
            "input": { "token": "approval-secret" },
        }),
    };
    let mapped =
        map_compat_stream_event(&approval_required).context("missing approval.required event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: approval.required"));
    assert!(text.contains("\"approval_id\":\"approval_test\""));
    assert!(!text.contains("approval-secret"));

    let approval_decided = RuntimeEventRecord {
        schema_version: 1,
        seq: 8,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "approval.decided".to_string(),
        payload: json!({
            "approval_id": "approval_test",
            "decision": "allow",
            "remember": false,
            "internal_secret": "approval-decision-secret",
        }),
    };
    let mapped =
        map_compat_stream_event(&approval_decided).context("missing approval.decided event")?;
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(mapped);
    };
    let body =
        axum::body::to_bytes(Sse::new(stream).into_response().into_body(), usize::MAX).await?;
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: approval.decided"));
    assert!(text.contains("\"decision\":\"allow\""));
    assert!(!text.contains("approval-decision-secret"));

    let unknown = RuntimeEventRecord {
        schema_version: 1,
        seq: 9,
        timestamp: chrono::Utc::now(),
        thread_id: "thr_test".to_string(),
        turn_id: Some("turn_test".to_string()),
        item_id: None,
        event: "item.delta".to_string(),
        payload: json!({
            "kind": "context_compaction",
            "delta": "ignored",
        }),
    };
    assert!(map_compat_stream_event(&unknown).is_none());
    Ok(())
}

#[tokio::test]
async fn stream_endpoint_remains_backward_compatible() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Create a thread and install a mock engine so /v1/stream doesn't call the real API.
    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    tokio::spawn(async move {
        if !matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            return;
        }
        let _ = tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "mock_stream".to_string(),
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
                content: "streamed".to_string(),
            })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageComplete { index: 0 })
            .await;
        let _ = tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 4,
                    output_tokens: 2,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    });

    // Start the turn and consume events via the SSE endpoint.
    let turn_start: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "prompt": "compatibility stream" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let turn_id = turn_start["turn"]["id"]
        .as_str()
        .context("missing turn id")?
        .to_string();

    let _ =
        wait_for_terminal_turn_status(&client, addr, &thread_id, &turn_id, Duration::from_secs(2))
            .await?;

    // Verify that the persisted events include the expected turn lifecycle events.
    let events = runtime_threads.events_since(&thread_id, None)?;
    assert!(
        events.iter().any(|ev| ev.event == "turn.started"),
        "expected turn.started event"
    );
    assert!(
        events.iter().any(|ev| ev.event == "turn.completed"),
        "expected turn.completed event"
    );

    // Verify the SSE endpoint returns event-stream content type.
    let events_resp = client
        .get(format!(
            "http://{addr}/v1/threads/{thread_id}/events?since_seq=0"
        ))
        .send()
        .await?
        .error_for_status()?;
    let content_type = events_resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/event-stream"));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_get_returns_404_for_missing_id() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .get(format!("http://{addr}/v1/sessions/nonexistent_id"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_endpoints_reject_invalid_id() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let get_resp = client
        .get(format!("http://{addr}/v1/sessions/invalid%20id"))
        .send()
        .await?;
    assert_eq!(get_resp.status(), StatusCode::BAD_REQUEST);

    let resume_resp = client
        .post(format!(
            "http://{addr}/v1/sessions/invalid%20id/resume-thread"
        ))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resume_resp.status(), StatusCode::BAD_REQUEST);

    let delete_resp = client
        .delete(format!("http://{addr}/v1/sessions/invalid%20id"))
        .send()
        .await?;
    assert_eq!(delete_resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_resume_thread_returns_404_for_missing_session() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!(
            "http://{addr}/v1/sessions/nonexistent_session/resume-thread"
        ))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_resume_thread_returns_400_when_saved_custom_provider_was_removed() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-session-removed-provider-{}",
        Uuid::new_v4()
    ));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&sessions_dir)?;
    let session = json!({
        "schema_version": 1,
        "metadata": {
            "id": "sess_removed_custom_provider",
            "title": "Removed custom provider",
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:10:00Z",
            "message_count": 1,
            "total_tokens": 10,
            "model": "local-code-model",
            "model_provider": "lm-studio",
            "workspace": "/tmp/test",
            "mode": "agent"
        },
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "Resume me" }]
        }],
        "system_prompt": null
    });
    fs::write(
        sessions_dir.join("sess_removed_custom_provider.json"),
        serde_json::to_string_pretty(&session)?,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(format!(
            "http://{addr}/v1/sessions/sess_removed_custom_provider/resume-thread"
        ))
        .json(&json!({}))
        .send()
        .await?;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await?;
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("[providers.lm-studio]"), "{message}");
    assert!(message.contains("will not fall back"), "{message}");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_resume_thread_creates_thread_from_saved_session() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-session-resume-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&sessions_dir)?;
    let session = json!({
        "schema_version": 1,
        "metadata": {
            "id": "sess_test_resume",
            "title": "Test resume session",
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:10:00Z",
            "message_count": 2,
            "total_tokens": 100,
            "model": "deepseek-v4-pro",
            "workspace": "/tmp/test",
            "mode": "agent"
        },
        "messages": [
            {
                "role": "user",
                "content": [{ "type": "text", "text": "Hello, world!" }]
            },
            {
                "role": "assistant",
                "content": [{ "type": "text", "text": "Hello! How can I help you?" }]
            }
        ],
        "system_prompt": null
    });
    fs::write(
        sessions_dir.join("sess_test_resume.json"),
        serde_json::to_string_pretty(&session)?,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!(
            "http://{addr}/v1/sessions/sess_test_resume/resume-thread"
        ))
        .json(&json!({ "model": "deepseek-v4-pro" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resumed: serde_json::Value = resp.json().await?;
    assert_eq!(resumed["session_id"], "sess_test_resume");
    assert_eq!(resumed["message_count"], 2);

    let thread_id = resumed["thread_id"]
        .as_str()
        .context("missing resumed thread id")?;
    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{thread_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["thread"]["id"], thread_id);
    assert_eq!(detail["thread"]["model_provider"], "deepseek");
    assert_eq!(detail["thread"]["workspace"], "/tmp/test");
    assert_eq!(detail["turns"].as_array().map_or(0, Vec::len), 1);
    assert_eq!(detail["items"].as_array().map_or(0, Vec::len), 2);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_create_from_completed_thread_saves_messages() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-thread-session-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "deepseek-v4-pro",
            "mode": "plan",
            "workspace": root.join("workspace")
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let patched: serde_json::Value = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({ "title": "Thread title fallback" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(patched["title"], "Thread title fallback");

    runtime_threads
        .seed_thread_from_messages(
            &thread_id,
            &[
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "Please save this runtime thread".to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Saved replies should round-trip.".to_string(),
                        cache_control: None,
                    }],
                },
            ],
        )
        .await?;

    let resp = client
        .post(format!("http://{addr}/v1/sessions"))
        .json(&json!({ "thread_id": thread_id }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let saved: serde_json::Value = resp.json().await?;
    assert_eq!(saved["thread_id"], thread_id);
    assert_eq!(saved["message_count"], 2);
    assert_eq!(saved["title"], "Thread title fallback");
    let saved_session_handle = saved["session_id"]
        .as_str()
        .context("missing session id")?
        .to_string();

    let session_manager = crate::session_manager::SessionManager::new(root.join("sessions"))?;
    let created_session = session_manager.load_session_by_prefix(&saved_session_handle)?;
    assert_eq!(created_session.metadata.title, "Thread title fallback");
    assert_eq!(created_session.metadata.model, "deepseek-v4-pro");
    assert_eq!(created_session.metadata.mode.as_deref(), Some("plan"));
    assert_eq!(created_session.metadata.message_count, 2);
    assert_eq!(created_session.messages[0].role, "user");
    assert_eq!(created_session.messages[1].role, "assistant");

    let mut endpoint_session = crate::session_manager::create_saved_session_with_id_and_mode(
        "sess_endpoint_fetch".to_string(),
        &created_session.messages,
        "deepseek-v4-pro",
        &root,
        0,
        None,
        Some("plan"),
    );
    endpoint_session.metadata.title = "Thread title fallback".to_string();
    session_manager.save_session(&endpoint_session)?;

    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/sessions/sess_endpoint_fetch"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["metadata"]["title"], "Thread title fallback");
    assert_eq!(detail["metadata"]["model"], "deepseek-v4-pro");
    assert_eq!(detail["metadata"]["mode"], "plan");
    assert_eq!(detail["metadata"]["message_count"], 2);
    assert_eq!(detail["messages"][0]["role"], "user");
    assert_eq!(
        detail["messages"][0]["content"][0]["text"],
        "Please save this runtime thread"
    );
    assert_eq!(detail["messages"][1]["role"], "assistant");

    let manual_title: serde_json::Value = client
        .post(format!("http://{addr}/v1/sessions"))
        .json(&json!({
            "thread_id": thread_id,
            "title": "Manual saved title"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(manual_title["title"], "Manual saved title");
    assert_ne!(manual_title["session_id"], saved_session_handle);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_create_from_thread_returns_404_for_missing_thread() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/sessions"))
        .json(&json!({ "thread_id": "thr_missing" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

/// Create a thread over HTTP and seed it with one user/assistant turn.
/// Shared setup for the undo/patch-undo/retry endpoint tests.
async fn create_seeded_thread(
    addr: &SocketAddr,
    runtime_threads: &SharedRuntimeThreadManager,
    root: &FsPath,
    user_text: &str,
) -> Result<String> {
    let client = crate::tls::reqwest_client();
    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "deepseek-v4-pro",
            "mode": "agent",
            "workspace": root.join("workspace")
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    runtime_threads
        .seed_thread_from_messages(
            &thread_id,
            &[
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: user_text.to_string(),
                        cache_control: None,
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Done — anything else?".to_string(),
                        cache_control: None,
                    }],
                },
            ],
        )
        .await?;
    Ok(thread_id)
}

#[tokio::test]
async fn undo_endpoint_forks_thread_and_returns_original_user_text() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-undo-endpoint-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let thread_id =
        create_seeded_thread(&addr, &runtime_threads, &root, "Please undo this turn").await?;
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/undo"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let undone: serde_json::Value = resp.json().await?;
    assert_eq!(undone["original_user_text"], "Please undo this turn");
    let forked_id = undone["thread"]["id"]
        .as_str()
        .context("missing forked thread id")?;
    assert_ne!(forked_id, thread_id, "undo must fork, not mutate in place");

    // The forked thread has the undone turn removed.
    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{forked_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["turns"].as_array().map_or(usize::MAX, Vec::len), 0);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn undo_endpoint_404s_for_missing_thread() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(format!("http://{addr}/v1/threads/thr_missing/undo"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn patch_undo_endpoint_forks_and_reports_file_rollback_state() -> Result<()> {
    let root =
        std::env::temp_dir().join(format!("deepseek-patch-undo-endpoint-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let thread_id =
        create_seeded_thread(&addr, &runtime_threads, &root, "Roll back the patch").await?;
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/patch-undo"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let undone: serde_json::Value = resp.json().await?;
    // The fresh workspace has no tool/pre-turn snapshots to roll back to,
    // so the file-restore step reports failure while the conversation
    // undo still forks the thread.
    assert_eq!(undone["patch_result"]["files_restored"], false);
    assert!(undone["patch_result"]["summary"].is_string());
    assert_eq!(undone["original_user_text"], "Roll back the patch");
    assert_ne!(undone["thread"]["id"].as_str(), Some(thread_id.as_str()));

    handle.abort();
    Ok(())
}

#[test]
fn patch_undo_helper_restores_only_the_bound_session() -> Result<()> {
    let _lock = lock_test_env();
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    fs::create_dir_all(&home)?;
    let _home = EnvVarGuard::set("HOME", &home);

    let workspace = root.path().join("workspace");
    fs::create_dir_all(&workspace)?;
    let repo = crate::snapshot::SnapshotRepo::open_or_init(&workspace)?;
    let file = workspace.join("a.txt");

    fs::write(&file, "legacy")?;
    repo.snapshot("pre-turn:legacy")?;
    fs::write(&file, "current-before")?;
    repo.snapshot_with_session("pre-turn:current", Some("session-current"))?;
    fs::write(&file, "foreign-before")?;
    repo.snapshot_with_session("pre-turn:foreign", Some("session-foreign"))?;
    fs::write(&file, "current-after")?;

    let restored = patch_undo_workspace_files(&workspace, Some("session-current"));
    assert!(restored.files_restored, "{:?}", restored.summary);
    assert_eq!(fs::read_to_string(&file)?, "current-before");

    fs::write(&file, "must-stay")?;
    let unbound = patch_undo_workspace_files(&workspace, None);
    assert!(!unbound.files_restored);
    assert_eq!(fs::read_to_string(&file)?, "must-stay");
    Ok(())
}

#[tokio::test]
async fn retry_endpoint_reuses_dropped_user_text_to_start_a_turn() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-retry-endpoint-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let thread_id =
        create_seeded_thread(&addr, &runtime_threads, &root, "Retry this request").await?;
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/retry"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let retried: serde_json::Value = resp.json().await?;
    let forked_id = retried["thread"]["id"]
        .as_str()
        .context("missing forked thread id")?;
    assert_ne!(forked_id, thread_id);
    assert_eq!(retried["turn"]["thread_id"], forked_id);

    handle.abort();
    Ok(())
}

#[test]
fn restore_snapshot_endpoint_helper_restores_workspace_files() -> Result<()> {
    let _lock = lock_test_env();
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    fs::create_dir_all(&home)?;
    let _home = EnvVarGuard::set("HOME", &home);

    let workspace = root.path().join("workspace");
    fs::create_dir_all(&workspace)?;
    let repo = crate::snapshot::SnapshotRepo::open_or_init(&workspace)?;
    fs::write(workspace.join("a.txt"), "v1")?;
    let snapshot_id = repo.snapshot("pre-turn:1")?;
    fs::write(workspace.join("a.txt"), "v2")?;

    restore_snapshot_for_workspace(&workspace, snapshot_id.as_str())
        .expect("snapshot restore should succeed");
    assert_eq!(fs::read_to_string(workspace.join("a.txt"))?, "v1");
    Ok(())
}

#[tokio::test]
async fn session_create_from_thread_rejects_active_turn() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread_id, harness.handle.clone())
        .await?;
    let mut rx_op = harness.rx_op;
    let tx_event = harness.tx_event;
    let (active_tx, active_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    tokio::spawn(async move {
        if !matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
            return;
        }
        let _ = tx_event
            .send(EngineEvent::TurnStarted {
                turn_id: "mock_active_session_save".to_string(),
                created_at: chrono::Utc::now(),
                route: None,
            })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageStarted { index: 0 })
            .await;
        let _ = active_tx.send(());
        let _ = finish_rx.await;
        let _ = tx_event
            .send(EngineEvent::MessageDelta {
                index: 0,
                content: "now complete".to_string(),
            })
            .await;
        let _ = tx_event
            .send(EngineEvent::MessageComplete { index: 0 })
            .await;
        let _ = tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
            })
            .await;
    });

    let started: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
        .json(&json!({ "prompt": "save me while active" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let turn_id = started["turn"]["id"]
        .as_str()
        .context("missing turn id")?
        .to_string();
    tokio::time::timeout(ci_scaled(Duration::from_secs(2)), active_rx)
        .await
        .context("timed out waiting for mock active turn")?
        .context("mock active turn sender dropped")?;
    wait_for_in_progress_item(&client, addr, &thread_id, Duration::from_secs(2)).await?;

    let resp = client
        .post(format!("http://{addr}/v1/sessions"))
        .json(&json!({ "thread_id": thread_id }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json().await?;
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("queued or active turn"))
    );

    let _ = finish_tx.send(());
    let terminal =
        wait_for_terminal_turn_status(&client, addr, &thread_id, &turn_id, Duration::from_secs(2))
            .await?;
    assert_eq!(terminal, "completed");

    handle.abort();
    Ok(())
}

#[test]
fn snapshots_endpoint_lists_workspace_snapshots() -> Result<()> {
    let _lock = lock_test_env();
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    fs::create_dir_all(&home)?;
    let _home = EnvVarGuard::set("HOME", &home);

    let workspace = root.path().join("workspace");
    fs::create_dir_all(&workspace)?;
    let repo = crate::snapshot::SnapshotRepo::open_or_init(&workspace)?;
    fs::write(workspace.join("a.txt"), "v1")?;
    repo.snapshot("pre-turn:1")?;
    fs::write(workspace.join("a.txt"), "v2")?;
    repo.snapshot("post-turn:1")?;

    let snapshots = snapshot_entries_for_workspace(&workspace, SnapshotsQuery { limit: Some(1) })
        .expect("snapshot listing should succeed");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].label, "post-turn:1");
    assert!(snapshots[0].id.len() >= 8);
    assert!(snapshots[0].timestamp > 0);

    let bad_limit = snapshot_entries_for_workspace(&workspace, SnapshotsQuery { limit: Some(101) })
        .expect_err("limit above cap should fail");
    assert_eq!(bad_limit.status, StatusCode::BAD_REQUEST);
    Ok(())
}

/// Seed a sessions directory and start a server against it.
///
/// The route tests below need to control what is on disk, so they cannot use
/// the `spawn_test_server()` convenience that hides its own temp paths.
async fn spawn_server_with_saved_sessions(
    sessions: &[(&str, &str, bool)],
) -> Result<Option<(SocketAddr, PathBuf, tokio::task::JoinHandle<()>)>> {
    let root = std::env::temp_dir().join(format!("codewhale-session-routes-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace)?;
    let manager = crate::session_manager::SessionManager::new(sessions_dir.clone())?;
    for (id, title, archived) in sessions {
        let mut saved = crate::session_manager::create_saved_session_with_id_and_mode(
            (*id).to_string(),
            &[
                crate::models::Message {
                    role: Role::User,
                    content: vec![crate::models::ContentBlock::Text {
                        text: format!("prompt for {title} with token=hunter2"),
                        cache_control: None,
                    }],
                },
                crate::models::Message {
                    role: Role::Assistant,
                    content: vec![crate::models::ContentBlock::Text {
                        text: "acknowledged".to_string(),
                        cache_control: None,
                    }],
                },
            ],
            "deepseek-chat",
            &workspace,
            12,
            None,
            Some("agent"),
        );
        saved.metadata.title = (*title).to_string();
        saved.metadata.archived = *archived;
        manager.save_session(&saved)?;
    }
    let Some((addr, _threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir.clone()).await?
    else {
        return Ok(None);
    };
    Ok(Some((addr, sessions_dir, handle)))
}

/// `/v1/sessions/summary` is routed, projects the shared row shape, and hides
/// archived rows until asked — the dashboard's whole session list depends on
/// all three being true at once.
#[tokio::test]
async fn session_summary_route_projects_rows_and_honours_archive_filters() -> Result<()> {
    let Some((addr, _dir, handle)) = spawn_server_with_saved_sessions(&[
        ("sess-active", "Active work", false),
        ("sess-putaway", "Put away", true),
    ])
    .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let active: Vec<serde_json::Value> = client
        .get(format!("http://{addr}/v1/sessions/summary"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        active
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["sess-active"],
        "archived sessions must not appear in the default listing"
    );
    // Field-compatible with /v1/threads/summary — the dashboard renders both
    // with one row renderer.
    for field in [
        "title",
        "preview",
        "model",
        "mode",
        "workspace",
        "updated_at",
    ] {
        assert!(!active[0][field].is_null(), "summary row missing {field}");
    }
    assert_eq!(active[0]["preview"], active[0]["title"]);

    let archived: Vec<serde_json::Value> = client
        .get(format!(
            "http://{addr}/v1/sessions/summary?archived_only=true"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        archived
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["sess-putaway"]
    );

    handle.abort();
    Ok(())
}

/// `PATCH /v1/sessions/{id}` renames and archives through the one writer, and
/// reports only what actually moved.
#[tokio::test]
async fn session_patch_route_renames_archives_and_reports_real_changes() -> Result<()> {
    let Some((addr, sessions_dir, handle)) =
        spawn_server_with_saved_sessions(&[("sess-patch", "Before", false)]).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let patched: serde_json::Value = client
        .patch(format!("http://{addr}/v1/sessions/sess-patch"))
        .json(&json!({ "title": "After", "archived": true }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(patched["session"]["title"], "After");
    assert_eq!(patched["session"]["archived"], true);
    assert_eq!(patched["changes"]["title"], "After");
    assert_eq!(patched["changes"]["archived"], true);

    // Durable, not just echoed back.
    let manager = crate::session_manager::SessionManager::new(sessions_dir)?;
    let reloaded = manager.load_session("sess-patch")?;
    assert_eq!(reloaded.metadata.title, "After");
    assert!(reloaded.metadata.archived);

    // A re-patch to the same state changes nothing, and says so.
    let repeat: serde_json::Value = client
        .patch(format!("http://{addr}/v1/sessions/sess-patch"))
        .json(&json!({ "archived": true }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        repeat["changes"]
            .as_object()
            .expect("changes object")
            .is_empty(),
        "a no-op patch must report no changes"
    );

    // An empty body is a client error, not a silent no-op.
    let empty = client
        .patch(format!("http://{addr}/v1/sessions/sess-patch"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    // A blank title is rejected with the reason, not accepted.
    let blank = client
        .patch(format!("http://{addr}/v1/sessions/sess-patch"))
        .json(&json!({ "title": "   " }))
        .send()
        .await?;
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

/// A session the TUI holds open is refused with a typed 409 rather than
/// written behind its back.
#[tokio::test]
async fn session_patch_route_refuses_a_live_session_with_a_conflict() -> Result<()> {
    // The live-session claim is process-global by construction (the embedded
    // API runs inside the TUI process), so this test must not run alongside
    // anything else that claims or clears it.
    let _lock = lock_test_env();
    let Some((addr, _dir, handle)) =
        spawn_server_with_saved_sessions(&[("sess-live", "Held open", false)]).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    crate::session_manager::set_live_session(Some("sess-live"));
    let conflict = client
        .patch(format!("http://{addr}/v1/sessions/sess-live"))
        .json(&json!({ "title": "Renamed from the dashboard" }))
        .send()
        .await?;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    crate::session_manager::set_live_session(None);
    let allowed = client
        .patch(format!("http://{addr}/v1/sessions/sess-live"))
        .json(&json!({ "title": "Renamed from the dashboard" }))
        .send()
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);

    handle.abort();
    Ok(())
}

/// `?peek=true` returns the bounded redacted projection, and the plain route
/// still returns the full detail shape.
#[tokio::test]
async fn session_detail_route_serves_a_bounded_redacted_peek_on_request() -> Result<()> {
    let Some((addr, _dir, handle)) =
        spawn_server_with_saved_sessions(&[("sess-peek", "Peekable", false)]).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let peek: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/sessions/sess-peek?peek=true&entries=12"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(peek["session_id"], "sess-peek");
    assert_eq!(peek["live"], false);
    assert!(peek["entries"].as_array().expect("entries").len() <= 12);
    // The seeded prompt carries `token=hunter2`; a peek must not re-emit it.
    let body = peek.to_string();
    assert!(
        !body.contains("hunter2"),
        "peek leaked a credential: {body}"
    );
    // No field a client could read as live turn state.
    for forbidden in ["status", "running", "active", "turn"] {
        assert!(peek.get(forbidden).is_none(), "peek exposed `{forbidden}`");
    }

    let detail: serde_json::Value = client
        .get(format!("http://{addr}/v1/sessions/sess-peek"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["metadata"]["id"], "sess-peek");
    assert!(
        detail["messages"].is_array(),
        "the plain route keeps returning full detail"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn session_delete_returns_404_for_missing_id() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .delete(format!("http://{addr}/v1/sessions/nonexistent-id"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    handle.abort();
    Ok(())
}

/// #561 / whalescale#255 — extra CORS origins from `RuntimeApiOptions`
/// are added on top of the built-in defaults and propagate through to the
/// `Access-Control-Allow-Origin` response header for preflight requests.
/// Built-in defaults must keep working unchanged.
#[tokio::test]
async fn cors_layer_appends_extra_origins_and_keeps_defaults() -> Result<()> {
    // The cors_layer fn is the layer factory — exercise it through a
    // Router with a single trivial route so we can issue OPTIONS preflights
    // and observe the response headers.
    let extra = vec!["http://localhost:5173".to_string()];
    let layer = cors_layer(&extra);
    let router: Router = Router::new()
        .route("/probe", get(|| async { "ok" }))
        .layer(layer);

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let client = crate::tls::reqwest_client();

    // The user-supplied origin is allowed.
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/probe"))
        .header("Origin", "http://localhost:5173")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await?;
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:5173")
    );

    // A built-in default origin still works.
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/probe"))
        .header("Origin", "http://localhost:1420")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await?;
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:1420")
    );

    // An origin that's neither configured nor a default is rejected
    // (CorsLayer omits the Allow-Origin header on mismatch).
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/probe"))
        .header("Origin", "http://malicious.example")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await?;
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "non-allowed origin must not be echoed back"
    );

    handle.abort();
    Ok(())
}

/// #561 — invalid origins (non-ASCII, etc.) are skipped without aborting
/// the layer build.
#[test]
fn cors_layer_skips_invalid_origins() {
    let extras = vec![
        "http://valid.example".to_string(),
        // Embedded NUL char makes `HeaderValue::from_str` fail.
        "http://invalid.example\0".to_string(),
        "  ".to_string(), // whitespace-only is dropped
    ];
    // Should not panic.
    let _ = cors_layer(&extras);
}

/// #562 / whalescale#256 — `PATCH /v1/threads/{id}` accepts the new
/// fields (allow_shell, trust_mode, auto_approve, model, mode, title,
/// system_prompt). Legacy mode aliases remain accepted as one-way inputs and
/// the response returns the canonical product mode. Each field is
/// independently optional; an empty string clears `title` / `system_prompt`
/// back to None.
#[tokio::test]
async fn patch_thread_accepts_extended_field_set() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "mode": "agent"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    // Patch every new field at once.
    let patched: serde_json::Value = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({
            "allow_shell": true,
            "trust_mode": true,
            "auto_approve": true,
            "model": "deepseek-v4-pro",
            "mode": "yolo",
            "title": "Whalescale UI test thread",
            "system_prompt": "You are a useful assistant."
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    assert_eq!(patched["allow_shell"], true);
    assert_eq!(patched["trust_mode"], true);
    assert_eq!(patched["auto_approve"], true);
    assert_eq!(patched["model"], "deepseek-v4-pro");
    assert_eq!(patched["mode"], "agent");
    assert_eq!(patched["permission_posture"], "full_access");
    assert_eq!(patched["title"], "Whalescale UI test thread");
    assert_eq!(patched["system_prompt"], "You are a useful assistant.");

    // Empty string clears title back to None.
    let cleared: serde_json::Value = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({ "title": "" }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        cleared["title"].is_null() || !cleared.as_object().unwrap().contains_key("title"),
        "empty title must serialize as None: {cleared:?}"
    );

    // Empty patch (no fields) is still rejected.
    let empty = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    // Empty model is rejected (validation).
    let bad_model = client
        .patch(format!("http://{addr}/v1/threads/{thread_id}"))
        .json(&json!({ "model": "  " }))
        .send()
        .await?;
    assert_eq!(bad_model.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

/// #563 / whalescale#260 — `archived_only=true` returns archived-only
/// (no active threads), distinct from `include_archived=true` which
/// returns both.
#[tokio::test]
async fn list_threads_archived_only_filter_matches_only_archived() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Two threads — keep one active, archive the other.
    let active: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let active_id = active["id"].as_str().unwrap().to_string();

    let archived: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let archived_id = archived["id"].as_str().unwrap().to_string();

    client
        .patch(format!("http://{addr}/v1/threads/{archived_id}"))
        .json(&json!({ "archived": true }))
        .send()
        .await?
        .error_for_status()?;

    // Default (active only) → only the unarchived one.
    let active_list: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ids: Vec<&str> = active_list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(ids.contains(&active_id.as_str()));
    assert!(!ids.contains(&archived_id.as_str()));

    // archived_only=true → only the archived one.
    let archived_list: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads?archived_only=true"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ids: Vec<&str> = archived_list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(ids, vec![archived_id.as_str()]);

    // archived_only=true takes precedence over include_archived=true.
    let archived_list: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads?include_archived=true&archived_only=true"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let ids: Vec<&str> = archived_list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(ids, vec![archived_id.as_str()]);

    // Same filter works on the summary endpoint.
    let summary: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads/summary?archived_only=true&limit=10"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let summary_ids: Vec<&str> = summary
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(summary_ids, vec![archived_id.as_str()]);

    handle.abort();
    Ok(())
}

/// #564 / whalescale#261 — `GET /v1/usage` aggregates per-turn token +
/// cost data. With no threads the response is well-formed and totals are
/// zero with empty buckets (never a 404).
#[tokio::test]
async fn usage_endpoint_returns_empty_aggregation_for_fresh_store() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let body: serde_json::Value = client
        .get(format!("http://{addr}/v1/usage"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(body["group_by"], "day");
    assert_eq!(body["totals"]["input_tokens"], 0);
    assert_eq!(body["totals"]["output_tokens"], 0);
    assert_eq!(body["totals"]["turns"], 0);
    assert!(
        body["buckets"].as_array().unwrap().is_empty(),
        "buckets must be empty when no turns exist: {body}"
    );

    // group_by query options are validated.
    let bad_group = client
        .get(format!("http://{addr}/v1/usage?group_by=galaxy"))
        .send()
        .await?;
    assert_eq!(bad_group.status(), StatusCode::BAD_REQUEST);

    // Each accepted group_by value succeeds.
    for gb in ["day", "model", "provider", "thread"] {
        let resp = client
            .get(format!("http://{addr}/v1/usage?group_by={gb}"))
            .send()
            .await?;
        assert!(resp.status().is_success(), "group_by={gb} failed: {resp:?}");
    }

    // Bad ISO-8601 timestamp rejected.
    let bad_since = client
        .get(format!("http://{addr}/v1/usage?since=not-a-date"))
        .send()
        .await?;
    assert_eq!(bad_since.status(), StatusCode::BAD_REQUEST);

    // since > until rejected.
    let inverted = client
        .get(format!(
            "http://{addr}/v1/usage?since=2030-01-02T00:00:00Z&until=2030-01-01T00:00:00Z"
        ))
        .send()
        .await?;
    assert_eq!(inverted.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

/// `GET /v1/threads/{id}/usage` reports the thread-scoped token + cost
/// totals the GUI's session-cost surface consumes. A thread with no turns
/// yields zeroed totals (both published currencies present in the payload);
/// a thread that does not exist is a 404, never zeros, so the GUI cannot
/// mistake a typo'd id for a free conversation.
#[tokio::test]
async fn thread_usage_endpoint_scopes_totals_to_one_thread() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"].as_str().expect("thread id").to_string();

    let usage: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{id}/usage"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(usage["thread_id"], id);
    assert_eq!(usage["totals"]["input_tokens"], 0);
    assert_eq!(usage["totals"]["output_tokens"], 0);
    assert_eq!(usage["totals"]["cost_usd"], 0.0);
    assert_eq!(usage["totals"]["cost_cny"], 0.0);
    assert_eq!(usage["totals"]["turns"], 0);
    assert_eq!(usage["totals"]["priced_turns"], 0);
    assert_eq!(usage["totals"]["cny_priced_turns"], 0);
    assert_eq!(usage["totals"]["cny_unpriced_turns"], 0);
    assert_eq!(usage["totals"]["cny_unpriced_reasons"], json!([]));
    assert_eq!(usage["totals"]["cost_complete"], true);

    let missing = client
        .get(format!("http://{addr}/v1/threads/does-not-exist/usage"))
        .send()
        .await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

/// `PUT /v1/sessions` persists the thread's audited cost with the same
/// field semantics the TUI writer uses: parent-turn spend in
/// `session_cost_*`, routed-child spend in `subagent_cost_*`, so a session
/// that already carries TUI-written sub-agent spend keeps one display total
/// instead of counting child spend twice. Coverage rides along (#4318):
/// `coverage_recorded` is set and the CNY counters describe the provider's
/// own CNY rows, so a reload reads the totals as known, not legacy-unknown.
#[tokio::test]
async fn session_save_merges_thread_cost_split_and_records_coverage() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-session-cost-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "workspace": root.join("workspace")
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    // Seed one completed turn: a DeepSeek first-party parent call plus a
    // large routed-child (sub-agent) call, both priced.
    let store = runtime_threads.test_store();
    let now = Utc::now();
    let mut turn = TurnRecord {
        schema_version: 2,
        id: "turn_cost_merge".to_string(),
        thread_id: thread_id.clone(),
        status: RuntimeTurnStatus::Completed,
        input_summary: "cost merge".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: Some(0),
        usage: Some(Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Usage::default()
        }),
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: vec![crate::cost_status::EffectiveRouteUsage {
            route: crate::cost_status::EffectiveRouteEnvelope {
                provider: ApiProvider::Deepseek,
                provider_identity: ApiProvider::Deepseek.as_str().to_string(),
                model: "deepseek-v4-flash".to_string(),
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: now,
            },
            usage: Usage {
                input_tokens: 20_000_000,
                output_tokens: 2_000_000,
                ..Usage::default()
            },
        }],
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: Vec::new(),
        steer_count: 0,
        agent_mail_message_id: None,
    };
    // Mirror `TurnRecord::persist_effective_route` (private to the runtime
    // threads module): persist the route envelope onto the turn so the
    // recorded-time pricer sees a complete dispatch record.
    turn.effective_provider = Some(ApiProvider::Deepseek.as_str().to_string());
    turn.effective_provider_id = Some(ApiProvider::Deepseek.as_str().to_string());
    turn.effective_billing_surface =
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string());
    turn.effective_endpoint_fingerprint = None;
    turn.effective_billing_mode = Some(crate::cost_status::RouteBillingMode::Metered);
    turn.effective_dispatched_at = Some(now);
    turn.effective_model = Some("deepseek-v4-flash".to_string());
    store.save_turn(&turn)?;
    let mut thread = store.load_thread(&thread_id)?;
    thread.latest_turn_id = Some(turn.id.clone());
    store.save_thread(&thread)?;

    let endpoint: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{thread_id}/usage"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let combined_usd = endpoint["totals"]["cost_usd"]
        .as_f64()
        .context("combined cost_usd")?;
    assert!(combined_usd > 0.0);

    // Pre-existing session file carrying TUI-written sub-agent spend — the
    // shape a GUI save of a resumed TUI session merges into.
    let session_manager = crate::session_manager::SessionManager::new(sessions_dir.clone())?;
    let mut prior = crate::session_manager::create_saved_session_with_id_and_mode(
        "sess_cost_merge".to_string(),
        &[],
        "deepseek-v4-flash",
        &root,
        0,
        None,
        None,
    );
    prior.metadata.cost.subagent_cost_usd = 0.5;
    prior.metadata.cost.subagent_cost_cny = 3.0;
    session_manager.save_session(&prior)?;

    client
        .put(format!("http://{addr}/v1/sessions"))
        .json(&json!({
            "thread_id": thread_id,
            "session_id": "sess_cost_merge"
        }))
        .send()
        .await?
        .error_for_status()?;

    let saved = session_manager.load_session_by_prefix("sess_cost_merge")?;
    let cost = &saved.metadata.cost;
    // Parent-turn spend lands in the parent field only, so the display
    // total keeps exactly one copy of the child spend.
    assert!(cost.session_cost_usd > 0.0);
    assert!(cost.session_cost_usd < combined_usd);
    let child_usd = combined_usd - cost.session_cost_usd;
    assert!(child_usd > 0.5);
    assert_eq!(cost.subagent_cost_usd, child_usd);
    assert_eq!(
        (cost.session_cost_usd + cost.subagent_cost_usd - combined_usd).abs(),
        0.0
    );
    assert!(cost.subagent_cost_cny > 3.0);
    assert!(cost.session_cost_cny > 0.0);
    // Coverage the runtime computed reads back as known, with CNY counted
    // under the provider's own CNY row.
    assert!(cost.coverage_recorded);
    assert!(!cost.coverage_is_legacy_unknown());
    assert_eq!(cost.priced_turns, 1);
    assert_eq!(cost.unpriced_turns, 0);
    assert_eq!(cost.cny_priced_turns, 1);
    assert_eq!(cost.cny_unpriced_turns, 0);
    assert!(cost.unpriced_reasons.is_empty());
    assert!(
        cost.cny_unpriced_reasons.is_empty(),
        "DeepSeek first-party parent is CNY-priced; reasons must not invent a gap"
    );

    // Re-saving is idempotent: max-merge never re-adds the same spend.
    client
        .put(format!("http://{addr}/v1/sessions"))
        .json(&json!({
            "thread_id": thread_id,
            "session_id": "sess_cost_merge"
        }))
        .send()
        .await?
        .error_for_status()?;
    let resaved = session_manager.load_session_by_prefix("sess_cost_merge")?;
    assert_eq!(
        resaved.metadata.cost.session_cost_usd,
        cost.session_cost_usd
    );
    assert_eq!(
        resaved.metadata.cost.subagent_cost_usd,
        cost.subagent_cost_usd
    );

    handle.abort();
    Ok(())
}

/// USD-unpriced parent + DeepSeek-priced child: coverage reasons persist
/// with the parent columns (#4318) while routed spend still lands only in
/// `subagent_cost_*` (no double-count on reload).
#[tokio::test]
async fn session_save_persists_parent_cny_unpriced_reasons_without_double_count() -> Result<()> {
    let root = std::env::temp_dir().join(format!("deepseek-session-cny-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "gpt-5.5",
            "workspace": root.join("workspace")
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"]
        .as_str()
        .context("missing thread id")?
        .to_string();

    let store = runtime_threads.test_store();
    let now = Utc::now();
    let mut turn = TurnRecord {
        schema_version: 2,
        id: "turn_cny_reasons".to_string(),
        thread_id: thread_id.clone(),
        status: RuntimeTurnStatus::Completed,
        input_summary: "cny reasons".to_string(),
        created_at: now,
        started_at: Some(now),
        ended_at: Some(now),
        duration_ms: Some(0),
        usage: Some(Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Usage::default()
        }),
        permission_posture: None,
        effective_provider: None,
        effective_provider_id: None,
        effective_billing_surface: None,
        effective_endpoint_fingerprint: None,
        effective_billing_mode: None,
        effective_dispatched_at: None,
        effective_model: None,
        routed_usage: vec![crate::cost_status::EffectiveRouteUsage {
            route: crate::cost_status::EffectiveRouteEnvelope {
                provider: ApiProvider::Deepseek,
                provider_identity: ApiProvider::Deepseek.as_str().to_string(),
                model: "deepseek-v4-flash".to_string(),
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: now,
            },
            usage: Usage {
                input_tokens: 20_000,
                output_tokens: 2_000,
                ..Usage::default()
            },
        }],
        routed_usage_source_ids: Vec::new(),
        routed_usage_dropped_records: 0,
        error: None,
        item_ids: Vec::new(),
        steer_count: 0,
        agent_mail_message_id: None,
    };
    turn.effective_provider = Some(ApiProvider::Openai.as_str().to_string());
    turn.effective_provider_id = Some(ApiProvider::Openai.as_str().to_string());
    turn.effective_billing_surface = Some(crate::pricing::UNCLASSIFIED_BILLING_SURFACE.to_string());
    turn.effective_endpoint_fingerprint = None;
    turn.effective_billing_mode = Some(crate::cost_status::RouteBillingMode::Metered);
    turn.effective_dispatched_at = Some(now);
    turn.effective_model = Some("gpt-5.5".to_string());
    store.save_turn(&turn)?;
    let mut thread = store.load_thread(&thread_id)?;
    thread.latest_turn_id = Some(turn.id.clone());
    store.save_thread(&thread)?;

    client
        .put(format!("http://{addr}/v1/sessions"))
        .json(&json!({
            "thread_id": thread_id,
            "session_id": "sess_cny_reasons"
        }))
        .send()
        .await?
        .error_for_status()?;

    let session_manager = crate::session_manager::SessionManager::new(sessions_dir)?;
    let saved = session_manager.load_session_by_prefix("sess_cny_reasons")?;
    let cost = &saved.metadata.cost;
    assert_eq!(
        cost.session_cost_usd, 0.0,
        "unpriced parent must not invent USD"
    );
    assert!(
        cost.subagent_cost_usd > 0.0,
        "routed DeepSeek child is priced"
    );
    assert_eq!(
        cost.priced_turns, 0,
        "parent coverage columns stay parent-only"
    );
    assert_eq!(cost.unpriced_turns, 1);
    assert_eq!(cost.cny_unpriced_turns, 1);
    assert!(!cost.unpriced_reasons.is_empty());
    assert!(
        !cost.cny_unpriced_reasons.is_empty(),
        "CNY reasons must persist with the parent coverage counts (#4318)"
    );
    assert!(cost.coverage_recorded);
    assert!(!cost.coverage_is_legacy_unknown());

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn runtime_info_reports_bind_state() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(info["service"], "codewhale-runtime-api");
    assert_eq!(info["runtime_api_version"], "1.0");
    assert_eq!(info["codewhale_version"], info["version"]);
    let commit = info["codewhale_commit"]
        .as_str()
        .expect("runtime build commit must be a string");
    // Since #5245 the commit is env-stamped only: a stamped build (CI /
    // release / a `CODEWHALE_BUILD_SHA=…` dogfood build) reports a full 40-hex
    // sha; an unstamped local build honestly reports "unknown" rather than
    // reading the checkout. Both are valid provenance — a fabricated sha
    // would be the bug.
    if commit == "unknown" {
        // Unstamped local build — the honest absence.
    } else {
        assert_eq!(commit.len(), 40, "a stamped build commit is a full sha");
        assert!(
            commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "runtime build commit must be hexadecimal"
        );
    }
    assert_eq!(info["bind_host"], "127.0.0.1");
    assert_eq!(info["auth_required"], false);
    assert!(info["version"].is_string());
    assert_eq!(info["transports"], json!(["http", "sse"]));
    assert_eq!(info["capabilities"]["threads"], true);
    assert_eq!(
        info["capabilities"]["turn_operation_idempotency"], true,
        "clients must have an explicit fail-closed gate before sending operation_key"
    );
    assert_eq!(info["capabilities"]["account_session"], true);
    assert_eq!(info["capabilities"]["external_tools"], true);
    assert_eq!(info["capabilities"]["worker_runtime"], true);
    assert_eq!(info["account"]["schema_version"], 1);
    assert_eq!(info["account"]["state"], "signed_out");
    assert_eq!(info["account"]["api_base"], "https://api.codewhale.net");
    assert_eq!(info["account"]["scopes"], json!([]));
    assert!(info["account"].get("access_token").is_none());
    assert!(info["account"].get("refresh_token").is_none());
    assert!(info["account"].get("email").is_none());
    assert!(info["experimental"].is_object());

    handle.abort();
    Ok(())
}

#[test]
fn unauthenticated_runtime_info_redacts_secure_account_identity_without_loading_it() {
    use codewhale_secrets::account::{AccountSessionState, RuntimeAccountInfo};

    let loaded = std::cell::Cell::new(false);
    let info = runtime_account_info_for_request(false, "https://api.codewhale.net", || {
        loaded.set(true);
        RuntimeAccountInfo {
            schema_version: 1,
            state: AccountSessionState::Authenticated,
            api_base: "https://api.codewhale.net".to_string(),
            account_id: Some("acct-private".to_string()),
            session_id: Some("session-private".to_string()),
            scopes: vec!["identity:read".to_string()],
            expires_at: Some("2030-01-01T00:00:00Z".to_string()),
        }
    });
    assert!(
        !loaded.get(),
        "unauthenticated probes must not read secure storage"
    );
    assert_eq!(info.state, AccountSessionState::SignedOut);
    let json = serde_json::to_string(&info).unwrap();
    for private in ["acct-private", "session-private", "identity:read"] {
        assert!(!json.contains(private));
    }
}

#[test]
fn runtime_account_api_origin_rejects_credentials_paths_and_non_loopback_http() {
    assert_eq!(
        normalize_runtime_account_api_base("https://api.codewhale.net/"),
        Some("https://api.codewhale.net".to_string())
    );
    assert_eq!(
        normalize_runtime_account_api_base("http://127.0.0.1:8787"),
        Some("http://127.0.0.1:8787".to_string())
    );
    for invalid in [
        "https://user:secret@example.test",
        "https://example.test/account",
        "https://example.test?token=secret",
        "http://api.codewhale.net",
    ] {
        assert_eq!(
            normalize_runtime_account_api_base(invalid),
            None,
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn create_thread_accepts_dynamic_tools_and_environments() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "test-model",
            "dynamic_tools": [
                {
                    "namespace": "tau_bench",
                    "name": "get_reservation",
                    "description": "Look up a reservation.",
                    "input_schema": { "type": "object" }
                }
            ],
            "environments": [
                { "environment_id": "local", "cwd": "/workspace" }
            ]
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(created["id"].is_string());

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn create_thread_normalizes_and_persists_named_permission_posture() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "test-model",
            "mode": "operate",
            "permission_posture": "auto-review"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(created["mode"], "operate");
    assert_eq!(created["permission_posture"], "auto_review");
    assert_eq!(created["auto_approve"], false);
    assert_eq!(created["trust_mode"], false);

    let authoritative_ask: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "test-model",
            "mode": "yolo",
            "permission_posture": "ask",
            "auto_approve": true
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(authoritative_ask["mode"], "agent");
    assert_eq!(authoritative_ask["permission_posture"], "ask");
    assert_eq!(authoritative_ask["auto_approve"], false);

    let invalid = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model": "test-model",
            "permission_posture": "owner"
        }))
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn start_turn_accepts_dynamic_tools_and_environment_id() -> Result<()> {
    Box::pin(async {
        let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
            return Ok(());
        };
        let client = crate::tls::reqwest_client();

        let created: serde_json::Value = client
            .post(format!("http://{addr}/v1/threads"))
            .json(&json!({ "model": "test-model" }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let thread_id = created["id"].as_str().context("missing thread id")?;

        let started: serde_json::Value = client
            .post(format!("http://{addr}/v1/threads/{thread_id}/turns"))
            .json(&json!({
                "prompt": "hello",
                "dynamic_tools": [
                    {
                        "name": "simple_tool",
                        "description": "A simple tool.",
                        "input_schema": { "type": "object" }
                    }
                ],
                "environment_id": "local",
                "permission_posture": "auto-review"
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(started["turn"]["thread_id"], thread_id);
        assert_eq!(started["thread"]["permission_posture"], "ask");
        assert_eq!(started["turn"]["permission_posture"], "auto_review");

        let stored: serde_json::Value = client
            .get(format!("http://{addr}/v1/threads/{thread_id}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(stored["turns"][0]["permission_posture"], "auto_review");

        handle.abort();
        Ok(())
    })
    .await
}

#[tokio::test]
async fn mobile_runtime_router_starts_without_duplicate_method_routes() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    let Some((_addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_and_mobile(root, sessions_dir, None, true).await?
    else {
        return Ok(());
    };

    // Axum panics during `build_router` when a method/path pair is registered
    // twice, so reaching the running server is the regression assertion.
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mobile_page_is_available_only_when_enabled() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    let Some((addr, runtime_threads, handle)) = spawn_test_server_with_root_token_and_mobile(
        root.clone(),
        sessions_dir.clone(),
        None,
        false,
    )
    .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let disabled = client.get(format!("http://{addr}/mobile")).send().await?;
    assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
    handle.abort();
    let _ = handle.await;
    drop(runtime_threads);

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_and_mobile(root, sessions_dir, None, true).await?
    else {
        return Ok(());
    };
    let enabled = client
        .get(format!("http://{addr}/mobile"))
        .send()
        .await?
        .error_for_status()?;
    let html = enabled.text().await?;
    assert!(html.contains("Codewhale Mobile"));
    assert!(html.contains("/v1/approvals/"));
    assert!(html.contains("MAX_VISIBLE_EVENTS = 100"));
    assert!(html.contains("replay_limit:"));
    assert!(html.contains("X-Codewhale-Mobile-Request"));
    assert!(!html.contains("localStorage.getItem"));
    assert!(!html.contains("localStorage.setItem"));
    assert!(!html.contains("setRuntimeTokenCookie"));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mobile_page_serves_shell_when_auth_enabled() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    let token = "abc ABC+/?:=&%".to_string();
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_and_mobile(root, sessions_dir, Some(token.clone()), true)
            .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let shell = client
        .get(format!("http://{addr}/mobile"))
        .send()
        .await?
        .error_for_status()?;
    let html = shell.text().await?;
    assert!(html.contains("Codewhale Mobile"));
    assert!(html.contains("codewhale_mobile_session_proof"));
    assert!(html.contains("/__codewhale/mobile/session"));
    assert!(!html.contains("codewhale_runtime_token="));
    assert!(!html.contains("localStorage.setItem"));

    let bearer = client
        .get(format!("http://{addr}/mobile"))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?;
    assert!(bearer.text().await?.contains("Codewhale Mobile"));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mobile_bootstrap_uses_opaque_origin_bound_session_and_single_use_stream_ticket()
-> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    let workspace = root.join("workspace");
    let token = "mobile-runtime-test-token".to_string();
    let (mobile_state, nonce) = mobile::RuntimeMobileState::new();
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root,
            sessions_dir,
            Some(token.clone()),
            true,
            workspace,
            TestServerOverrides {
                mobile: Some(mobile_state),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let origin = format!("http://{addr}");

    let shell = client.get(format!("{origin}/mobile")).send().await?;
    assert_eq!(shell.status(), StatusCode::OK);
    assert_eq!(
        shell
            .headers()
            .get(header::REFERRER_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some("no-referrer")
    );
    let shell = shell.text().await?;
    assert!(!shell.contains(&token));
    assert!(!shell.contains("localStorage.setItem"));
    assert!(!shell.contains("localStorage.getItem"));
    assert!(!shell.contains("codewhale_runtime_token="));

    let unauthenticated_session = client
        .post(format!("{origin}/__codewhale/mobile/session"))
        .send()
        .await?;
    assert_eq!(unauthenticated_session.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthenticated_session
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let manual_session = client
        .post(format!("{origin}/__codewhale/mobile/session"))
        .bearer_auth(&token)
        .send()
        .await?;
    assert_eq!(manual_session.status(), StatusCode::OK);
    let manual_cookie = manual_session
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .context("manual mobile session Set-Cookie")?
        .to_string();
    assert!(manual_cookie.starts_with("codewhale_mobile_session=cwms_"));
    assert!(manual_cookie.ends_with("; Max-Age=1800; HttpOnly; SameSite=Strict; Path=/"));
    assert!(!manual_cookie.contains(&token));
    let manual_body = manual_session.text().await?;
    assert!(!manual_body.contains(&token));

    let bootstrap = client
        .get(format!("{origin}/__codewhale/mobile/bootstrap/{nonce}"))
        .send()
        .await?;
    assert_eq!(bootstrap.status(), StatusCode::SEE_OTHER);
    let bootstrap_cookie = bootstrap
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .context("bootstrap mobile session Set-Cookie")?
        .to_string();
    assert!(bootstrap_cookie.starts_with("codewhale_mobile_session=cwms_"));
    assert!(!bootstrap_cookie.contains(&token));
    let location = bootstrap
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .context("bootstrap mobile redirect")?;
    assert!(location.starts_with("/mobile#request_proof=cwmr_"));
    assert!(location.contains("&stream_ticket=cwmt_"));
    assert!(!location.contains(&token));

    let bootstrap_cookie_pair = bootstrap_cookie
        .split(';')
        .next()
        .context("bootstrap mobile cookie pair")?
        .to_string();
    let fragment = location
        .strip_prefix("/mobile#")
        .context("bootstrap redirect must use a fragment")?;
    let request_proof = fragment
        .split('&')
        .find_map(|part| part.strip_prefix("request_proof="))
        .context("bootstrap request proof")?
        .to_string();
    let stream_ticket = fragment
        .split('&')
        .find_map(|part| part.strip_prefix("stream_ticket="))
        .context("bootstrap stream ticket")?
        .to_string();

    let cookie_only = client
        .get(format!("{origin}/v1/threads/summary"))
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .send()
        .await?;
    assert_eq!(cookie_only.status(), StatusCode::UNAUTHORIZED);

    let sibling_port_replay = client
        .get(format!("{origin}/v1/threads/summary"))
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(mobile::MOBILE_REQUEST_HEADER, &request_proof)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-site")
        .send()
        .await?;
    assert_eq!(sibling_port_replay.status(), StatusCode::UNAUTHORIZED);

    let same_origin = client
        .get(format!("{origin}/v1/threads/summary"))
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(mobile::MOBILE_REQUEST_HEADER, &request_proof)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?;
    assert_eq!(same_origin.status(), StatusCode::OK);

    let created: serde_json::Value = client
        .post(format!("{origin}/v1/threads"))
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(mobile::MOBILE_REQUEST_HEADER, &request_proof)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = created["id"].as_str().context("mobile-created thread id")?;

    let stream_url =
        format!("{origin}/v1/threads/{thread_id}/events?mobile_stream_ticket={stream_ticket}");
    let stream = client
        .get(&stream_url)
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?;
    assert_eq!(stream.status(), StatusCode::OK);
    drop(stream);

    let replayed_stream = client
        .get(&stream_url)
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?;
    assert_eq!(replayed_stream.status(), StatusCode::UNAUTHORIZED);

    let refreshed: serde_json::Value = client
        .post(format!("{origin}/__codewhale/mobile/stream-ticket"))
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(mobile::MOBILE_REQUEST_HEADER, &request_proof)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let refreshed_ticket = refreshed["stream_ticket"]
        .as_str()
        .context("refreshed stream ticket")?;
    let refreshed_stream_url =
        format!("{origin}/v1/threads/{thread_id}/events?mobile_stream_ticket={refreshed_ticket}");
    let sibling_stream_replay = client
        .get(&refreshed_stream_url)
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-site")
        .send()
        .await?;
    assert_eq!(sibling_stream_replay.status(), StatusCode::UNAUTHORIZED);

    let same_origin_stream = client
        .get(&refreshed_stream_url)
        .header(header::COOKIE, &bootstrap_cookie_pair)
        .header(header::ORIGIN, &origin)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?;
    assert_eq!(same_origin_stream.status(), StatusCode::OK);
    drop(same_origin_stream);

    let reused_bootstrap = client
        .get(format!("{origin}/__codewhale/mobile/bootstrap/{nonce}"))
        .send()
        .await?;
    assert_eq!(reused_bootstrap.status(), StatusCode::UNAUTHORIZED);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mobile_insecure_mode_allows_page_and_v1_routes_without_token() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_and_mobile(root, sessions_dir, None, true).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let page = client
        .get(format!("http://{addr}/mobile"))
        .send()
        .await?
        .error_for_status()?;
    assert!(page.text().await?.contains("Codewhale Mobile"));

    let summary = client
        .get(format!("http://{addr}/v1/threads/summary"))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(summary.status(), StatusCode::OK);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn thread_summary_projects_typed_pending_attention_count() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let attention_thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let attention_id = attention_thread["id"]
        .as_str()
        .context("missing attention thread id")?;
    let _approval_rx = runtime_threads
        .register_pending_approval_for_thread_for_test(attention_id, "approval-summary-attention");
    runtime_threads
        .register_pending_user_input_for_thread_for_test(attention_id, "input-summary-attention");

    let recent_thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let recent_id = recent_thread["id"]
        .as_str()
        .context("missing recent thread id")?;

    let summaries: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/summary?limit=100"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let rows = summaries.as_array().context("summary should be an array")?;
    let attention = rows
        .iter()
        .find(|row| row["id"] == attention_id)
        .context("attention thread summary")?;
    let recent = rows
        .iter()
        .find(|row| row["id"] == recent_id)
        .context("recent thread summary")?;
    assert_eq!(attention["pending_attention_count"], 2);
    assert_eq!(recent["pending_attention_count"], 0);

    handle.abort();
    Ok(())
}

/// `GET /v1/threads/summary?search=` bounded the *store read* by `limit`
/// before matching, so a thread older than the newest `limit` rows could not
/// be found by searching for it — the embedded dashboard's search box asks
/// for `limit=100`, which silently made every older thread unsearchable.
/// `limit` bounds the returned rows, not how far the search looks.
#[tokio::test]
async fn thread_summary_search_finds_a_match_older_than_the_row_limit() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // `POST /v1/threads` has no title field; the title is a PATCH. Patching
    // also re-stamps `updated_at`, so ordering is not left to two creates
    // landing inside the same clock tick.
    let create = |title: &str| {
        let client = client.clone();
        let title = title.to_string();
        async move {
            let thread: serde_json::Value = client
                .post(format!("http://{addr}/v1/threads"))
                .json(&json!({}))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let id = thread["id"]
                .as_str()
                .context("missing thread id")?
                .to_string();
            client
                .patch(format!("http://{addr}/v1/threads/{id}"))
                .json(&json!({ "title": title }))
                .send()
                .await?
                .error_for_status()?;
            anyhow::Ok(id)
        }
    };

    // Oldest thread carries the needle; three newer threads crowd it out of
    // any `limit`-sized window.
    let needle_id = create("zebracrossing handoff").await?;
    for title in ["decoy one", "decoy two", "decoy three"] {
        create(title).await?;
    }

    let summaries: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads/summary?limit=2&search=zebracrossing"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let rows = summaries.as_array().context("summary should be an array")?;
    assert_eq!(
        rows.len(),
        1,
        "search must reach past the row limit; got {summaries}"
    );
    assert_eq!(rows[0]["id"], needle_id);

    // `limit` still bounds the returned rows.
    let bounded: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/summary?limit=2"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        bounded
            .as_array()
            .context("summary should be an array")?
            .len(),
        2
    );

    handle.abort();
    Ok(())
}

/// `GET /v1/threads/summary?search=` used to call `get_thread_detail` for every
/// thread *before* matching. Detail walks the entire turns directory and the
/// entire items directory, so a non-matching dashboard keystroke was
/// O(threads × (all_turns + all_items)) JSON reads. Matching on the thread
/// record first (and peeking one latest-turn file only when the title is
/// unset) must not scale that way: adding items must not multiply reads by
/// thread count.
#[tokio::test]
async fn thread_summary_search_does_not_scan_the_whole_store_per_thread() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    const THREADS: usize = 8;
    const TURNS_PER_THREAD: usize = 4;
    const ITEMS_PER_TURN: usize = 4;
    const PREVIEW_TOKEN: &str = "previewonlyneedlenowhereontherecord";
    const TITLE_NEEDLE: &str = "zebracrossing-summary-search";

    let mut thread_ids = Vec::with_capacity(THREADS);
    for index in 0..THREADS {
        let thread: serde_json::Value = client
            .post(format!("http://{addr}/v1/threads"))
            .json(&json!({}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let id = thread["id"]
            .as_str()
            .context("missing thread id")?
            .to_string();
        let title = if index == 0 {
            TITLE_NEEDLE
        } else {
            "decoy summary search title"
        };
        client
            .patch(format!("http://{addr}/v1/threads/{id}"))
            .json(&json!({ "title": title }))
            .send()
            .await?
            .error_for_status()?;
        seed_summary_search_transcript(
            runtime_threads.test_store(),
            &id,
            index,
            TURNS_PER_THREAD,
            ITEMS_PER_TURN,
            PREVIEW_TOKEN,
        )?;
        thread_ids.push(id);
    }

    let total_turns = (THREADS * TURNS_PER_THREAD) as u64;
    let total_items = (THREADS * TURNS_PER_THREAD * ITEMS_PER_TURN) as u64;
    let quadratic_file_reads = (THREADS as u64) * (total_turns + total_items);

    runtime_threads.reset_whole_store_scan_file_reads();
    let missed: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads/summary?limit=2&search={PREVIEW_TOKEN}"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let (turn_files, item_files) = runtime_threads.whole_store_scan_file_reads();
    let scan_reads = turn_files + item_files;
    assert!(
        scan_reads.saturating_mul(2) < quadratic_file_reads,
        "non-matching search read {turn_files} turn files and {item_files} item files \
         across {THREADS} threads ({total_turns} turns, {total_items} items); \
         a per-thread get_thread_detail would have been ~{quadratic_file_reads} \
         whole-store file reads"
    );
    assert!(
        missed
            .as_array()
            .context("summary should be an array")?
            .is_empty(),
        "preview text is display-only and must not be a search key; got {missed}"
    );

    runtime_threads.reset_whole_store_scan_file_reads();
    let found: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/threads/summary?limit=2&search={TITLE_NEEDLE}"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let rows = found.as_array().context("summary should be an array")?;
    assert_eq!(
        rows.len(),
        1,
        "title search must still find the needle; got {found}"
    );
    assert_eq!(rows[0]["id"], thread_ids[0]);
    assert!(
        rows[0]["preview"]
            .as_str()
            .is_some_and(|preview| preview.contains(PREVIEW_TOKEN)),
        "matching rows still load detail so the preview is filled; got {found}"
    );

    handle.abort();
    Ok(())
}

fn seed_summary_search_transcript(
    store: &crate::runtime_threads::RuntimeThreadStore,
    thread_id: &str,
    thread_index: usize,
    turns: usize,
    items_per_turn: usize,
    preview_token: &str,
) -> Result<()> {
    let mut thread = store.load_thread(thread_id)?;
    let base = Utc::now();
    let mut latest_turn_id = None;
    for turn_offset in 0..turns {
        let created_at = base + chrono::Duration::milliseconds(turn_offset as i64);
        let turn_id = format!("turn_sum_{thread_index}_{turn_offset}");
        let mut item_ids = Vec::with_capacity(items_per_turn);
        for item_offset in 0..items_per_turn {
            let item_id = format!("item_sum_{thread_index}_{turn_offset}_{item_offset}");
            let kind = if item_offset == 0 {
                TurnItemKind::UserMessage
            } else {
                TurnItemKind::AgentMessage
            };
            let text = format!("{preview_token} {thread_index} {turn_offset} {item_offset}");
            store.save_item(&crate::runtime_threads::TurnItemRecord {
                schema_version: 2,
                id: item_id.clone(),
                turn_id: turn_id.clone(),
                kind,
                status: TurnItemLifecycleStatus::Completed,
                summary: text.clone(),
                detail: Some(text),
                metadata: None,
                artifact_refs: Vec::new(),
                started_at: Some(created_at),
                ended_at: Some(created_at),
            })?;
            item_ids.push(item_id);
        }
        store.save_turn(&TurnRecord {
            schema_version: 2,
            id: turn_id.clone(),
            thread_id: thread_id.to_string(),
            status: RuntimeTurnStatus::Completed,
            input_summary: format!("decoy prompt {thread_index} {turn_offset}"),
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
            item_ids,
            steer_count: 0,
            agent_mail_message_id: None,
        })?;
        latest_turn_id = Some(turn_id);
    }
    thread.latest_turn_id = latest_turn_id;
    store.save_thread(&thread)?;
    Ok(())
}

#[tokio::test]
async fn decide_approval_404s_when_nothing_pending() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(format!("http://{addr}/v1/approvals/no_such_id"))
        .json(&json!({ "decision": "allow" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn submit_user_input_404s_without_entering_engine_mailbox_for_unknown_id() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let thread = runtime_threads
        .create_thread(CreateThreadRequest::default())
        .await?;
    let mut harness = crate::core::engine::mock_engine_handle();
    runtime_threads
        .install_test_engine(&thread.id, harness.handle.clone())
        .await?;

    let response = crate::tls::reqwest_client()
        .post(format!(
            "http://{addr}/v1/user-input/{}/input-missing",
            thread.id
        ))
        .json(&json!({
            "answers": [{
                "id": "choice",
                "label": "Missing",
                "value": "must-not-enter-engine-mailbox",
            }],
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            harness.recv_user_input_submission()
        )
        .await
        .is_err(),
        "unknown user input reached the engine mailbox"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn events_endpoint_rejects_unbounded_tail_requests() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let thread = runtime_threads
        .create_thread(CreateThreadRequest::default())
        .await?;
    let response = crate::tls::reqwest_client()
        .get(format!(
            "http://{addr}/v1/threads/{}/events?replay_limit={}",
            thread.id,
            MAX_RUNTIME_EVENT_REPLAY_TAIL.saturating_add(1),
        ))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn decide_approval_400s_on_bad_decision() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(format!("http://{addr}/v1/approvals/whatever"))
        .json(&json!({ "decision": "yolo" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn decide_approval_delivers_to_runtime() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let rx = runtime_threads.register_pending_approval_for_test("ext_id");

    let resp = client
        .post(format!("http://{addr}/v1/approvals/ext_id"))
        .json(&json!({ "decision": "allow", "remember": false }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await?;
    assert_eq!(body["ok"], true);
    assert_eq!(body["decision"], "allow");
    assert_eq!(body["delivered"], true);

    let received = tokio::time::timeout(ci_scaled(Duration::from_secs(1)), rx).await??;
    assert_eq!(
        received,
        ExternalApprovalDecision::Allow { remember: false }
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn dynamic_tool_result_endpoint_delivers_to_runtime() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = thread["id"].as_str().context("thread id")?;
    let rx =
        runtime_threads.register_pending_dynamic_tool_for_test(thread_id, "turn_1", "call_1")?;

    let wrong_turn = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/turn_wrong/tool-calls/call_1/result"
        ))
        .json(&json!({ "success": false }))
        .send()
        .await?;
    assert_eq!(wrong_turn.status(), StatusCode::NOT_FOUND);

    let resp = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/turn_1/tool-calls/call_1/result"
        ))
        .json(&json!({
            "success": true,
            "content": [{ "type": "input_text", "text": "ok" }]
        }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let received = tokio::time::timeout(ci_scaled(Duration::from_secs(1)), rx).await??;
    assert!(received.success);
    assert_eq!(received.content.len(), 1);
    let resolved = runtime_threads
        .events_since(thread_id, None)?
        .into_iter()
        .filter(|event| event.event == "tool_call.resolved")
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].payload["call_id"], "call_1");
    assert!(resolved[0].payload.get("content").is_none());

    let duplicate = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/turns/turn_1/tool-calls/call_1/result"
        ))
        .json(&json!({ "success": true }))
        .send()
        .await?;
    assert_eq!(duplicate.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        runtime_threads
            .events_since(thread_id, None)?
            .iter()
            .filter(|event| event.event == "tool_call.resolved")
            .count(),
        1
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skills_endpoint_includes_enabled_field() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let body: serde_json::Value = client
        .get(format!("http://{addr}/v1/skills"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(skills) = body["skills"].as_array() {
        for skill in skills {
            assert!(skill.get("enabled").is_some());
        }
    }

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skills_endpoint_exposes_safe_plugin_provenance_and_shared_toggle() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().join("workspace");
    let plugin_root = tmp.path().join("plugins/demo");
    fs::create_dir_all(plugin_root.join("skills/review"))?;
    fs::write(
        plugin_root.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n[skills]\npath = \"skills\"\n",
    )?;
    fs::write(
        plugin_root.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: reviewed plugin Skill\n---\nbody\n",
    )?;
    let plugin_config = crate::plugins::discovery::DiscoveryConfig {
        workspace: workspace.clone(),
        user_plugins_dir: tmp.path().join("plugins"),
        workspace_plugins_dir: workspace.join(".codewhale/plugins"),
        builtin_plugin_dirs: Vec::new(),
        state_path: tmp.path().join("plugin-state/state.json"),
    };
    let discovery = crate::plugins::PluginDiscoveryContext::from_config_and_environment(
        &plugin_config,
        crate::plugins::HostEnvironment::default(),
    );
    let mut plugins = discovery.registry_for_workspace(&workspace);
    Arc::make_mut(&mut plugins)
        .trust("demo")
        .map_err(anyhow::Error::msg)?;
    Arc::make_mut(&mut plugins)
        .enable("demo")
        .map_err(anyhow::Error::msg)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root.clone(),
            root.join("sessions"),
            None,
            false,
            workspace,
            TestServerOverrides {
                plugin_discovery: Some(discovery),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let list = client
        .get(format!("http://{addr}/v1/skills"))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let plugin_skill = list["skills"]
        .as_array()
        .and_then(|skills| skills.iter().find(|skill| skill["name"] == "demo:review"))
        .context("plugin Skill in runtime API catalog")?;
    assert_eq!(plugin_skill["enabled"], true);
    assert_eq!(plugin_skill["path"], serde_json::Value::Null);
    assert_eq!(plugin_skill["source"], "reviewed-plugin-snapshot:demo");
    assert!(plugin_skill["plugin_id"].as_str().is_some());
    assert!(plugin_skill["plugin_generation"].as_u64().is_some());
    assert!(plugin_skill["plugin_content_hash"].as_str().is_some());
    assert!(
        !plugin_skill
            .to_string()
            .contains(&plugin_root.display().to_string()),
        "runtime API must not expose mutable or staged plugin paths"
    );

    client
        .post(format!("http://{addr}/v1/skills/demo:review"))
        .json(&json!({ "enabled": false }))
        .send()
        .await?
        .error_for_status()?;
    let after = client
        .get(format!("http://{addr}/v1/skills"))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let plugin_skill = after["skills"]
        .as_array()
        .and_then(|skills| skills.iter().find(|skill| skill["name"] == "demo:review"))
        .context("plugin Skill after toggle")?;
    assert_eq!(plugin_skill["enabled"], false);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_toggle_endpoint_404s_for_unknown_skill() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(format!("http://{addr}/v1/skills/no-such-skill"))
        .json(&json!({ "enabled": false }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[test]
fn resolve_skills_scenario() {
    // Scenario consolidation of: resolve_skills_dir_finds_workspace_local_agents_skills, resolve_skills_dir_finds_workspace_local_skills_fallback, resolve_skills_dir_respects_codewhale_only_scan, resolve_skills_dir_preserves_explicit_dir_in_codewhale_only_scan
    // from resolve_skills_dir_finds_workspace_local_agents_skills
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path();
        let local_skills = workspace.join(".agents").join("skills");
        fs::create_dir_all(&local_skills).expect("create skills dir");

        let config = Config::default();
        let resolved = resolve_skills_dir(&config, workspace);

        let expected = fs::canonicalize(&local_skills).expect("canonical local skills");
        assert_eq!(resolved, expected);
    }
    // from resolve_skills_dir_finds_workspace_local_skills_fallback
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path();
        let local_skills = workspace.join("skills");
        fs::create_dir_all(&local_skills).expect("create skills dir");

        let config = Config::default();
        let resolved = resolve_skills_dir(&config, workspace);

        let expected = fs::canonicalize(&local_skills).expect("canonical local skills");
        assert_eq!(resolved, expected);
    }
    // from resolve_skills_dir_respects_codewhale_only_scan
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path();
        let agents_skills = workspace.join(".agents").join("skills");
        let codewhale_skills = workspace.join(".codewhale").join("skills");
        fs::create_dir_all(&agents_skills).expect("create agents skills dir");
        fs::create_dir_all(&codewhale_skills).expect("create codewhale skills dir");

        let config = Config {
            skills: Some(crate::config::SkillsConfig {
                scan_codewhale_only: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let resolved = resolve_skills_dir(&config, workspace);

        let expected = fs::canonicalize(&codewhale_skills).expect("canonical codewhale skills");
        assert_eq!(resolved, expected);
    }
    // from resolve_skills_dir_preserves_explicit_dir_in_codewhale_only_scan
    {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let codewhale_skills = workspace.join(".codewhale").join("skills");
        let configured_skills = tmp.path().join("configured-skills");
        fs::create_dir_all(&codewhale_skills).expect("create codewhale skills dir");
        fs::create_dir_all(&configured_skills).expect("create configured skills dir");

        let config = Config {
            skills_dir: Some(configured_skills.to_string_lossy().into_owned()),
            skills: Some(crate::config::SkillsConfig {
                scan_codewhale_only: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let resolved = resolve_skills_dir(&config, &workspace);

        assert_eq!(resolved, configured_skills);
    }
}

#[test]
fn skills_search_directories_includes_custom_skills_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let custom_skills = tmp.path().join("custom-skills");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&custom_skills).expect("create custom skills");

    let directories = skills_search_directories(
        &workspace,
        &custom_skills,
        crate::skills::SkillDiscoveryMode::Compatible,
    );

    assert!(
        directories.iter().any(|dir| dir == &custom_skills),
        "custom skills_dir must be reported when discovery searches it"
    );
    let message = format_skill_search_paths(&directories);
    assert!(message.contains("custom-skills"));
}

#[test]
fn skill_entry_is_bundled_requires_configured_bundle_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bundled_skills_dir = tmp.path().join("bundled-skills");
    let bundled_skill_path = bundled_skills_dir.join("delegate").join("SKILL.md");
    let override_skill_path = tmp
        .path()
        .join("workspace")
        .join(".agents")
        .join("skills")
        .join("delegate")
        .join("SKILL.md");
    fs::create_dir_all(bundled_skill_path.parent().expect("bundled parent"))
        .expect("create bundled skill dir");
    fs::create_dir_all(override_skill_path.parent().expect("override parent"))
        .expect("create override skill dir");
    fs::write(
        &bundled_skill_path,
        "---\nname: delegate\ndescription: bundled\n---\n",
    )
    .expect("write bundled skill");
    fs::write(
        &override_skill_path,
        "---\nname: delegate\ndescription: override\n---\n",
    )
    .expect("write override skill");

    let bundled_skill = crate::skills::Skill {
        name: "delegate".to_string(),
        description: String::new(),
        localized_descriptions: std::collections::HashMap::new(),
        invocation: crate::skills::SkillInvocation::ModelAndUser,
        aliases: Vec::new(),
        body: String::new(),
        path: bundled_skill_path,
        source: crate::skills::SkillSource::Native,
    };
    let override_skill = crate::skills::Skill {
        name: "delegate".to_string(),
        description: String::new(),
        localized_descriptions: std::collections::HashMap::new(),
        invocation: crate::skills::SkillInvocation::ModelAndUser,
        aliases: Vec::new(),
        body: String::new(),
        path: override_skill_path,
        source: crate::skills::SkillSource::Native,
    };

    assert!(skill_entry_is_bundled(&bundled_skill, &bundled_skills_dir));
    assert!(!skill_entry_is_bundled(
        &override_skill,
        &bundled_skills_dir
    ));
}

/// A `skills` symlink that points outside the workspace must NOT be
/// returned as the resolved skills directory. Containment check ensures
/// the canonicalized candidate stays under the canonicalized workspace
/// root, so a malicious or misconfigured symlink can't promote
/// `/etc` (or any other path) into the skills loader.
#[cfg(unix)]
#[test]
fn resolve_skills_dir_rejects_symlink_escaping_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _env_lock = crate::test_support::lock_test_env();
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
    let workspace_root = tmp.path().join("workspace");
    let escape_target = tmp.path().join("escape_target");
    fs::create_dir_all(&workspace_root).expect("create workspace");
    fs::create_dir_all(&escape_target).expect("create escape target");

    let dotagents = workspace_root.join(".agents");
    fs::create_dir_all(&dotagents).expect("create .agents");
    let bad_link = dotagents.join("skills");
    std::os::unix::fs::symlink(&escape_target, &bad_link).expect("symlink");

    let config = Config::default();
    let resolved = resolve_skills_dir(&config, &workspace_root);

    let canon_escape = fs::canonicalize(&escape_target).expect("canon escape");
    assert_ne!(
        resolved, canon_escape,
        "symlink escaping workspace must not be resolved as skills dir"
    );
    assert_eq!(
        resolved,
        config.skills_dir(),
        "with no valid in-workspace skills dir, resolution should fall back to config"
    );
}

#[cfg(unix)]
#[test]
fn resolve_skills_dir_rejects_codewhale_only_symlink_escaping_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _env_lock = crate::test_support::lock_test_env();
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
    let workspace_root = tmp.path().join("workspace");
    let escape_target = tmp.path().join("escape_target");
    fs::create_dir_all(&workspace_root).expect("create workspace");
    fs::create_dir_all(&escape_target).expect("create escape target");

    let dotcodewhale = workspace_root.join(".codewhale");
    fs::create_dir_all(&dotcodewhale).expect("create .codewhale");
    let bad_link = dotcodewhale.join("skills");
    std::os::unix::fs::symlink(&escape_target, &bad_link).expect("symlink");

    let config = Config {
        skills: Some(crate::config::SkillsConfig {
            scan_codewhale_only: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let resolved = resolve_skills_dir(&config, &workspace_root);

    let canon_escape = fs::canonicalize(&escape_target).expect("canon escape");
    assert_ne!(
        resolved, canon_escape,
        "CodeWhale-only symlink escaping workspace must not be resolved as skills dir"
    );
    assert_eq!(
        resolved,
        config.skills_dir(),
        "with no valid in-workspace CodeWhale skills dir, resolution should fall back to config"
    );
}

// ---------------------------------------------------------------------------
// /v1/config + /v1/config/reload endpoint tests
// ---------------------------------------------------------------------------

/// Helper: POST to `/v1/config` with the given key/value and return the
/// response status + body JSON.
async fn post_set_config(
    client: &reqwest::Client,
    addr: &SocketAddr,
    key: &str,
    value: &str,
    persist: bool,
) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = client
        .post(format!("http://{addr}/v1/config"))
        .json(&serde_json::json!({
            "key": key,
            "value": value,
            "persist": persist,
        }))
        .send()
        .await
        .expect("POST /v1/config should not fail at transport level");
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or_else(|_| serde_json::json!({"_error": "non-json response body"}));
    (status, body)
}

#[tokio::test]
async fn set_config_rejects_unknown_key_with_bad_request() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-config-unknown-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "nonexistent_key", "x", true).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown key should return 400, body: {body}"
    );
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        message.contains("unknown config key"),
        "error message should mention 'unknown config key', got: {message}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_validates_max_history_input() -> Result<()> {
    // Fix #4: invalid max_history input must return 400 instead of silently
    // falling back to a default value.
    let root = std::env::temp_dir().join(format!("codewhale-config-maxhist-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Non-integer input must be rejected.
    let (status, body) = post_set_config(&client, &addr, "max_history", "not-a-number", true).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid max_history should return 400, body: {body}"
    );

    // Negative input must also be rejected (parse::<usize> rejects negatives).
    let (status, body) = post_set_config(&client, &addr, "max_history", "-5", true).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "negative max_history should return 400, body: {body}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_validates_subagents_enabled_input() -> Result<()> {
    // Fix #1: subagents_enabled must validate input and reject non-boolean
    // values with a descriptive 400 error.
    let root = std::env::temp_dir().join(format!("codewhale-config-subenabled-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "subagents_enabled", "maybe", true).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-boolean subagents_enabled should return 400, body: {body}"
    );
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        message.contains("subagents_enabled"),
        "error message should name the key, got: {message}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_validates_subagents_max_depth_input() -> Result<()> {
    // Fix #1: subagents_max_depth must validate input and reject non-integer
    // values with a descriptive 400 error.
    let root = std::env::temp_dir().join(format!("codewhale-config-subdepth-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "subagents_max_depth", "deep", true).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "non-integer subagents_max_depth should return 400, body: {body}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_with_config_path_writes_to_specified_file() -> Result<()> {
    // Fix #2: when the server is started with --config, set_config must
    // persist to that specific file rather than the default discovery path.
    let root =
        std::env::temp_dir().join(format!("codewhale-config-path-persist-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Persist a subagents_max_depth value above the ceiling to also verify
    // clamping (Fix #1).
    let over_ceiling = u64::from(codewhale_config::MAX_SPAWN_DEPTH_CEILING) + 10;
    let (status, body) = post_set_config(
        &client,
        &addr,
        "subagents_max_depth",
        &over_ceiling.to_string(),
        true,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "persisting subagents_max_depth should succeed, body: {body}"
    );
    assert!(
        body["persisted"].as_bool().unwrap_or(false),
        "response should report persisted=true, body: {body}"
    );

    // Read the config file and verify the value was clamped and written.
    let contents = fs::read_to_string(&config_file)
        .with_context(|| format!("config file should exist at {}", config_file.display()))?;
    assert!(
        contents.contains("max_depth"),
        "config file should contain max_depth key, got: {contents}"
    );
    // The value should be clamped to MAX_SPAWN_DEPTH_CEILING.
    let expected = format!(
        "max_depth = {}",
        u64::from(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
    );
    assert!(
        contents.contains(&expected),
        "config file should contain clamped value '{expected}', got: {contents}"
    );

    // Also verify a subagents_enabled persistence writes to the same file.
    let (status, body) = post_set_config(&client, &addr, "subagents_enabled", "true", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let contents = fs::read_to_string(&config_file)?;
    assert!(
        contents.contains("enabled = true"),
        "config file should contain enabled = true, got: {contents}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_endpoint_returns_success() -> Result<()> {
    // Basic smoke test that /v1/config/reload returns 200 with a message.
    let root = std::env::temp_dir().join(format!("codewhale-config-reload-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await?;
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        !message.is_empty(),
        "reload response should include a non-empty message"
    );

    handle.abort();
    Ok(())
}

/// Helper: GET `/v1/config` and return the parsed response body.
async fn get_config(client: &reqwest::Client, addr: &SocketAddr) -> serde_json::Value {
    client
        .get(format!("http://{addr}/v1/config"))
        .send()
        .await
        .expect("GET /v1/config should not fail at transport level")
        .error_for_status()
        .expect("GET /v1/config should return 200")
        .json()
        .await
        .expect("GET /v1/config should return valid JSON")
}

async fn get_providers(client: &reqwest::Client, addr: &SocketAddr) -> serde_json::Value {
    client
        .get(format!("http://{addr}/v1/providers"))
        .send()
        .await
        .expect("GET /v1/providers should not fail at transport level")
        .error_for_status()
        .expect("GET /v1/providers should return 200")
        .json()
        .await
        .expect("GET /v1/providers should return valid JSON")
}

async fn get_provider_models(
    client: &reqwest::Client,
    addr: &SocketAddr,
    provider: &str,
) -> serde_json::Value {
    client
        .get(format!("http://{addr}/v1/providers/{provider}/models"))
        .send()
        .await
        .expect("GET /v1/providers/{id}/models should not fail at transport level")
        .error_for_status()
        .expect("GET /v1/providers/{id}/models should return 200")
        .json()
        .await
        .expect("GET /v1/providers/{id}/models should return valid JSON")
}

#[tokio::test]
async fn get_config_returns_active_provider_model() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-active-provider-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        format!(
            "default_text_model = \"deepseek-v4-pro\"\nprovider = \"volcengine\"\n\n[providers.volcengine]\nmodel = \"{}\"\n",
            crate::config::DEFAULT_VOLCENGINE_FLASH_MODEL
        ),
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let body = get_config(&client, &addr).await;
    assert_eq!(body["provider"].as_str(), Some("volcengine"));
    assert_eq!(
        body["model"].as_str(),
        Some(crate::config::DEFAULT_VOLCENGINE_FLASH_MODEL),
        "GET /v1/config should expose the active provider model, not the root DeepSeek default"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn api_surfaces_only_configured_model_for_custom_provider_route() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-custom-provider-model-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        "provider = \"volcengine\"\n\n[providers.volcengine]\nbase_url = \"https://ark.cn-beijing.volces.com/api/plan/v3\"\nmodel = \"glm-5.2\"\napi_key = \"ark-test\"\n",
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let config_body = get_config(&client, &addr).await;
    assert_eq!(config_body["provider"].as_str(), Some("volcengine"));
    assert_eq!(
        config_body["model"].as_str(),
        Some("glm-5.2"),
        "GET /v1/config should preserve the active provider's explicit custom model"
    );

    let providers = get_providers(&client, &addr).await;
    let volcengine = providers["providers"]
        .as_array()
        .and_then(|providers| {
            providers
                .iter()
                .find(|entry| entry["id"].as_str() == Some("volcengine"))
        })
        .expect("volcengine provider entry");
    assert_eq!(providers["current"].as_str(), Some("volcengine"));
    assert_eq!(
        volcengine["default_model"].as_str(),
        Some("glm-5.2"),
        "GET /v1/providers should mirror the /provider default route when a saved model override exists"
    );

    let provider_models = get_provider_models(&client, &addr, "volcengine").await;
    let model_ids: Vec<_> = provider_models["models"]
        .as_array()
        .expect("models array")
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert_eq!(
        model_ids.first().copied(),
        Some("glm-5.2"),
        "configured volcengine model should be the only model exposed for a custom provider route"
    );
    assert_eq!(model_ids, vec!["glm-5.2"]);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn api_surfaces_only_active_model_when_runtime_route_passes_ids_through() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-runtime-pass-through-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        "provider = \"volcengine\"\nbase_url = \"https://ark.cn-beijing.volces.com/api/plan/v3\"\n\n[providers.volcengine]\nmodel = \"glm-5.2\"\napi_key = \"ark-test\"\n",
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let config_body = get_config(&client, &addr).await;
    assert_eq!(config_body["provider"].as_str(), Some("volcengine"));
    assert_eq!(config_body["model"].as_str(), Some("glm-5.2"));

    let providers = get_providers(&client, &addr).await;
    let volcengine = providers["providers"]
        .as_array()
        .and_then(|providers| {
            providers
                .iter()
                .find(|entry| entry["id"].as_str() == Some("volcengine"))
        })
        .expect("volcengine provider entry");
    assert_eq!(providers["current"].as_str(), Some("volcengine"));
    assert_eq!(volcengine["default_model"].as_str(), Some("glm-5.2"));

    let provider_models = get_provider_models(&client, &addr, "volcengine").await;
    let model_ids: Vec<_> = provider_models["models"]
        .as_array()
        .expect("models array")
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert_eq!(model_ids, vec!["glm-5.2"]);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn provider_models_expose_exact_image_input_facts_and_thread_selection_stays_local()
-> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-provider-model-capabilities-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        "provider = \"deepseek\"\ndefault_text_model = \"deepseek-v4-pro\"\n",
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let models = get_provider_models(&client, &addr, "deepseek").await;
    let entries = models["models"].as_array().context("models array")?;
    let vision = entries
        .iter()
        .find(|entry| entry["id"] == "deepseek-v4-flash-vision-exp")
        .context("DeepSeek vision model entry")?;
    assert_eq!(vision["image_input"], "supported");
    let text_only = entries
        .iter()
        .find(|entry| entry["id"] == "deepseek-v4-pro")
        .context("DeepSeek text model entry")?;
    assert_eq!(text_only["image_input"], "unsupported");

    let config_before = get_config(&client, &addr).await;
    let response = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model_provider": "deepseek",
            "model": "deepseek-v4-flash-vision-exp",
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let thread: serde_json::Value = response.json().await?;
    assert_eq!(thread["model_provider"], "deepseek");
    assert_eq!(thread["model"], "deepseek-v4-flash-vision-exp");

    let config_after = get_config(&client, &addr).await;
    assert_eq!(config_after["provider"], config_before["provider"]);
    assert_eq!(config_after["model"], config_before["model"]);

    handle.abort();
    Ok(())
}

#[test]
fn provider_catalog_keeps_official_deepseek_facts_but_not_custom_proxy_claims() {
    for official_base_url in [
        "https://api.deepseek.com/v1",
        "https://api.deepseek.com/beta/",
    ] {
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Config::default()
        };
        let provider_config = config.provider_config_for_mut(ApiProvider::Deepseek);
        provider_config.base_url = Some(official_base_url.to_string());
        provider_config.model = Some("deepseek-v4-pro".to_string());

        assert!(
            !provider_uses_custom_route_for_api(&config, ApiProvider::Deepseek),
            "official DeepSeek endpoint must retain the shared model catalog: {official_base_url}"
        );
        let models = provider_models_for_api(&config, ApiProvider::Deepseek, ApiProvider::Deepseek);
        assert!(
            models
                .iter()
                .any(|model| model == "deepseek-v4-flash-vision-exp"),
            "official DeepSeek endpoint must expose the experimental vision model: {official_base_url}"
        );
    }

    let mut custom = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some("private-deepseek-deployment".to_string()),
        ..Config::default()
    };
    let provider_config = custom.provider_config_for_mut(ApiProvider::Deepseek);
    provider_config.base_url = Some("https://deepseek-proxy.example.test/v1".to_string());
    provider_config.model = Some("private-deepseek-deployment".to_string());

    assert!(provider_uses_custom_route_for_api(
        &custom,
        ApiProvider::Deepseek
    ));
    assert_eq!(
        provider_models_for_api(&custom, ApiProvider::Deepseek, ApiProvider::Deepseek),
        vec!["private-deepseek-deployment".to_string()],
        "a real custom endpoint must expose only its explicitly configured model namespace"
    );

    custom.default_text_model = Some("deepseek-v4-flash-vision-exp".to_string());
    custom.provider_config_for_mut(ApiProvider::Deepseek).model =
        Some("deepseek-v4-flash-vision-exp".to_string());
    assert_eq!(
        provider_models_for_api(&custom, ApiProvider::Deepseek, ApiProvider::Deepseek),
        vec!["deepseek-v4-flash-vision-exp".to_string()],
        "a custom proxy may legitimately reuse a first-party model id"
    );
    assert_eq!(
        provider_model_image_input_for_api(
            &custom,
            ApiProvider::Deepseek,
            "deepseek-v4-flash-vision-exp",
        ),
        codewhale_config::route::CapabilityState::Unknown,
        "same-name custom proxy must not surface first-party vision capability as verified"
    );
}

#[test]
fn provider_credential_state_wire_values_are_stable_and_sanitized() {
    use crate::provider_readiness::CredentialState;

    for (internal, expected) in [
        (CredentialState::Saved, "configured"),
        (CredentialState::ImportedToken, "configured"),
        (CredentialState::MissingLogin, "login_required"),
        (CredentialState::ExternalConsent, "login_required"),
        (CredentialState::MissingKey, "missing"),
        (CredentialState::NoAuth, "no_auth"),
        (CredentialState::Local, "local"),
        (CredentialState::Legacy, "legacy"),
    ] {
        assert_eq!(
            serde_json::to_value(ProviderCredentialState::from(internal))
                .expect("provider credential state should serialize"),
            expected,
        );
    }
}

#[tokio::test]
async fn provider_catalog_reports_exact_named_custom_credential_state_without_secrets() -> Result<()>
{
    let _env_lock = lock_test_env();
    let _key_source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
    let root = std::env::temp_dir().join(format!(
        "codewhale-provider-credential-state-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;

    let cases = [
        (
            "private-gateway",
            "configured",
            r#"provider = "private-gateway"

[providers.private-gateway]
kind = "openai-compatible"
base_url = "https://secret-route.example.test/tenant/private/v1"
model = "private-model"
api_key = "sk-provider-fixture-do-not-leak"
api_key_env = "CODEWHALE_PROVIDER_FIXTURE_ENV"
"#,
        ),
        (
            "missing-gateway",
            "missing",
            r#"provider = "missing-gateway"

[providers.missing-gateway]
kind = "openai-compatible"
base_url = "https://missing-route.example.test/private/v1"
model = "missing-model"
"#,
        ),
        (
            "local-runtime",
            "local",
            r#"provider = "local-runtime"

[providers.local-runtime]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18190/private/v1"
model = "local-model"
"#,
        ),
    ];

    for (route_id, expected_state, config) in cases {
        let config_file = root.join(format!("{route_id}.toml"));
        fs::write(&config_file, config)?;
        let Some((addr, _runtime_threads, handle)) =
            spawn_test_server_with_config_path(config_file).await?
        else {
            return Ok(());
        };
        let providers = get_providers(&crate::tls::reqwest_client(), &addr).await;
        assert_eq!(providers["current"], "custom");
        let custom = providers["providers"]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["id"] == "custom"))
            .context("custom provider entry")?;
        assert_eq!(
            custom["model_provider_id"], route_id,
            "credential state must describe the exact active named custom route"
        );
        assert_eq!(custom["credentialState"], expected_state);

        let allowed_fields: std::collections::BTreeSet<_> = [
            "credentialState",
            "default_model",
            "display_name",
            "has_model_catalog",
            "id",
            "model_provider_id",
        ]
        .into_iter()
        .collect();
        for entry in providers["providers"]
            .as_array()
            .context("providers array")?
        {
            let actual_fields: std::collections::BTreeSet<_> = entry
                .as_object()
                .context("provider entry object")?
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                actual_fields, allowed_fields,
                "provider catalog must remain a non-secret projection"
            );
        }

        let serialized = serde_json::to_string(&providers)?;
        for forbidden in [
            "sk-provider-fixture-do-not-leak",
            "CODEWHALE_PROVIDER_FIXTURE_ENV",
            "secret-route.example.test",
            "missing-route.example.test",
            "127.0.0.1:18190",
            "default_base_url",
            "env_vars",
            "api_key",
            "api_key_env",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "provider catalog leaked forbidden detail: {forbidden}"
            );
        }

        handle.abort();
    }

    Ok(())
}

#[tokio::test]
async fn provider_catalog_preserves_named_custom_identity_for_new_threads() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-provider-named-custom-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "lm-studio"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18190/v1"
model = "local-vision-model"
api_key = "local-test-key"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let config_before = get_config(&client, &addr).await;
    assert_eq!(config_before["provider"], "lm-studio");
    assert_eq!(config_before["model"], "local-vision-model");

    let providers = get_providers(&client, &addr).await;
    assert_eq!(providers["current"], "custom");
    let custom = providers["providers"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["id"] == "custom"))
        .context("custom provider entry")?;
    assert_eq!(custom["model_provider_id"], "lm-studio");
    assert_eq!(custom["default_model"], "local-vision-model");

    let response = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({
            "model_provider": "custom",
            "model_provider_id": custom["model_provider_id"],
            "model": custom["default_model"],
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let thread: serde_json::Value = response.json().await?;
    assert_eq!(thread["model_provider"], "custom");
    assert_eq!(thread["model_provider_id"], "lm-studio");
    assert_eq!(thread["model"], "local-vision-model");

    let config_after = get_config(&client, &addr).await;
    assert_eq!(config_after["provider"], config_before["provider"]);
    assert_eq!(config_after["model"], config_before["model"]);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_reads_from_config_path_and_updates_in_memory_state() -> Result<()> {
    // Fix #2 + reload behavior: This test proves that reload reads from the
    // `--config` path (not default discovery) and actually updates the
    // in-memory state visible to GET /v1/config.
    //
    // If Fix #2 is reverted (reload uses Config::load(None, None) instead of
    // state.config_path), the reload will read an empty/default config and
    // the persisted value will NOT appear in GET /v1/config → test fails.
    let root =
        std::env::temp_dir().join(format!("codewhale-config-reload-path-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Step 1: Record initial model value (should be the default, since
    // Config::default() has default_text_model = None).
    let before = get_config(&client, &addr).await;
    let initial_model = before["model"].as_str().unwrap_or_default().to_string();
    assert!(
        !initial_model.is_empty(),
        "initial model should not be empty"
    );
    // The initial subagents_max_depth should be DEFAULT_SPAWN_DEPTH (3)
    // since Config::default() has no subagents config.
    let initial_depth = before["subagents_max_depth"]
        .as_u64()
        .expect("subagents_max_depth should be a number");
    assert_eq!(
        initial_depth,
        u64::from(codewhale_config::DEFAULT_SPAWN_DEPTH),
        "initial subagents_max_depth should be DEFAULT_SPAWN_DEPTH"
    );

    // Step 2: Persist a new model value to the config file.
    // set_config must NOT mutate in-memory state (by design — the caller
    // must call /v1/config/reload to apply changes).
    // Use a valid DeepSeek model ID so Config::validate() doesn't reject
    // the reloaded config.
    let test_model = "deepseek-v4-flash";
    let (status, body) = post_set_config(&client, &addr, "model", test_model, true).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "set_config should succeed, body: {body}"
    );

    // Step 3: Verify in-memory state is NOT mutated by set_config alone.
    let after_set = get_config(&client, &addr).await;
    assert_eq!(
        after_set["model"].as_str().unwrap_or_default(),
        initial_model,
        "set_config must NOT update in-memory state before reload"
    );

    // Step 4: Also persist subagents_max_depth = 5 (below ceiling of 8).
    let (status, body) = post_set_config(&client, &addr, "subagents_max_depth", "5", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Step 5: Reload — this must read from config_file (not default discovery).
    let reload_resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(reload_resp.status(), StatusCode::OK);

    // Step 6: Verify in-memory state IS now updated after reload.
    let after_reload = get_config(&client, &addr).await;

    // Model should reflect the persisted value.
    assert_eq!(
        after_reload["model"].as_str().unwrap_or_default(),
        test_model,
        "after reload, model should be the persisted value — \
         if this fails, reload is not reading from config_path"
    );

    // subagents_max_depth should reflect the persisted value (5).
    assert_eq!(
        after_reload["subagents_max_depth"].as_u64(),
        Some(5),
        "after reload, subagents_max_depth should be 5"
    );

    handle.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// POST /v1/providers/{id}/switch endpoint tests
//
// These tests pin down the TUI-parity contract for the GUI's provider
// picker: a bare switch (no model arg) MUST NOT overwrite the user's
// `[providers.<id>].model` config. Regression for the bug where clicking
// volcengine in the picker forced `model = "deepseek-v4-pro"` even when
// the user had configured `model = "glm-2"`.
// ---------------------------------------------------------------------------

/// Helper: POST to `/v1/providers/{id}/switch` and return the response
/// status + body JSON.
async fn post_switch_provider(
    client: &reqwest::Client,
    addr: &SocketAddr,
    provider: &str,
    body: &serde_json::Value,
) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = client
        .post(format!("http://{addr}/v1/providers/{provider}/switch"))
        .json(body)
        .send()
        .await
        .expect("POST /v1/providers/{id}/switch should not fail at transport level");
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or_else(|_| serde_json::json!({"_error": "non-json response body"}));
    (status, body)
}

#[tokio::test]
async fn switch_provider_without_model_arg_preserves_user_per_provider_model() -> Result<()> {
    // Regression: clicking volcengine in the GUI picker used to send
    // `POST /v1/config { key: "model", value: "deepseek-v4-pro" }` (the
    // catalog default), clobbering the user's `[providers.volcengine].model
    // = "glm-2"`. The new /v1/providers/{id}/switch endpoint MUST NOT
    // touch the model key when no model arg is provided — mirroring the
    // TUI's `/provider volcengine` (model: None) flow in
    // `commands/groups/core/provider.rs` + `tui/ui.rs::switch_provider`.
    let root = std::env::temp_dir().join(format!("codewhale-switch-no-model-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.volcengine]
api_key = "ark-test"
base_url = "https://ark.cn-beijing.volces.com/api/plan/v3"
model = "glm-2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Switch to volcengine WITHOUT a model arg — simulates a picker click.
    let (status, body) =
        post_switch_provider(&client, &addr, "volcengine", &serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch should succeed, body: {body}"
    );

    // Response must report the user's configured model, NOT the catalog
    // default "deepseek-v4-pro".
    assert_eq!(
        body["provider"].as_str(),
        Some("volcengine"),
        "response should echo the switched-to provider"
    );
    assert_eq!(
        body["model"].as_str(),
        Some("glm-2"),
        "resolved model must be the user's `[providers.volcengine].model`, \
         not the catalog default — if this fails the switch endpoint is \
         clobbering per-provider config"
    );

    // The config file on disk must NOT contain a `model = "deepseek-v4-pro"`
    // override for volcengine — `glm-2` must be preserved verbatim.
    let persisted = fs::read_to_string(&config_file)?;
    assert!(
        persisted.contains("model = \"glm-2\""),
        "user's `[providers.volcengine].model = \"glm-2\"` must be preserved on disk. \
         Actual config:\n{persisted}"
    );
    assert!(
        !persisted
            .matches("model = \"deepseek-v4-pro\"")
            .count()
            .ge(&2),
        "switch must not add a second `model = \"deepseek-v4-pro\"` line for volcengine. \
         Actual config:\n{persisted}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_with_explicit_model_arg_persists_model() -> Result<()> {
    // When the user explicitly chooses a model (e.g. `/provider volcengine
    // glm-2.5` or a model-picker selection), the switch endpoint MUST
    // persist that model — mirroring `switch_provider`'s
    // `if model_override.is_some()` branch (ui.rs:9400-9405).
    let root = std::env::temp_dir().join(format!("codewhale-switch-with-model-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.volcengine]
api_key = "ark-test"
base_url = "https://ark.cn-beijing.volces.com/api/plan/v3"
model = "glm-2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Switch to volcengine WITH an explicit model arg.
    let (status, body) = post_switch_provider(
        &client,
        &addr,
        "volcengine",
        &serde_json::json!({ "model": "deepseek-v4-flash" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch with explicit model should succeed, body: {body}"
    );

    // The persisted config must reflect the explicit override.
    let persisted = fs::read_to_string(&config_file)?;
    assert!(
        persisted.contains("model = \"deepseek-v4-flash\""),
        "explicit model arg must be persisted to `[providers.volcengine].model`. \
         Actual config:\n{persisted}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_rejects_unknown_provider_id() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-switch-unknown-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# empty\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, _body) = post_switch_provider(
        &client,
        &addr,
        "not-a-real-provider",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown provider id should return 400"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_rejects_legacy_deepseek_cn_alias() -> Result<()> {
    // The legacy `deepseek-cn` alias has no ProviderKind metadata; the
    // GUI must use `deepseek` instead. Same guard as list_provider_models.
    let root = std::env::temp_dir().join(format!("codewhale-switch-cn-alias-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# empty\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) =
        post_switch_provider(&client, &addr, "deepseek-cn", &serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "deepseek-cn should be rejected, body: {body}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_with_deepseek_and_explicit_model_updates_default_text_model() -> Result<()>
{
    // When switching TO a DeepSeek provider with an explicit model, the
    // endpoint must persist `default_text_model` (the DeepSeek-specific
    // root key) in addition to the provider change, mirroring
    // `switch_provider` in ui.rs which pins `default_model` for DeepSeek.
    let root = std::env::temp_dir().join(format!(
        "codewhale-switch-deepseek-model-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "volcengine"
default_text_model = "old-model"

[providers.volcengine]
api_key = "ark-test"
base_url = "https://ark.cn-beijing.volces.com/api/plan/v3"
model = "glm-2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Switch to deepseek WITH an explicit model override.
    let (status, body) = post_switch_provider(
        &client,
        &addr,
        "deepseek",
        &serde_json::json!({ "model": "deepseek-v4-pro" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch to deepseek with model should succeed, body: {body}"
    );

    // The persisted config must have provider = "deepseek" and
    // default_text_model updated to the explicit model.
    let persisted = fs::read_to_string(&config_file)?;
    assert!(
        persisted.contains("provider = \"deepseek\""),
        "provider should be persisted as deepseek. Actual config:\n{persisted}"
    );
    assert!(
        persisted.contains("default_text_model = \"deepseek-v4-pro\""),
        "DeepSeek explicit model must be persisted as default_text_model. \
         Actual config:\n{persisted}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_empty_model_string_treated_as_no_override() -> Result<()> {
    // An empty string model (`{ "model": "" }`) must be treated the same
    // as no model at all — the endpoint should NOT persist a model key,
    // matching the TUI's behavior where a blank model arg is ignored.
    let root =
        std::env::temp_dir().join(format!("codewhale-switch-empty-model-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.volcengine]
api_key = "ark-test"
base_url = "https://ark.cn-beijing.volces.com/api/plan/v3"
model = "glm-2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_switch_provider(
        &client,
        &addr,
        "volcengine",
        &serde_json::json!({ "model": "" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch with empty model should succeed, body: {body}"
    );

    // The user's `model = "glm-2"` must NOT be overwritten.
    let persisted = fs::read_to_string(&config_file)?;
    assert!(
        persisted.contains("model = \"glm-2\""),
        "user's model must be preserved when empty-string model is sent. \
         Actual config:\n{persisted}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn switch_provider_persists_provider_key_on_disk() -> Result<()> {
    // Verify that the root `provider = "..."` key is correctly written to
    // the config file on disk, not just in the response body.
    let root =
        std::env::temp_dir().join(format!("codewhale-switch-provider-disk-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.volcengine]
api_key = "ark-test"
base_url = "https://ark.cn-beijing.volces.com/api/plan/v3"
model = "glm-2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, _body) =
        post_switch_provider(&client, &addr, "volcengine", &serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let persisted = fs::read_to_string(&config_file)?;
    assert!(
        persisted.contains("provider = \"volcengine\""),
        "root `provider` key must be updated on disk. Actual config:\n{persisted}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn zai_model_update_is_provider_scoped_and_preserves_deepseek_fallback() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-zai-model-scope-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "zai"
default_text_model = "deepseek-v4-pro"

[providers.zai]
api_key = "zai-test-key"
model = "GLM-5.2"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let response = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let before = get_config(&client, &addr).await;
    assert_eq!(before["provider"], "zai");
    assert_eq!(before["model"], "GLM-5.2");
    assert_eq!(before["default_model"], "deepseek-v4-pro");

    let (status, body) = post_set_config(&client, &addr, "model", "glm-5-turbo", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["value"], "GLM-5-Turbo");

    let persisted = fs::read_to_string(&config_file)?;
    let persisted: toml::Value = toml::from_str(&persisted)?;
    assert_eq!(
        persisted["default_text_model"].as_str(),
        Some("deepseek-v4-pro"),
        "the active Z.ai model must not overwrite the DeepSeek fallback"
    );
    assert_eq!(
        persisted["providers"]["zai"]["model"].as_str(),
        Some("GLM-5-Turbo")
    );

    let (status, body) = post_set_config(&client, &addr, "model", "deepseek-v4-flash", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");

    let response = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let after = get_config(&client, &addr).await;
    assert_eq!(after["provider"], "zai");
    assert_eq!(after["model"], "GLM-5-Turbo");
    assert_eq!(after["default_model"], "deepseek-v4-pro");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_preserves_profile_selected_named_custom_route() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-reload-profile-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[profiles.local]
provider = "lm-studio"

[profiles.local.providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18190/v1"
model = "profile-local-model"
api_key = "profile-test-key"
"#,
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path_and_profile(config_file, "local".to_string()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let response = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let config = get_config(&client, &addr).await;
    assert_eq!(config["provider"], "lm-studio");
    assert_eq!(config["model"], "profile-local-model");
    assert_eq!(config["base_url"], "http://127.0.0.1:18190/v1");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_refreshes_mcp_config_path() -> Result<()> {
    // Fix #3: After reload, list_mcp_servers should see the new mcp_config_path
    // from the reloaded config (not a stale cached value).
    //
    // This test works by:
    // 1. Starting with config_path pointing to custom-config.toml (initially empty)
    // 2. Writing mcp_config_path = <new_path> to the config file via set_config
    // 3. Reloading
    // 4. GET /v1/config and verifying mcp_config_path field changed
    //
    // If Fix #3 were still needed (stale mcp_config_path field in state),
    // this test would fail because the old field wouldn't update. Since we
    // removed the stale field and read directly from config, this test also
    // validates that architectural decision.
    let root =
        std::env::temp_dir().join(format!("codewhale-config-mcp-refresh-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Record initial mcp_config_path (set by test helper to root/mcp.json).
    let before = get_config(&client, &addr).await;
    let initial_mcp_path = before["mcp_config_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !initial_mcp_path.is_empty(),
        "initial mcp_config_path should not be empty"
    );

    // Persist a new mcp_config_path to the config file.
    let new_mcp_path = root.join("custom-mcp.json");
    let new_mcp_path_str = new_mcp_path.to_string_lossy().to_string();
    let (status, body) =
        post_set_config(&client, &addr, "mcp_config_path", &new_mcp_path_str, true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Before reload, GET should still return the old path.
    let after_set = get_config(&client, &addr).await;
    assert_eq!(
        after_set["mcp_config_path"].as_str().unwrap_or_default(),
        initial_mcp_path,
        "set_config must NOT update in-memory mcp_config_path before reload"
    );

    // Reload.
    let reload_resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(reload_resp.status(), StatusCode::OK);

    // After reload, GET should return the new path.
    let after_reload = get_config(&client, &addr).await;
    let reloaded_mcp_path = after_reload["mcp_config_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        reloaded_mcp_path, new_mcp_path_str,
        "after reload, mcp_config_path should reflect the persisted value — \
         if this fails, the MCP path is stale after reload"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_with_persist_false_does_not_write_to_disk() -> Result<()> {
    // Verify the persist:false branch: response reports persisted:false and
    // the config file on disk is NOT modified. This is the "dry run" path
    // the GUI can use to validate input without committing changes.
    let root = std::env::temp_dir().join(format!("codewhale-config-nopersist-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    let initial_contents = "# initial empty config\n";
    fs::write(&config_file, initial_contents)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "model", "deepseek-v4-flash", false).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "persist:false should still return 200, body: {body}"
    );
    assert_eq!(
        body["persisted"].as_bool(),
        Some(false),
        "persisted should be false when persist:false, body: {body}"
    );
    assert_eq!(
        body["requires_reload"].as_bool(),
        Some(false),
        "requires_reload should be false when persist:false, body: {body}"
    );
    assert_eq!(
        body["key"].as_str().unwrap_or_default(),
        "model",
        "key should echo the request key, body: {body}"
    );

    // The config file on disk must NOT have been modified.
    let contents = fs::read_to_string(&config_file)?;
    assert_eq!(
        contents, initial_contents,
        "persist:false must not modify the config file on disk"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_subagents_max_depth_below_ceiling_not_clamped() -> Result<()> {
    // Verify that values at and below the ceiling pass through unchanged.
    // The existing clamping test only verifies over-ceiling clamping; this
    // test ensures legitimate values are not accidentally modified.
    let root =
        std::env::temp_dir().join(format!("codewhale-config-depth-noclamp-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Test a value at the ceiling (should not be clamped).
    let ceiling = u64::from(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
    let (status, body) = post_set_config(
        &client,
        &addr,
        "subagents_max_depth",
        &ceiling.to_string(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let contents = fs::read_to_string(&config_file)?;
    let expected = format!("max_depth = {ceiling}");
    assert!(
        contents.contains(&expected),
        "value at ceiling should be written as-is: expected '{expected}', got: {contents}"
    );

    // Test a value below the ceiling (should not be clamped).
    let below = ceiling.saturating_sub(1);
    let (status, body) = post_set_config(
        &client,
        &addr,
        "subagents_max_depth",
        &below.to_string(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let contents = fs::read_to_string(&config_file)?;
    let expected = format!("max_depth = {below}");
    assert!(
        contents.contains(&expected),
        "value below ceiling should be written as-is: expected '{expected}', got: {contents}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_subagents_enabled_false_persists() -> Result<()> {
    // Verify that subagents_enabled=false is properly persisted. The
    // existing test only verifies the true branch; this covers the false
    // branch to ensure both boolean values round-trip correctly.
    let root = std::env::temp_dir().join(format!("codewhale-config-subfalse-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "[subagents]\nenabled = true\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "subagents_enabled", "false", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body["persisted"].as_bool().unwrap_or(false),
        "should report persisted=true, body: {body}"
    );

    let contents = fs::read_to_string(&config_file)?;
    assert!(
        contents.contains("enabled = false"),
        "config file should contain 'enabled = false', got: {contents}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_with_malformed_file_returns_error() -> Result<()> {
    // Verify error handling: if the config file contains invalid TOML,
    // reload should return 500 instead of crashing or silently succeeding.
    // This catches regressions where the map_err is accidentally removed.
    let root = std::env::temp_dir().join(format!("codewhale-config-malformed-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Corrupt the config file with invalid TOML.
    fs::write(&config_file, "this is = = not valid toml [[[\n")?;

    let resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "reload with malformed config should return 500"
    );

    // Verify the error response has a meaningful message.
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        message.contains("failed to reload config"),
        "error message should mention reload failure, got: {message}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_model_follows_persisted_provider_before_reload() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "codewhale-config-provider-model-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(
        &config_file,
        format!(
            "provider = \"deepseek\"\ndefault_text_model = \"deepseek-v4-pro\"\n\n[providers.volcengine]\nmodel = \"{}\"\n",
            crate::config::DEFAULT_VOLCENGINE_MODEL
        ),
    )?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let (status, body) = post_set_config(&client, &addr, "provider", "volcengine", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let target_model = crate::config::DEFAULT_VOLCENGINE_FLASH_MODEL;
    let (status, body) = post_set_config(&client, &addr, "model", target_model, true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let config_body = fs::read_to_string(&config_file)?;
    assert!(
        config_body.contains("provider = \"volcengine\""),
        "provider should be persisted before reload"
    );
    // The persisted value is the normalized wire id (lowercase), not the
    // display spelling of DEFAULT_VOLCENGINE_FLASH_MODEL.
    let expected_wire = crate::config::normalize_model_name_for_provider(
        crate::config::ApiProvider::Volcengine,
        target_model,
    )
    .expect("volcengine flash model should normalize");
    assert!(
        config_body.contains(&format!("model = \"{expected_wire}\"")),
        "volcengine model should be written to the provider table as its wire id"
    );
    assert!(
        config_body.contains("default_text_model = \"deepseek-v4-pro\""),
        "switching provider model must not overwrite DeepSeek's root default_text_model"
    );

    let reload_resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(reload_resp.status(), StatusCode::OK);

    let after_reload = get_config(&client, &addr).await;
    assert_eq!(after_reload["provider"].as_str(), Some("volcengine"));
    assert_eq!(after_reload["model"].as_str(), Some(expected_wire.as_str()));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn reload_config_applies_multiple_persisted_keys() -> Result<()> {
    // Verify that multiple set_config calls accumulate on disk and a single
    // reload picks up ALL changes. This catches regressions where reload
    // only applies the last-written key or where set_config overwrites
    // prior keys unexpectedly.
    let root = std::env::temp_dir().join(format!("codewhale-config-multi-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Record initial values.
    let before = get_config(&client, &addr).await;
    let initial_model = before["model"].as_str().unwrap_or_default().to_string();
    let initial_depth = before["subagents_max_depth"].as_u64().unwrap_or(0);
    let initial_enabled = before["subagents_enabled"].as_bool().unwrap_or(false);

    // Persist three different keys.
    // Use a valid DeepSeek model ID so Config::validate() doesn't reject
    // the reloaded config.
    let test_model = "deepseek-v4-pro";
    let (status, body) = post_set_config(&client, &addr, "model", test_model, true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (status, body) = post_set_config(&client, &addr, "subagents_max_depth", "4", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Flip subagents_enabled to the opposite of its initial value.
    let target_enabled = !initial_enabled;
    let (status, body) = post_set_config(
        &client,
        &addr,
        "subagents_enabled",
        &target_enabled.to_string(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    // Before reload, in-memory state should be unchanged for all three keys.
    let after_set = get_config(&client, &addr).await;
    assert_eq!(
        after_set["model"].as_str().unwrap_or_default(),
        initial_model,
        "model should be unchanged before reload"
    );
    assert_eq!(
        after_set["subagents_max_depth"].as_u64(),
        Some(initial_depth),
        "subagents_max_depth should be unchanged before reload"
    );
    assert_eq!(
        after_set["subagents_enabled"].as_bool(),
        Some(initial_enabled),
        "subagents_enabled should be unchanged before reload"
    );

    // Reload.
    let reload_resp = client
        .post(format!("http://{addr}/v1/config/reload"))
        .send()
        .await?;
    assert_eq!(reload_resp.status(), StatusCode::OK);

    // After reload, ALL three keys should reflect their persisted values.
    let after_reload = get_config(&client, &addr).await;
    assert_eq!(
        after_reload["model"].as_str().unwrap_or_default(),
        test_model,
        "model should be updated after reload"
    );
    assert_eq!(
        after_reload["subagents_max_depth"].as_u64(),
        Some(4),
        "subagents_max_depth should be 4 after reload"
    );
    assert_eq!(
        after_reload["subagents_enabled"].as_bool(),
        Some(target_enabled),
        "subagents_enabled should be {} after reload",
        target_enabled
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn set_config_response_contains_all_expected_fields() -> Result<()> {
    // Verify the SetConfigResponse shape: key, value, message, persisted,
    // requires_reload. This catches serialization regressions and ensures
    // the GUI client can rely on these fields being present and correct.
    let root = std::env::temp_dir().join(format!("codewhale-config-shape-{}", Uuid::new_v4()));
    fs::create_dir_all(&root)?;
    let config_file = root.join("custom-config.toml");
    fs::write(&config_file, "# initial\n")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_config_path(config_file.clone()).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // persist:true → persisted=true, requires_reload=true
    let (status, body) = post_set_config(&client, &addr, "model", "deepseek-v4-flash", true).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["key"].as_str(),
        Some("model"),
        "key field, body: {body}"
    );
    assert_eq!(
        body["value"].as_str(),
        Some("deepseek-v4-flash"),
        "value field, body: {body}"
    );
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "message should be non-empty, body: {body}"
    );
    assert_eq!(
        body["persisted"].as_bool(),
        Some(true),
        "persisted should be true, body: {body}"
    );
    assert_eq!(
        body["requires_reload"].as_bool(),
        Some(true),
        "requires_reload should be true when persist:true, body: {body}"
    );

    // persist:false → persisted=false, requires_reload=false
    let (status, body) = post_set_config(&client, &addr, "model", "deepseek-v4-pro", false).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["key"].as_str(),
        Some("model"),
        "key field, body: {body}"
    );
    assert_eq!(
        body["value"].as_str(),
        Some("deepseek-v4-pro"),
        "value field, body: {body}"
    );
    assert_eq!(
        body["persisted"].as_bool(),
        Some(false),
        "persisted should be false, body: {body}"
    );
    assert_eq!(
        body["requires_reload"].as_bool(),
        Some(false),
        "requires_reload should be false when persist:false, body: {body}"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn cors_layer_advertises_exact_supported_headers_and_never_an_extra() -> Result<()> {
    let layer = cors_layer(&[]);
    let router: Router = Router::new()
        .route("/probe", get(|| async { "ok" }))
        .layer(layer);

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let client = crate::tls::reqwest_client();

    let allowed = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/probe"))
        .header("Origin", "http://localhost:1420")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization, content-type, accept, x-codewhale-runtime-token, x-deepseek-runtime-token",
        )
        .send()
        .await?;

    assert!(allowed.status().is_success());
    assert_eq!(
        allowed
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:1420")
    );
    let allow_headers = allowed
        .headers()
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let advertised = allow_headers
        .split(',')
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        "accept",
        "authorization",
        "content-type",
        "x-codewhale-runtime-token",
        "x-deepseek-runtime-token",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(advertised, expected);

    let unapproved = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/probe"))
        .header("Origin", "http://localhost:1420")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization, x-malicious-header",
        )
        .send()
        .await?;
    let unapproved_headers = unapproved
        .headers()
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        !unapproved_headers.contains("x-malicious-header"),
        "an unapproved request header must never be advertised to the browser"
    );

    handle.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// Goal-loop endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thread_goal_crud_and_invalid_transition() -> Result<()> {
    let Some((addr, runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Create a thread to associate goals with.
    let thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = thread["id"].as_str().expect("thread id").to_string();

    // GET before any goal exists → 404.
    let no_goal = client
        .get(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .send()
        .await?;
    assert_eq!(no_goal.status(), 404, "no goal yet");

    // PUT creates the goal (201).
    let created: serde_json::Value = client
        .put(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .json(&json!({"objective": "write tests", "token_budget": 5000}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        created["status"].as_str(),
        Some("active"),
        "new goal is active"
    );
    assert_eq!(
        created["objective"].as_str(),
        Some("write tests"),
        "objective round-trips"
    );
    assert_eq!(created["token_budget"], 5000, "budget round-trips");

    // GET after create → 200 with the same fields.
    let fetched: serde_json::Value = client
        .get(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(fetched["objective"], created["objective"]);
    assert_eq!(fetched["goal_id"], created["goal_id"]);

    // Simulate progress on the first goal before replacing it.
    let mut accrued: codewhale_protocol::ThreadGoal = serde_json::from_value(created.clone())?;
    accrued.tokens_used = 1_234;
    accrued.time_used_seconds = 45;
    accrued.continuation_count = 3;
    accrued.created_at -= 60;
    runtime_threads.save_goal(accrued.clone()).await?;

    // PUT again (replacement) → 200 with a fresh goal lifecycle.
    let updated: serde_json::Value = client
        .put(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .json(&json!({"objective": "write even better tests"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        updated["objective"].as_str(),
        Some("write even better tests")
    );
    assert_ne!(updated["goal_id"], created["goal_id"]);
    assert!(
        updated["goal_id"]
            .as_str()
            .is_some_and(|goal_id| goal_id.starts_with("goal-"))
    );
    assert_eq!(updated["status"].as_str(), Some("active"));
    assert_eq!(updated["tokens_used"], 0);
    assert_eq!(updated["time_used_seconds"], 0);
    assert_eq!(updated["continuation_count"], 0);
    assert_eq!(updated["token_budget"], serde_json::Value::Null);
    assert!(updated["created_at"].as_i64() > Some(accrued.created_at));
    assert_eq!(updated["created_at"], updated["updated_at"]);

    // POST /complete → 200 with status = complete.
    let completed: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/goal/complete"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(completed["status"].as_str(), Some("complete"));

    // POST /complete again on a terminal goal → 409 Conflict.
    let double_complete = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/goal/complete"
        ))
        .send()
        .await?;
    assert_eq!(
        double_complete.status(),
        409,
        "completing an already-complete goal must be 409"
    );

    // POST /block on a terminal goal → 409 Conflict.
    let block_after_complete = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/goal/block"))
        .send()
        .await?;
    assert_eq!(
        block_after_complete.status(),
        409,
        "blocking a complete goal must be 409"
    );

    // DELETE → 204.
    let deleted = client
        .delete(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .send()
        .await?;
    assert_eq!(deleted.status(), 204, "delete returns 204");

    // DELETE again → 404.
    let double_delete = client
        .delete(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .send()
        .await?;
    assert_eq!(double_delete.status(), 404, "second delete is 404");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn thread_goal_block_transition() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let thread: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let thread_id = thread["id"].as_str().expect("thread id").to_string();

    // Create an active goal.
    client
        .put(format!("http://{addr}/v1/threads/{thread_id}/goal"))
        .json(&json!({"objective": "block me"}))
        .send()
        .await?
        .error_for_status()?;

    // Block it.
    let blocked: serde_json::Value = client
        .post(format!("http://{addr}/v1/threads/{thread_id}/goal/block"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(blocked["status"].as_str(), Some("blocked"));

    // Can still complete from blocked state.
    let completed: serde_json::Value = client
        .post(format!(
            "http://{addr}/v1/threads/{thread_id}/goal/complete"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(completed["status"].as_str(), Some("complete"));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn thread_goal_on_unknown_thread_returns_404() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .get(format!("http://{addr}/v1/threads/nonexistent-id/goal"))
        .send()
        .await?;
    assert_eq!(resp.status(), 404);

    let put_resp = client
        .put(format!("http://{addr}/v1/threads/nonexistent-id/goal"))
        .json(&json!({"objective": "ghost"}))
        .send()
        .await?;
    assert_eq!(put_resp.status(), 404);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn runtime_info_advertises_thread_goals_capability() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        info["capabilities"]["thread_goals"].as_bool(),
        Some(true),
        "runtime info must advertise thread_goals capability"
    );

    handle.abort();
    Ok(())
}

#[test]
fn fleet_receipt_json_pass_result_has_no_failure_fields() {
    use codewhale_protocol::fleet::{FleetReceipt, FleetRunId, FleetTaskResult};
    let receipt = FleetReceipt {
        run_id: FleetRunId::from("run-1"),
        task_id: "task-a".to_string(),
        worker_id: "worker-1".to_string(),
        attempt: Some(1),
        terminal_seq: Some(42),
        completed_at: "2025-01-01T00:00:00Z".to_string(),
        result: FleetTaskResult::Pass,
        failure_kind: None,
        artifacts: Vec::new(),
        score: None,
        resolved_route: None,
        effective_permissions: None,
    };
    let value = fleet_receipt_json(&receipt);
    assert_eq!(value["run_id"], "run-1");
    assert_eq!(value["task_id"], "task-a");
    assert_eq!(value["worker_id"], "worker-1");
    assert_eq!(value["attempt"], 1);
    assert_eq!(value["terminal_seq"], 42);
    assert_eq!(value["result"], "pass");
    assert!(value["failure_kind"].is_null());
    assert!(value["failure_class"].is_null());
    assert_eq!(value["retry_eligible"], false);
    assert_eq!(value["evidence_available"], false);
    assert!(value["score"].is_null());
}

#[test]
fn fleet_receipt_json_verifier_failure_is_not_retry_eligible() {
    use codewhale_protocol::fleet::{
        FleetReceipt, FleetRunId, FleetTaskFailureKind, FleetTaskResult,
    };
    let receipt = FleetReceipt {
        run_id: FleetRunId::from("run-v"),
        task_id: "task-v".to_string(),
        worker_id: "worker-v".to_string(),
        attempt: Some(1),
        terminal_seq: None,
        completed_at: "2025-01-01T00:00:00Z".to_string(),
        result: FleetTaskResult::Fail,
        failure_kind: Some(FleetTaskFailureKind::Verifier),
        artifacts: Vec::new(),
        score: None,
        resolved_route: None,
        effective_permissions: None,
    };
    let value = fleet_receipt_json(&receipt);
    assert_eq!(value["result"], "fail");
    assert_eq!(value["failure_kind"], "verifier");
    assert_eq!(value["retry_eligible"], false);
    assert!(
        value["failure_class"]
            .as_str()
            .is_some_and(|s| s.contains("Verifier")),
        "failure_class should describe a verifier rejection"
    );
}

#[test]
fn fleet_receipt_json_transport_failure_is_retry_eligible() {
    use codewhale_protocol::fleet::{
        FleetReceipt, FleetRunId, FleetTaskFailureKind, FleetTaskResult,
    };
    let receipt = FleetReceipt {
        run_id: FleetRunId::from("run-t"),
        task_id: "task-t".to_string(),
        worker_id: "worker-t".to_string(),
        attempt: Some(1),
        terminal_seq: None,
        completed_at: "2025-01-01T00:00:00Z".to_string(),
        result: FleetTaskResult::Fail,
        failure_kind: Some(FleetTaskFailureKind::Transport),
        artifacts: Vec::new(),
        score: None,
        resolved_route: None,
        effective_permissions: None,
    };
    let value = fleet_receipt_json(&receipt);
    assert_eq!(value["failure_kind"], "transport");
    assert_eq!(value["retry_eligible"], true);
}

#[test]
fn fleet_receipt_json_receipt_artifact_sets_evidence_available() {
    use codewhale_protocol::fleet::{
        FleetArtifactKind, FleetArtifactRef, FleetReceipt, FleetRunId, FleetScore, FleetTaskResult,
    };
    use std::path::PathBuf;
    let receipt = FleetReceipt {
        run_id: FleetRunId::from("run-e"),
        task_id: "task-e".to_string(),
        worker_id: "worker-e".to_string(),
        attempt: Some(1),
        terminal_seq: None,
        completed_at: "2025-01-01T00:00:00Z".to_string(),
        result: FleetTaskResult::Pass,
        failure_kind: None,
        artifacts: vec![FleetArtifactRef {
            kind: FleetArtifactKind::Receipt,
            path: PathBuf::from(".codewhale/fleet/run-e/task-e/worker-e/receipt.json"),
            checksum: Some("sha256:abc123".to_string()),
            mime_type: Some("application/json".to_string()),
            size_bytes: Some(512),
        }],
        score: Some(FleetScore {
            value: 1.0,
            max: Some(1.0),
            notes: Some("all checks pass".to_string()),
        }),
        resolved_route: None,
        effective_permissions: None,
    };
    let value = fleet_receipt_json(&receipt);
    assert_eq!(value["evidence_available"], true);
    assert_eq!(value["score"]["value"], 1.0);
    assert_eq!(value["score"]["max"], 1.0);
    assert_eq!(value["score"]["notes"], "all checks pass");
    assert_eq!(value["artifacts"][0]["kind"], "receipt");
}

#[tokio::test]
async fn fleet_receipt_api_list_and_get_round_trip() -> Result<()> {
    use crate::fleet::ledger::FleetLedger;
    use crate::fleet::task_spec::FleetTaskVerificationInput;
    use crate::fleet::task_spec::{
        FleetTaskSpecDocument, FleetTaskVerification, prepare_verification_receipt,
    };
    use codewhale_protocol::fleet::{FleetScore, FleetTaskResult};

    let root = std::env::temp_dir().join(format!("codewhale-receipt-api-{}", Uuid::new_v4()));
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace)?;

    let task = codewhale_protocol::fleet::FleetTaskSpec {
        id: "task-receipt".to_string(),
        name: "Receipt Task".to_string(),
        description: None,
        objective: Some("Test receipt API".to_string()),
        instructions: "run tests".to_string(),
        worker: Some(codewhale_protocol::fleet::FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: Vec::new(),
            capabilities: Vec::new(),
        }),
        workspace: None,
        input_files: Vec::new(),
        context: Vec::new(),
        budget: None,
        tags: Vec::new(),
        expected_artifacts: Vec::new(),
        scorer: None,
        retry_policy: None,
        alert_policy: None,
        timeout_seconds: None,
        metadata: std::collections::BTreeMap::new(),
    };
    let manager = crate::fleet::manager::FleetManager::open(&workspace)?
        .with_session_model(crate::config::DEFAULT_TEXT_MODEL)
        .with_route_config(test_fleet_route_config());
    let report = manager.create_run(
        FleetTaskSpecDocument {
            name: Some("receipt-api-smoke".to_string()),
            labels: std::collections::BTreeMap::new(),
            security_policy: None,
            workers: Vec::new(),
            tasks: vec![task],
            usage_ceiling: None,
        },
        1,
    )?;
    let run_id = report.run_id.clone();

    // Directly record a synthetic receipt so we don't need a live worker.
    let ledger = FleetLedger::open(&workspace)?;
    let verification_input = FleetTaskVerificationInput {
        run_id: run_id.clone(),
        task_id: "task-receipt".to_string(),
        worker_id: "worker-1".to_string(),
        attempt: 1,
        exit_code: Some(0),
        artifacts: Vec::new(),
        resolved_route: None,
        effective_permissions: None,
    };
    let receipt = prepare_verification_receipt(
        &workspace,
        &verification_input,
        FleetTaskVerification {
            result: FleetTaskResult::Pass,
            failure_kind: None,
            score: FleetScore {
                value: 1.0,
                max: Some(1.0),
                notes: Some("exit_code=0".to_string()),
            },
            evidence: vec!["exit_code=0".to_string()],
        },
    )?;
    ledger.record_receipt(receipt)?;

    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root.clone(),
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // List receipts for the run.
    let list: serde_json::Value = client
        .get(format!("http://{addr}/v1/fleet/runs/{}/receipts", run_id.0))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(list["run_id"], run_id.0.as_str());
    assert_eq!(list["receipts"].as_array().map(|a| a.len()), Some(1));
    let receipt_entry = &list["receipts"][0];
    assert_eq!(receipt_entry["task_id"], "task-receipt");
    assert_eq!(receipt_entry["result"], "pass");
    assert_eq!(receipt_entry["retry_eligible"], false);
    assert_eq!(receipt_entry["evidence_available"], true);

    // Get specific receipt by task_id.
    let detail: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/fleet/runs/{}/receipts/task-receipt",
            run_id.0
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["run_id"], run_id.0.as_str());
    assert_eq!(detail["task_id"], "task-receipt");
    assert_eq!(detail["worker_id"], "worker-1");
    assert_eq!(detail["attempt"], 1);
    assert_eq!(detail["result"], "pass");
    assert!(detail["failure_kind"].is_null());
    assert_eq!(detail["evidence_available"], true);

    // Inspect evidence content.
    let evidence: serde_json::Value = client
        .get(format!(
            "http://{addr}/v1/fleet/runs/{}/receipts/task-receipt/evidence",
            run_id.0
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(evidence["run_id"], run_id.0.as_str());
    assert_eq!(evidence["task_id"], "task-receipt");
    assert_eq!(evidence["truncated"], false);
    assert!(
        evidence["content"].is_object(),
        "evidence content should parse as JSON object"
    );
    assert_eq!(evidence["content"]["task_id"], "task-receipt");

    // Missing task returns 404.
    let missing = client
        .get(format!(
            "http://{addr}/v1/fleet/runs/{}/receipts/no-such-task",
            run_id.0
        ))
        .send()
        .await?;
    assert_eq!(missing.status(), 404);

    handle.abort();
    Ok(())
}

// ── Memory API tests ──

#[tokio::test]
async fn memory_info_capability_is_advertised() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        info["capabilities"]["memory"], true,
        "memory capability must be advertised in runtime/info"
    );
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_list_returns_empty_for_fresh_store() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-list-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root, sessions_dir).await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let body: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(body["entries"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["total"], 0);

    // scope=global should also be empty.
    let global: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory?scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(global["total"], 0);

    // Invalid scope returns 400.
    let bad = client
        .get(format!("http://{addr}/v1/memory?scope=invalid"))
        .send()
        .await?;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    // limit=0 returns 400.
    let bad_limit = client
        .get(format!("http://{addr}/v1/memory?limit=0"))
        .send()
        .await?;
    assert_eq!(bad_limit.status(), StatusCode::BAD_REQUEST);

    // limit above max returns 400.
    let over_limit = client
        .get(format!("http://{addr}/v1/memory?limit=201"))
        .send()
        .await?;
    assert_eq!(over_limit.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_create_list_and_get_entry() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-create-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Create a global memory entry.
    let create_resp = client
        .post(format!("http://{addr}/v1/memory"))
        .json(&json!({ "text": "prefer snake_case for identifiers", "scope": "global" }))
        .send()
        .await?;
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json().await?;
    assert_eq!(created["entry"]["scope"], "global");
    assert_eq!(created["entry"]["status"], "active");
    assert!(created["entry"]["id"].is_number());
    assert!(
        created["entry"]["summary"]
            .as_str()
            .unwrap_or("")
            .contains("snake_case"),
        "summary must include the note text"
    );

    let entry_id = created["entry"]["id"].as_i64().unwrap();

    // GET /v1/memory lists the entry.
    let list: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory?scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(list["total"], 1);
    assert_eq!(list["entries"][0]["id"], entry_id);
    assert_eq!(list["entries"][0]["scope"], "global");
    // workspace_id must be absent for global entries.
    assert!(list["entries"][0]["workspace_id"].is_null());

    // GET /v1/memory/{id} returns the entry.
    let single: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory/{entry_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(single["entry"]["id"], entry_id);
    assert_eq!(single["entry"]["scope"], "global");
    assert!(single["entry"]["stale"].is_boolean());

    // GET /v1/memory/{id} for a missing id returns 404.
    let missing = client
        .get(format!("http://{addr}/v1/memory/999999"))
        .send()
        .await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_summary_is_redacted_to_max_chars() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-redact-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let long_note = "x".repeat(350);
    let create_resp = client
        .post(format!("http://{addr}/v1/memory"))
        .json(&json!({ "text": long_note, "scope": "global" }))
        .send()
        .await?;
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json().await?;
    let summary = created["entry"]["summary"].as_str().unwrap_or("");
    // Summary must be bounded: at most 300 chars + 3 for the "…" suffix.
    assert!(
        summary.chars().count() <= 303,
        "summary must be bounded; got {} chars",
        summary.chars().count()
    );
    assert!(
        summary.ends_with("…") || summary.chars().count() <= 300,
        "overlong text must be truncated with an ellipsis"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_clear_removes_global_scope() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-clear-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Seed two entries.
    for note in ["first note", "second note"] {
        client
            .post(format!("http://{addr}/v1/memory"))
            .json(&json!({ "text": note, "scope": "global" }))
            .send()
            .await?
            .error_for_status()?;
    }

    let before: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory?scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        before["total"], 2,
        "seed entries must be present before clear"
    );

    // Clear global scope.
    let clear: serde_json::Value = client
        .delete(format!("http://{addr}/v1/memory?scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(clear["cleared"], true);

    let after: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory?scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(after["total"], 0, "global scope must be empty after clear");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_create_rejects_empty_text_and_bad_scope() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-invalid-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Empty text must be rejected with 400.
    let empty = client
        .post(format!("http://{addr}/v1/memory"))
        .json(&json!({ "text": "", "scope": "global" }))
        .send()
        .await?;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    // An unknown scope must be rejected with 400.
    let bad_scope = client
        .post(format!("http://{addr}/v1/memory"))
        .json(&json!({ "text": "valid note", "scope": "thread" }))
        .send()
        .await?;
    assert_eq!(bad_scope.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn memory_search_query_filters_results() -> Result<()> {
    let root = std::env::temp_dir().join(format!("cw-memory-search-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let _lock = lock_test_env();
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
    let Some((addr, _rt, handle)) = spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Seed two entries with distinct text.
    for note in ["prefer functional style", "always use snake_case"] {
        client
            .post(format!("http://{addr}/v1/memory"))
            .json(&json!({ "text": note, "scope": "global" }))
            .send()
            .await?
            .error_for_status()?;
    }

    // Searching for "functional" returns only the matching entry.
    let resp: serde_json::Value = client
        .get(format!("http://{addr}/v1/memory?q=functional&scope=global"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(resp["total"], 1);
    let summary = resp["entries"][0]["summary"].as_str().unwrap_or("");
    assert!(
        summary.contains("functional"),
        "search must return the matching entry"
    );

    // An empty q must be rejected with 400.
    let empty_q = client
        .get(format!("http://{addr}/v1/memory?q="))
        .send()
        .await?;
    assert_eq!(empty_q.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mcp_server_management_crud() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-mcp-mgmt-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let base = format!("http://{addr}/v1/apps/mcp/servers");

    // 1. Create a new server.
    let created: serde_json::Value = client
        .post(&base)
        .json(&serde_json::json!({
            "name": "test-stdio",
            "command": "echo",
            "args": ["hello"],
            "url": "https://example.com/mcp",
            "transport": "sse",
            "connect_timeout": 11,
            "execute_timeout": 22,
            "read_timeout": 33,
            "bearer_token_env_var": "MCP_TOKEN",
            "oauth_resource": "https://example.com/resource",
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(created["name"], "test-stdio");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["command"], "echo");

    // 2. GET the server back — config should be on disk.
    let fetched: serde_json::Value = client
        .get(format!("{base}/test-stdio"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(fetched["name"], "test-stdio");
    assert_eq!(fetched["command"], "echo");

    // 3. Duplicate create returns 409 Conflict.
    let conflict = client
        .post(&base)
        .json(&serde_json::json!({
            "name": "test-stdio",
            "command": "echo",
        }))
        .send()
        .await?;
    assert_eq!(conflict.status(), 409);

    // 4. PATCH (update) the server.
    let updated: serde_json::Value = client
        .patch(format!("{base}/test-stdio"))
        .json(&serde_json::json!({
            "args": ["updated"],
            "required": true,
            "url": null,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(updated["required"], true);
    assert_eq!(updated["args"][0], "updated");
    assert_eq!(updated["url"], serde_json::Value::Null);
    assert_eq!(updated["transport"], "sse", "missing fields are retained");

    let cleared: serde_json::Value = client
        .patch(format!("{base}/test-stdio"))
        .json(&json!({
            "url": "https://example.com/replacement",
            "command": null,
            "transport": null,
            "connect_timeout": null,
            "execute_timeout": null,
            "read_timeout": null,
            "bearer_token_env_var": null,
            "oauth_resource": null
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for field in [
        "command",
        "transport",
        "connect_timeout",
        "execute_timeout",
        "read_timeout",
        "oauth_resource",
    ] {
        assert_eq!(cleared[field], serde_json::Value::Null, "{field}");
    }
    assert_eq!(cleared["url"], "https://example.com/replacement");
    assert_eq!(cleared["has_bearer_token_env_var"], false);

    // Clearing the final endpoint is invalid and must not alter persisted state.
    let invalid = client
        .patch(format!("{base}/test-stdio"))
        .json(&json!({ "url": null }))
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let retained: serde_json::Value = client
        .get(format!("{base}/test-stdio"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(retained["url"], "https://example.com/replacement");

    // 5. Disable the server.
    let disabled: serde_json::Value = client
        .post(format!("{base}/test-stdio/disable"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(disabled["action"], "disabled");
    assert_eq!(disabled["ok"], true);

    // Verify disabled state via GET.
    let after_disable: serde_json::Value = client
        .get(format!("{base}/test-stdio"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(after_disable["enabled"], false);

    // 6. Re-enable the server.
    let enabled: serde_json::Value = client
        .post(format!("{base}/test-stdio/enable"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(enabled["action"], "enabled");
    assert_eq!(enabled["ok"], true);

    // 7. Reconnect (schedules a reconnect — no live pool present so always succeeds).
    let reconnected: serde_json::Value = client
        .post(format!("{base}/test-stdio/reconnect"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(reconnected["action"], "reconnect_scheduled");
    assert_eq!(reconnected["ok"], true);

    // 8. Delete the server.
    let deleted: serde_json::Value = client
        .delete(format!("{base}/test-stdio"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(deleted["action"], "deleted");
    assert_eq!(deleted["ok"], true);

    // 9. GET after delete returns 404.
    let not_found = client.get(format!("{base}/test-stdio")).send().await?;
    assert_eq!(not_found.status(), 404);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mcp_server_management_create_requires_command_or_url() -> Result<()> {
    let root =
        std::env::temp_dir().join(format!("codewhale-mcp-mgmt-validation-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Missing both command and url → 400.
    let resp = client
        .post(format!("http://{addr}/v1/apps/mcp/servers"))
        .json(&serde_json::json!({ "name": "bad-server" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 400);

    // Missing name → 400.
    let resp = client
        .post(format!("http://{addr}/v1/apps/mcp/servers"))
        .json(&serde_json::json!({ "command": "echo" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 400);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn mcp_server_management_redacts_credentials() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-mcp-mgmt-redact-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root.clone(), sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let base = format!("http://{addr}/v1/apps/mcp/servers");

    // Create a server with sensitive fields.
    let created: serde_json::Value = client
        .post(&base)
        .json(&serde_json::json!({
            "name": "secret-server",
            "url": "https://example.com/mcp",
            "env_headers": { "Authorization": "MY_SECRET_ENV_VAR" },
            "bearer_token_env_var": "MY_BEARER_ENV",
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // The response must NOT contain the actual header value or env variable value.
    assert!(
        !created.to_string().contains("MY_SECRET_ENV_VAR_VALUE"),
        "credential values must be redacted from API responses"
    );
    // env_header_keys should list the header name (not the env-var value).
    assert_eq!(
        created["env_header_keys"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some("Authorization"))),
        Some(true),
        "env_header_keys should list header names"
    );
    // has_bearer_token_env_var should be true, not the env var name.
    assert_eq!(created["has_bearer_token_env_var"], true);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn runtime_info_advertises_mcp_server_management() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-mcp-capability-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root(root, sessions_dir).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    assert_eq!(
        info["capabilities"]["mcp_server_management"], true,
        "runtime/info must advertise mcp_server_management capability"
    );

    handle.abort();
    Ok(())
}

// ─── Skill lifecycle API tests ──────────────────────────────────────────────

/// Create a minimal skill package under `root_dir/.codewhale/skills/<name>`.
/// Returns the skill dir path plus a digest that can be used in requests.
fn create_managed_skill(root_dir: &std::path::Path, name: &str) -> Result<(PathBuf, String)> {
    let skill_dir = root_dir.join(".codewhale").join("skills").join(name);
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n---\nbody\n"),
    )?;
    // Write an `.installed-from` marker so the mutation module considers it
    // managed (and therefore eligible for update/remove/trust).
    let digest = crate::skills::audit::compute_package_digest(&skill_dir).expect("package digest");
    crate::skills::install::write_installed_from_v2(
        &skill_dir,
        &format!("github:test/{name}"),
        None,
        "sha256:test",
        &digest,
        name,
    )?;
    Ok((skill_dir, digest))
}

#[tokio::test]
async fn skill_lifecycle_uninstall_removes_installed_skill() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    create_managed_skill(&workspace, "hello")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root,
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Confirm skill is visible.
    let list: serde_json::Value = client
        .get(format!("http://{addr}/v1/skills"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        list["skills"]
            .as_array()
            .is_some_and(|s| s.iter().any(|sk| sk["name"] == "hello")),
        "hello skill must appear in GET /v1/skills before uninstall"
    );

    // Uninstall it.
    let resp = client
        .delete(format!("http://{addr}/v1/skills/hello"))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(resp["outcome"], "removed");
    assert_eq!(resp["name"], "hello");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_uninstall_404s_for_unknown_skill() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .delete(format!("http://{addr}/v1/skills/no-such-skill"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_uninstall_rejects_invalid_scope() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .delete(format!("http://{addr}/v1/skills/hello?scope=badscope"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_trust_marks_installed_skill() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    create_managed_skill(&workspace, "trustme")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root,
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/trustme/trust"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(resp["outcome"], "trusted");
    assert_eq!(resp["name"], "trustme");
    // The trust note must be present and must carry the advisory wording.
    let trust_note = resp["trust_note"].as_str().expect("trust_note");
    assert!(
        trust_note.contains("advisory") && trust_note.contains("digest-bound"),
        "trust_note must preserve advisory and digest-bound wording"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_trust_404s_for_unknown_skill() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/no-such/trust"))
        .json(&json!({}))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_trust_rejects_invalid_scope() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/hello/trust"))
        .json(&json!({ "scope": "invalid" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_trust_rejects_digest_drift() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    create_managed_skill(&workspace, "drifted")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root,
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // Pass a deliberately wrong digest to confirm drift detection.
    let resp = client
        .post(format!("http://{addr}/v1/skills/drifted/trust"))
        .json(&json!({ "expected_digest": "sha256:definitely_wrong_digest" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_audit_returns_receipt_for_installed_skill() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().to_path_buf();
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&root)?;

    create_managed_skill(&workspace, "auditable")?;

    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace(
            root,
            sessions_dir,
            None,
            false,
            workspace,
        )
        .await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp: serde_json::Value = client
        .get(format!("http://{addr}/v1/skills/auditable/audit"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(resp["ambiguous"], false);
    let skills = resp["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1);
    let entry = &skills[0];
    assert_eq!(entry["name"], "auditable");
    assert_eq!(entry["source_kind"], "codewhale_managed");
    // Digest must be known for a properly written managed skill.
    assert_eq!(entry["digest"]["state"], "known");
    assert!(
        entry["digest"]["value"].as_str().is_some(),
        "digest.value must be present when state=known"
    );
    // Trust state: untrusted because we haven't run trust.
    assert_eq!(entry["trust"], "untrusted");

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_audit_404s_for_unknown_skill() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .get(format!("http://{addr}/v1/skills/no-such-skill/audit"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_audit_rejects_invalid_scope() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .get(format!("http://{addr}/v1/skills/hello/audit?scope=nope"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_install_rejects_empty_source() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/install"))
        .json(&json!({ "source": "  " }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_install_rejects_invalid_scope() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/install"))
        .json(&json!({ "source": "github:owner/repo", "scope": "badscope" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_update_rejects_invalid_scope() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/skills/hello/update"))
        .json(&json!({ "scope": "wrong" }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_endpoints_require_auth_when_token_is_set() -> Result<()> {
    let root = std::env::temp_dir().join(format!("codewhale-skill-auth-{}", Uuid::new_v4()));
    let sessions_dir = root.join("sessions");
    let token = "skill-lifecycle-test-token".to_string();
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_and_token(root, sessions_dir, Some(token)).await?
    else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    // All skill lifecycle endpoints must require auth.
    for (method, path) in &[
        ("GET", "/v1/skills/any/audit"),
        ("POST", "/v1/skills/install"),
        ("POST", "/v1/skills/any/update"),
        ("DELETE", "/v1/skills/any"),
        ("POST", "/v1/skills/any/trust"),
    ] {
        let resp = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("http://{addr}{path}"),
            )
            .json(&json!({}))
            .send()
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path} must require auth"
        );
    }

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn skill_lifecycle_runtime_info_advertises_skill_lifecycle_capability() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        info["capabilities"]["skill_lifecycle"], true,
        "runtime/info must advertise skill_lifecycle capability"
    );

    handle.abort();
    Ok(())
}

#[test]
fn receipt_evidence_paths_must_stay_confined_to_the_workspace() {
    use std::path::Path;

    assert!(super::receipt_evidence_path_is_confined(Path::new(
        "receipts/run-1/task-a.json"
    )));
    assert!(!super::receipt_evidence_path_is_confined(Path::new(
        "/etc/passwd"
    )));
    assert!(!super::receipt_evidence_path_is_confined(Path::new(
        "receipts/../../escape.json"
    )));
    assert!(!super::receipt_evidence_path_is_confined(Path::new(
        "../escape.json"
    )));
}

// ─── Plugin management API tests ────────────────────────────────────────────

/// Write a minimal but real plugin bundle: manifest plus one skill, so the
/// bundle has a supported declarative component and can be trusted+enabled.
fn write_plugin_source_bundle(dir: &Path, name: &str) -> Result<PathBuf> {
    let bundle = dir.join(name);
    fs::create_dir_all(bundle.join("skills").join("greet"))?;
    fs::write(
        bundle.join("plugin.toml"),
        format!(
            "schema_version = 1\n[plugin]\nname = \"{name}\"\nversion = \"1.0.0\"\n\
             description = \"test bundle\"\n\n[skills]\npath = \"skills\"\n"
        ),
    )?;
    fs::write(
        bundle.join("skills").join("greet").join("SKILL.md"),
        "---\nname: greet\ndescription: Greets a person\n---\nSay hi.\n",
    )?;
    Ok(bundle)
}

/// An isolated plugin discovery context: user plugins, workspace plugins,
/// state, and marketplaces all live under the test root instead of the real
/// `~/.codewhale`.
fn isolated_plugin_discovery(
    root: &Path,
    workspace: &Path,
) -> Arc<crate::plugins::PluginDiscoveryContext> {
    crate::plugins::PluginDiscoveryContext::from_config_and_environment(
        &crate::plugins::discovery::DiscoveryConfig {
            workspace: workspace.to_path_buf(),
            user_plugins_dir: root.join("plugins"),
            workspace_plugins_dir: crate::plugins::discovery::default_workspace_plugins_dir(
                workspace,
            ),
            builtin_plugin_dirs: Vec::new(),
            state_path: root.join("plugins").join("state.json"),
        },
        crate::plugins::HostEnvironment::from_entries(Vec::new()),
    )
}

async fn spawn_plugin_api_server(
    root: PathBuf,
    workspace: PathBuf,
) -> Result<Option<(SocketAddr, tokio::task::JoinHandle<()>)>> {
    let Some((addr, _runtime_threads, handle)) =
        spawn_test_server_with_root_token_mobile_workspace_and_overrides(
            root.clone(),
            root.join("sessions"),
            None,
            false,
            workspace.clone(),
            TestServerOverrides {
                plugin_discovery: Some(isolated_plugin_discovery(&root, &workspace)),
                ..TestServerOverrides::default()
            },
        )
        .await?
    else {
        return Ok(None);
    };
    Ok(Some((addr, handle)))
}

#[tokio::test]
async fn runtime_info_advertises_plugin_management_capability() -> Result<()> {
    let Some((addr, _runtime_threads, handle)) = spawn_test_server().await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let info: serde_json::Value = client
        .get(format!("http://{addr}/v1/runtime/info"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        info["capabilities"]["plugin_management"], true,
        "runtime/info must advertise plugin_management capability"
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn plugin_lifecycle_over_http_installs_reviews_enables_and_uninstalls() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&root)?;
    let source = write_plugin_source_bundle(tmp.path(), "demo")?;

    let Some((addr, handle)) = spawn_plugin_api_server(root, workspace).await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let base = format!("http://{addr}/v1/apps/plugins");

    // Empty inventory to start.
    let list: serde_json::Value = client
        .get(&base)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(list["plugins"].as_array().is_some_and(Vec::is_empty));

    // Install from a local path: lands disabled and untrusted.
    let install: serde_json::Value = client
        .post(format!("{base}/install"))
        .json(&serde_json::json!({
            "source": format!("path:{}", source.display())
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(install["outcome"], "installed");
    assert_eq!(install["name"], "demo");
    assert_eq!(install["plugin"]["state"], "disabled");
    assert_eq!(install["plugin"]["trust_status"], "not-reviewed");
    assert!(
        install["note"]
            .as_str()
            .is_some_and(|n| n.contains("untrusted")),
        "install note must route to review: {}",
        install["note"]
    );

    // Detail carries the structured review payload and token. The summary is
    // flattened into the detail object.
    let detail: serde_json::Value = client
        .get(format!("{base}/demo"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail["inventory"]["skills"], 1);
    assert_eq!(detail["review"]["token"], {
        let summary = &detail;
        format!(
            "{}.{}",
            summary["content_hash"].as_str().unwrap(),
            summary["capability_hash"].as_str().unwrap()
        )
    });
    assert!(
        detail["review"]["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|label| label == "skills"))
    );

    // Trust requires the exact review token.
    let wrong = client
        .post(format!("{base}/demo/trust"))
        .json(&serde_json::json!({"token": "not-the-token"}))
        .send()
        .await?;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);

    let token = detail["review"]["token"].as_str().unwrap().to_string();
    let trusted: serde_json::Value = client
        .post(format!("{base}/demo/trust"))
        .json(&serde_json::json!({"token": token}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(trusted["action"], "trusted");
    assert_eq!(trusted["state"], "disabled");

    // Enable only works after trust; the bundle becomes active.
    let enabled: serde_json::Value = client
        .post(format!("{base}/demo/enable"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(enabled["action"], "enabled");
    assert_eq!(enabled["state"], "active");

    let disabled: serde_json::Value = client
        .post(format!("{base}/demo/disable"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(disabled["action"], "disabled");
    assert_eq!(disabled["state"], "disabled");

    // Uninstall removes it.
    let uninstalled: serde_json::Value = client
        .delete(format!("{base}/demo"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(uninstalled["outcome"], "uninstalled");

    let list: serde_json::Value = client
        .get(&base)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(list["plugins"].as_array().is_some_and(Vec::is_empty));

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn plugin_api_404s_for_unknown_selector() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&root)?;

    let Some((addr, handle)) = spawn_plugin_api_server(root, workspace).await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let detail = client
        .get(format!("http://{addr}/v1/apps/plugins/ghost"))
        .send()
        .await?;
    assert_eq!(detail.status(), StatusCode::NOT_FOUND);

    let trust = client
        .post(format!("http://{addr}/v1/apps/plugins/ghost/trust"))
        .json(&serde_json::json!({"token": "x"}))
        .send()
        .await?;
    assert_eq!(trust.status(), StatusCode::NOT_FOUND);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn marketplace_catalog_lifecycle_over_http_lists_installs_and_removes() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&root)?;

    // The catalog directory holds both the document and the bundle it points
    // at via a relative `./demo` source.
    let catalog_dir = tmp.path().join("catalog");
    fs::create_dir_all(&catalog_dir)?;
    write_plugin_source_bundle(&catalog_dir, "demo")?;
    let catalog_path = catalog_dir.join("catalog.json");
    fs::write(
        &catalog_path,
        r#"{
            "name": "team",
            "description": "Team plugins",
            "version": "1",
            "plugins": [
                {"name": "demo", "source": "path:./demo", "description": "Demo bundle"}
            ]
        }"#,
    )?;

    let Some((addr, handle)) = spawn_plugin_api_server(root, workspace).await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();
    let base = format!("http://{addr}/v1/apps/marketplaces");

    let add_resp = client
        .post(&base)
        .json(&serde_json::json!({
            "name": "team",
            "path": catalog_path.display().to_string()
        }))
        .send()
        .await?;
    let add: serde_json::Value = add_resp.json().await?;
    assert!(add["action"] == "added", "marketplace add failed: {add}");
    assert_eq!(add["candidate_count"], 1);

    // Listing shows the candidate with an honest, resolved install plan.
    let list: serde_json::Value = client
        .get(&base)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let candidate = &list["marketplaces"][0]["candidates"][0];
    assert_eq!(candidate["name"], "demo");
    assert_eq!(candidate["install"]["installable"], true);
    assert!(
        candidate["install"]["spec"]
            .as_str()
            .is_some_and(|spec| spec.contains("demo")),
        "relative ./demo must resolve against the catalog directory: {}",
        candidate["install"]["spec"]
    );

    // Install through the marketplace routes into the reviewed installer and
    // lands disabled + untrusted, exactly like a direct install.
    let install: serde_json::Value = client
        .post(format!("{base}/team/install"))
        .json(&serde_json::json!({"candidate": "demo"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(install["outcome"], "installed");
    assert_eq!(install["plugin"]["trust_status"], "not-reviewed");

    // Unknown candidate and unknown catalog are honest 404s.
    let missing = client
        .post(format!("{base}/team/install"))
        .json(&serde_json::json!({"candidate": "ghost"}))
        .send()
        .await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // Removing the catalog never touches installed bundles.
    let plugins: serde_json::Value = client
        .get(format!("http://{addr}/v1/apps/plugins"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        plugins["plugins"]
            .as_array()
            .is_some_and(|p| p.iter().any(|entry| entry["name"] == "demo"))
    );

    let removed: serde_json::Value = client
        .delete(format!("{base}/team"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(removed["action"], "removed");

    let plugins_after: serde_json::Value = client
        .get(format!("http://{addr}/v1/apps/plugins"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        plugins_after["plugins"]
            .as_array()
            .is_some_and(|p| p.iter().any(|entry| entry["name"] == "demo"))
    );

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn marketplace_add_rejects_symlink_documents_over_http() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("runtime");
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&root)?;

    let catalog_dir = tmp.path().join("catalog");
    fs::create_dir_all(&catalog_dir)?;
    let real = catalog_dir.join("real.json");
    fs::write(&real, r#"{"plugins":[]}"#)?;
    #[cfg(unix)]
    let link = catalog_dir.join("link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link)?;
    #[cfg(not(unix))]
    let link = real;

    let Some((addr, handle)) = spawn_plugin_api_server(root, workspace).await? else {
        return Ok(());
    };
    let client = crate::tls::reqwest_client();

    let resp = client
        .post(format!("http://{addr}/v1/apps/marketplaces"))
        .json(&serde_json::json!({
            "name": "team",
            "path": link.display().to_string()
        }))
        .send()
        .await?;
    #[cfg(unix)]
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    #[cfg(not(unix))]
    assert!(resp.status().is_success());

    handle.abort();
    Ok(())
}
