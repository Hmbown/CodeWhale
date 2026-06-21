//! Parallel verifier ensemble tool: `run_verifiers`.
//!
//! This is the agent-facing path for "parallelize the verifier, not the
//! generator": one tool call fans out to independent project checks across
//! common ecosystems and returns a single structured verdict.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shlex::try_join;

use crate::dependencies::ExternalTool;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

const MAX_GATE_OUTPUT_CHARS: usize = 16_000;
const DEFAULT_MAX_PYTHON_FILES: usize = 200;
const MAX_CUSTOM_GATES: usize = 12;
const BACKGROUND_GATE_TIMEOUT_MS: u64 = 600_000;

/// Tool for running independent verifier gates concurrently.
pub struct RunVerifiersTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VerifierProfile {
    Auto,
    Rust,
    Node,
    Python,
    Go,
}

impl VerifierProfile {
    fn parse(raw: &str) -> Result<Self, ToolError> {
        match raw {
            "auto" => Ok(Self::Auto),
            "rust" => Ok(Self::Rust),
            "node" => Ok(Self::Node),
            "python" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            other => Err(ToolError::invalid_input(format!(
                "Unsupported profile '{other}'. Expected one of: auto, rust, node, python, go"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VerifierLevel {
    Quick,
    Full,
}

impl VerifierLevel {
    fn parse(raw: &str) -> Result<Self, ToolError> {
        match raw {
            "quick" => Ok(Self::Quick),
            "full" => Ok(Self::Full),
            other => Err(ToolError::invalid_input(format!(
                "Unsupported level '{other}'. Expected one of: quick, full"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RunVerifiersInput {
    profile: String,
    level: String,
    max_python_files: usize,
    commands: Vec<CustomVerifierInput>,
    background: bool,
}

impl Default for RunVerifiersInput {
    fn default() -> Self {
        Self {
            profile: "auto".to_string(),
            level: "quick".to_string(),
            max_python_files: DEFAULT_MAX_PYTHON_FILES,
            commands: Vec::new(),
            background: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CustomVerifierInput {
    name: String,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct VerifierGate {
    name: String,
    ecosystem: String,
    cwd: PathBuf,
    program: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GateResult {
    name: String,
    ecosystem: String,
    status: GateStatus,
    command: String,
    cwd: String,
    exit_code: Option<i32>,
    duration_ms: u64,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GateStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunVerifiersOutput {
    success: bool,
    profile: String,
    level: String,
    workspace: String,
    gate_count: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    summary: String,
    gates: Vec<GateResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackgroundGateJob {
    name: String,
    ecosystem: String,
    status: String,
    command: String,
    cwd: String,
    task_id: Option<String>,
    skipped_reason: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunVerifiersBackgroundOutput {
    success: bool,
    profile: String,
    level: String,
    workspace: String,
    background: bool,
    gate_count: usize,
    started: usize,
    skipped: usize,
    failed_to_start: usize,
    summary: String,
    jobs: Vec<BackgroundGateJob>,
}

#[async_trait]
impl ToolSpec for RunVerifiersTool {
    fn name(&self) -> &'static str {
        "run_verifiers"
    }

    fn description(&self) -> &'static str {
        "Run independent verifier gates in parallel across detected Rust, Node, Python, and Go projects. Supports explicit custom verifier commands as program+args without requiring Bash."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "profile": {
                    "type": "string",
                    "enum": ["auto", "rust", "node", "python", "go"],
                    "default": "auto",
                    "description": "Which ecosystem verifier set to run. 'auto' detects all supported project types in the workspace."
                },
                "level": {
                    "type": "string",
                    "enum": ["quick", "full"],
                    "default": "quick",
                    "description": "Quick runs fast syntax/drift/build checks. Full adds heavier test/lint gates where available."
                },
                "max_python_files": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": DEFAULT_MAX_PYTHON_FILES,
                    "description": "Maximum Python files to syntax-parse in the built-in python-syntax gate."
                },
                "commands": {
                    "type": "array",
                    "description": "Optional explicit verifier gates. Commands run directly as program+args, not through a shell. Use program='bash', args=['-lc', '...'] only when Bash is intentionally part of the verifier.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Short unique gate name."
                            },
                            "program": {
                                "type": "string",
                                "description": "Executable to spawn, for example 'uv', 'pytest', 'npm', 'make', 'cmd', 'powershell', or 'bash'."
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "default": [],
                                "description": "Arguments passed directly to the executable."
                            },
                            "cwd": {
                                "type": "string",
                                "description": "Optional working directory relative to the workspace."
                            }
                        },
                        "required": ["name", "program"],
                        "additionalProperties": false
                    },
                    "default": []
                },
                "background": {
                    "type": "boolean",
                    "default": false,
                    "description": "Start verifier gates as background shell jobs and return task_ids immediately. Use for long build/test/lint gates; completion is tracked in task/status state, and exec_shell_wait/task_shell_wait are only for early output, final output, or true dependency barriers."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ExecutesCode, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    fn starts_detached_for(&self, input: &Value) -> bool {
        input.get("background").and_then(Value::as_bool) == Some(true)
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let input: RunVerifiersInput = serde_json::from_value(input)
            .map_err(|err| ToolError::invalid_input(err.to_string()))?;
        let profile = VerifierProfile::parse(input.profile.as_str())?;
        let level = VerifierLevel::parse(input.level.as_str())?;
        if input.max_python_files == 0 || input.max_python_files > 1000 {
            return Err(ToolError::invalid_input(
                "max_python_files must be between 1 and 1000",
            ));
        }
        if input.commands.len() > MAX_CUSTOM_GATES {
            return Err(ToolError::invalid_input(format!(
                "commands may contain at most {MAX_CUSTOM_GATES} custom gates"
            )));
        }

        let gates = build_gate_plan(
            context,
            profile,
            level,
            input.max_python_files,
            &input.commands,
        )?;
        if gates.is_empty() {
            let output = RunVerifiersOutput {
                success: false,
                profile: profile.as_str().to_string(),
                level: level.as_str().to_string(),
                workspace: context.workspace.display().to_string(),
                gate_count: 0,
                passed: 0,
                failed: 0,
                skipped: 0,
                summary: "No verifier gates were detected. Provide custom commands or choose a profile that matches this workspace.".to_string(),
                gates: Vec::new(),
            };
            return ToolResult::json(&output)
                .map_err(|err| ToolError::execution_failed(err.to_string()));
        }

        if input.background {
            return start_background_gates(context, profile, level, gates);
        }

        let mut handles = Vec::with_capacity(gates.len());
        for gate in gates {
            handles.push(tokio::task::spawn_blocking(move || run_gate(gate)));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(err) => results.push(GateResult {
                    name: "internal-join".to_string(),
                    ecosystem: "internal".to_string(),
                    status: GateStatus::Failed,
                    command: "tokio::task::spawn_blocking".to_string(),
                    cwd: context.workspace.display().to_string(),
                    exit_code: None,
                    duration_ms: 0,
                    stdout: String::new(),
                    stderr: format!("Verifier task join failed: {err}"),
                    stdout_truncated: false,
                    stderr_truncated: false,
                    skipped_reason: None,
                }),
            }
        }
        results.sort_by(|a, b| a.name.cmp(&b.name));

        let passed = results
            .iter()
            .filter(|result| result.status == GateStatus::Passed)
            .count();
        let failed = results
            .iter()
            .filter(|result| result.status == GateStatus::Failed)
            .count();
        let skipped = results
            .iter()
            .filter(|result| result.status == GateStatus::Skipped)
            .count();
        let success = failed == 0 && skipped == 0;
        let summary = if success {
            format!("All {passed} verifier gates passed.")
        } else {
            format!("{passed} passed, {failed} failed, {skipped} skipped.")
        };

        let output = RunVerifiersOutput {
            success,
            profile: profile.as_str().to_string(),
            level: level.as_str().to_string(),
            workspace: context.workspace.display().to_string(),
            gate_count: results.len(),
            passed,
            failed,
            skipped,
            summary,
            gates: results,
        };

        ToolResult::json(&output).map_err(|err| ToolError::execution_failed(err.to_string()))
    }
}

fn start_background_gates(
    context: &ToolContext,
    profile: VerifierProfile,
    level: VerifierLevel,
    gates: Vec<VerifierGate>,
) -> Result<ToolResult, ToolError> {
    let mut jobs = Vec::with_capacity(gates.len());
    let mut started = 0usize;
    let mut skipped = 0usize;
    let mut failed_to_start = 0usize;

    for gate in gates {
        let cwd = gate.cwd.display().to_string();
        let Some(program) = gate.program.as_deref() else {
            skipped += 1;
            jobs.push(BackgroundGateJob {
                name: gate.name,
                ecosystem: gate.ecosystem,
                status: "skipped".to_string(),
                command: String::new(),
                cwd,
                task_id: None,
                skipped_reason: gate.skipped_reason,
                error: None,
            });
            continue;
        };

        let command = render_gate_command(program, &gate.args)?;
        let env: HashMap<String, String> = gate.env.into_iter().collect();
        let spawn_result = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_with_options_env(
                &command,
                Some(&cwd),
                BACKGROUND_GATE_TIMEOUT_MS,
                true,
                None,
                false,
                context.elevated_sandbox_policy.clone(),
                env,
            )
        };

        match spawn_result {
            Ok(result) => {
                started += 1;
                jobs.push(BackgroundGateJob {
                    name: gate.name,
                    ecosystem: gate.ecosystem,
                    status: "running".to_string(),
                    command,
                    cwd,
                    task_id: result.task_id,
                    skipped_reason: None,
                    error: None,
                });
            }
            Err(err) => {
                failed_to_start += 1;
                jobs.push(BackgroundGateJob {
                    name: gate.name,
                    ecosystem: gate.ecosystem,
                    status: "failed_to_start".to_string(),
                    command,
                    cwd,
                    task_id: None,
                    skipped_reason: None,
                    error: Some(err.to_string()),
                });
            }
        }
    }

    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    let success = failed_to_start == 0 && started > 0;
    let summary = if failed_to_start == 0 {
        format!(
            "Started {started} verifier gate(s) in the background; {skipped} skipped. Completion is tracked in task/status state. Continue inspecting or implementing while they run."
        )
    } else {
        format!(
            "Started {started} verifier gate(s), failed to start {failed_to_start}, and skipped {skipped}. Completion is tracked in task/status state. Continue inspecting or implementing while they run."
        )
    };
    let task_ids = jobs
        .iter()
        .filter_map(|job| job.task_id.clone())
        .collect::<Vec<_>>();
    let output = RunVerifiersBackgroundOutput {
        success,
        profile: profile.as_str().to_string(),
        level: level.as_str().to_string(),
        workspace: context.workspace.display().to_string(),
        background: true,
        gate_count: jobs.len(),
        started,
        skipped,
        failed_to_start,
        summary,
        jobs,
    };

    let mut result =
        ToolResult::json(&output).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    result.success = success;
    Ok(result.with_metadata(json!({
        "backgrounded": true,
        "detached_start": true,
        "verifier_background": true,
        "auto_resume_on_completion": false,
        "completion_surface": "task_status",
        "background_policy": "nonblocking",
        "task_ids": task_ids,
        "poll_with": ["exec_shell_wait", "task_shell_wait"]
    })))
}

fn render_gate_command(program: &str, args: &[String]) -> Result<String, ToolError> {
    try_join(std::iter::once(program).chain(args.iter().map(String::as_str)))
        .map_err(|err| ToolError::execution_failed(format!("failed to render gate command: {err}")))
}

fn build_gate_plan(
    context: &ToolContext,
    profile: VerifierProfile,
    level: VerifierLevel,
    max_python_files: usize,
    custom_commands: &[CustomVerifierInput],
) -> Result<Vec<VerifierGate>, ToolError> {
    let workspace = &context.workspace;
    let mut gates = Vec::new();

    if profile == VerifierProfile::Auto && workspace.join(".git").exists() {
        gates.push(gate(
            "git-whitespace",
            "git",
            workspace,
            "git",
            ["diff", "--check"],
        ));
    }

    if profile_matches(profile, VerifierProfile::Rust) && workspace.join("Cargo.toml").exists() {
        add_rust_gates(&mut gates, workspace, level);
    }
    if profile_matches(profile, VerifierProfile::Node) && workspace.join("package.json").exists() {
        add_node_gates(&mut gates, workspace, level);
    }
    if profile_matches(profile, VerifierProfile::Python) && has_python_project(workspace) {
        add_python_gates(&mut gates, workspace, level, max_python_files);
    }
    if profile_matches(profile, VerifierProfile::Go) && workspace.join("go.mod").exists() {
        add_go_gates(&mut gates, workspace, level);
    }

    for custom in custom_commands {
        gates.push(custom_gate(context, custom)?);
    }

    Ok(gates)
}

fn profile_matches(selected: VerifierProfile, candidate: VerifierProfile) -> bool {
    selected == VerifierProfile::Auto || selected == candidate
}

fn add_rust_gates(gates: &mut Vec<VerifierGate>, workspace: &Path, level: VerifierLevel) {
    let locked = workspace.join("Cargo.lock").exists();
    gates.push(gate(
        "rust-fmt",
        "rust",
        workspace,
        "cargo",
        ["fmt", "--all", "--", "--check"],
    ));

    let metadata_args = if locked {
        vec!["metadata", "--locked", "--format-version", "1", "--no-deps"]
    } else {
        vec!["metadata", "--format-version", "1", "--no-deps"]
    };
    gates.push(gate_vec(
        "rust-metadata",
        "rust",
        workspace,
        "cargo",
        metadata_args,
    ));

    let mut check_args = vec!["check", "--workspace", "--all-targets"];
    if locked {
        check_args.push("--locked");
    }
    gates.push(gate_vec(
        "rust-check",
        "rust",
        workspace,
        "cargo",
        check_args,
    ));

    if level == VerifierLevel::Full {
        let mut clippy_args = vec!["clippy", "--workspace", "--all-targets", "--all-features"];
        if locked {
            clippy_args.push("--locked");
        }
        clippy_args.extend(["--", "-D", "warnings"]);
        gates.push(gate_vec(
            "rust-clippy",
            "rust",
            workspace,
            "cargo",
            clippy_args,
        ));

        let mut test_args = vec!["test", "--workspace", "--all-features"];
        if locked {
            test_args.push("--locked");
        }
        gates.push(gate_vec("rust-test", "rust", workspace, "cargo", test_args));
    }
}

fn add_node_gates(gates: &mut Vec<VerifierGate>, workspace: &Path, level: VerifierLevel) {
    let scripts = package_json_scripts(workspace);
    let Some(scripts) = scripts else {
        gates.push(skipped_gate(
            "node-package-json",
            "node",
            workspace,
            "package.json is missing or could not be parsed",
        ));
        return;
    };
    let package_manager = detect_node_package_manager(workspace);
    for script in ["format:check", "check", "typecheck", "lint"] {
        if has_meaningful_script(&scripts, script) {
            gates.push(node_script_gate(workspace, &package_manager, script));
        }
    }
    if level == VerifierLevel::Full && has_meaningful_script(&scripts, "test") {
        gates.push(node_script_gate(workspace, &package_manager, "test"));
    }
}

fn add_python_gates(
    gates: &mut Vec<VerifierGate>,
    workspace: &Path,
    level: VerifierLevel,
    max_python_files: usize,
) {
    let python_files = collect_python_files(workspace, max_python_files);
    match python_files {
        PythonFiles::Files(files) if !files.is_empty() => {
            gates.push(python_syntax_gate(workspace, &files));
        }
        PythonFiles::TooMany { limit, found } => gates.push(skipped_gate(
            "python-syntax",
            "python",
            workspace,
            format!(
                "found more than {limit} Python files ({found}); raise max_python_files to verify them"
            ),
        )),
        PythonFiles::Files(_) => {}
    }

    if level == VerifierLevel::Full && has_pytest_signal(workspace) {
        gates.push(python_module_gate(
            "python-pytest",
            workspace,
            ["-m", "pytest"],
        ));
    }
}

fn add_go_gates(gates: &mut Vec<VerifierGate>, workspace: &Path, level: VerifierLevel) {
    gates.push(gate("go-test", "go", workspace, "go", ["test", "./..."]));
    if level == VerifierLevel::Full {
        gates.push(gate("go-vet", "go", workspace, "go", ["vet", "./..."]));
    }
}

fn gate<const N: usize>(
    name: &str,
    ecosystem: &str,
    cwd: &Path,
    program: &str,
    args: [&str; N],
) -> VerifierGate {
    gate_vec(name, ecosystem, cwd, program, args)
}

fn gate_vec<I, S>(name: &str, ecosystem: &str, cwd: &Path, program: &str, args: I) -> VerifierGate
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    VerifierGate {
        name: name.to_string(),
        ecosystem: ecosystem.to_string(),
        cwd: cwd.to_path_buf(),
        program: Some(program.to_string()),
        args: args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect(),
        env: Vec::new(),
        skipped_reason: None,
    }
}

fn skipped_gate(
    name: &str,
    ecosystem: &str,
    cwd: &Path,
    reason: impl Into<String>,
) -> VerifierGate {
    VerifierGate {
        name: name.to_string(),
        ecosystem: ecosystem.to_string(),
        cwd: cwd.to_path_buf(),
        program: None,
        args: Vec::new(),
        env: Vec::new(),
        skipped_reason: Some(reason.into()),
    }
}

fn custom_gate(
    context: &ToolContext,
    custom: &CustomVerifierInput,
) -> Result<VerifierGate, ToolError> {
    if custom.name.trim().is_empty() {
        return Err(ToolError::invalid_input(
            "Custom verifier command is missing 'name'",
        ));
    }
    if custom.program.trim().is_empty() {
        return Err(ToolError::invalid_input(format!(
            "Custom verifier '{}' is missing 'program'",
            custom.name
        )));
    }
    let cwd = match custom.cwd.as_deref() {
        Some(raw) if !raw.trim().is_empty() => context.resolve_path(raw)?,
        _ => context.workspace.clone(),
    };
    Ok(VerifierGate {
        name: custom.name.clone(),
        ecosystem: "custom".to_string(),
        cwd,
        program: Some(custom.program.clone()),
        args: custom.args.clone(),
        env: Vec::new(),
        skipped_reason: None,
    })
}

fn node_script_gate(
    workspace: &Path,
    package_manager: &NodePackageManager,
    script: &str,
) -> VerifierGate {
    let (program, args) = package_manager.command_for_script(script);
    gate_vec(&format!("node-{script}"), "node", workspace, program, args)
}

fn python_syntax_gate(workspace: &Path, files: &[PathBuf]) -> VerifierGate {
    let Some((program, mut args)) = python_command_parts() else {
        return skipped_gate(
            "python-syntax",
            "python",
            workspace,
            "Python interpreter is not installed or not in PATH",
        );
    };
    args.push("-c".to_string());
    args.push(PYTHON_SYNTAX_SCRIPT.to_string());
    args.extend(files.iter().map(|path| path.display().to_string()));
    let mut gate = gate_vec("python-syntax", "python", workspace, &program, args);
    gate.env
        .push(("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()));
    gate
}

fn python_module_gate<const N: usize>(
    name: &str,
    workspace: &Path,
    module_args: [&str; N],
) -> VerifierGate {
    let Some((program, mut args)) = python_command_parts() else {
        return skipped_gate(
            name,
            "python",
            workspace,
            "Python interpreter is not installed or not in PATH",
        );
    };
    args.extend(module_args.into_iter().map(str::to_string));
    gate_vec(name, "python", workspace, &program, args)
}

fn python_command_parts() -> Option<(String, Vec<String>)> {
    let spec = crate::dependencies::Python::resolve()?;
    Some(crate::dependencies::split_interpreter_spec(&spec))
}

const PYTHON_SYNTAX_SCRIPT: &str = r#"
import ast
import pathlib
import sys

failures = []
for raw in sys.argv[1:]:
    path = pathlib.Path(raw)
    try:
        source = path.read_text(encoding="utf-8")
        ast.parse(source, filename=raw)
    except Exception as exc:
        failures.append(f"{raw}: {exc.__class__.__name__}: {exc}")

if failures:
    print("\n".join(failures), file=sys.stderr)
    sys.exit(1)

print(f"parsed {len(sys.argv) - 1} Python file(s)")
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodePackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl NodePackageManager {
    fn command_for_script(self, script: &str) -> (&'static str, Vec<String>) {
        match self {
            Self::Npm => ("npm", vec!["run".to_string(), script.to_string()]),
            Self::Pnpm => ("pnpm", vec!["run".to_string(), script.to_string()]),
            Self::Yarn => ("yarn", vec!["run".to_string(), script.to_string()]),
            Self::Bun => ("bun", vec!["run".to_string(), script.to_string()]),
        }
    }
}

fn detect_node_package_manager(workspace: &Path) -> NodePackageManager {
    if workspace.join("pnpm-lock.yaml").exists() {
        NodePackageManager::Pnpm
    } else if workspace.join("yarn.lock").exists() {
        NodePackageManager::Yarn
    } else if workspace.join("bun.lock").exists() || workspace.join("bun.lockb").exists() {
        NodePackageManager::Bun
    } else {
        NodePackageManager::Npm
    }
}

fn package_json_scripts(workspace: &Path) -> Option<HashMap<String, String>> {
    let raw = fs::read_to_string(workspace.join("package.json")).ok()?;
    let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let scripts = parsed.get("scripts")?.as_object()?;
    Some(
        scripts
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|script| (key.clone(), script.to_string()))
            })
            .collect(),
    )
}

fn has_meaningful_script(scripts: &HashMap<String, String>, name: &str) -> bool {
    let Some(script) = scripts.get(name).map(|value| value.trim()) else {
        return false;
    };
    !(script.is_empty()
        || name == "test"
            && script.contains("Error: no test specified")
            && script.contains("exit 1"))
}

fn has_python_project(workspace: &Path) -> bool {
    workspace.join("pyproject.toml").exists()
        || workspace.join("setup.py").exists()
        || workspace.join("setup.cfg").exists()
        || workspace.join("requirements.txt").exists()
        || match collect_python_files(workspace, 1) {
            PythonFiles::Files(files) => !files.is_empty(),
            PythonFiles::TooMany { .. } => true,
        }
}

fn has_pytest_signal(workspace: &Path) -> bool {
    if workspace.join("pytest.ini").exists()
        || workspace.join("tox.ini").exists()
        || workspace.join("tests").is_dir()
    {
        return true;
    }
    let pyproject = workspace.join("pyproject.toml");
    fs::read_to_string(pyproject)
        .map(|raw| raw.contains("pytest") || raw.contains("[tool.pytest"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PythonFiles {
    Files(Vec<PathBuf>),
    TooMany { limit: usize, found: usize },
}

fn collect_python_files(workspace: &Path, limit: usize) -> PythonFiles {
    let mut files = BTreeSet::new();
    collect_python_files_inner(workspace, workspace, limit, &mut files);
    let found = files.len();
    if found > limit {
        PythonFiles::TooMany { limit, found }
    } else {
        PythonFiles::Files(files.into_iter().collect())
    }
}

fn collect_python_files_inner(
    root: &Path,
    dir: &Path,
    limit: usize,
    files: &mut BTreeSet<PathBuf>,
) {
    if files.len() > limit {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() > limit {
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if should_skip_dir_name(&name.to_string_lossy()) {
                continue;
            }
            collect_python_files_inner(root, &path, limit, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("py")
            && let Ok(relative) = path.strip_prefix(root)
        {
            files.insert(relative.to_path_buf());
        }
    }
}

fn should_skip_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".venv"
            | "venv"
            | "env"
            | "__pycache__"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".tox"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
    )
}

fn run_gate(gate: VerifierGate) -> GateResult {
    let command = render_command(gate.program.as_deref(), &gate.args);
    if let Some(reason) = gate.skipped_reason {
        return GateResult {
            name: gate.name,
            ecosystem: gate.ecosystem,
            status: GateStatus::Skipped,
            command,
            cwd: gate.cwd.display().to_string(),
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            skipped_reason: Some(reason),
        };
    }

    let Some(program) = gate.program else {
        return GateResult {
            name: gate.name,
            ecosystem: gate.ecosystem,
            status: GateStatus::Skipped,
            command,
            cwd: gate.cwd.display().to_string(),
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            skipped_reason: Some("verifier has no executable program".to_string()),
        };
    };

    let started = Instant::now();
    let mut cmd = Command::new(&program);
    cmd.args(&gate.args)
        .current_dir(&gate.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &gate.env {
        cmd.env(key, value);
    }

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return GateResult {
                name: gate.name,
                ecosystem: gate.ecosystem,
                status: GateStatus::Skipped,
                command,
                cwd: gate.cwd.display().to_string(),
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout: String::new(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                skipped_reason: Some(format!("{program} is not installed or not in PATH")),
            };
        }
        Err(err) => {
            return GateResult {
                name: gate.name,
                ecosystem: gate.ecosystem,
                status: GateStatus::Failed,
                command,
                cwd: gate.cwd.display().to_string(),
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout: String::new(),
                stderr: format!("Failed to spawn verifier: {err}"),
                stdout_truncated: false,
                stderr_truncated: false,
                skipped_reason: None,
            };
        }
    };

    let (stdout, stdout_truncated) = truncate_with_note(
        &String::from_utf8_lossy(&output.stdout),
        MAX_GATE_OUTPUT_CHARS,
    );
    let (stderr, stderr_truncated) = truncate_with_note(
        &String::from_utf8_lossy(&output.stderr),
        MAX_GATE_OUTPUT_CHARS,
    );
    GateResult {
        name: gate.name,
        ecosystem: gate.ecosystem,
        status: if output.status.success() {
            GateStatus::Passed
        } else {
            GateStatus::Failed
        },
        command,
        cwd: gate.cwd.display().to_string(),
        exit_code: output.status.code(),
        duration_ms: started.elapsed().as_millis() as u64,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        skipped_reason: None,
    }
}

fn render_command(program: Option<&str>, args: &[String]) -> String {
    let mut parts = Vec::new();
    parts.push(program.unwrap_or("<unavailable>").to_string());
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

fn truncate_with_note(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let end = char_boundary_index(text, max_chars);
    let truncated = &text[..end];
    let omitted_chars = text
        .chars()
        .count()
        .saturating_sub(truncated.chars().count());
    (
        format!(
            "{truncated}\n\n[output truncated to {max_chars} characters; {omitted_chars} characters omitted]"
        ),
        true,
    )
}

fn char_boundary_index(text: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return 0;
    }
    for (count, (idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            return idx;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::shell::ShellStatus;
    use std::time::Duration;
    use tempfile::tempdir;

    const BACKGROUND_COMPLETION_WAIT_MS: u64 = 30_000;

    fn wait_for_completed_shell(
        manager: &mut crate::tools::shell::ShellManager,
        task_id: &str,
    ) -> crate::tools::shell::ShellResult {
        let deadline = Instant::now() + Duration::from_millis(BACKGROUND_COMPLETION_WAIT_MS);

        loop {
            let result = manager
                .get_output(task_id, true, 1_000)
                .expect("background output");
            if result.status != ShellStatus::Running || Instant::now() >= deadline {
                return result;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn run_verifiers_requires_user_approval() {
        let tool = RunVerifiersTool;
        assert_eq!(
            tool.approval_requirement(),
            ApprovalRequirement::Required,
            "run_verifiers executes project code and must require approval"
        );
    }

    #[test]
    fn run_verifiers_background_advertises_detached_start() {
        let tool = RunVerifiersTool;
        let schema = tool.input_schema();
        let background_description = schema["properties"]["background"]["description"]
            .as_str()
            .expect("background description");

        assert!(background_description.contains("exec_shell_wait"));
        assert!(background_description.contains("task_shell_wait"));
        assert!(tool.starts_detached_for(&json!({"background": true})));
        assert!(!tool.starts_detached_for(&json!({"profile": "auto"})));
    }

    #[test]
    fn auto_profile_detects_multiple_ecosystems_without_bash() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").expect("cargo manifest");
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"lint":"eslint .","test":"echo ok"}}"#,
        )
        .expect("package json");
        fs::write(tmp.path().join("main.py"), "print('ok')\n").expect("python file");
        fs::write(tmp.path().join("go.mod"), "module example.com/app\n").expect("go mod");

        let ctx = ToolContext::new(tmp.path());
        let gates = build_gate_plan(
            &ctx,
            VerifierProfile::Auto,
            VerifierLevel::Quick,
            DEFAULT_MAX_PYTHON_FILES,
            &[],
        )
        .expect("plan");
        let names: BTreeSet<&str> = gates.iter().map(|gate| gate.name.as_str()).collect();

        assert!(names.contains("rust-fmt"));
        assert!(names.contains("node-lint"));
        assert!(names.contains("python-syntax"));
        assert!(names.contains("go-test"));
        assert!(
            gates
                .iter()
                .filter_map(|gate| gate.program.as_deref())
                .all(|program| program != "bash"),
            "built-in verifier gates must not require bash"
        );
    }

    #[test]
    fn custom_commands_can_choose_bash_explicitly() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path());
        let custom = CustomVerifierInput {
            name: "shell-check".to_string(),
            program: "bash".to_string(),
            args: vec!["-lc".to_string(), "echo ok".to_string()],
            cwd: None,
        };

        let gate = custom_gate(&ctx, &custom).expect("custom gate");

        assert_eq!(gate.program.as_deref(), Some("bash"));
        assert_eq!(gate.args, vec!["-lc", "echo ok"]);
    }

    #[test]
    fn node_default_npm_init_test_script_is_not_a_verifier() {
        let mut scripts = HashMap::new();
        scripts.insert(
            "test".to_string(),
            "echo \"Error: no test specified\" && exit 1".to_string(),
        );

        assert!(!has_meaningful_script(&scripts, "test"));
    }

    #[tokio::test]
    async fn run_verifiers_executes_custom_direct_command() {
        if !crate::dependencies::RustC::available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path());
        let tool = RunVerifiersTool;
        let result = tool
            .execute(
                json!({
                    "profile": "auto",
                    "commands": [
                        {
                            "name": "rustc-version",
                            "program": crate::dependencies::RustC::resolve().expect("rustc"),
                            "args": ["--version"]
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .expect("execute");

        let parsed: RunVerifiersOutput =
            serde_json::from_str(&result.content).expect("verifier output json");
        assert!(parsed.success, "result: {}", result.content);
        assert_eq!(parsed.passed, 1);
        assert_eq!(parsed.failed, 0);
        assert_eq!(parsed.skipped, 0);
        assert!(
            parsed.gates[0].stdout.contains("rustc"),
            "stdout should include rustc version: {:?}",
            parsed.gates[0].stdout
        );
    }

    #[tokio::test]
    async fn run_verifiers_background_starts_shell_jobs_and_returns_task_ids() {
        if !crate::dependencies::RustC::available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path());
        let tool = RunVerifiersTool;
        let result = tool
            .execute(
                json!({
                    "profile": "auto",
                    "background": true,
                    "commands": [
                        {
                            "name": "rustc-version",
                            "program": crate::dependencies::RustC::resolve().expect("rustc"),
                            "args": ["--version"]
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .expect("execute");

        let parsed: RunVerifiersBackgroundOutput =
            serde_json::from_str(&result.content).expect("background verifier output json");
        assert!(parsed.success, "result: {}", result.content);
        assert!(parsed.background);
        assert_eq!(parsed.started, 1);
        assert_eq!(parsed.failed_to_start, 0);
        assert!(parsed.summary.contains("Completion is tracked"));
        let task_id = parsed.jobs[0]
            .task_id
            .as_deref()
            .expect("background task id");
        let metadata = result.metadata.as_ref().expect("metadata");
        assert!(
            metadata
                .get("verifier_background")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "metadata should mark verifier background start"
        );
        assert_eq!(
            metadata
                .get("auto_notify_on_completion")
                .and_then(Value::as_bool),
            None
        );
        assert_eq!(
            metadata
                .get("auto_resume_on_completion")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            metadata.get("completion_surface").and_then(Value::as_str),
            Some("task_status")
        );
        assert_eq!(
            metadata.get("background_policy").and_then(Value::as_str),
            Some("nonblocking")
        );

        let output = wait_for_completed_shell(
            &mut ctx.shell_manager.lock().expect("shell manager"),
            task_id,
        );
        assert_eq!(output.status, ShellStatus::Completed);
        assert!(
            output.stdout.contains("rustc"),
            "stdout should include rustc version: {:?}",
            output.stdout
        );
    }
}
