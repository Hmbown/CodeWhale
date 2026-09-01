use std::path::PathBuf;

use super::detect::{
    DetectEnv, DshDetection, DshRunner, classify_version, detect, settings_namespaces,
};
use super::identity::{
    CodewhaleRouteIdentity, DshAdapter, DshPermissionMode, WireProtocol, dsh_reasoning_effort,
    map_identity, permission_mode_for, render_overlay,
};
use super::receipt::{DshReceiptDocument, DshReceiptEvent};
use super::*;

struct StubRunner {
    version: Option<(bool, String)>,
    help: String,
    fail: bool,
}

impl DshRunner for StubRunner {
    fn run(&self, _binary: &std::path::Path, args: &[&str]) -> std::io::Result<(bool, String)> {
        if self.fail {
            return Err(std::io::Error::other("cannot exec"));
        }
        match args {
            ["--version"] => Ok(self.version.clone().unwrap_or((false, String::new()))),
            ["--help"] => Ok((true, self.help.clone())),
            _ => Ok((false, String::new())),
        }
    }
}

fn verified_runner() -> StubRunner {
    StubRunner {
        version: Some((true, "0.1.0-rc.6\n".to_string())),
        help: "Options:\n  --profile <name>\n  --patch <path>\n".to_string(),
        fail: false,
    }
}

fn lab_env(with_dsh: bool) -> (tempfile::TempDir, DetectEnv) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    if with_dsh {
        std::fs::write(bin.join("dsh"), "#!/bin/sh\necho 0.1.0-rc.6\n").unwrap();
    }
    let dsh_home = dir.path().join("dsh-home");
    let env = DetectEnv {
        path: Some(bin.into_os_string()),
        home: Some(dir.path().to_path_buf()),
        dsh_home: Some(dsh_home.into_os_string()),
    };
    (dir, env)
}

fn identity(
    provider: &str,
    model: &str,
    base_url: &str,
    protocol: WireProtocol,
) -> CodewhaleRouteIdentity {
    CodewhaleRouteIdentity {
        provider_id: provider.to_string(),
        provider_label: provider.to_uppercase(),
        model: model.to_string(),
        base_url: base_url.to_string(),
        protocol,
        api_key_env: Some(format!(
            "{}_API_KEY",
            provider.to_uppercase().replace('-', "_")
        )),
        keyless_local: false,
        reasoning_effort: None,
        sandbox_mode: None,
        approval_policy: None,
        yolo: false,
        workspace: "/ws".to_string(),
    }
}

#[test]
fn version_classification_is_exact_about_the_verified_line() {
    assert_eq!(
        classify_version("0.1.0-rc.6", true),
        DshCompatibility::Verified
    );
    assert!(matches!(
        classify_version("0.1.0-rc.7", true),
        DshCompatibility::NewerUnverified { .. }
    ));
    assert!(matches!(
        classify_version("0.1.0", true),
        DshCompatibility::NewerUnverified { .. }
    ));
    assert!(matches!(
        classify_version("0.2.0-rc.1", true),
        DshCompatibility::NewerUnverified { .. }
    ));
    assert!(matches!(
        classify_version("0.1.0-rc.3", true),
        DshCompatibility::Incompatible { .. }
    ));
    assert!(matches!(
        classify_version("0.0.1-rc.1", true),
        DshCompatibility::Incompatible { .. }
    ));
    assert!(matches!(
        classify_version("0.1.0-rc.6", false),
        DshCompatibility::Incompatible { .. }
    ));
    assert!(matches!(
        classify_version("nightly", true),
        DshCompatibility::Unparsed { .. }
    ));
}

#[test]
fn detection_reports_missing_offline_and_verified_without_writing() {
    let (dir, env) = lab_env(false);
    let d = detect(&env, &verified_runner());
    assert!(!d.installed());
    assert!(matches!(d.compatibility, DshCompatibility::Offline { .. }));
    assert!(!d.dsh_home_exists);
    assert!(d.dsh_home_from_env);

    let (dir2, env2) = lab_env(true);
    let d = detect(&env2, &verified_runner());
    assert!(d.installed());
    assert_eq!(d.version.as_deref(), Some("0.1.0-rc.6"));
    assert_eq!(d.compatibility, DshCompatibility::Verified);
    assert!(d.supports_patch);
    // Nothing was created under DSH_HOME by detection.
    assert!(!env2_home(&env2).exists());

    let offline = StubRunner {
        version: None,
        help: String::new(),
        fail: true,
    };
    let d = detect(&env2, &offline);
    assert!(matches!(d.compatibility, DshCompatibility::Offline { .. }));
    drop(dir);
    drop(dir2);
}

fn env2_home(env: &DetectEnv) -> PathBuf {
    PathBuf::from(env.dsh_home.clone().unwrap())
}

#[test]
fn detection_inventories_profiles_settings_and_credentials_presence_only() {
    let (_dir, env) = lab_env(true);
    let home = env2_home(&env);
    std::fs::create_dir_all(home.join("profiles/web")).unwrap();
    std::fs::create_dir_all(home.join("profiles/node_modules")).unwrap();
    std::fs::write(
        home.join("settings.yaml"),
        "ui-onboarding:\n  welcomeNoticeVersion: 1\nagent-default-model:\n  provider: deepseek-official\n  model: deepseek-v4-pro\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".credentials.yaml"),
        "DEEPSEEK_API_KEY: not-a-real-key\n",
    )
    .unwrap();
    let d = detect(&env, &verified_runner());
    assert_eq!(d.profiles, vec!["web".to_string()]);
    assert_eq!(
        d.settings_namespaces,
        vec![
            "ui-onboarding".to_string(),
            "agent-default-model".to_string()
        ]
    );
    assert!(d.credentials_present);
    let json = serde_json::to_string(&d).unwrap();
    assert!(
        !json.contains("not-a-real-key"),
        "detection must never carry a credential value"
    );
}

#[test]
fn settings_namespace_scan_ignores_nested_keys_and_comments() {
    let ns = settings_namespaces(
        "# c\nllm-deepseek:\n  baseURL: x\n  models:\n    - id: y\nlocale: en\n- list\n",
    );
    assert_eq!(ns, vec!["llm-deepseek", "locale"]);
}

#[test]
fn reasoning_effort_maps_onto_dsh_tiers() {
    assert_eq!(dsh_reasoning_effort(None), None);
    assert_eq!(dsh_reasoning_effort(Some("off")), Some("off"));
    assert_eq!(dsh_reasoning_effort(Some("low")), Some("high"));
    assert_eq!(dsh_reasoning_effort(Some("high")), Some("high"));
    assert_eq!(dsh_reasoning_effort(Some("ultra")), Some("max"));
    assert_eq!(dsh_reasoning_effort(Some("max")), Some("max"));
    assert_eq!(dsh_reasoning_effort(Some("weird")), None);
}

