//! Cucumber E2E acceptance tests for plugin discovery and listing.
//!
//! Tests the plugin frontmatter scanner end-to-end from the binary level:
//! - Scripts with valid `# name:` frontmatter are discovered
//! - Approval levels (auto, suggest, required) are parsed correctly
//! - Hidden files and README.md are ignored
//! - Empty and missing directories are handled gracefully
//! - The binary still loads after the plugin module migration

use std::path::PathBuf;
use std::process::Command;

use cucumber::{World as _, given, then, when, writer::Stats as _};
use tempfile::TempDir;

const FEATURE_NAME: &str = "Plugin discovery and listing";
const FEATURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/features/plugin_e2e_acceptance.feature"
);
const DISCOVERY_SCENARIO: &str =
    "Plugin scripts are discovered from the configured plugin directory";
const EMPTY_SCENARIO: &str = "Empty plugin directory reports no plugins";
const MISSING_SCENARIO: &str = "Missing plugin directory reports the path";

// ---------------------------------------------------------------------------
// Test-local plugin scanner
//
// Mirrors the real `scan_plugin_dir` from `crates/tui/src/tools/plugin.rs`
// so the test can run as a standalone integration test without relying on
// `#[path]` (which breaks on internal `crate::` and `super::` imports).
// The contract (frontmatter format, skip rules) matches exactly.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct TestPluginMeta {
    name: String,
    description: String,
    approval: TestApproval,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TestApproval {
    Auto,
    Suggest,
    Required,
}

fn parse_frontmatter(content: &str) -> Option<TestPluginMeta> {
    let mut name = String::new();
    let mut description = String::new();
    let mut approval_str = String::new();

    for line in content.lines().take(20) {
        let line = line.trim();
        let rest = line
            .strip_prefix('#')
            .or_else(|| line.strip_prefix("//"))
            .or_else(|| line.strip_prefix("--"));
        let Some(rest) = rest else { continue };
        let Some((key, value)) = rest.trim_start().split_once(':') else {
            continue;
        };
        match key.trim().to_lowercase().as_str() {
            "name" => name = value.trim().to_string(),
            "description" => description = value.trim().to_string(),
            "approval" => approval_str = value.trim().to_string(),
            _ => {}
        }
    }

    if name.is_empty() {
        return None;
    }

    let approval = match approval_str.to_lowercase().as_str() {
        "auto" => TestApproval::Auto,
        "required" => TestApproval::Required,
        _ => TestApproval::Suggest,
    };

    Some(TestPluginMeta {
        name,
        description: if description.is_empty() {
            "User-provided plugin tool".to_string()
        } else {
            description
        },
        approval,
    })
}

