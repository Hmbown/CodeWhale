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
fn unsupported_protocols_and_credentialed_urls_are_refused() {
    let id = identity(
        "anthropic",
        "claude",
        "https://api.anthropic.com",
        WireProtocol::AnthropicMessages,
    );
    let mapped = map_identity(&id, false);
    assert!(matches!(mapped.adapter, DshAdapter::Unsupported { .. }));
    assert!(render_overlay(&mapped).is_none());
    let id = identity(
        "openai-codex",
        "gpt",
        "https://x/responses",
        WireProtocol::Responses,
    );
    assert!(!map_identity(&id, false).mappable());
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

    let plan = super::plan(&paths, &detection, &id, "web", false, true).unwrap();
    assert!(plan.overlay_text.contains("deepseek-official"));
    let record = apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    assert!(paths.overlay.is_file());
    assert!(paths.skin.is_file());
    assert!(paths.receipt.is_file());
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
    let plan2 = super::plan(&paths, &detection, &moved, "web", false, false).unwrap();
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
    let plan = super::plan(&paths, &detection, &id, "headless", false, false).unwrap();
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
    let plan = super::plan(&paths, &detection, &id, "web", false, false).unwrap();
    assert_eq!(plan.shadowing_namespaces, vec!["agent-default-model"]);
    assert!(plan.disclosures.iter().any(|d| d.contains("shadow")));
    assert!(
        plan.launch_command
            .contains("DSH_PERMISSION_MODE=workspace-write dsh --profile web --patch")
    );
}

#[test]
fn skin_css_is_generated_from_palette_and_labels_itself_unsupported() {
    let css = skin::skin_css();
    assert!(css.contains("--cw-surface-bg: #03070d"));
    assert!(css.contains("--cw-accent-action: #f6c453"));
    assert!(css.contains("--cw-water-surface: #102a45"));
    assert!(css.contains("--cw-water-middle: #0a1e33"));
    assert!(css.contains("--cw-water-deep: #061320"));
    assert!(css.contains("--cw-permission-full-access: #ff7a59"));
    assert!(css.contains("--cw-mode-plan: #b9dcec"));
    assert!(css.contains("--dsw-alias-bg-base: var(--cw-surface-bg)"));
    assert!(css.contains("prefers-reduced-motion: reduce"));
    assert!(css.contains("UNSUPPORTED OVERLAY"));
    assert!(css.contains("DeepSeek Harness connected through Codewhale"));
    assert!(css.contains("MIT"));
    assert!(css.contains("data:image/svg+xml"));
    let preview = skin::skin_preview_html();
    assert!(preview.contains("PREVIEW ONLY"));
}

#[test]
fn launch_strips_only_codewhale_injected_credentials() {
    let none = launch_env_strip_list(None, &["ZAI_API_KEY".to_string()]);
    assert_eq!(none, ["CODEWHALE_CLI_API_KEY", "DEEPSEEK_API_KEY_SOURCE"]);
    let cli = launch_env_strip_list(Some("cli"), &["ZAI_API_KEY".to_string()]);
    assert!(cli.contains(&"DEEPSEEK_API_KEY".to_string()));
    assert!(cli.contains(&"ZAI_API_KEY".to_string()));
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
    let files = bundle::render_bundle_files("0.9.8", "- id: agent-default-model\n");
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
    assert!(pkg["version"].as_str().unwrap().starts_with("0.9.8+dsh."));
    assert_eq!(files[1].1, "- id: agent-default-model\n");
    assert!(files[3].1.contains("Copyright (c) 2026 DeepSeek"));
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
    let plan = super::plan(&paths, &detection, &id, "web", false, false).unwrap();
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
        plan.overlay_text
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
    let plan2 = super::plan(&paths, &detection, &moved, "web", false, false).unwrap();
    apply_plan(&paths, &detection, &plan2, DshReceiptEvent::Update).unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.bundle_dir.join("cordis.patch.yml")).unwrap(),
        plan2.overlay_text
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
    let plan = super::plan(&paths, &detection, &id, "web", false, false).unwrap();
    apply_plan(&paths, &detection, &plan, DshReceiptEvent::Connect).unwrap();
    let err = install_bundle(&paths, &detection, &runner, &avail(), DshAppBundle::Web)
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed"), "{err}");
    assert!(!paths.bundle_dir.join("package.json").exists());
    let doc = DshReceiptDocument::load(&paths.receipt).unwrap();
    assert!(doc.current.as_ref().unwrap().bundle.is_none());
}