#[test]
fn permission_never_broadens_without_explicit_confirmation() {
    let mut id = identity(
        "deepseek",
        "deepseek-v4-pro",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    assert_eq!(
        permission_mode_for(&id, false).0,
        DshPermissionMode::WorkspaceWrite
    );
    id.sandbox_mode = Some("read-only".to_string());
    assert_eq!(
        permission_mode_for(&id, false).0,
        DshPermissionMode::ReadOnly
    );
    id.sandbox_mode = Some("danger-full-access".to_string());
    let (mode, note) = permission_mode_for(&id, false);
    assert_eq!(mode, DshPermissionMode::WorkspaceWrite);
    assert!(note.unwrap().contains("--allow-full-access"));
    assert_eq!(
        permission_mode_for(&id, true).0,
        DshPermissionMode::DangerFullAccess
    );
    // Codewhale at workspace-write can never be lifted to full access.
    id.sandbox_mode = Some("workspace-write".to_string());
    assert_eq!(
        permission_mode_for(&id, true).0,
        DshPermissionMode::WorkspaceWrite
    );
}

#[test]
fn deepseek_route_maps_to_native_adapter_with_exact_identity() {
    let mut id = identity(
        "deepseek",
        "deepseek-v4-pro",
        "https://api.deepseek.com/beta",
        WireProtocol::ChatCompletions,
    );
    id.reasoning_effort = Some("ultra".to_string());
    let mapped = map_identity(&id, false);
    assert_eq!(mapped.adapter, DshAdapter::DeepseekNative);
    assert_eq!(mapped.dsh_reasoning_effort.as_deref(), Some("max"));
    let overlay = render_overlay(&mapped).unwrap();
    assert!(overlay.contains("provider: deepseek-official"));
    assert!(overlay.contains("model: 'deepseek-v4-pro'"));
    assert!(overlay.contains("baseURL: 'https://api.deepseek.com/beta'"));
    assert!(overlay.contains("reasoningEffort: max"));
    assert!(overlay.contains("DeepSeek Harness connected through Codewhale"));
    assert!(
        !overlay.contains("apiKeyEnv"),
        "native adapter resolves its own default key ref"
    );
}

#[test]
fn ollama_keyless_route_writes_no_credential_reference() {
    let mut id = identity(
        "ollama",
        "qwen3:8b",
        "http://127.0.0.1:11434/v1",
        WireProtocol::ChatCompletions,
    );
    id.keyless_local = true;
    let mapped = map_identity(&id, false);
    assert_eq!(
        mapped.adapter,
        DshAdapter::PiAiOpenAiCompatible {
            route_id: "codewhale-ollama".to_string()
        }
    );
    let overlay = render_overlay(&mapped).unwrap();
    assert!(overlay.contains("provider: 'codewhale-ollama'"));
    assert!(overlay.contains("api: openai-completions"));
    assert!(overlay.contains("baseURL: 'http://127.0.0.1:11434/v1'"));
    assert!(!overlay.contains("apiKeyEnv"));
    assert!(
        mapped
            .disclosures
            .iter()
            .any(|d| d.contains("Keyless local route"))
    );
}

#[test]
fn keyed_openai_compatible_route_names_only_the_env_var() {
    let secret = "sk-this-must-never-appear";
    let mut id = identity(
        "zai",
        "GLM-5.3",
        "https://api.z.ai/api/coding/paas/v4",
        WireProtocol::ChatCompletions,
    );
    id.api_key_env = Some("ZAI_API_KEY".to_string());
    id.reasoning_effort = Some("high".to_string());
    let mapped = map_identity(&id, false);
    let overlay = render_overlay(&mapped).unwrap();
    assert!(overlay.contains("apiKeyEnv: 'ZAI_API_KEY'"));
    assert!(!overlay.contains(secret));
    assert!(!overlay.contains("reasoningEffort"));
    let json = serde_json::to_string(&mapped).unwrap();
    assert!(!json.contains(secret));
    assert!(mapped.disclosures.iter().any(|d| d.contains("ZAI_API_KEY")));
    assert!(
        mapped
            .disclosures
            .iter()
            .any(|d| d.contains("Reasoning tier is not mapped"))
    );
}

#[test]
fn credentialed_base_urls_are_refused_and_the_error_names_the_route() {
    let id = identity(
        "custom",
        "m",
        "https://user:token@gateway/v1",
        WireProtocol::ChatCompletions,
    );
    let mapped = map_identity(&id, false);
    match mapped.adapter {
        DshAdapter::Unsupported { reason } => assert!(reason.contains("userinfo")),
        other => panic!("expected refusal, got {other:?}"),
    }
    let id = identity(
        "custom",
        "m",
        "https://gateway/v1?key=abc",
        WireProtocol::ChatCompletions,
    );
    assert!(!map_identity(&id, false).mappable());
    // A Responses-dialect route with a credentialed URL is still refused —
    // carrying the dialect never relaxes the structural-URL guard.
    let id = identity(
        "custom",
        "m",
        "https://user:token@gateway/v1",
        WireProtocol::Responses,
    );
    assert!(!map_identity(&id, false).mappable());

    // The plan refusal names the current route so it is actionable.
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let id = identity(
        "custom",
        "secret-gateway-model",
        "https://user:token@gateway/v1",
        WireProtocol::ChatCompletions,
    );
    let error = super::plan(&paths, &detection, &id, "web", false, false, true)
        .expect_err("credentialed URL must refuse");
    let text = format!("{error:#}");
    assert!(text.contains("custom/secret-gateway-model"), "{text}");
    assert!(text.contains("userinfo"), "{text}");
}

#[test]
fn responses_dialect_route_is_carried_not_approximated() {
    // The default DeepSeek route from #5434: deepseek-v4-flash speaks the
    // Responses dialect at https://api.deepseek.com/beta.
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com/beta",
        WireProtocol::Responses,
    );
    let mapped = map_identity(&id, false);
    assert_eq!(
        mapped.adapter,
        DshAdapter::PiAiOpenAiCompatible {
            route_id: "codewhale-deepseek".to_string()
        }
    );
    assert_eq!(mapped.dsh_provider(), Some("codewhale-deepseek"));
    let overlay = render_overlay(&mapped).unwrap();
    assert!(overlay.contains("provider: 'codewhale-deepseek'"));
    assert!(overlay.contains("api: openai-responses"));
    assert!(!overlay.contains("api: openai-completions"));
    assert!(overlay.contains("baseURL: 'https://api.deepseek.com/beta'"));
    assert!(overlay.contains("apiKeyEnv: 'DEEPSEEK_API_KEY'"));
    assert!(overlay.contains("model: 'deepseek-v4-flash'"));
    assert!(
        mapped
            .disclosures
            .iter()
            .any(|d| d.contains("openai-responses") && d.contains("never approximated"))
    );

    // And it plans cleanly end-to-end.
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let plan = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    assert!(plan.overlay_text.contains("api: openai-responses"));
    assert_eq!(plan.mapped.dsh_provider(), Some("codewhale-deepseek"));
}

#[test]
fn anthropic_messages_route_is_carried_in_its_own_dialect() {
    let id = identity(
        "anthropic",
        "claude-sonnet-5",
        "https://api.anthropic.com",
        WireProtocol::AnthropicMessages,
    );
    let mapped = map_identity(&id, false);
    assert!(matches!(
        mapped.adapter,
        DshAdapter::PiAiOpenAiCompatible { .. }
    ));
    let overlay = render_overlay(&mapped).unwrap();
    assert!(overlay.contains("api: anthropic-messages"));
    assert!(overlay.contains("apiKeyEnv: 'ANTHROPIC_API_KEY'"));
    assert!(overlay.contains("baseURL: 'https://api.anthropic.com'"));
}