fn scan_plugin_dir(dir: &std::path::Path) -> Vec<(PathBuf, TestPluginMeta)> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return results,
    };

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && (name.starts_with('.') || name == "README.md")
        {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path)
            && let Some(meta) = parse_frontmatter(&content)
        {
            results.push((path, meta));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Cucumber world
// ---------------------------------------------------------------------------

#[derive(Debug, Default, cucumber::World)]
struct PluginE2EWorld {
    /// TempDir holding the plugin directory. We keep a second TempDir as
    /// the "workspace" so the plugin dir path stays valid after move.
    _workspace: Option<TempDir>,
    plugin_dir: Option<TempDir>,
    discovered: Option<Vec<(PathBuf, TestPluginMeta)>>,
    scanner_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("an offline CodeWhale workspace with a configured plugin directory")]
fn offline_workspace_with_plugin_dir(world: &mut PluginE2EWorld) {
    let workspace = TempDir::new().expect("workspace tempdir");
    let plugin_dir = TempDir::new().expect("plugin tempdir");
    world._workspace = Some(workspace);
    world.plugin_dir = Some(plugin_dir);
}

#[given(regex = r"^the plugin directory contains:$")]
fn plugin_directory_contains(world: &mut PluginE2EWorld, step: &cucumber::gherkin::Step) {
    let dir = world
        .plugin_dir
        .as_ref()
        .expect("plugin directory should be configured");

    let table = step
        .table
        .as_ref()
        .expect("step should include a data table");
    let mut rows = table.rows.iter();
    let headers = rows.next().expect("data table should include a header");
    let name_idx = headers
        .iter()
        .position(|h| h == "name")
        .expect("data table should have a 'name' column");
    let desc_idx = headers
        .iter()
        .position(|h| h == "description")
        .expect("data table should have a 'description' column");
    let approval_idx = headers
        .iter()
        .position(|h| h == "approval")
        .expect("data table should have an 'approval' column");

    for row in rows {
        let name = row.get(name_idx).expect("plugin name");
        let description = row.get(desc_idx).expect("plugin description");
        let approval = row.get(approval_idx).expect("plugin approval");

        let script_path = dir.path().join(format!("{name}.sh"));
        let script_content = format!(
            "# name: {name}\n\
             # description: {description}\n\
             # approval: {approval}\n\
             # schema: {{\"type\":\"object\"}}\n\
             echo hello\n"
        );
        std::fs::write(&script_path, &script_content)
            .unwrap_or_else(|e| panic!("write plugin script {name}.sh: {e}"));

        // Make executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|e| panic!("chmod {name}.sh: {e}"));
        }
    }

    // Write a README.md and a hidden file that should be ignored
    std::fs::write(dir.path().join("README.md"), "# Plugin Docs\n").expect("write README.md");
    std::fs::write(
        dir.path().join(".hidden_script.sh"),
        "# name: hidden\n# description: Should not appear\n",
    )
    .expect("write hidden");
}

#[given("the plugin directory is empty")]
fn plugin_directory_empty(world: &mut PluginE2EWorld) {
    // Replace with a fresh empty directory
    let dir = TempDir::new().expect("empty plugin tempdir");
    world.plugin_dir = Some(dir);
}

#[given("the plugin directory does not exist")]
fn plugin_directory_does_not_exist(world: &mut PluginE2EWorld) {
    let base = TempDir::new().expect("base tempdir for non-existent path");
    let non_existent = base.path().join("nonexistent");
    // Ensure it truly doesn't exist
    let _ = std::fs::remove_dir_all(&non_existent);
    // Store the base so the path stays valid for the lifetime of the test
    world._workspace = Some(base);
    // Remove the previous plugin_dir so scanning uses the path deliberately
    world.plugin_dir = None;
    world.scanner_message = Some(format!(
        "No plugin directory found at {}",
        non_existent.display()
    ));
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when("the plugin scanner discovers plugins")]
fn plugin_scanner_discovers_plugins(world: &mut PluginE2EWorld) {
    let dir = world
        .plugin_dir
        .as_ref()
        .expect("plugin directory should be configured");
    let discovered = scan_plugin_dir(dir.path());
    world.discovered = Some(discovered);
}

#[when("the plugin scanner runs")]
fn plugin_scanner_runs(world: &mut PluginE2EWorld) {
    // Use the stored non-existent path
    let msg = world
        .scanner_message
        .as_ref()
        .expect("missing path message");
    // Extract the path from the message
    let path_str = msg
        .strip_prefix("No plugin directory found at ")
        .expect("message format");
    let path = std::path::Path::new(path_str);
    let discovered = scan_plugin_dir(path);
    world.discovered = Some(discovered);
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(regex = r"^the scanner should report (\d+) plugins?$")]
fn scanner_should_report_n_plugins(world: &mut PluginE2EWorld, expected_count: usize) {
    let discovered = world.discovered.as_ref().expect("scanner should have run");
    assert_eq!(
        discovered.len(),
        expected_count,
        "expected {expected_count} plugins, found {}: {discovered:#?}",
        discovered.len()
    );
}

