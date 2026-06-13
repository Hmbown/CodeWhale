//! Cucumber acceptance test for directory listing.

use std::path::{Path, PathBuf};
use std::process::Command;

use cucumber::{World as _, gherkin::Step, given, then, when, writer::Stats as _};
use tempfile::TempDir;

const FEATURE_NAME: &str = "Directory listing acceptance";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/list_dir_happy_path.feature"
);
const HAPPY_PATH_SCENARIO: &str = "Happy path lists a workspace directory";

#[derive(Debug, Default, cucumber::World)]
struct DirectoryListingWorld {
    record_dir: Option<TempDir>,
    report: Option<serde_json::Value>,
    fixture_records: Vec<serde_json::Value>,
}

#[given("an offline CodeWhale evaluation workspace")]
fn offline_codewhale_evaluation_workspace(world: &mut DirectoryListingWorld) {
    world.record_dir = Some(TempDir::new().expect("record tempdir"));
}

#[when(regex = r#"^the user asks "([^"]+)"$"#)]
fn user_asks(world: &mut DirectoryListingWorld, prompt: String) {
    assert_eq!(prompt, "list the current directory");

    let record_dir = world
        .record_dir
        .as_ref()
        .expect("offline evaluation workspace should be initialized");
    let output = Command::new(codewhale_tui_binary())
        .args(["eval", "--json", "--shell-command", "echo eval-harness"])
        .arg("--record")
        .arg(record_dir.path())
        .output()
        .expect("run codewhale-tui eval");

    assert!(
        output.status.success(),
        "eval command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    world.report = Some(
        serde_json::from_slice(&output.stdout)
            .expect("eval --json should emit a serializable report"),
    );
    world.fixture_records = read_jsonl_records(&record_dir.path().join("offline-tool-loop.jsonl"));
}

#[then(regex = r#"^the simulated LLM should call the "([^"]+)" tool$"#)]
fn simulated_llm_should_call_tool(world: &mut DirectoryListingWorld, expected_tool: String) {
    let first_step = first_report_step(world);

    assert_eq!(
        first_step.get("kind").and_then(|value| value.as_str()),
        Some("List")
    );
    assert_eq!(
        first_step.get("tool_name").and_then(|value| value.as_str()),
        Some(expected_tool.as_str())
    );
    assert_eq!(
        first_step.get("success").and_then(|value| value.as_bool()),
        Some(true)
    );

    let first_record = world
        .fixture_records
        .first()
        .expect("recorded list_dir fixture");
    assert_eq!(
        first_record
            .get("request")
            .and_then(|request| request.get("step"))
            .and_then(|step| step.as_str()),
        Some(expected_tool.as_str())
    );
}

#[then("the tool output should include:")]
fn tool_output_should_include(world: &mut DirectoryListingWorld, step: &Step) {
    let first_step = first_report_step(world);
    let list_output = first_step
        .get("output")
        .and_then(|value| value.as_str())
        .expect("list_dir output");

    for expected_entry in data_table_column(step, "entry") {
        assert!(
            list_output.contains(&expected_entry),
            "list_dir output should include {expected_entry}: {list_output}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn happy_path_lists_a_workspace_directory() {
    run_scenario(HAPPY_PATH_SCENARIO).await;
}

async fn run_scenario(name: &'static str) {
    let writer = DirectoryListingWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == name
        })
        .await;
    assert_eq!(writer.failed_steps(), 0, "scenario failed: {name}");
    assert_eq!(writer.skipped_steps(), 0, "scenario skipped steps: {name}");
    assert_eq!(writer.passed_steps(), 4, "scenario did not run: {name}");
}

fn first_report_step(world: &DirectoryListingWorld) -> &serde_json::Value {
    world
        .report
        .as_ref()
        .expect("evaluation report should exist")
        .get("steps")
        .and_then(|value| value.as_array())
        .and_then(|steps| steps.first())
        .expect("report should include at least one step")
}

fn data_table_column(step: &Step, header: &str) -> Vec<String> {
    let table = step
        .table
        .as_ref()
        .expect("step should include a data table");
    let mut rows = table.rows.iter();
    let header_row = rows.next().expect("data table should include a header");
    let column_index = header_row
        .iter()
        .position(|value| value == header)
        .expect("data table should include expected header");

    let values: Vec<String> = rows
        .map(|row| {
            row.get(column_index)
                .unwrap_or_else(|| panic!("data table row missing {header} value"))
                .clone()
        })
        .collect();
    assert!(
        !values.is_empty(),
        "data table should include at least one {header} value"
    );
    values
}

fn read_jsonl_records(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("read fixture records")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line should parse"))
        .collect()
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