#[test]
fn status_surfaces_route_carryability_before_plan() {
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com/beta",
        WireProtocol::Responses,
    );
    let report = compute_status(&paths, detection.clone(), Ok(id), false, avail()).unwrap();
    let line = status_line(&report);
    assert!(
        line.contains("deepseek/deepseek-v4-flash is carryable via codewhale-deepseek"),
        "{line}"
    );

    let id = identity(
        "custom",
        "m",
        "https://user:token@gateway/v1",
        WireProtocol::ChatCompletions,
    );
    let report = compute_status(&paths, detection, Ok(id), false, avail()).unwrap();
    let line = status_line(&report);
    assert!(line.contains("custom/m cannot be carried by DSH"), "{line}");
    assert!(line.contains("userinfo"), "{line}");
}

#[test]
fn overlay_hash_is_deterministic_and_yaml_quotes_apostrophes() {
    let mut id = identity(
        "custom",
        "it's",
        "http://10.0.0.5:8000/v1",
        WireProtocol::ChatCompletions,
    );
    id.provider_label = "O'Brien Gateway".to_string();
    let a = render_overlay(&map_identity(&id, false)).unwrap();
    let b = render_overlay(&map_identity(&id, false)).unwrap();
    assert_eq!(sha256_hex(a.as_bytes()), sha256_hex(b.as_bytes()));
    assert!(a.contains("'it''s'"));
    assert!(a.contains("O''Brien"));
}

fn avail() -> BundleAvailability {
    BundleAvailability::Available {
        pnpm_version: "10.23.0".to_string(),
    }
}

fn lab_paths() -> (tempfile::TempDir, DshPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = DshPaths::under(&dir.path().join("codewhale-home"));
    (dir, paths)
}

fn detection_ok() -> DshDetection {
    let (_dir, env) = lab_env(true);
    let mut d = detect(&env, &verified_runner());
    d.binary = Some(PathBuf::from("/fake/dsh"));
    d
}

#[test]
fn connect_update_disable_enable_remove_lifecycle_writes_only_owned_files() {
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );

    // Not connected yet.
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(matches!(report.state, DshIntegrationState::Detected { .. }));
    assert!(launch_spec(&report, None, &[], std::path::Path::new("/ws")).is_err());

    let plan = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    assert!(plan.overlay_text.contains("deepseek-official"));
    let record = apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    assert!(paths.overlay.is_file());
    assert!(
        !paths.skin.is_file(),
        "connect --skin records the palette decision; it does not write a stylesheet"
    );
    assert!(!paths.skin_preview.is_file());
    assert!(paths.receipt.is_file());
    assert!(record.skin_enabled);
    assert_eq!(
        record.skin_sha256.as_deref(),
        Some(skin::skin_tokens_sha256().as_str())
    );
    assert_eq!(record.overlay_sha256, plan.overlay_sha256);

    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );
    let spec = launch_spec(
        &report,
        None,
        &["--port".to_string(), "0".to_string()],
        std::path::Path::new("/ws"),
    )
    .unwrap();
    assert_eq!(spec.args[0], "--profile");
    assert_eq!(spec.args[1], "web");
    assert_eq!(spec.args[2], "--patch");
    assert!(spec.args[3].ends_with(OVERLAY_FILE));
    assert_eq!(spec.args[4..], ["--port", "0"]);
    assert_eq!(
        spec.env,
        vec![(
            "DSH_PERMISSION_MODE".to_string(),
            "workspace-write".to_string()
        )]
    );

    // Route drift → stale-config, launch refused.
    let mut moved = id.clone();
    moved.model = "deepseek-v4-pro".to_string();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::StaleConfig { .. }),
        "{:?}",
        report.state
    );
    let err = launch_spec(&report, None, &[], std::path::Path::new("/ws"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("stale"), "{err}");

    // Update re-derives.
    let plan2 = super::plan(&paths, &detection, &moved, "web", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan2, DshReceiptEvent::Update).unwrap();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::Connected { .. }
    ));

    // Tampered overlay → stale.
    std::fs::write(&paths.overlay, "- id: x\n").unwrap();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::StaleConfig { .. }
    ));
    apply_plan(&paths, &detection, &plan2, DshReceiptEvent::Update).unwrap();

    // Disable / enable.
    set_disabled(&paths, true).unwrap();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(matches!(report.state, DshIntegrationState::Disabled { .. }));
    assert!(launch_spec(&report, None, &[], std::path::Path::new("/ws")).is_err());
    set_disabled(&paths, false).unwrap();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::Connected { .. }
    ));

    // Remove: files gone, history kept, current cleared.
    let removed = remove(&paths).unwrap();
    assert!(removed.contains(&paths.overlay));
    assert!(!paths.overlay.exists());
    assert!(!paths.skin.exists());
    assert!(!paths.skin_preview.exists());
    let doc = DshReceiptDocument::load(&paths.receipt).unwrap();
    assert!(doc.current.is_none());
    let events: Vec<_> = doc.history.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        events,
        ["connect", "update", "update", "disable", "enable", "remove"]
    );
    let report = compute_status(&paths, detection, Ok(moved), false, avail()).unwrap();
    assert!(matches!(report.state, DshIntegrationState::Detected { .. }));
    // Every write stayed under the integration root.
    for entry in walk(&paths.root.parent().unwrap().parent().unwrap().to_path_buf()) {
        assert!(
            entry.starts_with(&paths.root),
            "unexpected file {}",
            entry.display()
        );
    }
}

fn walk(root: &PathBuf) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn newer_dsh_reports_stale_version_but_stays_launchable() {
    let (_dir, paths) = lab_paths();
    let mut detection = detection_ok();
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "headless", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    detection.version = Some("0.1.0-rc.9".to_string());
    detection.compatibility = classify_version("0.1.0-rc.9", true);
    let report = compute_status(&paths, detection, Ok(id), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::StaleVersion { .. }
    ));
    assert!(report.state.launchable());
    let spec = launch_spec(&report, None, &[], std::path::Path::new("/ws")).unwrap();
    assert_eq!(spec.args[1], "headless");
}

#[test]
fn incompatible_and_missing_dsh_states_are_honest() {
    let (_dir, paths) = lab_paths();
    let mut detection = detection_ok();
    detection.version = Some("0.0.1-rc.1".to_string());
    detection.compatibility = classify_version("0.0.1-rc.1", true);
    let id = identity(
        "deepseek",
        "m",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::Incompatible { .. }
    ));
    assert!(status_line(&report).starts_with("incompatible"));
    detection.binary = None;
    let report = compute_status(&paths, detection, Ok(id), false, avail()).unwrap();
    assert_eq!(report.state, DshIntegrationState::NotInstalled);
    assert!(status_line(&report).contains("not installed"));
}