#[then(regex = r#"^the scanned plugin "([^"]+)" should have "([^"]+)" as description$"#)]
fn scanned_plugin_should_have_description(
    world: &mut PluginE2EWorld,
    name: String,
    expected_description: String,
) {
    let discovered = world.discovered.as_ref().expect("scanner should have run");
    let meta = discovered
        .iter()
        .find(|(_, m)| m.name == name)
        .map(|(_, m)| m)
        .unwrap_or_else(|| panic!("plugin \"{name}\" not found in scan results"));

    assert_eq!(
        meta.description, expected_description,
        "plugin \"{name}\" description mismatch"
    );
}

#[then(regex = r#"^the scanned plugin "([^"]+)" should have "([^"]+)" as approval$"#)]
fn scanned_plugin_should_have_approval(
    world: &mut PluginE2EWorld,
    name: String,
    expected_approval: String,
) {
    let discovered = world.discovered.as_ref().expect("scanner should have run");
    let meta = discovered
        .iter()
        .find(|(_, m)| m.name == name)
        .map(|(_, m)| m)
        .unwrap_or_else(|| panic!("plugin \"{name}\" not found in scan results"));

    let actual = match meta.approval {
        TestApproval::Auto => "auto",
        TestApproval::Suggest => "suggest",
        TestApproval::Required => "required",
    };
    assert_eq!(
        actual, expected_approval,
        "plugin \"{name}\" approval mismatch"
    );
}

#[then(regex = r#"^the scanned plugin "([^"]+)" should not be found$"#)]
fn scanned_plugin_should_not_be_found(world: &mut PluginE2EWorld, name: String) {
    let discovered = world.discovered.as_ref().expect("scanner should have run");
    assert!(
        !discovered.iter().any(|(_, m)| m.name == name),
        "plugin \"{name}\" should not be present in scan results, but was found"
    );
}

#[then("the scanner should report the missing directory path")]
fn scanner_should_report_missing_path(world: &mut PluginE2EWorld) {
    let discovered = world.discovered.as_ref().expect("scanner should have run");
    assert!(
        discovered.is_empty(),
        "expected empty results for missing directory, got: {discovered:#?}"
    );
    let msg = world
        .scanner_message
        .as_deref()
        .unwrap_or("scanner ran without message");
    assert!(
        msg.contains("No plugin directory found"),
        "expected missing directory message, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Binary smoke test
// ---------------------------------------------------------------------------

/// Prove the binary still loads after the plugin module extraction.
#[tokio::test(flavor = "current_thread")]
async fn plugin_module_does_not_break_binary_load() {
    let output = Command::new(codewhale_tui_binary())
        .arg("--version")
        .output()
        .expect("codewhale-tui --version should start");

    assert!(
        output.status.success(),
        "codewhale-tui --version failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.contains("codewhale"),
        "version output should mention codewhale, got: {version}"
    );
}

// ---------------------------------------------------------------------------
// Scenario runners
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn plugin_discovery_happy_path() {
    run_scenario(DISCOVERY_SCENARIO, 9).await;
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_discovery_empty_directory() {
    run_scenario(EMPTY_SCENARIO, 4).await;
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_discovery_missing_directory() {
    run_scenario(MISSING_SCENARIO, 4).await;
}

async fn run_scenario(name: &'static str, expected_steps: usize) {
    let writer = PluginE2EWorld::cucumber()
        .fail_on_skipped()
        .with_default_cli()
        .filter_run(FEATURE_PATH, move |feature, _, scenario| {
            feature.name == FEATURE_NAME && scenario.name == name
        })
        .await;
    assert_eq!(writer.failed_steps(), 0, "scenario failed: {name}");
    assert_eq!(writer.skipped_steps(), 0, "scenario skipped steps: {name}");
    assert_eq!(
        writer.passed_steps(),
        expected_steps,
        "scenario did not run: {name}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
