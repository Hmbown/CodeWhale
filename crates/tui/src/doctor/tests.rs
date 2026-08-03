use super::*;
use clap::Parser as _;

#[test]
fn default_probe_request_is_fully_offline() {
    let request = DoctorProbeRequest::default();
    assert!(!request.should_check_updates());
    assert!(!request.should_probe_api(false));
    assert!(!request.should_probe_api(true));
    assert!(!request.should_probe_mcp());
}

#[test]
fn update_renderer_omits_untrusted_release_tags_and_errors() {
    let release_sentinel = "v9.9.9?token=doctor-update-sentinel";
    let error_sentinel = "https://user:doctor-update-error-sentinel@example.test/path";

    let metadata = doctor_update_report("0.9.3", Ok::<String, ()>(release_sentinel.to_string()));
    let transport = doctor_update_report("0.9.3", Err(error_sentinel.to_string()));
    let rendered = [
        doctor_update_report_lines(&metadata).join("\n"),
        doctor_update_report_lines(&transport).join("\n"),
    ]
    .join("\n");

    assert_eq!(metadata, DoctorUpdateReport::ReleaseMetadataInvalid);
    assert_eq!(transport, DoctorUpdateReport::ReleaseCheckFailed);
    assert!(!rendered.contains(release_sentinel));
    assert!(!rendered.contains(error_sentinel));
    assert!(rendered.contains("details omitted"));
}

#[test]
fn update_renderer_canonicalizes_safe_release_tags() {
    let report = doctor_update_report("0.9.3", Ok::<String, ()>(" v0.9.4 ".to_string()));
    assert_eq!(
        doctor_update_report_lines(&report),
        vec![
            "latest: v0.9.4".to_string(),
            "Update available. Run `codewhale update` to install.".to_string(),
        ]
    );
}

#[test]
fn live_probe_flags_open_only_their_owned_boundary() {
    let update = DoctorProbeRequest {
        check_updates: true,
        ..DoctorProbeRequest::default()
    };
    assert!(update.should_check_updates());
    assert!(!update.should_probe_api(false));
    assert!(!update.should_probe_api(true));

    let hosted = DoctorProbeRequest {
        probe_api: true,
        ..DoctorProbeRequest::default()
    };
    assert!(hosted.should_probe_api(false));
    assert!(!hosted.should_probe_api(true));

    let local = DoctorProbeRequest {
        probe_local: true,
        ..DoctorProbeRequest::default()
    };
    assert!(!local.should_probe_api(false));
    assert!(local.should_probe_api(true));
}

#[test]
fn cli_defaults_doctor_offline_and_keeps_json_incompatible_with_live_flags() {
    let cli =
        crate::Cli::try_parse_from(["codewhale-tui", "doctor"]).expect("parse default doctor");
    let Some(crate::Commands::Doctor(args)) = cli.command else {
        panic!("expected doctor command");
    };
    assert!(!args.check_updates);
    assert!(!args.probe_api);
    assert!(!args.probe_local);
    assert!(!args.probe_mcp);

    for flag in [
        "--check-updates",
        "--probe-api",
        "--probe-local",
        "--probe-mcp",
    ] {
        assert!(
            crate::Cli::try_parse_from(["codewhale-tui", "doctor", "--json", flag]).is_err(),
            "--json unexpectedly accepted live flag {flag}"
        );
    }
}