#[test]
fn plan_discloses_shadowing_settings_namespaces() {
    let (_dir, paths) = lab_paths();
    let mut detection = detection_ok();
    detection.settings_namespaces = vec!["agent-default-model".to_string(), "locale".to_string()];
    let id = identity(
        "deepseek",
        "m",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    assert_eq!(plan.shadowing_namespaces, vec!["agent-default-model"]);
    assert!(plan.disclosures.iter().any(|d| d.contains("shadow")));
    assert!(
        plan.launch_command
            .contains("DSH_PERMISSION_MODE=workspace-write dsh --profile web --patch")
    );
}

#[test]
fn skin_token_table_is_alias_pairs_that_round_trip_json() {
    let table = skin::skin_tokens();
    assert!(!table.is_empty());
    for (key, (light, dark)) in &table {
        assert!(
            key.starts_with("--dsw-alias-"),
            "token key must be a DSH alias, got {key}"
        );
        assert!(!light.is_empty(), "{key} light is empty");
        assert!(!dark.is_empty(), "{key} dark is empty");
    }
    let bg = table.get("--dsw-alias-bg-base").expect("bg-base is mapped");
    let label = table
        .get("--dsw-alias-label-primary")
        .expect("label-primary is mapped");
    assert_ne!(bg.0, bg.1, "light and dark surface colors must differ");
    assert_ne!(
        label.0, label.1,
        "light and dark primary labels must differ"
    );
    let parsed: std::collections::BTreeMap<String, skin::SkinTokens> =
        serde_json::from_str(&skin::skin_tokens_json()).unwrap();
    let round_trip: std::collections::BTreeMap<String, (String, String)> = parsed
        .into_iter()
        .map(|(k, v)| (k, (v.light, v.dark)))
        .collect();
    assert_eq!(round_trip, table);
}

#[test]
fn skin_flag_does_not_change_the_patch_overlay_bytes() {
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let on = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    let off = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    assert_eq!(on.overlay_text, off.overlay_text);
    assert_eq!(on.overlay_sha256, off.overlay_sha256);
    assert!(on.skin);
    assert!(!off.skin);
    assert!(on.skin_path.is_none());
    assert!(off.skin_path.is_none());
}

#[test]
fn brand_lockup_css_rules_are_not_accidentally_nested() {
    // The Signal Current mark's `svg` rule was authored *inside* the
    // `#codewhale-brand-mark { ... }` block. CSS nesting resolves a bare
    // nested selector against its parent, so `#codewhale-brand-mark svg`
    // nested under `#codewhale-brand-mark` means
    // "#codewhale-brand-mark #codewhale-brand-mark svg" — which matches
    // nothing, and the mark silently fell back to its inline width/height
    // attributes. Brace depth is the cheap invariant that catches it.
    let js = skin::bundle_client_js(true);
    let css_start = js
        .find("#codewhale-brand-lockup{")
        .expect("brand lockup css block");
    // Walk only the concatenated CSS string literals for the brand block.
    let css_region = &js[css_start..];
    let end = css_region
        .find("return React.createElement(")
        .unwrap_or(css_region.len());
    let css_region = &css_region[..end];

    let mut depth = 0i32;
    let mut max_depth = 0i32;
    for ch in css_region.chars() {
        match ch {
            '{' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            panic!("unbalanced brace in brand lockup css");
        }
    }
    assert_eq!(depth, 0, "brand lockup css must close every rule it opens");
    // A media query is the only legitimate nesting level in this sheet.
    assert!(
        max_depth <= 2,
        "brand lockup css nests {max_depth} deep; only @media may nest, \
         so a selector was authored inside a declaration block"
    );

    // The mark's own sizing rule must be a top-level rule, not nested: it is
    // what makes the inline SVG a block box inside the grid cell. In the
    // emitted CSS that means it is preceded by a closing brace, never by a
    // declaration.
    let marker = "#codewhale-brand-mark svg{display:block;width:34px;height:34px;}";
    let at = css_region
        .find(marker)
        .expect("the mark's svg sizing rule must be emitted");
    // The bundle embeds this file's JS source verbatim, so the text before the
    // rule still carries string-concatenation punctuation. Strip that and the
    // last meaningful CSS character must be a closing brace.
    let preceding: String = css_region[..at]
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '"' && *c != '+')
        .collect();
    assert!(
        preceding.ends_with('}'),
        "the mark's svg rule must follow a closing brace, not sit inside a \
         declaration block; it is preceded by: {:?}",
        &preceding[preceding.len().saturating_sub(60)..]
    );
}

#[test]
fn bundle_client_js_is_deterministic_override_tokens_and_not_a_stylesheet() {
    let a = skin::bundle_client_js(true);
    let b = skin::bundle_client_js(true);
    assert_eq!(a, b);
    assert!(a.contains(&format!("codewhale-skin/{}", env!("CARGO_PKG_VERSION"))));
    assert!(a.contains(skin::SKIN_SOURCE));
    assert!(a.contains("ctx.effect"));
    assert!(a.contains("overrideTokens"));
    assert!(a.contains("function CodewhaleBrand()"));
    assert!(a.contains("ctx.slots.inject(\"shell.overlay\""));
    assert!(a.contains("ctx.slots.register("));
    assert!(a.contains("React.createElement(CodewhaleBrand)"));
    assert!(a.contains("if (!ctx.theme || !ctx.slots) return;"));
    assert!(
        a.contains("exports.inject = [\"theme\", \"slots\"];"),
        "cordis exposes ctx.theme and ctx.slots only through inject"
    );
    assert!(
        a.contains("ctx.theme?.overrideTokens"),
        "disposal shape: effect callback returns the overrideTokens disposer"
    );
    assert!(!a.contains("skin_css"));
    assert!(!a.contains("<style"));
    assert!(!a.contains("skin_preview"));
    // TOKENS JSON inside the script is the same table.
    let start = a.find("const TOKENS = ").expect("TOKENS literal");
    let json_start = a[start..].find('{').expect("{") + start;
    let mut depth = 0i32;
    let mut json_end = json_start;
    for (i, ch) in a[json_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    json_end = json_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let tokens_json = &a[json_start..json_end];
    let parsed: std::collections::BTreeMap<String, skin::SkinTokens> =
        serde_json::from_str(tokens_json).unwrap();
    assert_eq!(parsed, skin::skin_token_objects());
}

#[test]
fn launch_strips_only_codewhale_injected_credentials() {
    let none = launch_env_strip_list(None, &["ZAI_API_KEY".to_string()]);
    assert_eq!(
        none,
        [
            "CODEWHALE_CLI_API_KEY",
            "CODEWHALE_CLI_API_KEY_SOURCE",
            "DEEPSEEK_API_KEY_SOURCE"
        ]
    );
    let cli = launch_env_strip_list(Some("cli"), &["ZAI_API_KEY".to_string()]);
    assert!(cli.contains(&"ZAI_API_KEY".to_string()));
    assert!(
        !cli.contains(&"DEEPSEEK_API_KEY".to_string()),
        "a bridged Z.ai credential must not claim or strip DeepSeek's slot"
    );
    let env = launch_env_strip_list(Some("env"), &["ZAI_API_KEY".to_string()]);
    assert!(
        !env.contains(&"DEEPSEEK_API_KEY".to_string()),
        "a user's own env key is left alone"
    );
}

/// Stub that records `dsh plugin` invocations and simulates DSH writing the
/// dedicated profile manifest.
struct PluginRunner {
    profile_dir: PathBuf,
    calls: std::cell::RefCell<Vec<Vec<String>>>,
    fail_add: bool,
}

impl DshRunner for PluginRunner {
    fn run(&self, _binary: &std::path::Path, args: &[&str]) -> std::io::Result<(bool, String)> {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.calls.borrow_mut().push(owned.clone());
        match args {
            ["--version"] => Ok((true, "0.1.0-rc.6\n".to_string())),
            ["--help"] => Ok((true, "--patch\n".to_string())),
            ["plugin", "--profile", "codewhale", "add", spec] => {
                if self.fail_add {
                    return Ok((false, "ERR_PNPM_NO_MATCHING_VERSION\n".to_string()));
                }
                std::fs::create_dir_all(&self.profile_dir).unwrap();
                let manifest = self.profile_dir.join("package.json");
                let mut bundles: Vec<String> = bundle::profile_bundles(&self.profile_dir)
                    .unwrap_or_else(|| vec!["@deepseek-ai/dsh-base".to_string()]);
                let name = if spec.ends_with("dsh-web-app") {
                    "@deepseek-ai/dsh-web-app".to_string()
                } else {
                    bundle::BUNDLE_PACKAGE_NAME.to_string()
                };
                if !bundles.contains(&name) {
                    bundles.push(name);
                }
                let json = serde_json::json!({"name": "dsh-profile-codewhale", "private": true, "dsh": {"profile": {"bundles": bundles}}});
                std::fs::write(manifest, serde_json::to_string_pretty(&json).unwrap()).unwrap();
                Ok((
                    true,
                    format!("+ {spec} link:\nDone in 100ms using pnpm v10.23.0\n"),
                ))
            }
            ["plugin", "--profile", "codewhale", "remove", name] => {
                let mut bundles = bundle::profile_bundles(&self.profile_dir).unwrap_or_default();
                bundles.retain(|b| b != name);
                let json = serde_json::json!({"name": "dsh-profile-codewhale", "private": true, "dsh": {"profile": {"bundles": bundles}}});
                std::fs::write(
                    self.profile_dir.join("package.json"),
                    serde_json::to_string_pretty(&json).unwrap(),
                )
                .unwrap();
                Ok((true, "- codewhale-dsh-bundle\n".to_string()))
            }
            _ => Ok((false, String::new())),
        }
    }
}

/// A fake installed launcher tree so `app_bundle_source` resolves.
fn fake_launcher(dir: &std::path::Path) -> PathBuf {
    // Unix npm: <prefix>/bin/dsh -> <prefix>/lib/node_modules/@deepseek-ai/dsh/lib/bin.js
    // Windows npm: <prefix>\dsh.cmd shim beside <prefix>\node_modules\@deepseek-ai\dsh
    #[cfg(unix)]
    let root = dir.join("npm/lib/node_modules/@deepseek-ai/dsh");
    #[cfg(not(unix))]
    let root = dir.join("npm/node_modules/@deepseek-ai/dsh");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\"name\":\"@deepseek-ai/dsh\",\"version\":\"0.1.0-rc.6\"}",
    )
    .unwrap();
    std::fs::write(root.join("lib/bin.js"), "// launcher").unwrap();
    let app = root.join("node_modules/@deepseek-ai/dsh-web-app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("package.json"), "{\"name\":\"@deepseek-ai/dsh-web-app\",\"dsh\":{\"bundle\":{\"patch\":\"./cordis.patch.yml\"}}}").unwrap();
    #[cfg(unix)]
    let bin = dir.join("bin");
    #[cfg(not(unix))]
    let bin = dir.join("npm");
    std::fs::create_dir_all(&bin).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("lib/bin.js"), bin.join("dsh")).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(root.join("lib/bin.js"), bin.join("dsh")).unwrap();
    bin.join("dsh")
}

