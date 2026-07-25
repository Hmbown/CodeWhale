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
