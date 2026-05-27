//! Integration tests for the offline evaluation harness.

use std::fs;

#[path = "../src/eval.rs"]
mod eval;
#[path = "../src/shell_dispatcher.rs"]
mod shell_dispatcher;

use eval::{EvalHarness, EvalHarnessConfig, ScenarioStepKind};
use tempfile::tempdir;

#[test]
fn runs_offline_tool_loop_successfully() {
    let harness = EvalHarness::default();
    let run = harness.run().expect("eval harness run should succeed");
    assert_eq!(
        ScenarioStepKind::parse("patch"),
        Some(ScenarioStepKind::ApplyPatch)
    );

    assert!(run.metrics.success, "expected success metrics: {run:#?}");
    assert_eq!(run.metrics.tool_errors, 0);
    assert_eq!(run.metrics.steps, 6);
    assert!(run.metrics.duration.as_millis() > 0);
    assert!(!run.scenario_name.is_empty());
    assert!(run.workspace_summary.file_count >= 3);

    for kind in [
        ScenarioStepKind::List,
        ScenarioStepKind::Read,
        ScenarioStepKind::Search,
        ScenarioStepKind::Edit,
        ScenarioStepKind::ApplyPatch,
        ScenarioStepKind::ExecShell,
    ] {
        let stats = run
            .metrics
            .per_tool
            .get(&kind)
            .expect("missing per-tool stats");
        assert_eq!(stats.invocations, 1, "unexpected invocations for {kind:?}");
        assert_eq!(stats.errors, 0, "unexpected errors for {kind:?}");
        assert!(stats.total_duration.as_nanos() > 0);
    }

    let notes_path = run.workspace_root().join("notes.txt");
    let notes = fs::read_to_string(&notes_path).expect("notes.txt should exist");
    assert!(notes.contains("edited = true"));
    assert!(notes.contains("todo: offline metrics (patched)"));

    let report = run.to_report();
    assert_eq!(report.metrics.success, run.metrics.success);
}

#[test]
fn records_tool_errors_when_step_fails() {
    let config = EvalHarnessConfig {
        fail_step: Some(ScenarioStepKind::ApplyPatch),
        ..EvalHarnessConfig::default()
    };
    let harness = EvalHarness::new(config);

    let run = harness
        .run()
        .expect("eval harness should return metrics even when a step fails");

    assert!(!run.metrics.success);
    assert!(run.metrics.tool_errors >= 1);

    let patch_stats = run
        .metrics
        .per_tool
        .get(&ScenarioStepKind::ApplyPatch)
        .expect("missing apply_patch stats");
    assert_eq!(patch_stats.invocations, 1);
    assert_eq!(patch_stats.errors, 1);

    let patch_step = run
        .steps
        .iter()
        .find(|step| step.kind == ScenarioStepKind::ApplyPatch)
        .expect("missing apply_patch step");
    assert!(!patch_step.success);
    assert!(patch_step.error.as_deref().is_some_and(|e| !e.is_empty()));
}

#[test]
fn validation_can_fail_without_tool_errors() {
    let config = EvalHarnessConfig {
        shell_expect_token: "definitely-not-in-output".to_string(),
        ..EvalHarnessConfig::default()
    };
    let harness = EvalHarness::new(config);

    let run = harness.run().expect("eval harness run should complete");

    assert_eq!(run.metrics.tool_errors, 0);
    assert!(
        !run.metrics.success,
        "validation should fail due to shell token"
    );
}

#[test]
fn record_flag_writes_one_jsonl_line_per_step() {
    let dir = tempdir().expect("tempdir");
    let config = EvalHarnessConfig {
        record_dir: Some(dir.path().to_path_buf()),
        ..EvalHarnessConfig::default()
    };
    let harness = EvalHarness::new(config);
    let run = harness.run().expect("eval harness run should succeed");

    let scenario_file = dir.path().join("offline-tool-loop.jsonl");
    assert!(
        scenario_file.exists(),
        "record_dir should contain {}",
        scenario_file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );

    let contents = fs::read_to_string(&scenario_file).expect("read jsonl");
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        run.metrics.steps,
        "one JSONL line per step expected"
    );

    // Each line is a self-contained JSON object with the documented schema.
    for line in lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("each fixture line is valid JSON");
        assert!(parsed.get("request").is_some(), "missing request");
        let events = parsed
            .get("response_events")
            .and_then(|v| v.as_array())
            .expect("response_events must be an array");
        assert!(!events.is_empty(), "every fixture must have ≥1 event");
    }
}