#[test]
fn launcher_package_root_resolves_a_copied_shim_beside_node_modules() {
    // The npm-on-Windows layout: no symlink, the shim sits next to the
    // prefix's node_modules. Exercised on every platform with a plain copy.
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("npm");
    let root = prefix.join("node_modules/@deepseek-ai/dsh");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\"name\":\"@deepseek-ai/dsh\",\"version\":\"0.1.0-rc.6\"}",
    )
    .unwrap();
    std::fs::write(root.join("lib/bin.js"), "// launcher").unwrap();
    std::fs::copy(root.join("lib/bin.js"), prefix.join("dsh")).unwrap();
    let found = super::bundle::launcher_package_root(&prefix.join("dsh")).expect("root");
    assert_eq!(
        std::fs::canonicalize(found).unwrap(),
        std::fs::canonicalize(root).unwrap()
    );
    // A shim with no package beside it and no symlink resolves to nothing.
    let lonely = temp.path().join("lonely");
    std::fs::create_dir_all(&lonely).unwrap();
    std::fs::write(lonely.join("dsh"), "// shim").unwrap();
    assert!(super::bundle::launcher_package_root(&lonely.join("dsh")).is_none());
}

#[test]
fn bundle_availability_reports_pnpm_truthfully() {
    let (_dir, env) = lab_env(true);
    let no_pnpm = bundle::bundle_availability(env.path.as_ref(), &verified_runner());
    assert!(
        matches!(no_pnpm, BundleAvailability::NotAvailable { ref reason } if reason.contains("pnpm missing"))
    );
    let bin = PathBuf::from(env.path.clone().unwrap());
    std::fs::write(bin.join("pnpm"), "#!/bin/sh\necho 10.23.0\n").unwrap();
    struct Pnpm;
    impl DshRunner for Pnpm {
        fn run(&self, _b: &std::path::Path, args: &[&str]) -> std::io::Result<(bool, String)> {
            assert_eq!(args, ["--version"]);
            Ok((true, "10.23.0\n".to_string()))
        }
    }
    assert_eq!(
        bundle::bundle_availability(env.path.as_ref(), &Pnpm),
        BundleAvailability::Available {
            pnpm_version: "10.23.0".to_string()
        }
    );
}

#[test]
fn bundle_files_are_npm_shaped_and_carry_the_overlay_rows() {
    let files = bundle::render_bundle_files("0.9.8", "- id: agent-default-model\n", false, true);
    let names: Vec<_> = files.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        ["package.json", "cordis.patch.yml", "README.md", "NOTICE.md"]
    );
    let pkg: serde_json::Value = serde_json::from_str(&files[0].1).unwrap();
    assert_eq!(pkg["name"], "codewhale-dsh-bundle");
    assert_eq!(pkg["private"], true);
    assert_eq!(pkg["license"], "MIT");
    assert_eq!(pkg["dsh"]["bundle"]["patch"], "./cordis.patch.yml");
    assert!(pkg["dsh"].get("client").is_none());
    assert!(pkg.get("exports").is_none());
    assert!(pkg["version"].as_str().unwrap().starts_with("0.9.8+dsh."));
    assert_eq!(files[1].1, "- id: agent-default-model\n");
    assert!(!files[1].1.contains("insert:"));
    assert!(files[3].1.contains("Copyright (c) 2026 DeepSeek"));
}

#[test]
fn bundle_files_with_skin_carry_client_half_and_insert_row() {
    let overlay = "- id: agent-default-model\n";
    let files = bundle::render_bundle_files("0.9.8", overlay, true, true);
    let by_name: std::collections::BTreeMap<&str, &str> =
        files.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let pkg: serde_json::Value = serde_json::from_str(by_name["package.json"]).unwrap();
    let inject = pkg["dsh"]["client"]["inject"]
        .as_array()
        .expect("dsh.client.inject");
    assert!(
        inject
            .iter()
            .any(|v| v.as_str() == Some("@deepseek-ai/dsh-client-ui-theme"))
    );
    assert_eq!(pkg["dsh"]["client"]["platform"], "web");
    assert_eq!(pkg["dsh"]["client"]["immediately"], true);
    assert_eq!(pkg["exports"]["./client"]["default"], "./lib/client.js");
    // Node's exports map is exhaustive: the loader imports the bare name and
    // dsh-client-modules resolves `<name>/package.json`.
    assert_eq!(pkg["exports"]["."]["default"], "./lib/index.js");
    assert_eq!(pkg["exports"]["./package.json"], "./package.json");
    assert!(by_name[bundle::BUNDLE_PATCH_FILE].ends_with(bundle::SKIN_INSERT_YAML));
    assert_eq!(
        by_name[bundle::BUNDLE_CLIENT_FILE],
        skin::bundle_client_js(true)
    );
    assert_eq!(by_name[bundle::BUNDLE_INDEX_FILE], skin::bundle_index_js());
    assert!(!by_name[bundle::BUNDLE_CLIENT_FILE].contains("<style"));
}

