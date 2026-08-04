//! `model resolve` must report the route the runtime would actually take.
//!
//! Regression coverage for #4832, where a Z.ai config reported
//! `provider: deepseek` because the subcommand read only the CLI flags and
//! never consulted the resolved runtime. A diagnostic that confidently
//! reports the wrong provider is worse than one that reports nothing, so
//! every provider is asserted here rather than DeepSeek alone.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

/// Run `model resolve` against a sealed HOME containing `config`.
///
/// `env_clear` plus a temporary HOME keeps this off the real
/// `~/.codewhale/config.toml`; the suite has written to real user state before
/// (#4831) and this test must never be the one that does it again.
fn resolve_with_config(config: &str, args: &[&str]) -> BTreeMap<String, String> {
    let fixture = TempDir::new().expect("fixture root");
    let home = fixture.path().join("sealed-home");
    fs::create_dir_all(home.join(".codewhale")).expect("sealed config dir");
    fs::write(home.join(".codewhale").join("config.toml"), config).expect("seed config");

    let mut command = Command::new(codewhale_binary());
    command.arg("model").arg("resolve").args(args);
    let output = command
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("CODEWHALE_HOME", home.join(".codewhale"))
        .env("CODEWHALE_SECRET_BACKEND", "file")
        .output()
        .expect("run model resolve");

    assert!(
        output.status.success(),
        "model resolve {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

#[test]
fn resolve_reports_the_configured_provider_not_a_deepseek_fallback() {
    let report = resolve_with_config(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &[],
    );

    assert_eq!(
        report.get("provider").map(String::as_str),
        Some("zai"),
        "configured provider must survive to the diagnostic: {report:?}"
    );
    assert_eq!(
        report.get("provider_source").map(String::as_str),
        Some("config"),
        "provenance must name the config file: {report:?}"
    );
}

#[test]
fn resolve_reports_a_provider_scoped_model_as_explicitly_configured() {
    let report = resolve_with_config(
        "provider = \"moonshot\"\n\n[providers.moonshot]\napi_key = \"k\"\nmodel = \"kimi-k3-turbo\"\n",
        &[],
    );

    assert_eq!(report.get("provider").map(String::as_str), Some("moonshot"));
    assert_eq!(
        report.get("requested").map(String::as_str),
        Some("kimi-k3-turbo"),
        "a configured model is a request, not a fallback: {report:?}"
    );
    assert_eq!(
        report.get("used_fallback").map(String::as_str),
        Some("false"),
        "{report:?}"
    );
    assert_eq!(
        report.get("model_source").map(String::as_str),
        Some("config [providers.*].model"),
        "{report:?}"
    );
}

#[test]
fn resolve_admits_when_nothing_was_configured() {
    // The honest answer to "what did the user ask for" is "nothing". The
    // built-in default may still be shown, but it must be labelled as ours.
    let report = resolve_with_config("", &[]);

    assert_eq!(
        report.get("requested").map(String::as_str),
        Some(""),
        "an unconfigured model must not be presented as a request: {report:?}"
    );
    assert_eq!(
        report.get("used_fallback").map(String::as_str),
        Some("true"),
        "{report:?}"
    );
    assert_eq!(
        report.get("model_source").map(String::as_str),
        Some("provider default"),
        "{report:?}"
    );
}

#[test]
fn an_explicit_model_argument_still_answers_the_hypothetical() {
    // Naming a model asks "what would this resolve to", which must keep
    // working even when the configured provider is something else.
    let report = resolve_with_config(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &["deepseek-v4-flash"],
    );

    assert_eq!(
        report.get("requested").map(String::as_str),
        Some("deepseek-v4-flash"),
        "{report:?}"
    );
    assert_eq!(
        report.get("model_source").map(String::as_str),
        Some("argument"),
        "{report:?}"
    );
    assert_eq!(
        report.get("used_fallback").map(String::as_str),
        Some("false"),
        "{report:?}"
    );
}

#[test]
fn an_explicit_provider_flag_is_reported_as_the_source() {
    let report = resolve_with_config(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &["--provider", "moonshot"],
    );

    assert_eq!(report.get("provider").map(String::as_str), Some("moonshot"));
    assert_eq!(
        report.get("provider_source").map(String::as_str),
        Some("--provider"),
        "{report:?}"
    );
}

/// Run `model resolve` with global flags placed before the subcommand, which
/// is where `--provider` / `--model` actually go.
fn resolve_with_global_flags(
    config: &str,
    global: &[&str],
    args: &[&str],
) -> BTreeMap<String, String> {
    let fixture = TempDir::new().expect("fixture root");
    let home = fixture.path().join("sealed-home");
    fs::create_dir_all(home.join(".codewhale")).expect("sealed config dir");
    fs::write(home.join(".codewhale").join("config.toml"), config).expect("seed config");

    let mut command = Command::new(codewhale_binary());
    command.args(global).arg("model").arg("resolve").args(args);
    let output = command
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("CODEWHALE_HOME", home.join(".codewhale"))
        .env("CODEWHALE_SECRET_BACKEND", "file")
        .output()
        .expect("run model resolve");

    assert!(
        output.status.success(),
        "model resolve {global:?} {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

/// v0.9.1 kimi-k3 dogfood report: `codewhale --provider moonshot --model kimi-k3 model resolve`
/// reported `kimi-k2.7-code`. The top-level flags are the route this process
/// is on, not a hypothetical, so the diagnostic has to answer with the runtime
/// resolution instead of re-deriving a registry default and ignoring `--model`.
#[test]
fn top_level_provider_and_model_flags_report_the_runtime_route() {
    let report = resolve_with_global_flags(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &["--provider", "moonshot", "--model", "kimi-k3"],
        &[],
    );

    assert_eq!(report.get("provider").map(String::as_str), Some("moonshot"));
    assert_eq!(
        report.get("resolved").map(String::as_str),
        Some("kimi-k3"),
        "the diagnostic must not contradict the model the run will use: {report:?}"
    );
    assert_eq!(
        report.get("requested").map(String::as_str),
        Some("kimi-k3"),
        "{report:?}"
    );
    assert_eq!(
        report.get("used_fallback").map(String::as_str),
        Some("false"),
        "{report:?}"
    );
    assert_eq!(
        report.get("model_source").map(String::as_str),
        Some("--model"),
        "{report:?}"
    );
}

/// Moonshot ships `kimi-k3` on the direct platform API and `k3` on the Kimi
/// Code coding-plan API. Both must resolve, and neither may be answered by
/// another provider's identically named model (OpenCode Go also serves a
/// `kimi-k3`).
#[test]
fn moonshot_k3_products_resolve_without_crossing_providers() {
    for model in ["kimi-k3", "k3"] {
        let report = resolve_with_global_flags(
            "provider = \"moonshot\"\n\n[providers.moonshot]\napi_key = \"k\"\n",
            &[],
            &[model, "--provider", "moonshot"],
        );

        assert_eq!(
            report.get("provider").map(String::as_str),
            Some("moonshot"),
            "a Moonshot question must not be answered by another provider: {report:?}"
        );
        assert_eq!(
            report.get("resolved").map(String::as_str),
            Some(model),
            "{report:?}"
        );
        assert_eq!(
            report.get("used_fallback").map(String::as_str),
            Some("false"),
            "{report:?}"
        );
    }
}

/// An id the selected provider cannot serve must be reported as a fallback,
/// never as if the request had been honoured.
#[test]
fn an_unservable_model_on_the_selected_provider_is_reported_as_a_fallback() {
    let report = resolve_with_global_flags(
        "provider = \"moonshot\"\n\n[providers.moonshot]\napi_key = \"k\"\n",
        &[],
        &["glm-5.2", "--provider", "moonshot"],
    );

    assert_eq!(report.get("provider").map(String::as_str), Some("moonshot"));
    assert_eq!(
        report.get("used_fallback").map(String::as_str),
        Some("true"),
        "an unservable id must not be presented as an honoured request: {report:?}"
    );
}

/// Adding a model to the catalog must make it servable on the provider that
/// carries it and nowhere else. `glm-5.3` was added as a peer of `glm-5.2`, so
/// it has to answer on Z.ai without a fallback while a Moonshot-scoped question
/// still refuses it — the same cross-provider boundary the `glm-5.2` case above
/// pins, asserted on the newest sibling so the boundary cannot rot as the
/// family grows.
#[test]
fn a_new_glm_sibling_is_servable_on_zai_but_not_on_moonshot() {
    let served = resolve_with_global_flags(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &[],
        &["glm-5.3", "--provider", "zai"],
    );

    assert_eq!(served.get("provider").map(String::as_str), Some("zai"));
    assert_eq!(
        served.get("resolved").map(String::as_str),
        Some("GLM-5.3"),
        "a catalogued model must resolve to itself, not to the provider default: {served:?}"
    );
    assert_eq!(
        served.get("used_fallback").map(String::as_str),
        Some("false"),
        "a model the provider serves must not be reported as a fallback: {served:?}"
    );

    let refused = resolve_with_global_flags(
        "provider = \"moonshot\"\n\n[providers.moonshot]\napi_key = \"k\"\n",
        &[],
        &["glm-5.3", "--provider", "moonshot"],
    );

    assert_eq!(
        refused.get("provider").map(String::as_str),
        Some("moonshot")
    );
    assert_eq!(
        refused.get("used_fallback").map(String::as_str),
        Some("true"),
        "a Z.ai id must not be presented as honoured by Moonshot: {refused:?}"
    );
    let resolved = refused
        .get("resolved")
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        !resolved.to_ascii_lowercase().contains("glm"),
        "a provider that cannot serve GLM must not be handed a fabricated GLM id: {refused:?}"
    );
}

/// The OpenRouter sibling carries a different wire id (`z-ai/glm-5.3`) than the
/// direct Z.ai row (`GLM-5.3`), so the bare family alias has to be rewritten
/// per provider rather than passed through. This pins the OpenRouter half of
/// that rewrite, which the Z.ai case above cannot observe, and pins that adding
/// the sibling left the OpenRouter default alone.
#[test]
fn the_openrouter_glm_sibling_resolves_to_its_own_gateway_wire_id() {
    let served = resolve_with_global_flags(
        "provider = \"openrouter\"\n\n[providers.openrouter]\napi_key = \"k\"\n",
        &[],
        &["glm-5.3", "--provider", "openrouter"],
    );

    assert_eq!(
        served.get("provider").map(String::as_str),
        Some("openrouter")
    );
    assert_eq!(
        served.get("resolved").map(String::as_str),
        Some("z-ai/glm-5.3"),
        "the bare alias must be rewritten to the OpenRouter wire id, not passed through: {served:?}"
    );
    assert_eq!(
        served.get("used_fallback").map(String::as_str),
        Some("false"),
        "a gateway row the provider serves must not be reported as a fallback: {served:?}"
    );

    let default_route = resolve_with_config(
        "provider = \"openrouter\"\n\n[providers.openrouter]\napi_key = \"k\"\n",
        &[],
    );
    let resolved = default_route
        .get("resolved")
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        !resolved.to_ascii_lowercase().contains("glm"),
        "adding a GLM sibling must not make GLM the OpenRouter default: {default_route:?}"
    );
}

/// Adding a sibling must not move anyone's route. A Z.ai config that names no
/// model still has to land on `GLM-5.2`: the newer `glm-5.3` is catalogued but
/// deliberately not the default, and this is the surface where that would
/// silently change under a user.
#[test]
fn adding_a_glm_sibling_leaves_the_zai_default_route_untouched() {
    let report = resolve_with_config(
        "provider = \"zai\"\n\n[providers.zai]\napi_key = \"k\"\n",
        &[],
    );

    assert_eq!(
        report.get("resolved").map(String::as_str),
        Some("GLM-5.2"),
        "the Z.ai default must stay GLM-5.2 after a newer sibling is added: {report:?}"
    );
    assert_eq!(
        report.get("model_source").map(String::as_str),
        Some("provider default"),
        "{report:?}"
    );
}

fn codewhale_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
    path
}