#[test]
fn explicit_codewhale_home_owns_every_default_user_path() {
    let _lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().join("isolated-codewhale-home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
    let _config = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _legacy_config = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
    let _automations = crate::test_support::EnvVarGuard::remove("CODEWHALE_AUTOMATIONS_DIR");
    let _legacy_automations = crate::test_support::EnvVarGuard::remove("DEEPSEEK_AUTOMATIONS_DIR");
    let _tasks = crate::test_support::EnvVarGuard::remove("CODEWHALE_TASKS_DIR");
    let _legacy_tasks = crate::test_support::EnvVarGuard::remove("DEEPSEEK_TASKS_DIR");
    let _runtime = crate::test_support::EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
    let _legacy_runtime = crate::test_support::EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

    let report = DoctorPathReport::resolve(None).expect("resolve doctor paths");
    let task_manager_root = crate::task_manager::default_tasks_dir();
    let runtime_config = crate::runtime_threads::RuntimeThreadManagerConfig::from_task_data_dir(
        task_manager_root.clone(),
    );
    let (secrets, legacy_secrets) =
        codewhale_secrets::FileKeyringStore::default_paths_read_only().expect("secret paths");

    assert_eq!(report.home, home);
    assert_eq!(report.config, home.join("config.toml"));
    assert_eq!(report.settings, home.join("settings.toml"));
    assert_eq!(report.sessions, home.join("sessions"));
    assert_eq!(report.logs, home.join("logs"));
    assert_eq!(report.automations, home.join("automations"));
    assert_eq!(report.task_manager_root, task_manager_root);
    assert_eq!(report.task_manager_tasks, task_manager_root.join("tasks"));
    assert_eq!(
        report.task_manager_artifacts,
        task_manager_root.join("artifacts")
    );
    assert_eq!(report.runtime_store, runtime_config.data_dir);
    assert_eq!(
        report.runtime_events,
        runtime_config.data_dir.join("events")
    );
    assert_eq!(
        report.personal_fleet_definitions,
        crate::fleet::exact::personal_fleet_definitions_dir().expect("personal fleets")
    );
    assert_eq!(
        report.personal_fleet_agents,
        crate::fleet::profile::personal_agent_profile_dir().expect("personal agents")
    );
    assert_eq!(report.secrets, secrets);
    assert_eq!(legacy_secrets, None);
    assert_eq!(report.entries().len(), 14);
    let json = serde_json::to_value(&report).expect("serialize path snapshot");
    for (label, path) in report.entries() {
        assert_eq!(
            json[label].as_str(),
            Some(path.to_string_lossy().as_ref()),
            "human and JSON path snapshots diverged for {label}"
        );
    }
    assert!(
        !home.exists(),
        "path reporting must not create the configured home"
    );
}

#[test]
fn explicit_relative_config_matches_the_canonical_loader_path() {
    let relative = Path::new("fixtures/relative-doctor-config.toml");
    let expected = codewhale_config::resolve_config_path(Some(relative.to_path_buf()))
        .expect("canonical config path");

    let report = DoctorPathReport::resolve(Some(relative)).expect("resolve doctor paths");

    assert_eq!(report.config, expected);
    assert!(report.config.is_absolute());
}

#[test]
fn path_report_json_contains_no_secret_file_contents() {
    let _lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().join("isolated-codewhale-home");
    let secret_path = home.join("secrets").join("secrets.json");
    std::fs::create_dir_all(secret_path.parent().unwrap()).expect("secret dir fixture");
    let sentinel = "doctor-path-report-secret-sentinel";
    std::fs::write(&secret_path, sentinel).expect("secret fixture");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());

    let report = DoctorPathReport::resolve(None).expect("resolve doctor paths");
    let json = serde_json::to_string(&report).expect("serialize path report");

    assert!(!json.contains(sentinel));
    assert_eq!(std::fs::read_to_string(secret_path).unwrap(), sentinel);
}

#[test]
fn human_and_json_backend_reports_never_include_secret_file_contents() {
    let _lock = crate::test_support::lock_test_env();
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().join("isolated-codewhale-home");
    let secret_path = home.join("secrets").join("secrets.json");
    std::fs::create_dir_all(secret_path.parent().unwrap()).expect("secret dir fixture");
    let sentinel = "doctor-render-secret-sentinel";
    std::fs::write(&secret_path, format!("not-json:{sentinel}")).expect("secret fixture");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
    let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let diagnostic = codewhale_secrets::diagnose_secret_backend();
    let human = secret_backend_human_lines(&diagnostic).join("\n");
    let json = serde_json::to_string(&diagnostic).expect("serialize backend diagnostic");

    assert!(!human.contains(sentinel));
    assert!(!json.contains(sentinel));
    assert_eq!(
        std::fs::read_to_string(secret_path).unwrap(),
        format!("not-json:{sentinel}")
    );
}

#[test]
fn structural_url_authority_omits_every_secret_capable_component() {
    let sentinels = [
        "URL-USER-SENTINEL",
        "URL-PASSWORD-SENTINEL",
        "URL-PATH-SENTINEL",
        "URL-QUERY-KEY-SENTINEL",
        "URL-QUERY-VALUE-SENTINEL",
        "URL-FRAGMENT-SENTINEL",
    ];
    let raw = format!(
        "https://{}:{}@example.invalid:8443/{}/child?{}={}#{}",
        sentinels[0], sentinels[1], sentinels[2], sentinels[3], sentinels[4], sentinels[5]
    );

    let authority = structural_url_authority(&raw);

    assert_eq!(authority, "https://example.invalid:8443");
    for sentinel in sentinels {
        assert!(!authority.contains(sentinel));
    }
}

#[test]
fn credential_shaped_config_values_are_flagged_by_key_name_only() {
    // Fixture tokens stay low-entropy on purpose: realistic random strings
    // trip secret scanners (GitGuardian flagged the originals as live credentials).
    let raw = r#"
# comment with sk-not-a-real-line
model = "deepseek-v4-flash"
base_url = "https://api.moonshot.ai/kimi-code/v1"
chatgpt_access_token = "eyJ0000000000000000000000000"
moonshot_api_key = "[redacted]"
provider_api_key = "sk-test0000000000000000"
workspace_token_note = "short"
random_id = "0123456789abcdef0123456789abcdef"
"#;
    let flagged = super::config_credential_shaped_keys(raw);
    assert_eq!(flagged, vec!["chatgpt_access_token", "provider_api_key"]);
}

#[test]
fn credential_scan_ignores_urls_models_and_redacted_entries() {
    let raw = r#"
model = "kimi-k3-instruct-preview-2026"
endpoint = "https://example.com/v1?key=nope"
api_key = "[redacted]"
"#;
    assert!(super::config_credential_shaped_keys(raw).is_empty());
}