#[test]
fn install_update_remove_bundle_lifecycle_uses_documented_plugin_commands() {
    let (dir, paths) = lab_paths();
    let dsh_bin = fake_launcher(dir.path());
    let mut detection = detection_ok();
    detection.binary = Some(dsh_bin);
    detection.dsh_home = dir.path().join("dsh-home");
    let profile_dir = detection.dsh_home.join("profiles").join("codewhale");
    let runner = PluginRunner {
        profile_dir: profile_dir.clone(),
        calls: Default::default(),
        fail_add: false,
    };
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );

    // Not connected → refused.
    assert!(install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web).is_err());
    let plan = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();

    // pnpm missing → truthful refusal, nothing written.
    let err = install_bundle(
        &paths,
        &detection,
        &runner,
        &BundleAvailability::NotAvailable {
            reason: "pnpm missing from PATH".into(),
        },
        DshAppBundle::Web,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("pnpm missing"));
    assert!(!paths.bundle_dir.exists());

    let record = install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web).unwrap();
    assert_eq!(record.profile, "codewhale");
    assert_eq!(record.patch_sha256, plan.overlay_sha256);
    assert!(paths.bundle_dir.join("cordis.patch.yml").is_file());
    assert_eq!(
        std::fs::read_to_string(paths.bundle_dir.join("cordis.patch.yml")).unwrap(),
        bundle::render_bundle_patch(&plan.overlay_text, true)
    );
    assert!(paths.bundle_dir.join(bundle::BUNDLE_CLIENT_FILE).is_file());
    let installed = DshReceiptDocument::load(&paths.receipt)
        .unwrap()
        .current
        .unwrap();
    let installed_json = serde_json::to_value(&installed).unwrap();
    assert_eq!(installed_json["skin"], true);
    assert_eq!(installed_json["skin_sha256"], skin::skin_tokens_sha256());
    assert!(installed.skin_enabled);
    assert_eq!(
        installed.skin_sha256.as_deref(),
        Some(skin::skin_tokens_sha256().as_str())
    );
    let calls = runner.calls.borrow().clone();
    let plugin_calls: Vec<_> = calls.iter().filter(|c| c[0] == "plugin").collect();
    assert_eq!(plugin_calls.len(), 2);
    assert!(
        plugin_calls[0][4].ends_with("dsh-web-app"),
        "app bundle first: {plugin_calls:?}"
    );
    assert_eq!(plugin_calls[1][4], paths.bundle_dir.display().to_string());
    assert_eq!(
        bundle::profile_bundles(&profile_dir).unwrap(),
        [
            "@deepseek-ai/dsh-base",
            "@deepseek-ai/dsh-web-app",
            "codewhale-dsh-bundle"
        ]
    );

    // Connected + launch prefers the bundle profile without --patch.
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );
    let spec = launch_spec(&report, None, &[], std::path::Path::new("/ws")).unwrap();
    assert_eq!(spec.args, ["--profile", "codewhale"]);
    let spec = launch_spec(&report, Some("web"), &[], std::path::Path::new("/ws")).unwrap();
    assert_eq!(spec.args[0..3], ["--profile", "web", "--patch"]);
    assert!(status_line(&report).contains("bundle in profile `codewhale`"));

    // Route drift → stale (covers the bundle), update rewrites the bundle patch.
    let mut moved = id.clone();
    moved.model = "deepseek-v4-pro".to_string();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(matches!(
        report.state,
        DshIntegrationState::StaleConfig { .. }
    ));
    let plan2 = super::plan(&paths, &detection, &moved, "web", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan2, DshReceiptEvent::Update).unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.bundle_dir.join("cordis.patch.yml")).unwrap(),
        plan2.overlay_text
    );
    assert!(
        !paths.bundle_dir.join(bundle::BUNDLE_CLIENT_FILE).exists(),
        "update --skin false drops the client half"
    );
    assert!(
        !std::fs::read_to_string(paths.bundle_dir.join("cordis.patch.yml"))
            .unwrap()
            .contains("insert:")
    );
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );
    assert_eq!(
        report
            .record
            .as_ref()
            .unwrap()
            .bundle
            .as_ref()
            .unwrap()
            .patch_sha256,
        plan2.overlay_sha256
    );

    // Tampered bundle patch → stale.
    std::fs::write(paths.bundle_dir.join("cordis.patch.yml"), "- id: x\n").unwrap();
    let report =
        compute_status(&paths, detection.clone(), Ok(moved.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::StaleConfig { ref reason, .. } if reason.contains("bundle"))
    );
    apply_plan(&paths, &detection, &plan2, DshReceiptEvent::Update).unwrap();

    // `remove` refuses while the bundle is installed.
    assert!(
        remove(&paths)
            .unwrap_err()
            .to_string()
            .contains("remove-bundle")
    );

    // remove-bundle: documented remove, owned files gone, profile dir left.
    let removed = remove_bundle(&paths, &detection, &runner).unwrap();
    assert!(!removed.is_empty());
    assert!(!paths.bundle_dir.join("cordis.patch.yml").exists());
    assert!(profile_dir.is_dir(), "DSH profile dir is left in place");
    assert_eq!(
        bundle::profile_bundles(&profile_dir).unwrap(),
        ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]
    );
    let last = runner.calls.borrow().last().cloned().unwrap();
    assert_eq!(
        last,
        [
            "plugin",
            "--profile",
            "codewhale",
            "remove",
            "codewhale-dsh-bundle"
        ]
    );
    let doc = DshReceiptDocument::load(&paths.receipt).unwrap();
    assert!(doc.current.as_ref().unwrap().bundle.is_none());
    let events: Vec<_> = doc.history.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        events,
        [
            "connect",
            "install_bundle",
            "update",
            "update",
            "remove_bundle"
        ]
    );
    // Launch falls back to the overlay path.
    let report = compute_status(&paths, detection.clone(), Ok(moved), false, avail()).unwrap();
    let spec = launch_spec(&report, None, &[], std::path::Path::new("/ws")).unwrap();
    assert_eq!(spec.args[0..3], ["--profile", "web", "--patch"]);
    // Now plain remove works.
    remove(&paths).unwrap();
}

#[test]
fn failed_plugin_add_leaves_no_bundle_record_or_files() {
    let (dir, paths) = lab_paths();
    let dsh_bin = fake_launcher(dir.path());
    let mut detection = detection_ok();
    detection.binary = Some(dsh_bin);
    detection.dsh_home = dir.path().join("dsh-home");
    let runner = PluginRunner {
        profile_dir: detection.dsh_home.join("profiles/codewhale"),
        calls: Default::default(),
        fail_add: true,
    };
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    let err = install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web)
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed"), "{err}");
    assert!(!paths.bundle_dir.join("package.json").exists());
    let doc = DshReceiptDocument::load(&paths.receipt).unwrap();
    assert!(doc.current.as_ref().unwrap().bundle.is_none());
}

