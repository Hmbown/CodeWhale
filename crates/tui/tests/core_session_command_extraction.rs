//! Gherkin binary health and eval harness smoke test for command extraction.
//!
//! This runs the binary through `codewhale-tui eval` and verifies that the
//! executable still loads and reports a successful JSON evaluation after the
//! core/session command modules are extracted.

use std::path::PathBuf;
use std::process::Command;

use cucumber::{World as _, given, then, when, writer::Stats as _};
use serde_json::Value;
use tempfile::TempDir;

const FEATURE_NAME: &str = "Core and session command extraction";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/core_session_command_extraction.feature"
);
const CORE_SCENARIO: &str = "The binary loads and runs the evaluation harness after extraction";

#[derive(Debug, Default, cucumber::World)]
struct CoreSessionExtractionWorld {
    record_dir: Option<TempDir>,
    report: Option<Value>,
}

#[given("a clean CodeWhale evaluation workspace")]
fn clean_codewhale_evaluation_workspace(world: &mut CoreSessionExtractionWorld) {
    world.record_dir = Some(TempDir::new().expect("evaluation TempDir"));
}

#[when("the evaluation harness runs a shell command")]
fn eval_harness_runs_shell_command(world: &mut CoreSessionExtractionWorld) {
    let record_dir = world
        .record_dir
        .as_ref()
        .expect("evaluation workspace should exist");

    let output = Command::new(codewhale_tui_binary())
        .args([
            "eval",
            "--json",
            "--shell-command",
            "echo eval-harness",
            "--record",
        ])
        .arg(record_dir.path())
        .output()
        .expect("codewhale-tui eval should start");

    assert!(
        output.status.success(),
        "codewhale-tui eval failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "eval --json should emit valid JSON: {err}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    });

    world.report = Some(report);
}

#[then("the harness completes successfully")]
fn harness_completes_successfully(world: &mut CoreSessionExtractionWorld) {
    let report = world.report.as_ref().expect("eval report should exist");

    let success = report
        .get("metrics")
        .and_then(|metrics| metrics.get("success"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    assert!(
        success,
        "eval report 'metrics.success' should be true, got: {report:?}"
    );
}

#[then("the JSON report contains a step with the expected kind")]
fn json_report_contains_step_with_expected_kind(world: &mut CoreSessionExtractionWorld) {
    let report = world.report.as_ref().expect("eval report should exist");

    let steps = report
        .get("steps")
        .and_then(|value| value.as_array())
        .expect("eval report should have a 'steps' array");

    assert!(
        !steps.is_empty(),
        "eval report should have at least one step"
    );

    let first_step = &steps[0];
    let kind = first_step
        .get("kind")
        .and_then(|value| value.as_str())
        .expect("step should have a 'kind' field");

    assert_eq!(
        kind, "List",
        "first step kind should be 'List', got: {kind}"
    );

    let step_success = first_step
        .get("success")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    assert!(
        step_success,
        "first step 'success' should be true, got: {first_step:?}"
    );

    let output = first_step
        .get("output")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    assert!(
        !output.is_empty(),
        "step output should not be empty: {first_step:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn codewhale_eval_runs_after_extraction() {
    let writer = CoreSessionExtractionWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == CORE_SCENARIO
        })
        .await;
    assert_eq!(writer.failed_steps(), 0, "scenario failed: {CORE_SCENARIO}");
    assert_eq!(
        writer.skipped_steps(),
        0,
        "scenario skipped steps: {CORE_SCENARIO}"
    );
    assert_eq!(
        writer.passed_steps(),
        4,
        "scenario did not run: {CORE_SCENARIO}"
    );
}

fn codewhale_tui_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codewhale-tui") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    path
}
