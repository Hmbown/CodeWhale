//! The kill switch has to reach the process that would emit.
//!
//! The dispatcher itself almost never emits: every interactive and headless
//! session is delegated to the sibling `codewhale-tui` binary, which re-resolves
//! telemetry from *its own* environment and config file. So the switch is only
//! real if the resolved value — not the raw flag — is in that child's
//! environment. This drives the real `codewhale` binary and reads what the
//! child actually received.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use codewhale_config::{SetupState, TELEMETRY_NOTICE_VERSION};
use tempfile::TempDir;

/// `CODEWHALE_TELEMETRY=0` beats `--telemetry true`, end to end.
///
/// The notice decision is recorded on this home on purpose: without it the run
/// would be off for want of consent, and the test would pass without the kill
/// switch ever being consulted.
#[test]
fn env_off_beats_cli_on_end_to_end() {
    // Positive control first: the flag does reach the child, so the assertion
    // below is about the floor and not about a flag that goes nowhere.
    let on = dispatch_and_read_child_env(&["--telemetry", "true", "exec", "hi"], None);
    assert_eq!(
        on.get("CODEWHALE_TELEMETRY").map(String::as_str),
        Some("true"),
        "`--telemetry true` must reach the delegated child at all"
    );

    let off = dispatch_and_read_child_env(&["--telemetry", "true", "exec", "hi"], Some("0"));
    assert_eq!(
        off.get("CODEWHALE_TELEMETRY").map(String::as_str),
        Some("false"),
        "`CODEWHALE_TELEMETRY=0` must beat `--telemetry true` in the child's environment"
    );
    assert_eq!(
        off.get("DEEPSEEK_TELEMETRY").map(String::as_str),
        Some("false"),
        "the legacy alias must carry the same resolved value"
    );
}

/// A value the resolver cannot parse resolves to off, rather than falling
/// through to the flag.
#[test]
fn an_unparseable_telemetry_env_value_reaches_the_child_as_off() {
    let child = dispatch_and_read_child_env(&["--telemetry", "true", "exec", "hi"], Some("maybe"));
    assert_eq!(
        child.get("CODEWHALE_TELEMETRY").map(String::as_str),
        Some("false"),
        "a typo in the kill switch must never resolve to on"
    );
}

/// Run the real dispatcher against a fake sibling TUI that dumps its
/// environment, and return that environment.
fn dispatch_and_read_child_env(
    args: &[&str],
    telemetry_env: Option<&str>,
) -> BTreeMap<String, String> {
    let fixture = TempDir::new().expect("fixture root");
    let home = fixture.path().join("home");
    let codewhale_home = fixture.path().join("codewhale-home");
    let workspace = fixture.path().join("workspace");
    for dir in [&home, &codewhale_home, &workspace] {
        fs::create_dir_all(dir).expect("create fixture dir");
    }

    // A recorded acceptance, so the run is not off for want of consent.
    let mut state = SetupState::default();
    state.record_telemetry_notice(TELEMETRY_NOTICE_VERSION, true);
    state
        .save_to(&codewhale_home.join("setup_state.json"))
        .expect("write setup state");

    let config_path = fixture.path().join("config.toml");
    fs::write(&config_path, "telemetry = true\n").expect("write config");

    let receipt = fixture.path().join("child-env.txt");
    let fake_tui = fixture.path().join("fake-codewhale-tui");
    fs::write(
        &fake_tui,
        format!("#!/bin/sh\nenv > '{}'\n", receipt.display()),
    )
    .expect("write fake TUI");
    let mut permissions = fs::metadata(&fake_tui)
        .expect("fake TUI metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&fake_tui, permissions).expect("make fake TUI executable");

    let mut command = Command::new(codewhale_binary());
    command
        .current_dir(&workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("CODEWHALE_HOME", &codewhale_home)
        .env("CODEWHALE_SECRET_BACKEND", "file")
        .env("CODEWHALE_TUI_BIN", &fake_tui)
        .env(
            "CODEWHALE_RELEASE_BASE_URL",
            "https://example.invalid/releases",
        )
        .arg("--config")
        .arg(&config_path)
        .args(args);
    if let Some(value) = telemetry_env {
        command.env("CODEWHALE_TELEMETRY", value);
    }
    let output = command.output().expect("run codewhale dispatcher");

    let dumped = fs::read_to_string(&receipt).unwrap_or_else(|error| {
        panic!(
            "the delegated child must have run and dumped its environment: {error}\n\
             stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });

    // A dispatcher run that never emits must also never create telemetry state
    // in the operator's home.
    assert!(
        !codewhale_home.join("telemetry").exists(),
        "the dispatcher must not create telemetry state for a delegated command"
    );

    dumped
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
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