#[test]
fn client_half_stale_covers_present_absent_and_modified() {
    let dir = tempfile::tempdir().unwrap();
    let bundle_dir = dir.path().join("bundle");
    std::fs::create_dir_all(bundle_dir.join("lib")).unwrap();

    assert!(
        bundle::client_half_stale(&bundle_dir, true, true)
            .unwrap()
            .contains("missing")
    );
    assert!(bundle::client_half_stale(&bundle_dir, false, true).is_none());

    std::fs::write(bundle_dir.join(bundle::BUNDLE_CLIENT_FILE), "nope\n").unwrap();
    assert!(
        bundle::client_half_stale(&bundle_dir, true, true)
            .unwrap()
            .contains("modified")
    );
    assert!(
        bundle::client_half_stale(&bundle_dir, false, true)
            .unwrap()
            .contains("present")
    );

    std::fs::write(
        bundle_dir.join(bundle::BUNDLE_CLIENT_FILE),
        skin::bundle_client_js(true),
    )
    .unwrap();
    assert!(bundle::client_half_stale(&bundle_dir, true, true).is_none());
}

#[test]
fn compute_status_reports_stale_config_when_client_half_drifts() {
    let (dir, paths) = lab_paths();
    let dsh_bin = fake_launcher(dir.path());
    let mut detection = detection_ok();
    detection.binary = Some(dsh_bin);
    detection.dsh_home = dir.path().join("dsh-home");
    let profile_dir = detection.dsh_home.join("profiles").join("codewhale");
    let runner = PluginRunner {
        profile_dir: profile_dir.clone(),
        calls: Default::default(),
        fail_add: false,
    };
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web).unwrap();

    let client = paths.bundle_dir.join(bundle::BUNDLE_CLIENT_FILE);
    std::fs::remove_file(&client).unwrap();
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::StaleConfig { ref reason, .. } if reason.contains("lib/client.js") && reason.contains("missing")),
        "{:?}",
        report.state
    );

    let plan_on = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    apply_plan(&paths, &detection, &plan_on, DshReceiptEvent::Update).unwrap();
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );

    std::fs::write(&client, "/* tampered */\n").unwrap();
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::StaleConfig { ref reason, .. } if reason.contains("modified")),
        "{:?}",
        report.state
    );

    let plan_off = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    apply_plan(&paths, &detection, &plan_off, DshReceiptEvent::Update).unwrap();
    assert!(!client.exists());
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );

    std::fs::create_dir_all(client.parent().unwrap()).unwrap();
    std::fs::write(&client, skin::bundle_client_js(true)).unwrap();
    let report = compute_status(&paths, detection, Ok(id), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::StaleConfig { ref reason, .. } if reason.contains("present") && reason.contains("disabled")),
        "{:?}",
        report.state
    );
}

#[test]
fn ocean_scene_fragment_mounts_a_canvas_and_honours_the_guards() {
    let js = scene::bundle_scene_js();
    assert!(js.contains("function createOcean(palette)"));
    assert!(
        !js.contains("\nexport "),
        "plain script: spliced into client.js"
    );
    assert!(
        !js.contains("\nimport "),
        "plain script: spliced into client.js"
    );
    // mount: fixed full-viewport canvas below the app root, no hit-testing
    assert!(js.contains("document.createElement(\"canvas\")"));
    assert!(js.contains("position:fixed;inset:0"));
    assert!(js.contains("z-index:-1"));
    assert!(js.contains("pointer-events:none"));
    assert!(js.contains("data-codewhale-ocean"));
    // motion guards
    assert!(js.contains("prefers-reduced-motion: reduce"));
    assert!(js.contains("requestAnimationFrame"));
    assert!(js.contains("visibilitychange"));
    assert!(js.contains("devicePixelRatio"));
    // off switch
    assert!(js.contains(scene::OCEAN_STORAGE_KEY));
    assert!(js.contains(scene::OCEAN_OFF_CLASS));
    assert!(js.contains(&format!("window.{}", scene::OCEAN_WINDOW_HANDLE)));
    // the cast
    assert!(js.contains("traceWhale"));
    assert!(js.contains("><>"));
    assert!(js.contains("><o>"));
    assert!(js.contains("drawBubbles"));
    assert!(js.contains("drawSpout"));
    assert!(!js.contains("eye"), "silhouette only, no eye dot");
    assert_eq!(scene::scene_sha256().len(), 64);
}

#[test]
fn brand_fragment_is_explicit_responsive_and_slot_safe() {
    let js = super::brand::bundle_brand_js();
    // The brand mark is the Codewhale whale silhouette on the deep-blue tile
    // (the same gradient and traced path as web/app/icon.svg), never an emoji.
    assert!(
        js.contains("viewBox: \"0 0 1254 1254\""),
        "whale mark viewBox"
    );
    assert!(js.contains("stopColor: \"#1D408A\""), "tile gradient start");
    assert!(js.contains("stopColor: \"#052366\""), "tile gradient end");
    assert!(js.contains("fill: \"#ffffff\""), "white whale silhouette");
    assert!(!js.contains("🐋"), "no whale emoji");
    assert!(js.contains("function CodewhaleBrand()"));
    assert!(js.contains("codewhale-brand-lockup"));
    assert!(js.contains("WHALE BROTHERS"));
    assert!(js.contains("CODEWHALE"));
    assert!(js.contains("DEEPSEEK HARNESS"));
    assert!(js.contains("pointer-events:none"));
    assert!(js.contains("@media(max-width:759px)"));
    assert!(js.contains("React.createElement"));
    assert!(!js.contains("document.body"));
    assert!(!js.contains("document.createElement"));
    assert!(!js.contains("window.addEventListener"));
    assert!(!js.contains("window.removeEventListener"));
    assert!(!js.contains("window.__codewhaleBrand"));
    assert_eq!(super::brand::brand_sha256().len(), 64);
}

#[test]
fn ocean_palette_and_veil_come_from_the_skin_palette() {
    let palette = scene::ocean_palette();
    for scheme in ["light", "dark"] {
        let p = &palette[scheme];
        for key in ["base", "accent", "ink", "dim"] {
            assert!(p[key].starts_with('#'), "{scheme}.{key} = {}", p[key]);
        }
    }
    assert_ne!(palette["light"]["base"], palette["dark"]["base"]);
    let tokens = skin::skin_tokens();
    assert_eq!(palette["light"]["base"], tokens["--dsw-alias-bg-base"].0);
    assert_eq!(palette["dark"]["base"], tokens["--dsw-alias-bg-base"].1);

    let veil = scene::ocean_veil_tokens();
    let base = &veil["--dsw-alias-bg-base"];
    assert!(base.light.starts_with("rgba(") && base.light.ends_with(",0.42)"));
    assert!(base.dark.starts_with("rgba(") && base.dark.ends_with(",0.42)"));
    assert!(
        veil["--dsw-specific-sidebar-fill"]
            .light
            .starts_with("rgba(")
    );
    // round-trips as the same {light, dark} shape overrideTokens validates
    let json: serde_json::Value = serde_json::from_str(&scene::ocean_veil_json()).unwrap();
    assert!(json["--dsw-alias-bg-base"]["light"].is_string());
    assert!(json["--dsw-alias-bg-base"]["dark"].is_string());
}

#[test]
fn bundle_client_js_splices_the_ocean_only_when_enabled() {
    let on = skin::bundle_client_js(true);
    let off = skin::bundle_client_js(false);
    assert_ne!(on, off);
    assert!(on.contains("const OCEAN = true;"));
    assert!(on.contains("function createOcean(palette)"));
    assert!(on.contains("const OCEAN_VEIL = "));
    assert!(on.contains("const OCEAN_PALETTE = "));
    assert!(on.contains("ocean.start()"));
    assert!(on.contains("theme/change"));
    assert!(on.contains("Object.assign({}, TOKENS, OCEAN_VEIL)"));
    assert!(on.contains("prefers-reduced-motion: reduce"));
    assert!(on.contains(scene::OCEAN_STORAGE_KEY));
    // the whole fragment rides inside the factory (dsh serves only client.js)
    assert!(
        on.contains(
            scene::bundle_scene_js()
                .trim_end()
                .replace('\n', "\n\t\t")
                .as_str()
        )
    );
    assert!(off.contains("const OCEAN = false;"));
    assert!(!off.contains("traceWhale"));
    assert!(!off.contains("prefers-reduced-motion"));
    // both keep the palette override contract
    for js in [&on, &off] {
        assert!(js.contains("overrideTokens"));
        assert!(js.contains("exports.inject = [\"theme\", \"slots\"];"));
        assert!(js.contains("if (!ctx.theme || !ctx.slots) return;"));
    }
}

#[test]
fn bundle_manifest_records_the_ocean_decision_and_scene_sha() {
    let overlay = "- id: agent-default-model\n";
    let on = bundle::render_bundle_files("0.9.9", overlay, true, true);
    let by_name: std::collections::BTreeMap<&str, &str> =
        on.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let pkg: serde_json::Value = serde_json::from_str(by_name["package.json"]).unwrap();
    assert_eq!(pkg["codewhale"]["ocean"], true);
    assert_eq!(
        pkg["codewhale"]["brand_sha256"],
        super::brand::brand_sha256()
    );
    assert_eq!(
        pkg["codewhale"]["ocean_scene_sha256"],
        scene::scene_sha256()
    );
    assert_eq!(
        by_name[bundle::BUNDLE_CLIENT_FILE],
        skin::bundle_client_js(true)
    );
    let names: Vec<_> = on.iter().map(|(n, _)| *n).collect();
    assert!(
        !names.iter().any(|n| n.contains("scene")),
        "no separate scene file: {names:?}"
    );

    let off = bundle::render_bundle_files("0.9.9", overlay, true, false);
    let by_name: std::collections::BTreeMap<&str, &str> =
        off.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let pkg: serde_json::Value = serde_json::from_str(by_name["package.json"]).unwrap();
    assert_eq!(pkg["codewhale"]["ocean"], false);
    assert!(pkg["codewhale"].get("ocean_scene_sha256").is_none());
    assert_eq!(
        by_name[bundle::BUNDLE_CLIENT_FILE],
        skin::bundle_client_js(false)
    );

    // skin off ⇒ ocean off regardless
    let none = bundle::render_bundle_files("0.9.9", overlay, false, true);
    let pkg: serde_json::Value = serde_json::from_str(&none[0].1).unwrap();
    assert_eq!(pkg["codewhale"]["ocean"], false);
}

#[test]
fn client_half_stale_distinguishes_ocean_on_and_off() {
    let dir = tempfile::tempdir().unwrap();
    let bundle_dir = dir.path().join("bundle");
    std::fs::create_dir_all(bundle_dir.join("lib")).unwrap();
    std::fs::write(
        bundle_dir.join(bundle::BUNDLE_CLIENT_FILE),
        skin::bundle_client_js(true),
    )
    .unwrap();
    assert!(bundle::client_half_stale(&bundle_dir, true, true).is_none());
    assert!(
        bundle::client_half_stale(&bundle_dir, true, false)
            .unwrap()
            .contains("modified")
    );
    std::fs::write(
        bundle_dir.join(bundle::BUNDLE_CLIENT_FILE),
        skin::bundle_client_js(false),
    )
    .unwrap();
    assert!(bundle::client_half_stale(&bundle_dir, true, false).is_none());
    assert!(bundle::client_half_stale(&bundle_dir, true, true).is_some());
}

#[test]
fn update_with_ocean_off_rewrites_the_client_half_and_receipt() {
    let (dir, paths) = lab_paths();
    let dsh_bin = fake_launcher(dir.path());
    let mut detection = detection_ok();
    detection.binary = Some(dsh_bin);
    detection.dsh_home = dir.path().join("dsh-home");
    let runner = PluginRunner {
        profile_dir: detection.dsh_home.join("profiles").join("codewhale"),
        calls: Default::default(),
        fail_add: false,
    };
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    assert!(plan.ocean);
    assert!(plan.disclosures.iter().any(|d| d.starts_with("Ocean:")));
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web).unwrap();
    let client = paths.bundle_dir.join(bundle::BUNDLE_CLIENT_FILE);
    assert_eq!(
        std::fs::read_to_string(&client).unwrap(),
        skin::bundle_client_js(true)
    );
    let record = DshReceiptDocument::load(&paths.receipt)
        .unwrap()
        .current
        .unwrap();
    assert!(record.ocean_enabled);
    assert_eq!(serde_json::to_value(&record).unwrap()["ocean"], true);
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );

    let off = super::plan(&paths, &detection, &id, "web", false, true, false).unwrap();
    assert!(!off.ocean);
    assert!(!off.disclosures.iter().any(|d| d.starts_with("Ocean:")));
    apply_plan(&paths, &detection, &off, DshReceiptEvent::Update).unwrap();
    assert_eq!(
        std::fs::read_to_string(&client).unwrap(),
        skin::bundle_client_js(false)
    );
    let record = DshReceiptDocument::load(&paths.receipt)
        .unwrap()
        .current
        .unwrap();
    assert!(record.skin_enabled);
    assert!(!record.ocean_enabled);
    let report = compute_status(&paths, detection.clone(), Ok(id.clone()), false, avail()).unwrap();
    assert!(
        matches!(report.state, DshIntegrationState::Connected { .. }),
        "{:?}",
        report.state
    );

    // skin off implies ocean off in the plan
    let no_skin = super::plan(&paths, &detection, &id, "web", false, false, true).unwrap();
    assert!(!no_skin.ocean);
}

#[test]
fn receipts_written_before_the_ocean_load_with_ocean_on() {
    let (_dir, paths) = lab_paths();
    let detection = detection_ok();
    let id = identity(
        "deepseek",
        "deepseek-v4-flash",
        "https://api.deepseek.com",
        WireProtocol::ChatCompletions,
    );
    let plan = super::plan(&paths, &detection, &id, "web", false, true, true).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    let text = std::fs::read_to_string(&paths.receipt).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["current"].as_object_mut().unwrap().remove("ocean");
    std::fs::write(&paths.receipt, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let record = DshReceiptDocument::load(&paths.receipt)
        .unwrap()
        .current
        .unwrap();
    assert!(record.ocean_enabled);
}
