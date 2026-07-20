//! CLI entry point for Codewhale.

#![allow(clippy::uninlined_format_args)]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::{BTreeSet, HashMap};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use tempfile::NamedTempFile;
use wait_timeout::ChildExt;

use crate::dependencies::ExternalTool;

use rust_i18n::i18n;
i18n!("locales", fallback = ["en"]);

mod acp_server;
mod artifacts;
mod audit;
mod auto_reasoning;
mod automation_manager;
mod child_env;
mod client;
mod codex_model_cache;
mod command_safety;
mod commands;
mod compaction;
mod composer_history;
mod composer_stash;
mod config;
mod config_persistence;
mod config_ui;
mod context_budget;
mod context_report;
mod core;
mod cost_status;
mod deepseek_theme;
mod dependencies;
mod error_taxonomy;
mod eval;
mod execpolicy;
mod external_credentials;
mod fast_hash;
mod features;
mod fleet;
mod goal_loop;
mod hashing;
mod hooks;
mod llm_client;
mod llm_response_cache;
mod localization;
mod logging;
mod lsp;
mod mcp;
mod mcp_server;
mod memory;
mod model_catalog;
mod model_context;
mod model_inventory;
mod model_profile;
mod model_registry;
mod model_routing;
mod models;
mod models_dev_live;
mod network_policy;
mod oauth;
mod palette;
mod plugins;
mod prefix_cache;
mod pricing;
mod project_context;
mod project_context_cache;
mod prompt_zones;
mod prompts;
mod provider_lake;
mod provider_readiness;
mod purge;
mod regex_cache;
mod remote_setup;
pub mod repl;
mod repo_law;
mod request_tuning;
mod resource_telemetry;
mod retry_status;
pub mod rlm;
mod route_billing;
mod route_budget;
mod route_runtime;
mod runtime_api;
mod runtime_handoff;
mod runtime_log;
mod runtime_threads;
mod sandbox;
mod scorecard;
mod seam_manager;
#[allow(dead_code)]
mod session_diagnostics;
#[allow(dead_code)]
mod session_manager;
mod settings;
mod shell_dispatcher;
mod skill_state;
mod skills;
mod slop_ledger;
mod snapshot;
mod startup_trace;
mod task_manager;
#[cfg(test)]
mod test_support;
mod tls;
mod tool_history_repair;
mod tool_output_receipts;
mod tools;
mod tui;
mod utils;
mod vision;
mod work_graph;
mod worker_profile;
mod working_set;
mod workspace_discovery;
mod workspace_trust;
mod xai_oauth;

use crate::config::{Config, DEFAULT_TEXT_MODEL, MAX_SUBAGENTS, effective_home_dir};
use crate::eval::{EvalHarness, EvalHarnessConfig, ScenarioStepKind};
use crate::features::{Feature, render_feature_table};
use crate::llm_client::LlmClient;
use crate::mcp::{
    McpCommandAvailability, McpConfig, McpPool, McpServerConfig, McpServerOAuthConfig,
    is_relative_stdio_path_arg, static_mcp_command_availability,
};
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};
use crate::session_manager::{SessionManager, create_saved_session, truncate_id};
use crate::tui::history::{summarize_tool_args, summarize_tool_output};

#[cfg(windows)]
fn configure_windows_console_utf8() {
    use windows::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};

    const CP_UTF8: u32 = 65001;
    unsafe {
        let _ = SetConsoleCP(CP_UTF8);
        let _ = SetConsoleOutputCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn configure_windows_console_utf8() {}

fn install_rustls_crypto_provider() {
    crate::tls::ensure_rustls_crypto_provider();
}

#[derive(Parser, Debug)]
#[command(
    name = "codewhale-tui",
    bin_name = "codewhale-tui",
    author,
    version = env!("DEEPSEEK_BUILD_VERSION"),
    about = "Codewhale terminal coding agent",
    long_about = "Terminal-native TUI and CLI for open-source and open-weight coding models.\n\nRun 'codewhale' to start.\n\nProvider routes include DeepSeek, Arcee, Hugging Face, OpenRouter, Xiaomi MiMo, local vLLM/SGLang/Ollama, and more."
)]
struct Cli {
    /// Subcommand to run
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    feature_toggles: FeatureToggles,

    /// Initial prompt to submit in the interactive TUI. Use `exec` for non-interactive runs.
    #[arg(short, long, value_name = "PROMPT", num_args = 1..)]
    prompt: Vec<String>,

    /// Legacy compatibility alias for Act + Full Access.
    #[arg(long, hide = true)]
    yolo: bool,

    /// Maximum number of concurrent sub-agents (1-20)
    #[arg(long)]
    max_subagents: Option<usize>,

    /// Path to config file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Config profile name
    #[arg(long)]
    profile: Option<String>,

    /// Workspace directory for file operations
    #[arg(short, long)]
    workspace: Option<PathBuf>,

    /// Resume a previous session by ID or prefix
    #[arg(short, long)]
    resume: Option<String>,

    /// Continue the most recent session in this workspace
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,

    /// Deprecated compatibility flag; the interactive TUI always owns the
    /// alternate screen so terminal scrollback cannot hijack the viewport.
    #[arg(long = "no-alt-screen", hide = true)]
    no_alt_screen: bool,

    /// Enable TUI mouse capture for internal scrolling, transcript selection,
    /// and scrollbar dragging
    /// (default off on Windows)
    #[arg(long = "mouse-capture", conflicts_with = "no_mouse_capture")]
    mouse_capture: bool,

    /// Disable TUI mouse capture so terminal-native text selection works
    #[arg(long = "no-mouse-capture", conflicts_with = "mouse_capture")]
    no_mouse_capture: bool,

    /// Skip onboarding screens
    #[arg(long)]
    skip_onboarding: bool,

    /// Start a fresh session, ignoring any crash-recovery checkpoint
    #[arg(long = "fresh")]
    fresh: bool,

    /// Skip loading project-level config from $WORKSPACE/.codewhale/config.toml
    #[arg(long = "no-project-config")]
    no_project_config: bool,
}

#[derive(Subcommand, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Run system diagnostics and check configuration
    Doctor(DoctorArgs),
    /// Summarize failure signals from a local JSONL session log without raw content
    SessionDiagnostics(SessionDiagnosticsArgs),
    /// Bootstrap MCP config and/or skills directories
    Setup(SetupArgs),
    /// Generate a remote Codewhale agent deploy bundle (cloud + chat bridge)
    RemoteSetup(remote_setup::RemoteSetupArgs),
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// List saved sessions
    Sessions {
        /// Maximum number of sessions to display
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Search sessions by title
        #[arg(short, long)]
        search: Option<String>,
    },
    /// Create default AGENTS.md in current directory
    Init,
    /// Save an API key to the shared user config
    Login {
        /// API key to store (otherwise read from stdin)
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Remove the saved API key
    Logout,
    /// Manage provider authentication flows.
    Auth(TuiAuthArgs),
    /// List available models from the configured API endpoint
    Models(ModelsArgs),
    /// Generate speech audio with Xiaomi MiMo TTS models
    #[command(visible_alias = "tts")]
    Speech(SpeechArgs),
    /// Run a non-interactive prompt. Use --auto for agent-with-tools mode.
    Exec(ExecArgs),
    /// Manage local Agent Fleet runs and workers
    Fleet(FleetArgs),
    /// Internal model-free Workflow tool dispatcher used by Lane Runtime.
    #[command(name = "workflow-tool", hide = true)]
    WorkflowTool(WorkflowToolArgs),
    /// Run a code review over a git diff
    Review(ReviewArgs),
    /// Open the TUI pre-seeded with a GitHub PR's title, body, and diff
    Pr {
        /// PR number
        #[arg(value_name = "NUMBER")]
        number: u32,
        /// Repository in `owner/name` form. Defaults to the current
        /// workspace's `gh` config (i.e. the repo gh thinks you're in).
        #[arg(short = 'R', long)]
        repo: Option<String>,
        /// Skip `gh pr checkout` even if gh is available. By default
        /// the working tree is left as-is — checkout is opt-in via
        /// `--checkout` because dirty trees fail it loudly.
        #[arg(long, default_value_t = false)]
        checkout: bool,
    },
    /// Apply a patch file (or stdin) to the working tree
    Apply(ApplyArgs),
    /// Run the offline evaluation harness (no network/LLM calls)
    Eval(EvalArgs),
    /// Score a run's token/cache/cost from recorded turns; flag regressions vs a baseline
    Scorecard(ScorecardArgs),
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Execpolicy tooling
    Execpolicy(ExecpolicyCommand),
    /// Inspect feature flags
    Features(FeaturesCli),
    /// Run a command inside the sandbox
    Sandbox(SandboxArgs),
    /// Run a local server (e.g. MCP)
    Serve(ServeArgs),
    /// Resume a previous session by ID (use --last for most recent)
    Resume {
        /// Conversation/session id (UUID or prefix)
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,
        /// Continue the most recent session in this workspace without a picker
        #[arg(long = "last", default_value_t = false, conflicts_with = "session_id")]
        last: bool,
    },
    /// Fork a previous session by ID (use --last for most recent)
    Fork {
        /// Conversation/session id (UUID or prefix)
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,
        /// Fork the most recent session in this workspace without a picker
        #[arg(long = "last", default_value_t = false, conflicts_with = "session_id")]
        last: bool,
    },
}

#[derive(Args, Debug, Clone)]
#[command(after_help = "\
Examples:
  codewhale exec \"explain this function\"
  codewhale exec --auto \"list crates/ with ls\"
  codewhale exec --auto --output-format stream-json \"fix the failing test\"

Plain `codewhale exec` is a one-shot model response. Use `--auto` for
non-interactive agent-with-tools execution. `--auto` does not change the
sandbox posture or elevate a denied tool. Use `--sandbox danger-full-access`
or `--allow-sandbox-elevation` to explicitly authorize sandbox elevation.
")]
struct ExecArgs {
    /// Override model for this run
    #[arg(long)]
    model: Option<String>,
    /// Override the provider for this run (e.g. `deepseek`, `openrouter`).
    /// Non-secret identifier only — credentials still resolve from the
    /// environment/config. Fleet uses this to launch a worker on its
    /// profile-pinned provider even when the parent session is on another
    /// one (#4093).
    #[arg(long)]
    provider: Option<String>,
    /// Override reasoning/thinking effort for this run.
    /// Accepted values: auto, off, low, medium, high, max.
    #[arg(long = "reasoning-effort", value_name = "EFFORT")]
    reasoning_effort: Option<String>,
    /// Enable agent-with-tools mode with automatic tool approvals. This does
    /// not authorize sandbox elevation.
    #[arg(long, default_value_t = false)]
    auto: bool,
    /// Sandbox policy for this exec run; independent from --auto.
    #[arg(long, value_name = "POLICY")]
    sandbox: Option<String>,
    /// Explicitly allow a denied tool to retry with danger-full-access.
    #[arg(long, default_value_t = false)]
    allow_sandbox_elevation: bool,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false, conflicts_with = "output_format")]
    json: bool,
    /// Resume a previous session by ID or prefix
    #[arg(long, value_name = "SESSION_ID", conflicts_with_all = ["session_id", "continue_session"])]
    resume: Option<String>,
    /// Resume a previous session by ID or prefix
    #[arg(long = "session-id", value_name = "SESSION_ID", conflicts_with_all = ["resume", "continue_session"])]
    session_id: Option<String>,
    /// Continue the most recent session for this workspace
    #[arg(long = "continue", default_value_t = false, conflicts_with_all = ["resume", "session_id"])]
    continue_session: bool,
    /// Output format for exec mode
    #[arg(long, value_enum, default_value_t = ExecOutputFormat::Text)]
    output_format: ExecOutputFormat,
    /// Comma-separated list of tools to allow (all others denied).
    /// Lowercase catalog names: read_file, write_file, exec_shell, grep_files, etc.
    #[arg(long, value_delimiter = ',')]
    allowed_tools: Option<Vec<String>>,
    /// Comma-separated list of tools to deny (deny wins over allow).
    #[arg(long, value_delimiter = ',')]
    disallowed_tools: Option<Vec<String>>,
    /// Maximum number of model steps (tool calls) before the run ends.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_turns: Option<u32>,
    /// Extra text appended to the system prompt for this run.
    #[arg(long)]
    append_system_prompt: Option<String>,
    /// Prompt to send to the model
    #[arg(
        value_name = "PROMPT",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    prompt: Vec<String>,
}

#[derive(Args, Debug, Clone)]
struct WorkflowToolArgs {
    /// Authority provenance stamped by the public `workflow run` command.
    #[arg(long, value_name = "SOURCE")]
    approval_source: String,
    /// Exact Workflow tool input serialized as one JSON object.
    #[arg(long, value_name = "JSON")]
    input_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExecOutputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Args, Debug, Clone)]
struct TuiAuthArgs {
    #[command(subcommand)]
    command: TuiAuthCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum TuiAuthCommand {
    /// Sign in to xAI/Grok with an SSH-friendly device code.
    #[command(name = "xai-device")]
    XaiDevice,
}

const CODEWHALE_TOOL_SURFACE_ENV: &str = "CODEWHALE_TOOL_SURFACE";
const SHELL_ONLY_EXEC_TOOLS: &[&str] = &["exec_shell", "exec_shell_wait", "exec_shell_interact"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecToolSurface {
    ShellOnly,
}

fn exec_tool_surface_from_env() -> Option<ExecToolSurface> {
    std::env::var(CODEWHALE_TOOL_SURFACE_ENV)
        .ok()
        .and_then(|value| {
            if should_warn_unknown_exec_tool_surface(&value) {
                eprintln!(
                    "warning: unrecognized {CODEWHALE_TOOL_SURFACE_ENV}; leaving exec tool surface unchanged. Use `shell-only`, `full`, or `native-tools`."
                );
            }
            parse_exec_tool_surface(&value)
        })
}

fn parse_exec_tool_surface(value: &str) -> Option<ExecToolSurface> {
    match value.trim().to_ascii_lowercase().as_str() {
        "shell-only" | "shell_only" | "shell" => Some(ExecToolSurface::ShellOnly),
        "full" | "native-tools" | "native_tools" | "" => None,
        _ => None,
    }
}

fn should_warn_unknown_exec_tool_surface(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !matches!(
        normalized.as_str(),
        "" | "shell-only" | "shell_only" | "shell" | "full" | "native-tools" | "native_tools"
    )
}

fn normalize_exec_tool_names(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .map(|name| name.to_ascii_lowercase().trim().to_string())
        .collect()
}

fn shell_only_exec_allowed_tools() -> Vec<String> {
    SHELL_ONLY_EXEC_TOOLS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

fn resolve_exec_allowed_tools(
    cli_allowed_tools: Option<&[String]>,
    env_tool_surface: Option<ExecToolSurface>,
) -> Option<Vec<String>> {
    if let Some(tools) = cli_allowed_tools {
        return Some(normalize_exec_tool_names(tools));
    }

    env_tool_surface.map(|ExecToolSurface::ShellOnly| shell_only_exec_allowed_tools())
}

#[derive(Args, Debug, Clone)]
struct FleetArgs {
    #[command(subcommand)]
    command: FleetCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum FleetCommand {
    /// Initialize the local fleet ledger for this workspace
    Init,
    /// Create a run from a task spec and start the foreground manager loop
    Run(FleetRunArgs),
    /// Show queued/running/completed/failed/stale fleet counts
    Status,
    /// Inspect one worker's status, heartbeat, latest event, and artifacts
    Inspect {
        /// Worker id printed by `codewhale fleet run`
        worker_id: String,
    },
    /// Print bounded log artifacts for one worker
    Logs {
        /// Worker id printed by `codewhale fleet run`
        worker_id: String,
    },
    /// List artifact refs for one worker
    Artifacts {
        /// Worker id printed by `codewhale fleet run`
        worker_id: String,
    },
    /// Interrupt a running worker task and record a terminal cancellation
    Interrupt {
        /// Worker id printed by `codewhale fleet run`
        worker_id: String,
    },
    /// Restart the latest task for a worker
    Restart {
        /// Worker id printed by `codewhale fleet run`
        worker_id: String,
    },
    /// Resume a run from durable ledger state, reconciling orphaned/stale leases
    Resume {
        /// Run id printed by `codewhale fleet run`
        run_id: String,
        /// Seconds without heartbeat before a leased task is treated as stale
        #[arg(long, default_value_t = 300)]
        stale_after_seconds: u64,
    },
    /// Stop all queued and running fleet work
    Stop {
        /// Confirm stopping all queued and running fleet tasks
        #[arg(long, required = true)]
        all: bool,
    },
    /// Render a redacted fleet alert payload without sending it
    AlertDryRun(FleetAlertDryRunArgs),
}

#[derive(Args, Debug, Clone)]
struct FleetRunArgs {
    /// JSON or TOML task spec to enqueue
    #[arg(value_name = "TASK_SPEC")]
    task_spec: PathBuf,
    /// Maximum local workers to lease concurrently
    #[arg(long, default_value_t = 4)]
    max_workers: usize,
    /// Seconds without heartbeat before a running task is counted stale
    #[arg(long, default_value_t = 300)]
    stale_after_seconds: u64,
    /// Schedule once and return instead of staying in the manager loop
    #[arg(long, hide = true, default_value_t = false)]
    once: bool,
}

#[derive(Args, Debug, Clone)]
struct FleetAlertDryRunArgs {
    /// Alert event class to render
    #[arg(long, value_enum)]
    event: FleetAlertEventArg,
    /// Fleet run id
    #[arg(long)]
    run_id: String,
    /// Worker id, when the event belongs to one worker
    #[arg(long)]
    worker_id: Option<String>,
    /// Task id, when the event belongs to one task
    #[arg(long)]
    task_id: Option<String>,
    /// Short human-readable reason for the alert
    #[arg(long, default_value = "manual fleet alert dry-run")]
    reason: String,
    /// Status label to include in the payload
    #[arg(long)]
    status: Option<String>,
    /// Adapter payload shape to render
    #[arg(long, value_enum, default_value_t = FleetAlertAdapterArg::Slack)]
    adapter: FleetAlertAdapterArg,
    /// Environment variable containing the Slack webhook URL
    #[arg(long, default_value = "CODEWHALE_FLEET_SLACK_WEBHOOK")]
    slack_webhook_env: String,
    /// Environment variable containing the generic webhook URL
    #[arg(long, default_value = "CODEWHALE_FLEET_WEBHOOK_URL")]
    webhook_url_env: String,
    /// Optional environment variable containing the generic webhook secret
    #[arg(long)]
    webhook_secret_env: Option<String>,
    /// Environment variable containing the PagerDuty routing key
    #[arg(long, default_value = "CODEWHALE_FLEET_PAGERDUTY_ROUTING_KEY")]
    pagerduty_routing_key_env: String,
    /// PagerDuty severity to render
    #[arg(long, default_value = "error")]
    pagerduty_severity: String,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum FleetAlertEventArg {
    Stale,
    RestartExhausted,
    NeedsHuman,
    BudgetExceeded,
    VerifierFailed,
    RunCompleted,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
enum FleetAlertAdapterArg {
    Slack,
    Webhook,
    PagerDuty,
}

/// Spawn a tokio task that listens for terminating signals (SIGINT
/// always; SIGTERM and SIGHUP on Unix) and, on receipt, restores the
/// terminal modes and exits with the conventional 128 + signal code.
/// Multiple deliveries are tolerated: once the cleanup runs, a second
/// signal short-circuits to plain exit so a stuck cleanup can never
/// trap a frustrated user pressing Ctrl+C repeatedly.
///
/// See the call site in `main` for the rationale (#1583).
fn spawn_signal_cleanup_task() {
    tokio::spawn(async {
        let exit_code = wait_for_terminating_signal().await;
        // If we get here a fatal signal arrived. Restore the terminal
        // and exit. A second signal during cleanup re-enters this
        // path and aborts via `std::process::exit` directly.
        static CLEANED_UP: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !CLEANED_UP.swap(true, std::sync::atomic::Ordering::SeqCst) {
            crate::tui::ui::emergency_restore_terminal();
        }
        std::process::exit(exit_code);
    });
}

#[cfg(unix)]
async fn wait_for_terminating_signal() -> i32 {
    use tokio::signal::unix::{SignalKind, signal};
    // Failing to install any individual stream is non-fatal: we still
    // want the others to work. The fallback never-resolving future
    // keeps `select!` well-typed when a stream fails to register.
    let mut sigint = signal(SignalKind::interrupt()).ok();
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sighup = signal(SignalKind::hangup()).ok();
    tokio::select! {
        _ = async { match sigint.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 130,
        _ = async { match sigterm.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 143,
        _ = async { match sighup.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 129,
    }
}

#[cfg(not(unix))]
async fn wait_for_terminating_signal() -> i32 {
    // Windows: tokio::signal::ctrl_c covers both Ctrl+C and Ctrl+Break
    // (CTRL_C_EVENT / CTRL_BREAK_EVENT). Console-close, logoff, and
    // shutdown events are not currently routed through tokio.
    let _ = tokio::signal::ctrl_c().await;
    130
}

fn join_prompt_parts(parts: &[String]) -> String {
    parts.join(" ")
}

fn resolve_exec_model(config: &Config, explicit_model: Option<&str>) -> String {
    explicit_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .or_else(exec_model_env_override)
        .unwrap_or_else(|| config.default_model())
}

fn apply_exec_provider_override(config: &mut Config, provider_arg: &str) -> Result<()> {
    let provider_arg = provider_arg.trim();
    if provider_arg.is_empty() {
        return Ok(());
    }
    if config
        .providers
        .as_ref()
        .and_then(|providers| providers.custom_provider_config(provider_arg))
        .is_some()
    {
        config.provider = Some(provider_arg.to_string());
        return Ok(());
    }
    if let Some(provider) = crate::config::ApiProvider::parse(provider_arg) {
        config.provider = Some(provider.as_str().to_string());
        return Ok(());
    }
    bail!(
        "Unrecognized --provider {provider_arg:?}. Known providers: {} \
         or a configured [providers.<name>] custom provider",
        crate::config::ApiProvider::names_hint()
    );
}

fn exec_model_env_override() -> Option<String> {
    ["CODEWHALE_MODEL", "DEEPSEEK_MODEL"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        })
}

fn top_level_prompt_initial_input(parts: &[String]) -> Option<tui::InitialInput> {
    (!parts.is_empty()).then(|| tui::InitialInput::Submit(join_prompt_parts(parts)))
}

fn resolve_exec_resume_session_id(args: &ExecArgs, workspace: &Path) -> Result<Option<String>> {
    if let Some(id) = args.resume.as_ref().or(args.session_id.as_ref()) {
        return Ok(Some(id.clone()));
    }
    if !args.continue_session {
        return Ok(None);
    }
    latest_session_id_for_workspace(workspace)?.map_or_else(
        || {
            bail!(
                "No saved sessions found for workspace {}. Use `codewhale sessions` to list sessions, or pass `codewhale exec --resume <SESSION_ID> ...`.",
                workspace.display()
            )
        },
        |id| Ok(Some(id)),
    )
}

fn load_exec_resume_session(session_id: &str) -> Result<session_manager::SavedSession> {
    let session_ref = exec_stream_session_ref(session_id);
    SessionManager::default_location()
        .context("could not open session manager for resume")?
        .load_session_by_prefix(session_id)
        .with_context(|| format!("could not load session {session_ref}"))
}

/// Select the route for `exec --resume` before any engine/client is built.
///
/// Precedence is intentionally field-aware:
/// - no explicit `--provider` or `--model`: restore the saved provider/model;
/// - explicit `--provider`: keep that route and use its configured/default model
///   unless `--model` is also present;
/// - explicit `--model` alone: restore the saved provider, then use that model.
fn resolve_exec_resume_route(
    config: &mut Config,
    saved: &session_manager::SavedSession,
    explicit_provider: bool,
    explicit_model: Option<&str>,
) -> Result<String> {
    if !explicit_provider {
        let saved_provider_identity = saved
            .metadata
            .model_provider_id
            .as_deref()
            .filter(|identity| !identity.trim().is_empty())
            .unwrap_or(&saved.metadata.model_provider);
        let identity = config
            .resolve_persisted_provider_identity(
                Some(&saved.metadata.model_provider),
                saved.metadata.model_provider_id.as_deref(),
            )
            .map_err(anyhow::Error::msg)
            .with_context(|| {
                format!(
                    "saved session provider '{}' is unavailable; Codewhale will not fall back",
                    saved_provider_identity
                )
            })?;
        config.scope_to_provider_identity(&identity);
    }

    if let Some(model) = explicit_model {
        return Ok(resolve_exec_model(config, Some(model)));
    }
    if explicit_provider {
        return Ok(resolve_exec_model(config, None));
    }
    Ok(saved.metadata.model.clone())
}

#[derive(Args, Debug, Clone, Default)]
struct SetupArgs {
    /// Initialize MCP configuration at the configured path
    #[arg(long, default_value_t = false)]
    mcp: bool,
    /// Initialize skills directory and an example skill
    #[arg(long, default_value_t = false)]
    skills: bool,
    /// Initialize tools directory with a self-describing example script
    #[arg(long, default_value_t = false)]
    tools: bool,
    /// Initialize plugins directory with a self-describing example
    #[arg(long, default_value_t = false)]
    plugins: bool,
    /// Initialize MCP config, skills, tools, and plugins
    #[arg(long, default_value_t = false)]
    all: bool,
    /// Create a local workspace skills directory (./skills)
    #[arg(long, default_value_t = false)]
    local: bool,
    /// Overwrite existing template files
    #[arg(long, default_value_t = false)]
    force: bool,
    /// Print a compact, read-only status report (no network calls)
    #[arg(long, default_value_t = false, conflicts_with_all = ["mcp", "skills", "tools", "plugins", "all", "local", "clean"])]
    status: bool,
    /// Remove regenerable session checkpoints (latest + offline_queue)
    #[arg(long, default_value_t = false, conflicts_with_all = ["mcp", "skills", "tools", "plugins", "all", "local", "status"])]
    clean: bool,
}

#[derive(Args, Debug, Clone, Default)]
struct DoctorArgs {
    /// Emit machine-readable JSON output (skips live API connectivity check)
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Emit only the diagnostic context source map as JSON
    #[arg(long, default_value_t = false, conflicts_with = "json")]
    context_json: bool,
    /// Opt in to probing a local provider endpoint (may start a local service)
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    probe_local: bool,
}

#[derive(Args, Debug, Clone)]
struct SessionDiagnosticsArgs {
    /// JSONL session log to inspect
    #[arg(value_name = "JSONL")]
    path: PathBuf,
    /// Emit machine-readable JSON with redacted source handles
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct ScorecardArgs {
    /// JSON file with the recorded turns to score: an array of
    /// `{ "turn_id", "provider", "model", "billing_surface", "usage": {…} }`.
    /// `turn_end` hooks emit this route provenance plus `created_at`; persisted
    /// runtime exports may instead use `id`, `effective_provider`,
    /// `effective_model`, and `effective_billing_surface`.
    /// Shell-only hook rows marked `model_backed: false` are excluded. Legacy
    /// rows without provider remain readable but their cost is unavailable.
    #[arg(long, value_name = "FILE")]
    input: PathBuf,
    /// Optional baseline scorecard-metrics JSON to compare against. When set,
    /// the command exits non-zero if any metric regresses past the threshold.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,
    /// Regression threshold, in percent increase over the baseline.
    #[arg(long, default_value_t = 5.0)]
    threshold: f64,
    /// Emit machine-readable JSON instead of the human summary.
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct EvalArgs {
    /// Intentionally fail a specific step (list, read, search, edit, patch, shell)
    #[arg(long, value_name = "STEP")]
    fail_step: Option<String>,
    /// Shell command to run during the exec step
    #[arg(long, default_value = "printf eval-harness")]
    shell_command: String,
    /// Token that must appear in shell output for validation
    #[arg(long, default_value = "eval-harness")]
    shell_expect_token: String,
    /// Maximum characters stored per step output summary
    #[arg(long, default_value_t = 240)]
    max_output_chars: usize,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Append one JSONL fixture line per step to `<DIR>/<scenario>.jsonl`.
    /// Mock LLM tests can later replay these fixtures.
    #[arg(long, value_name = "DIR")]
    record: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default)]
struct ModelsArgs {
    /// Print models as pretty JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct SpeechArgs {
    /// Text to synthesize. This is sent as the assistant message content.
    #[arg(value_name = "TEXT")]
    text: String,

    /// Output audio path. Defaults to speech.<format> in --output-dir,
    /// [speech].output_dir, or the current directory.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Directory for the default speech.<format> output file when -o/--output is omitted.
    #[arg(long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// TTS model. Defaults to built-in voices, or is inferred from --voice-prompt/--clone-voice.
    #[arg(long)]
    model: Option<String>,

    /// Built-in voice ID, or a data:audio/...;base64,... URI for voice clone.
    #[arg(long)]
    voice: Option<String>,

    /// Natural language style instruction; not spoken verbatim.
    #[arg(long)]
    instruction: Option<String>,

    /// Voice design prompt. Implies mimo-v2.5-tts-voicedesign when --model is omitted.
    #[arg(long = "voice-prompt")]
    voice_prompt: Option<String>,

    /// MP3/WAV sample used for voice cloning. Implies mimo-v2.5-tts-voiceclone when --model is omitted.
    #[arg(long = "clone-voice", value_name = "FILE")]
    clone_voice: Option<PathBuf>,

    /// Output audio format requested from the API
    #[arg(long, default_value = "wav")]
    format: String,

    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug, Default, Clone)]
struct FeatureToggles {
    /// Enable a feature (repeatable). Equivalent to `features.<name>=true`.
    #[arg(long = "enable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    enable: Vec<String>,

    /// Disable a feature (repeatable). Equivalent to `features.<name>=false`.
    #[arg(long = "disable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    disable: Vec<String>,
}

impl FeatureToggles {
    fn apply(&self, config: &mut Config) -> Result<()> {
        for feature in &self.enable {
            config.set_feature(feature, true)?;
        }
        for feature in &self.disable {
            config.set_feature(feature, false)?;
        }
        Ok(())
    }
}

#[derive(Args, Debug, Clone)]
struct ReviewArgs {
    /// Review staged changes instead of the working tree
    #[arg(long, conflicts_with = "base")]
    staged: bool,
    /// Base ref to diff against (e.g. origin/main)
    #[arg(long)]
    base: Option<String>,
    /// Limit diff to a specific path
    #[arg(long)]
    path: Option<PathBuf>,
    /// Override model for this review
    #[arg(long)]
    model: Option<String>,
    /// Maximum diff characters to include
    #[arg(long, default_value_t = 200_000)]
    max_chars: usize,
    /// Write a durable pre-push review receipt after a successful review
    #[arg(long, default_value_t = false)]
    write_receipt: bool,
    /// Validate the current diff against a durable review receipt without calling a model
    #[arg(long, default_value_t = false)]
    check_receipt: bool,
    /// Override where the review receipt is written or read
    #[arg(long)]
    receipt_path: Option<PathBuf>,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct ApplyArgs {
    /// Patch file to apply (defaults to stdin)
    #[arg(value_name = "PATCH_FILE")]
    patch_file: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
struct ServeArgs {
    /// Start MCP server over stdio
    #[arg(long)]
    mcp: bool,
    /// Start runtime HTTP/SSE API server
    #[arg(long)]
    http: bool,
    /// Start runtime HTTP/SSE API server with the built-in mobile control page
    #[arg(long)]
    mobile: bool,
    /// Start the embedded loopback-only browser client and open it
    #[arg(long)]
    web: bool,
    /// Show a QR code for the mobile URL in the terminal (requires --mobile)
    #[arg(long, requires = "mobile")]
    qr: bool,
    /// Start ACP server over stdio for editor clients such as Zed
    #[arg(long)]
    acp: bool,
    /// Bind host for HTTP server (default localhost; --mobile defaults to 0.0.0.0)
    #[arg(long)]
    host: Option<String>,
    /// Bind port for HTTP server
    #[arg(long, default_value_t = 7878)]
    port: u16,
    /// Background task worker count (1-8)
    #[arg(long, default_value_t = 2)]
    workers: usize,
    /// Additional CORS origin to allow (repeatable). Stacks on top of the
    /// built-in defaults (localhost:3000, localhost:1420, tauri://localhost).
    /// Also reads `CODEWHALE_CORS_ORIGINS` (comma-separated), then
    /// `DEEPSEEK_CORS_ORIGINS` as an alias, and `[runtime_api] cors_origins`
    /// from `config.toml`. Whalescale#255.
    #[arg(long = "cors-origin", value_name = "URL")]
    cors_origin: Vec<String>,
    /// Require this bearer token for `/v1/*` runtime API routes. Also reads
    /// `CODEWHALE_RUNTIME_TOKEN` when omitted, then `DEEPSEEK_RUNTIME_TOKEN`
    /// as an alias.
    #[arg(long = "auth-token", value_name = "TOKEN")]
    auth_token: Option<String>,
    /// Disable runtime API auth when no token is configured. Only use on a trusted loopback.
    #[arg(long = "insecure")]
    insecure_no_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServeBindHost {
    host: String,
    mobile_rebound_to_lan: bool,
}

fn resolve_serve_bind_host(mobile: bool, host: Option<String>) -> ServeBindHost {
    match (mobile, host) {
        (true, None) => ServeBindHost {
            host: "0.0.0.0".to_string(),
            mobile_rebound_to_lan: true,
        },
        (_, Some(host)) => ServeBindHost {
            host,
            mobile_rebound_to_lan: false,
        },
        (false, None) => ServeBindHost {
            host: "127.0.0.1".to_string(),
            mobile_rebound_to_lan: false,
        },
    }
}

fn validate_serve_mode_selection(
    mcp: bool,
    http: bool,
    mobile: bool,
    web: bool,
    acp: bool,
) -> Result<bool> {
    if http && mobile {
        bail!("--http and --mobile are mutually exclusive; choose one");
    }
    if web && (http || mobile) {
        bail!("--web is mutually exclusive with --http and --mobile");
    }
    let http_selected = http || mobile || web;
    let selected_modes = [mcp, http_selected, acp]
        .into_iter()
        .filter(|selected| *selected)
        .count();
    if selected_modes != 1 {
        bail!("Choose exactly one server mode: --mcp, --http/--mobile/--web, or --acp");
    }
    Ok(http_selected)
}

#[derive(Subcommand, Debug, Clone)]
enum McpCommand {
    /// List configured MCP servers
    List,
    /// Create a template MCP config at the configured path
    Init {
        /// Overwrite an existing MCP config file
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Connect to MCP servers and report status
    Connect {
        /// Optional server name to connect to
        #[arg(value_name = "SERVER")]
        server: Option<String>,
    },
    /// List tools discovered from MCP servers
    Tools {
        /// Optional server name to list tools for
        #[arg(value_name = "SERVER")]
        server: Option<String>,
    },
    /// Add an MCP server entry
    Add {
        /// Server name
        name: String,
        /// Command to launch stdio server
        #[arg(long, conflicts_with = "url")]
        command: Option<String>,
        /// URL for streamable HTTP/SSE server
        #[arg(long, conflicts_with = "command")]
        url: Option<String>,
        /// Explicit URL transport override. Use "sse" for legacy SSE endpoints.
        #[arg(long, requires = "url")]
        transport: Option<String>,
        /// Environment variable containing a bearer token for URL-based servers
        #[arg(long, requires = "url")]
        bearer_token_env_var: Option<String>,
        /// OAuth client ID for servers that do not support dynamic registration
        #[arg(long, requires = "url")]
        oauth_client_id: Option<String>,
        /// OAuth resource parameter to append to the authorization URL
        #[arg(long, requires = "url")]
        oauth_resource: Option<String>,
        /// OAuth scope to request during login. Repeat or comma-separate.
        #[arg(long = "scope", requires = "url", value_delimiter = ',')]
        scopes: Vec<String>,
        /// Arguments for command-based servers
        #[arg(long = "arg")]
        args: Vec<String>,
    },
    /// Authenticate to a URL-based MCP server using OAuth
    Login {
        /// Server name
        name: String,
        /// OAuth scope to request. Repeat or comma-separate; defaults to config/discovery.
        #[arg(long = "scope", value_delimiter = ',')]
        scopes: Vec<String>,
    },
    /// Delete stored OAuth credentials for a URL-based MCP server
    Logout {
        /// Server name
        name: String,
    },
    /// Remove an MCP server entry
    Remove {
        /// Server name
        name: String,
    },
    /// Enable an MCP server
    Enable {
        /// Server name
        name: String,
    },
    /// Disable an MCP server
    Disable {
        /// Server name
        name: String,
    },
    /// Validate MCP config and required servers
    Validate,
    /// Register this Codewhale binary as a local MCP stdio server.
    ///
    /// This adds a config entry that runs `codewhale serve --mcp` (stdio protocol).
    /// For the HTTP/SSE runtime API, use `codewhale serve --http` directly instead.
    #[command(
        name = "add-self",
        long_about = "Register this Codewhale binary as a local MCP stdio server.\n\nAdds a config entry to ~/.codewhale/mcp.json that launches `codewhale serve --mcp`\nvia the stdio transport. Other Codewhale sessions (or any MCP client) can then\ndiscover and call tools exposed by this server.\n\nUse `codewhale serve --http` instead if you need the HTTP/SSE runtime API."
    )]
    AddSelf {
        /// Server name in mcp.json (default: "codewhale")
        #[arg(long, default_value = "codewhale")]
        name: String,
        /// Workspace directory for the MCP server
        #[arg(long)]
        workspace: Option<String>,
    },
}

#[derive(Args, Debug, Clone)]
struct ExecpolicyCommand {
    #[command(subcommand)]
    command: ExecpolicySubcommand,
}

#[derive(Subcommand, Debug, Clone)]
enum ExecpolicySubcommand {
    /// Check execpolicy files against a command
    Check(execpolicy::ExecPolicyCheckCommand),
}

#[derive(Args, Debug, Clone)]
struct FeaturesCli {
    #[command(subcommand)]
    command: FeaturesSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
enum FeaturesSubcommand {
    /// List known feature flags and their state
    List,
}

#[derive(Args, Debug, Clone)]
struct SandboxArgs {
    #[command(subcommand)]
    command: SandboxCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum SandboxCommand {
    /// Run a command with sandboxing
    Run {
        /// Sandbox policy (danger-full-access, read-only, external-sandbox, workspace-write)
        #[arg(long, default_value = "workspace-write")]
        policy: String,
        /// Allow outbound network access
        #[arg(long)]
        network: bool,
        /// Additional writable roots (repeatable)
        #[arg(long, value_name = "PATH")]
        writable_root: Vec<PathBuf>,
        /// Exclude TMPDIR from writable paths
        #[arg(long)]
        exclude_tmpdir: bool,
        /// Exclude /tmp from writable paths
        #[arg(long)]
        exclude_slash_tmp: bool,
        /// Command working directory
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Timeout in milliseconds
        #[arg(long, default_value_t = 60_000)]
        timeout_ms: u64,
        /// Command and arguments to run
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
}

const CODEWHALE_MAIN_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> Result<()> {
    // Match the dispatcher entrypoint: Unix shells and supervisors may inherit
    // SIGPIPE ignored, which turns short pipelines such as `codewhale doctor |
    // head` into BrokenPipe panics once this delegated TUI binary prints.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    startup_trace::mark_process_start();
    configure_windows_console_utf8();
    install_rustls_crypto_provider();

    // ── Process hardening (#2183) ─────────────────────────────────────────
    // MUST run before Tokio is booted and before any threads are spawned.
    // See crates/tui/src/sandbox/process_hardening.rs for ordering rationale.
    crate::sandbox::process_hardening::apply_process_hardening();

    // Set up process panic hook before anything else — writes crash dumps
    // to ~/.deepseek/crashes/ even if the panic happens before tokio is up,
    // and restores the terminal so a panicked TUI doesn't leave the user's
    // shell stuck in alt-screen mode.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Restore the terminal first so the panic message itself, plus the
        // user's shell after exit, are visible. Best-effort — we may not be
        // in raw / alt-screen mode if the panic happens pre-TUI. Shared
        // with the signal handler installed below so both exit paths leave
        // the terminal in the same well-defined state.
        crate::tui::ui::emergency_restore_terminal();

        let msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            format!("{:?}", panic_info.payload())
        };
        let location = panic_info
            .location()
            .map(|loc| loc.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        tracing::error!(target: "panic", "Process panicked at {location}: {msg}");
        // Write crash dump best-effort
        if let Some(home) = dirs::home_dir() {
            let crash_dir = home.join(".deepseek").join("crashes");
            let _ = std::fs::create_dir_all(&crash_dir);
            use chrono::Utc;
            let ts = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
            let path = crash_dir.join(format!("{ts}-process-panic.log"));
            let contents =
                format!("Process panicked\nLocation: {location}\nTimestamp: {ts}\nPanic: {msg}\n",);
            let _ = std::fs::write(&path, contents);
        }
        // Invoke the original hook (prints to stderr, etc.)
        orig_hook(panic_info);
    }));

    // Parse and freeze every startup authority before Tokio or any other
    // worker thread exists. A workspace `.env` is intentionally a narrow
    // credential convenience surface: it must never redirect product state,
    // configuration, MCP, trust, sandbox, executable lookup, or plugin
    // discovery. Plugin discovery therefore runs first, and the loader below
    // admits only built-in provider credential names from a stable file read.
    let cli = Cli::parse();
    let workspace = resolve_workspace(&cli);
    let mut plugin_discovery = None;
    let mut plugin_registry = None;
    let (cli, command) = prepare_cli_startup(
        cli,
        || {
            let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
            plugin_registry = Some(discovery.registry_for_workspace(&workspace));
            plugin_discovery = Some(discovery);
        },
        warn_on_workspace_dotenv_result,
    );
    let plugin_discovery = plugin_discovery
        .expect("plugin discovery initialization must precede workspace dotenv loading");
    let plugin_registry = plugin_registry
        .expect("plugin discovery initialization must precede workspace dotenv loading");

    // The interactive runtime intentionally carries a large state machine:
    // terminal rendering, modal dispatch, provider setup, and fleet/workflow
    // events all share one async owner. Debug builds retain enough stack
    // temporaries that nesting a modal event over the TUI loop can exceed the
    // platform main-thread default (8 MiB on macOS). Give that owner an
    // explicit stack while keeping process hardening and the global panic hook
    // above this boundary, before Tokio or any worker thread exists.
    let runtime_thread = std::thread::Builder::new()
        .name("codewhale-main".to_string())
        .stack_size(CODEWHALE_MAIN_STACK_BYTES)
        .spawn(move || run_async_main(cli, command, plugin_discovery, plugin_registry))
        .context("Failed to start the Codewhale runtime thread")?;
    match runtime_thread.join() {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            Err(anyhow!("Codewhale runtime thread panicked: {message}"))
        }
    }
}

#[tokio::main]
async fn run_async_main(
    cli: Cli,
    command: Option<Commands>,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    // Install signal handlers that restore the terminal before the
    // process exits. Without this, Ctrl+C delivered while raw mode /
    // kitty keyboard enhancement / alt-screen are active (or in the
    // brief windows around startup and teardown where they're being
    // toggled) leaves the user's shell receiving raw CSI sequences
    // like `^[[>5u` until they run `reset` (#1583).
    //
    // Once the TUI's raw mode is engaged the terminal driver delivers
    // Ctrl+C as the byte 0x03 rather than SIGINT, so the in-TUI key
    // handler — not this handler — is what processes user interrupts
    // during normal operation. This handler exists for the gaps:
    // pre-TUI subcommands (--version, doctor, login, …), the moments
    // around enable_raw_mode / disable_raw_mode, the external-editor
    // suspend path, and SIGTERM / SIGHUP from the OS.
    spawn_signal_cleanup_task();

    logging::set_verbose(cli.verbose || logging::env_requests_verbose_logging());

    // Install any user prompt overrides from the config directory before an
    // engine can compose a system prompt. The override cells are
    // first-call-wins; doing this once here keeps every downstream turn
    // consistent. Missing files are a no-op (bundled defaults). See #3638.
    crate::prompts::load_prompt_overrides_from_config_home();

    // Plugins own one read-only discovery snapshot per process. Initialize it
    // before the subcommand match so plain launch, resume, fork, exec, serve,
    // and every other runtime surface feed Skills and MCP from the same trust
    // decision (#3916, #4399). Discovery never enables, trusts, executes, or
    // persists a bundle.

    // Handle subcommands first
    if let Some(command) = command {
        return match command {
            Commands::Doctor(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                if args.context_json {
                    run_doctor_context_json(&config, &workspace)
                } else if args.json {
                    run_doctor_json(
                        &config,
                        &workspace,
                        cli.config.as_deref(),
                        plugin_registry.as_ref(),
                    )
                } else {
                    run_doctor(
                        &config,
                        &workspace,
                        cli.config.as_deref(),
                        args.probe_local,
                        plugin_registry.as_ref(),
                    )
                    .await;
                    Ok(())
                }
            }
            Commands::SessionDiagnostics(args) => run_session_diagnostics(args),
            Commands::Setup(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                run_setup(&config, &workspace, args, plugin_registry.as_ref())
            }
            Commands::RemoteSetup(args) => remote_setup::run_remote_setup(args),
            Commands::Completions { shell } => {
                generate_completions(shell);
                Ok(())
            }
            Commands::Sessions { limit, search } => list_sessions(limit, search),
            Commands::Init => init_project(),
            Commands::Login { api_key } => run_login(api_key),
            Commands::Logout => run_logout(),
            Commands::Auth(args) => match args.command {
                TuiAuthCommand::XaiDevice => run_xai_device_auth(cli.config.as_deref()).await,
            },
            Commands::Models(args) => {
                let config = load_config_from_cli(&cli)?;
                run_models(&config, args).await
            }
            Commands::Speech(args) => {
                let config = load_config_from_cli(&cli)?;
                run_speech(&config, args).await
            }
            Commands::Exec(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = cli.workspace.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                let mut config = config.clone();
                merge_user_workspace_config(&mut config, cli.config.clone(), &workspace);
                if let Some(sandbox) = args.sandbox.as_deref() {
                    let _ = parse_sandbox_policy(sandbox, true, Vec::new(), false, false)?;
                    config.sandbox_mode = Some(sandbox.to_ascii_lowercase());
                }
                // Honour CODEWHALE_BASE_URL / DEEPSEEK_BASE_URL forwarded by
                // the CLI dispatcher from --base-url.
                if let Ok(env_url) = std::env::var("CODEWHALE_BASE_URL")
                    .or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
                {
                    let trimmed = env_url.trim();
                    if !trimmed.is_empty() {
                        config.base_url = Some(trimmed.to_string());
                    }
                }
                // Honour `--provider` (#4093): a Fleet worker whose profile pins
                // a provider launches on that provider even when the parent
                // session is on another one. This sets ONLY the non-secret
                // provider identity (`config.provider`); credentials/base URL
                // still resolve from the worker's own env/config, and for a
                // non-DeepSeek provider the legacy root `base_url` above is
                // ignored by `deepseek_base_url()`. Must precede model
                // resolution so an `auto`/default model resolves to the
                // overridden provider's default.
                let explicit_provider = args
                    .provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty());
                if let Some(provider_arg) = explicit_provider {
                    apply_exec_provider_override(&mut config, provider_arg)?;
                }
                if let Some(reasoning_arg) = args
                    .reasoning_effort
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    config.reasoning_effort = normalize_cli_reasoning_effort(reasoning_arg)?;
                }
                let prompt = join_prompt_parts(&args.prompt);
                let resume_session_id = resolve_exec_resume_session_id(&args, &workspace)?;
                let resume_session = resume_session_id
                    .as_deref()
                    .map(load_exec_resume_session)
                    .transpose()?;
                let explicit_model = args
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|model| !model.is_empty());
                let model = if let Some(saved) = resume_session.as_ref() {
                    resolve_exec_resume_route(
                        &mut config,
                        saved,
                        explicit_provider.is_some(),
                        explicit_model,
                    )?
                } else {
                    resolve_exec_model(&config, explicit_model)
                };
                let force_configured_route = should_force_configured_exec_route(
                    resume_session.is_some(),
                    explicit_provider,
                    explicit_model,
                );
                // The `deepseek` launcher forwards `--yolo` to this binary via
                // the DEEPSEEK_YOLO env var (which the config loader folds into
                // `config.yolo`), not as a CLI flag. Honour either source.
                let yolo = cli.yolo || config.yolo.unwrap_or(false);
                let env_tool_surface = exec_tool_surface_from_env();
                let needs_engine = args.auto
                    || yolo
                    || resume_session_id.is_some()
                    || args.output_format == ExecOutputFormat::StreamJson
                    || args.max_turns.is_some()
                    || args.allowed_tools.is_some()
                    || args.disallowed_tools.is_some()
                    || args.append_system_prompt.is_some()
                    || args.sandbox.is_some()
                    || args.allow_sandbox_elevation
                    || env_tool_surface.is_some();
                if needs_engine {
                    let provider = config.api_provider();
                    let max_subagents = cli.max_subagents.map_or_else(
                        || config.max_subagents_for_provider(provider),
                        |value| value.clamp(1, MAX_SUBAGENTS),
                    );
                    let auto_mode = args.auto || yolo;
                    let max_turns = args.max_turns.unwrap_or(100);
                    let allowed_tools =
                        resolve_exec_allowed_tools(args.allowed_tools.as_deref(), env_tool_surface);
                    let disallowed_tools = args
                        .disallowed_tools
                        .as_deref()
                        .map(normalize_exec_tool_names);
                    run_exec_agent(
                        &config,
                        &model,
                        &prompt,
                        workspace,
                        max_subagents,
                        auto_mode,
                        args.allow_sandbox_elevation,
                        args.sandbox.as_deref(),
                        auto_mode,
                        args.json,
                        resume_session,
                        force_configured_route,
                        args.output_format,
                        max_turns,
                        allowed_tools,
                        disallowed_tools,
                        args.append_system_prompt.clone(),
                        std::sync::Arc::clone(&plugin_registry),
                    )
                    .await
                } else if args.json {
                    run_one_shot_json(&config, &model, &prompt, force_configured_route).await
                } else {
                    run_one_shot(&config, &model, &prompt, force_configured_route).await
                }
            }
            Commands::Fleet(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                run_fleet_command(&workspace, &config, args).await
            }
            Commands::WorkflowTool(args) => {
                run_workflow_tool_command(&cli, args, std::sync::Arc::clone(&plugin_registry)).await
            }
            Commands::Review(args) => {
                let config = load_config_from_cli(&cli)?;
                run_review(&config, args).await
            }
            Commands::Pr {
                number,
                repo,
                checkout,
            } => {
                let config = load_config_from_cli(&cli)?;
                run_pr(
                    &cli,
                    &config,
                    number,
                    repo.as_deref(),
                    checkout,
                    Arc::clone(&plugin_registry),
                )
                .await
            }
            Commands::Apply(args) => run_apply(args),
            Commands::Eval(args) => run_eval(args),
            Commands::Scorecard(args) => run_scorecard(args),
            Commands::Mcp { command } => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                run_mcp_command(&config, &workspace, command, plugin_registry.as_ref()).await
            }
            Commands::Execpolicy(command) => {
                let config = load_config_from_cli(&cli)?;
                if !config.features().enabled(Feature::ExecPolicy) {
                    bail!(
                        "The `exec_policy` feature is disabled. Enable it in [features] or via profile."
                    );
                }
                run_execpolicy_command(command)
            }
            Commands::Features(command) => {
                let config = load_config_from_cli(&cli)?;
                run_features_command(&config, command)
            }
            Commands::Sandbox(args) => run_sandbox_command(args),
            Commands::Serve(args) => {
                let workspace = cli.workspace.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                let http_selected = validate_serve_mode_selection(
                    args.mcp,
                    args.http,
                    args.mobile,
                    args.web,
                    args.acp,
                )?;
                if args.mcp {
                    tokio::task::block_in_place(|| mcp_server::run_mcp_server(workspace))
                } else if http_selected {
                    let (config, config_profile) =
                        load_config_from_cli_with_effective_profile(&cli)?;
                    let cors_origins = resolve_cors_origins(&config, &args.cors_origin);
                    let bind_host = resolve_serve_bind_host(args.mobile, args.host);
                    if args.web && bind_host.host != "127.0.0.1" {
                        bail!("Codewhale web is loopback-only and must bind to 127.0.0.1");
                    }
                    if bind_host.mobile_rebound_to_lan {
                        println!(
                            "WARNING: --mobile is binding to 0.0.0.0 so LAN devices can reach the mobile control page. Use --host 127.0.0.1 to keep mobile loopback-only."
                        );
                    }
                    runtime_api::run_http_server(
                        config,
                        workspace,
                        std::sync::Arc::clone(&plugin_discovery),
                        runtime_api::RuntimeApiOptions {
                            host: bind_host.host,
                            port: args.port,
                            workers: args.workers.clamp(1, 8),
                            cors_origins,
                            auth_token: args.auth_token,
                            insecure_no_auth: args.insecure_no_auth,
                            mobile: args.mobile,
                            web: args.web,
                            show_qr: args.qr,
                            config_path: cli.config.clone(),
                            config_profile,
                        },
                    )
                    .await
                } else if args.acp {
                    let config = load_config_from_cli(&cli)?;
                    let model = config.default_model();
                    acp_server::run_acp_server(config, model, workspace).await
                } else {
                    unreachable!("server mode count checked above")
                }
            }
            Commands::Resume { session_id, last } => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                let resume_id = resolve_session_id(session_id, last, &workspace)?;
                run_interactive(
                    &cli,
                    &config,
                    Some(resume_id),
                    None,
                    std::sync::Arc::clone(&plugin_registry),
                )
                .await
            }
            Commands::Fork { session_id, last } => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                let new_session_id = fork_session(&config, session_id, last, &workspace)?;
                run_interactive(
                    &cli,
                    &config,
                    Some(new_session_id),
                    None,
                    std::sync::Arc::clone(&plugin_registry),
                )
                .await
            }
        };
    }

    // Top-level prompt mode: submit the initial prompt, then keep the TUI alive
    // for follow-up messages. Use `codewhale exec` for explicit non-interactive
    // one-shot behavior (#2370).
    let config = load_config_from_cli(&cli)?;
    if let Some(initial_input) = top_level_prompt_initial_input(&cli.prompt) {
        return run_interactive(
            &cli,
            &config,
            None,
            Some(initial_input),
            std::sync::Arc::clone(&plugin_registry),
        )
        .await;
    }

    // Handle session resume. Plain `codewhale` starts fresh: interrupted
    // snapshots are preserved for explicit resume, but never auto-attached.
    let resume_session_id = if cli.continue_session {
        let workspace = resolve_workspace(&cli);
        recover_interrupted_checkpoint_for_resume(&workspace)
            .or_else(|| latest_session_id_for_workspace(&workspace).ok().flatten())
    } else if let Some(id) = cli.resume.clone() {
        Some(id)
    } else if !cli.fresh {
        let workspace = resolve_workspace(&cli);
        preserve_interrupted_checkpoint_for_explicit_resume(&workspace);
        None
    } else {
        None
    };

    // Default: Interactive TUI
    // --yolo starts in YOLO mode (auto-approve; shell enabled)
    run_interactive(&cli, &config, resume_session_id, None, plugin_registry).await
}

fn prepare_cli_startup(
    cli: Cli,
    initialize_plugins: impl FnOnce(),
    load_dotenv: impl FnOnce(),
) -> (Cli, Option<Commands>) {
    initialize_plugins();
    load_dotenv();
    let command = cli.command.clone();
    (cli, command)
}

const MAX_WORKSPACE_DOTENV_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Default)]
struct WorkspaceDotenvReport {
    path: PathBuf,
    loaded: BTreeSet<String>,
    ignored: BTreeSet<String>,
}

/// Load the narrow, data-plane subset of a workspace `.env` before Tokio.
///
/// Repository content is not product authority. In particular, a committed
/// `.env` must not be able to redirect `CODEWHALE_HOME`, config/profile files,
/// MCP servers, plugin trust, executable lookup, sandbox/approval posture, or
/// network destinations. Shell-exported values and config/CLI arguments remain
/// the explicit surfaces for those controls.
fn warn_on_workspace_dotenv_result() {
    match load_workspace_dotenv_credentials() {
        Ok(Some(report)) if !report.ignored.is_empty() => {
            eprintln!(
                "Codewhale ignored non-credential settings in {}: {}. Use config.toml, CLI flags, or the launching shell for control settings.",
                report.path.display(),
                display_env_key_set(&report.ignored)
            );
        }
        Ok(_) => {}
        Err(error) => {
            // The error intentionally contains no file contents or parsed
            // values. A malformed or unsafe workspace file fails closed while
            // shell/config credentials remain available.
            eprintln!("Codewhale did not load workspace .env: {error}");
        }
    }
}

fn display_env_key_set(keys: &BTreeSet<String>) -> String {
    const MAX_DISPLAYED: usize = 12;
    let mut labels = keys
        .iter()
        .take(MAX_DISPLAYED)
        .map(|key| {
            if key
                .chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
            {
                key.as_str()
            } else {
                "<invalid-name>"
            }
        })
        .collect::<Vec<_>>();
    if keys.len() > MAX_DISPLAYED {
        labels.push("...");
    }
    labels.join(", ")
}

fn load_workspace_dotenv_credentials() -> Result<Option<WorkspaceDotenvReport>> {
    let Some(path) = find_workspace_dotenv()? else {
        return Ok(None);
    };
    load_workspace_dotenv_credentials_from_path(&path).map(Some)
}

fn find_workspace_dotenv() -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().context("could not resolve the current workspace")?;
    let boundary = cwd
        .ancestors()
        .find(|ancestor| std::fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .unwrap_or(cwd.as_path());

    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(".env");
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow!(
                    "could not inspect {}: {error}",
                    candidate.display()
                ));
            }
        }
        if ancestor == boundary {
            break;
        }
    }
    Ok(None)
}

fn load_workspace_dotenv_credentials_from_path(path: &Path) -> Result<WorkspaceDotenvReport> {
    let contents = read_stable_workspace_dotenv(path)?;
    let text = std::str::from_utf8(&contents)
        .map_err(|_| anyhow!("{} is not valid UTF-8", path.display()))?;
    if dotenv_has_variable_expansion(text) {
        bail!(
            "{} uses variable expansion; workspace .env values must be literal to prevent ambient-secret substitution",
            path.display()
        );
    }

    let mut report = WorkspaceDotenvReport {
        path: path.to_path_buf(),
        ..WorkspaceDotenvReport::default()
    };
    let entries = dotenvy::from_read_iter(std::io::Cursor::new(contents))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| anyhow!("{} could not be parsed safely", path.display()))?;
    for entry in entries {
        let (key, value) = entry;
        if !is_workspace_dotenv_credential_key(&key) {
            report.ignored.insert(key);
            continue;
        }
        if std::env::var_os(&key).is_some() {
            continue;
        }

        // SAFETY: this loader runs synchronously in `main` before the runtime
        // owner or Tokio workers are spawned. No concurrent environment reader
        // exists inside Codewhale, and later startup code treats this process
        // environment as immutable.
        unsafe { std::env::set_var(&key, value) };
        report.loaded.insert(key);
    }
    Ok(report)
}

fn is_workspace_dotenv_credential_key(key: &str) -> bool {
    codewhale_config::provider::providers_sorted_for_display()
        .into_iter()
        .any(|provider| provider.env_vars().contains(&key))
        || matches!(
            key,
            "DEEPSEEK_SEARCH_API_KEY"
                | "SOFYA_API_KEY"
                | "METASO_API_KEY"
                | "BAIDU_SEARCH_API_KEY"
                | "DEEPSEEK_SANDBOX_API_KEY"
        )
}

fn dotenv_has_variable_expansion(contents: &str) -> bool {
    let mut escaped = false;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut comment = false;

    for ch in contents.chars() {
        if comment {
            // Reject expansion markers even in comments. This is deliberately
            // conservative, and ignoring other comment text prevents an
            // unmatched quote there from changing how the next line is read.
            if ch == '$' {
                return true;
            }
            if ch == '\n' {
                comment = false;
                escaped = false;
            }
            continue;
        }
        if single_quoted {
            if ch == '\'' {
                single_quoted = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '\'' && !double_quoted {
            single_quoted = true;
            continue;
        }
        if ch == '"' {
            double_quoted = !double_quoted;
            continue;
        }
        if ch == '#' && !double_quoted {
            comment = true;
            continue;
        }
        if ch == '$' {
            return true;
        }
    }
    false
}

fn read_stable_workspace_dotenv(path: &Path) -> Result<Vec<u8>> {
    let mut file = open_workspace_dotenv_without_following_links(path)?;
    let metadata = file
        .metadata()
        .map_err(|error| anyhow!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    if workspace_dotenv_has_multiple_links(&file, &metadata)? {
        bail!(
            "{} has multiple filesystem links, not a unique workspace-owned file",
            path.display()
        );
    }
    if metadata.len() > MAX_WORKSPACE_DOTENV_BYTES {
        bail!(
            "{} exceeds the {} byte workspace .env limit",
            path.display(),
            MAX_WORKSPACE_DOTENV_BYTES
        );
    }

    let mut contents = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_WORKSPACE_DOTENV_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| anyhow!("could not read {}: {error}", path.display()))?;
    if contents.len() as u64 > MAX_WORKSPACE_DOTENV_BYTES {
        bail!(
            "{} exceeds the {} byte workspace .env limit",
            path.display(),
            MAX_WORKSPACE_DOTENV_BYTES
        );
    }
    Ok(contents)
}

#[cfg(unix)]
fn workspace_dotenv_has_multiple_links(
    _file: &std::fs::File,
    metadata: &std::fs::Metadata,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    Ok(metadata.nlink() > 1)
}

#[cfg(windows)]
fn workspace_dotenv_has_multiple_links(
    file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a live kernel handle for the already-open `.env`;
    // `information` remains writable for the duration of this synchronous
    // call. No path lookup or re-open occurs here.
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
            .map_err(|error| anyhow!("could not inspect workspace .env link count: {error}"))?;
    }
    Ok(information.nNumberOfLinks > 1)
}

#[cfg(not(any(unix, windows)))]
fn workspace_dotenv_has_multiple_links(
    _file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<bool> {
    Ok(false)
}

#[cfg(unix)]
fn open_workspace_dotenv_without_following_links(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        // `O_NONBLOCK` is inert for regular files but prevents a FIFO named
        // `.env` from hanging startup before the metadata check can reject it.
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| anyhow!("could not securely open {}: {error}", path.display()))
}

#[cfg(windows)]
fn open_workspace_dotenv_without_following_links(path: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| anyhow!("could not securely open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| anyhow!("could not inspect {}: {error}", path.display()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        bail!(
            "{} is a reparse point, not a workspace-owned file",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_workspace_dotenv_without_following_links(path: &Path) -> Result<std::fs::File> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| anyhow!("could not inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "{} is a symbolic link, not a workspace-owned file",
            path.display()
        );
    }
    std::fs::File::open(path)
        .map_err(|error| anyhow!("could not securely open {}: {error}", path.display()))
}

/// Generate shell completions for the given shell
fn generate_completions(shell: Shell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    generate(shell, &mut cmd, name, &mut io::stdout());
}

/// Run the offline evaluation harness (no network/LLM calls).
fn run_eval(args: EvalArgs) -> Result<()> {
    let fail_step = match args.fail_step.as_deref() {
        Some(value) => ScenarioStepKind::parse(value)
            .map(Some)
            .ok_or_else(|| anyhow!("invalid --fail-step '{value}'"))?,
        None => None,
    };

    let config = EvalHarnessConfig {
        fail_step,
        shell_command: args.shell_command,
        shell_expect_token: args.shell_expect_token,
        max_output_chars: args.max_output_chars,
        record_dir: args.record.clone(),
        ..EvalHarnessConfig::default()
    };

    let harness = EvalHarness::new(config);
    let run = harness.run().context("evaluation harness failed")?;
    let report = run.to_report();

    if args.json {
        let json = serde_json::to_string_pretty(&report)?;
        println!("{json}");
    } else {
        println!("Offline Eval Harness");
        println!("scenario: {}", report.scenario_name);
        println!("workspace: {}", report.workspace_root.display());
        println!("success: {}", report.metrics.success);
        println!("steps: {}", report.metrics.steps);
        println!("tool_errors: {}", report.metrics.tool_errors);
        println!("duration_ms: {}", report.metrics.duration.as_millis());

        if !report.metrics.per_tool.is_empty() {
            println!("per_tool:");
            for (kind, stats) in &report.metrics.per_tool {
                println!(
                    "  {} invocations={} errors={} duration_ms={}",
                    kind.tool_name(),
                    stats.invocations,
                    stats.errors,
                    stats.total_duration.as_millis()
                );
            }
        }

        let failed_steps: Vec<_> = report.steps.iter().filter(|s| !s.success).collect();
        if !failed_steps.is_empty() {
            println!("failed_steps:");
            for step in failed_steps {
                let error = step.error.as_deref().unwrap_or("unknown error");
                println!(
                    "  {} tool={} error={}",
                    step.kind.tool_name(),
                    step.tool_name,
                    error
                );
            }
        }
    }

    if report.metrics.success {
        Ok(())
    } else {
        bail!("offline evaluation harness reported failure")
    }
}

/// Score a run's token/cache/cost from recorded turns and (optionally) flag
/// regressions against a committed baseline. Offline: reads recorded usage from
/// a JSON file, reuses the pricing layer, never calls a model. Exits non-zero
/// when a baseline is supplied and a metric regresses past the threshold, so it
/// can be wired as a release gate (#3388).
fn run_scorecard(args: ScorecardArgs) -> Result<()> {
    use crate::scorecard::{RecordedTurn, Scorecard, ScorecardMetrics};

    let raw = std::fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read scorecard input {}", args.input.display()))?;
    let recorded: Vec<RecordedTurn> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse scorecard input {}", args.input.display()))?;

    let card = Scorecard::from_recorded_turns(&recorded);

    let regressions = match &args.baseline {
        Some(path) => {
            let baseline_raw = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read baseline {}", path.display()))?;
            let baseline: ScorecardMetrics = serde_json::from_str(&baseline_raw)
                .with_context(|| format!("failed to parse baseline {}", path.display()))?;
            card.metrics.regressions_against(&baseline, args.threshold)
        }
        None => Vec::new(),
    };

    if args.json {
        let out = serde_json::json!({
            "per_turn": card.per_turn,
            "metrics": card.metrics,
            "regressions": regressions,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{}", card.to_summary());
        for r in &regressions {
            println!(
                "REGRESSION {}: baseline {:.4} -> current {:.4} (+{:.1}%)",
                r.metric, r.baseline, r.current, r.pct_increase
            );
        }
    }

    if regressions.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} metric(s) regressed past the {:.1}% threshold",
            regressions.len(),
            args.threshold
        )
    }
}

async fn run_fleet_command(workspace: &Path, config: &Config, args: FleetArgs) -> Result<()> {
    use crate::fleet::alerts::{
        FleetAlertAdapterConfig, FleetAlertConfig, FleetAlertDispatcher, FleetAlertEvent,
        FleetEnvSecretResolver,
    };
    use crate::fleet::executor::FleetExecutor;
    use crate::fleet::manager::{FleetManager, FleetStatusSnapshot, FleetWorkerInspection};
    use codewhale_protocol::fleet::{
        FleetAlertEventClass, FleetArtifactKind, FleetRunId, FleetWorkerEventPayload,
        FleetWorkerStatus,
    };

    fn worker_status_label(status: &FleetWorkerStatus) -> &'static str {
        match status {
            FleetWorkerStatus::Unknown => "unknown",
            FleetWorkerStatus::Online => "online",
            FleetWorkerStatus::Busy => "busy",
            FleetWorkerStatus::Offline => "offline",
            FleetWorkerStatus::Unhealthy => "unhealthy",
            FleetWorkerStatus::Draining => "draining",
            FleetWorkerStatus::Retired => "retired",
        }
    }

    fn artifact_kind_label(kind: &FleetArtifactKind) -> String {
        match kind {
            FleetArtifactKind::Log => "log".to_string(),
            FleetArtifactKind::Patch => "patch".to_string(),
            FleetArtifactKind::TestResult => "test_result".to_string(),
            FleetArtifactKind::Report => "report".to_string(),
            FleetArtifactKind::Checkpoint => "checkpoint".to_string(),
            FleetArtifactKind::Receipt => "receipt".to_string(),
            FleetArtifactKind::Other(value) => value.clone(),
        }
    }

    fn event_label(payload: &FleetWorkerEventPayload) -> String {
        match payload {
            FleetWorkerEventPayload::Queued => "queued".to_string(),
            FleetWorkerEventPayload::Leased { .. } => "leased".to_string(),
            FleetWorkerEventPayload::Starting => "starting".to_string(),
            FleetWorkerEventPayload::Running => "running".to_string(),
            FleetWorkerEventPayload::ModelWait { model } => model
                .as_ref()
                .map(|model| format!("model_wait model={model}"))
                .unwrap_or_else(|| "model_wait".to_string()),
            FleetWorkerEventPayload::RunningTool { tool, call_id } => call_id
                .as_ref()
                .map(|call_id| format!("running_tool tool={tool} call_id={call_id}"))
                .unwrap_or_else(|| format!("running_tool tool={tool}")),
            FleetWorkerEventPayload::WorkflowEvent {
                workflow_run_id,
                event,
            } => event
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(|kind| format!("workflow_event run_id={workflow_run_id} type={kind}"))
                .unwrap_or_else(|| format!("workflow_event run_id={workflow_run_id}")),
            FleetWorkerEventPayload::Heartbeat { .. } => "heartbeat".to_string(),
            FleetWorkerEventPayload::Artifact(artifact) => {
                format!("artifact kind={}", artifact_kind_label(&artifact.kind))
            }
            FleetWorkerEventPayload::Completed { exit_code, summary } => match (exit_code, summary)
            {
                (Some(code), Some(summary)) => format!("completed exit_code={code} {summary}"),
                (Some(code), None) => format!("completed exit_code={code}"),
                (None, Some(summary)) => format!("completed {summary}"),
                (None, None) => "completed".to_string(),
            },
            FleetWorkerEventPayload::Failed {
                reason,
                recoverable,
            } => {
                format!("failed recoverable={recoverable} reason={reason}")
            }
            FleetWorkerEventPayload::Cancelled { cancelled_by } => cancelled_by
                .as_ref()
                .map(|by| format!("cancelled by={by}"))
                .unwrap_or_else(|| "cancelled".to_string()),
            FleetWorkerEventPayload::Interrupted { signal } => signal
                .as_ref()
                .map(|signal| format!("interrupted signal={signal}"))
                .unwrap_or_else(|| "interrupted".to_string()),
            FleetWorkerEventPayload::Stale { last_heartbeat_at } => last_heartbeat_at
                .as_ref()
                .map(|ts| format!("stale last_heartbeat_at={ts}"))
                .unwrap_or_else(|| "stale".to_string()),
            FleetWorkerEventPayload::Restarted { restart_count } => {
                format!("restarted count={restart_count}")
            }
            FleetWorkerEventPayload::Escalated { channel, alert_id } => alert_id
                .as_ref()
                .map(|alert_id| format!("escalated channel={channel} alert_id={alert_id}"))
                .unwrap_or_else(|| format!("escalated channel={channel}")),
        }
    }

    fn print_status(status: &FleetStatusSnapshot) {
        println!(
            "fleet: runs={} queued={} running={} completed={} partial={} failed={} restarted={} escalated={} transport_failed={} task_failed={} verifier_failed={} cancelled={} stale={}",
            status.runs,
            status.queued,
            status.running,
            status.completed,
            status.partial,
            status.failed,
            status.restarted,
            status.escalated,
            status.transport_failed,
            status.task_failed,
            status.verifier_failed,
            status.cancelled,
            status.stale
        );
        if !status.workers.is_empty() {
            println!("workers:");
            for (worker_id, worker_status) in &status.workers {
                println!("  {worker_id} {}", worker_status_label(worker_status));
            }
        }
    }

    fn print_inspection(inspection: &FleetWorkerInspection) {
        println!("worker: {}", inspection.worker_id);
        println!("status: {}", worker_status_label(&inspection.status));
        if let Some(run_id) = &inspection.current_run_id {
            println!("run: {}", run_id.0);
        }
        if let Some(task_id) = &inspection.current_task_id {
            println!("task: {task_id}");
        }
        if let Some(objective) = &inspection.objective {
            println!("objective: {objective}");
        }
        if let Some(role) = &inspection.role {
            println!("role: {role}");
        }
        if let Some(host) = &inspection.host {
            println!("host: {host}");
        }
        if let Some(heartbeat) = &inspection.latest_heartbeat_at {
            println!("heartbeat: {heartbeat}");
        }
        if let Some(event) = &inspection.latest_event {
            println!(
                "latest_event: seq={} {}",
                event.seq,
                event_label(&event.payload)
            );
        }
        if !inspection.artifacts.is_empty() {
            println!("artifacts:");
            for artifact in &inspection.artifacts {
                println!(
                    "  {} {}",
                    artifact_kind_label(&artifact.kind),
                    artifact.path.display()
                );
            }
        }
        if let Some(receipt) = &inspection.receipt_summary {
            println!("receipt: {receipt}");
        }
        if let Some(error) = &inspection.last_error {
            println!("last_error: {error}");
        }
        if let Some(alert) = &inspection.alert_state {
            println!("alert: {alert}");
        }
    }

    fn print_artifacts(inspection: &FleetWorkerInspection) {
        if inspection.artifacts.is_empty() {
            println!("artifacts: none");
            return;
        }
        println!("artifacts:");
        for artifact in &inspection.artifacts {
            let size = artifact
                .size_bytes
                .map(|size| format!(" size={size}"))
                .unwrap_or_default();
            let mime = artifact
                .mime_type
                .as_ref()
                .map(|mime| format!(" mime={mime}"))
                .unwrap_or_default();
            println!(
                "  {} {}{}{}",
                artifact_kind_label(&artifact.kind),
                artifact.path.display(),
                size,
                mime
            );
        }
    }

    fn print_logs(workspace: &Path, inspection: &FleetWorkerInspection) -> Result<()> {
        let mut printed = false;
        for artifact in inspection
            .artifacts
            .iter()
            .filter(|artifact| matches!(artifact.kind, FleetArtifactKind::Log))
        {
            let path = workspace.join(&artifact.path);
            println!("== {} ==", artifact.path.display());
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading fleet log {}", path.display()))?;
            let preview: String = contents.chars().take(16 * 1024).collect();
            print!("{preview}");
            if contents.chars().count() > preview.chars().count() {
                println!("\n[truncated]");
            } else if !preview.ends_with('\n') {
                println!();
            }
            printed = true;
        }
        if !printed {
            println!("logs: none");
        }
        Ok(())
    }

    fn alert_event_class(arg: FleetAlertEventArg) -> FleetAlertEventClass {
        match arg {
            FleetAlertEventArg::Stale => FleetAlertEventClass::Stale,
            FleetAlertEventArg::RestartExhausted => FleetAlertEventClass::RestartExhausted,
            FleetAlertEventArg::NeedsHuman => FleetAlertEventClass::NeedsHuman,
            FleetAlertEventArg::BudgetExceeded => FleetAlertEventClass::BudgetExceeded,
            FleetAlertEventArg::VerifierFailed => FleetAlertEventClass::VerifierFailed,
            FleetAlertEventArg::RunCompleted => FleetAlertEventClass::RunCompleted,
        }
    }

    fn alert_status(class: FleetAlertEventClass, override_status: Option<String>) -> String {
        if let Some(status) = override_status {
            return status;
        }
        match class {
            FleetAlertEventClass::Stale => "stale",
            FleetAlertEventClass::RestartExhausted => "failed",
            FleetAlertEventClass::NeedsHuman => "needs_human",
            FleetAlertEventClass::BudgetExceeded => "budget_exceeded",
            FleetAlertEventClass::VerifierFailed => "verifier_failed",
            FleetAlertEventClass::RunCompleted => "completed",
        }
        .to_string()
    }

    fn alert_adapter(args: &FleetAlertDryRunArgs) -> FleetAlertAdapterConfig {
        match args.adapter {
            FleetAlertAdapterArg::Slack => FleetAlertAdapterConfig::Slack {
                webhook_env: args.slack_webhook_env.clone(),
                channel: None,
            },
            FleetAlertAdapterArg::Webhook => FleetAlertAdapterConfig::Webhook {
                url_env: args.webhook_url_env.clone(),
                secret_env: args.webhook_secret_env.clone(),
            },
            FleetAlertAdapterArg::PagerDuty => FleetAlertAdapterConfig::PagerDuty {
                routing_key_env: args.pagerduty_routing_key_env.clone(),
                severity: args.pagerduty_severity.clone(),
            },
        }
    }

    let fleet_config = config.fleet_config();
    // The configured route is the operator: fleet workers without a
    // task/profile model pin inherit the session's active model.
    let manager = FleetManager::open(workspace)?
        .with_exec_config(fleet_config.exec.clone())
        .with_fleet_config(fleet_config)
        .with_session_model(config.default_model())
        .with_route_config(config.clone());
    match args.command {
        FleetCommand::Init => {
            println!("fleet ledger: {}", manager.ledger_path().display());
            Ok(())
        }
        FleetCommand::Run(args) => {
            let max_workers = args.max_workers.clamp(1, 128);
            let manager =
                manager.with_stale_after(Duration::from_secs(args.stale_after_seconds.max(1)));
            let report = manager.create_run_from_task_spec_path(&args.task_spec, max_workers)?;
            println!(
                "fleet run: {} tasks={} leased={} queued={}",
                report.run_id.0, report.task_count, report.leased, report.queued
            );
            println!("workers:");
            for worker_id in &report.worker_ids {
                println!("  {worker_id}");
            }
            if args.once {
                print_status(&manager.run_status(&report.run_id)?);
                return Ok(());
            }
            println!(
                "manager loop running; use `codewhale fleet status`, `inspect`, `interrupt`, or `stop --all` from another terminal."
            );
            let mut executor = FleetExecutor::new(workspace);
            let codewhale_binary = fleet::executor::configured_codewhale_binary();
            let status = manager
                .run_to_completion(
                    &report.run_id,
                    max_workers,
                    &mut executor,
                    &codewhale_binary,
                    None,
                    Duration::from_secs(2),
                )
                .await?;
            print_status(&status);
            Ok(())
        }
        FleetCommand::Status => {
            print_status(&manager.status()?);
            Ok(())
        }
        FleetCommand::Inspect { worker_id } => {
            print_inspection(&manager.inspect_worker(&worker_id)?);
            Ok(())
        }
        FleetCommand::Logs { worker_id } => {
            let inspection = manager.inspect_worker(&worker_id)?;
            print_logs(workspace, &inspection)
        }
        FleetCommand::Artifacts { worker_id } => {
            let inspection = manager.inspect_worker(&worker_id)?;
            print_artifacts(&inspection);
            Ok(())
        }
        FleetCommand::Interrupt { worker_id } => {
            let inspection = manager.interrupt_worker(&worker_id)?;
            print_inspection(&inspection);
            Ok(())
        }
        FleetCommand::Restart { worker_id } => {
            let report = manager.restart_worker(&worker_id)?;
            print_inspection(&report.inspection);
            println!(
                "manager loop running for restarted run {}; use `codewhale fleet status`, `inspect`, `interrupt`, or `stop --all` from another terminal.",
                report.run_id.0
            );
            let mut executor = FleetExecutor::new(workspace);
            let codewhale_binary = fleet::executor::configured_codewhale_binary();
            let status = manager
                .run_to_completion(
                    &report.run_id,
                    report.max_workers,
                    &mut executor,
                    &codewhale_binary,
                    None,
                    Duration::from_secs(2),
                )
                .await?;
            print_status(&status);
            Ok(())
        }
        FleetCommand::Resume {
            run_id,
            stale_after_seconds,
        } => {
            let manager = manager.with_stale_after(Duration::from_secs(stale_after_seconds.max(1)));
            let report = manager.resume_run(&FleetRunId::from(run_id))?;
            println!(
                "fleet resume: {} reclaimed_stale={} restarted={} failed={} escalated={}",
                report.run_id.0,
                report.reclaimed_stale,
                report.restarted,
                report.failed,
                report.escalated
            );
            print_status(&report.status);
            Ok(())
        }
        FleetCommand::Stop { all } => {
            if !all {
                bail!("pass --all to stop all fleet work");
            }
            let stopped = manager.stop_all()?;
            println!("stopped: {stopped}");
            Ok(())
        }
        FleetCommand::AlertDryRun(args) => {
            let class = alert_event_class(args.event);
            let adapter = alert_adapter(&args);
            let event = FleetAlertEvent {
                class,
                run_id: FleetRunId::from(args.run_id.clone()),
                worker_id: args.worker_id.clone(),
                task_id: args.task_id.clone(),
                status: alert_status(class, args.status.clone()),
                reason: args.reason.clone(),
            };
            let dispatcher = FleetAlertDispatcher::new(
                FleetAlertConfig::dry_run_for_adapter(adapter),
                FleetEnvSecretResolver,
            );
            let deliveries = dispatcher.dispatch(&event)?;
            for delivery in deliveries {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&delivery.redacted_payload)?
                );
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteStatus {
    Created,
    Overwritten,
    SkippedExists,
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory for {}", parent.display()))?;
    }
    Ok(())
}

fn write_template_file(path: &Path, contents: &str, force: bool) -> Result<WriteStatus> {
    ensure_parent_dir(path)?;

    if path.exists() && !force {
        return Ok(WriteStatus::SkippedExists);
    }

    let status = if path.exists() {
        WriteStatus::Overwritten
    } else {
        WriteStatus::Created
    };

    std::fs::write(path, contents)
        .with_context(|| format!("Failed to write template at {}", path.display()))?;

    Ok(status)
}

fn mcp_template_json() -> Result<String> {
    let mut cfg = McpConfig::default();
    cfg.servers.insert(
        "example".to_string(),
        McpServerConfig {
            command: Some("node".to_string()),
            args: vec!["./path/to/your-mcp-server.js".to_string()],
            env: std::collections::HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: std::collections::HashMap::new(),
            env_headers: std::collections::HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
            reviewed_plugin: None,
        },
    );
    cfg.servers.insert(
        "moraine-mcp".to_string(),
        McpServerConfig {
            command: Some("moraine".to_string()),
            args: vec!["mcp".to_string()],
            env: std::collections::HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: std::collections::HashMap::new(),
            env_headers: std::collections::HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
            reviewed_plugin: None,
        },
    );
    serde_json::to_string_pretty(&cfg)
        .map_err(|e| anyhow!("Failed to render MCP template JSON: {e}"))
}

fn init_mcp_config(path: &Path, force: bool) -> Result<WriteStatus> {
    let template = mcp_template_json()?;
    write_template_file(path, &template, force)
}

fn skills_template(name: &str) -> String {
    format!(
        "\
---\n\
name: {name}\n\
description: Quick repo diagnostics and setup guidance\n\
allowed-tools: diagnostics, list_dir, read_file, grep_files, git_status, git_diff\n\
---\n\n\
When this skill is active:\n\
1. Run the diagnostics tool to report workspace and sandbox status.\n\
2. Skim key project files (README.md, Cargo.toml, AGENTS.md) before editing.\n\
3. Prefer small, validated changes and summarize what you verified.\n\
"
    )
}

fn init_skills_dir(skills_dir: &Path, force: bool) -> Result<(PathBuf, WriteStatus)> {
    std::fs::create_dir_all(skills_dir)
        .with_context(|| format!("Failed to create skills dir {}", skills_dir.display()))?;

    let skill_name = "getting-started";
    let skill_path = skills_dir.join(skill_name).join("SKILL.md");
    ensure_parent_dir(&skill_path)?;

    let status = write_template_file(&skill_path, &skills_template(skill_name), force)?;
    Ok((skill_path, status))
}

fn tools_readme_template() -> &'static str {
    "# Local tools\n\n\
     Drop self-describing scripts here so they can be discovered by\n\
     `codewhale-tui setup --status` and surfaced in `codewhale-tui doctor`.\n\n\
     When `[tools.plugin_dir]` is set in config.toml (or when the default\n\
     `~/.codewhale/tools/` directory exists), they are auto-discovered and\n\
     registered as model-visible tools.\n\n\
     Each script should start with a frontmatter-style header so the\n\
     description is visible without executing the file and the agent knows\n\
     the tool name, description, and input schema:\n\n\
     ```\n\
     # name: my-tool\n\
     # description: One-line summary of what this tool does\n\
     # usage: my-tool [args...]\n\
     ```\n\n\
     The directory is intentionally not auto-loaded into the agent's tool\n\
     catalog. Wire individual tools through MCP, hooks, or skills when you\n\
     want them available inside a session.\n"
}

fn tools_example_script() -> &'static str {
    "#!/usr/bin/env sh\n\
     # name: example\n\
     # description: Print a confirmation that local tool discovery works\n\
     # usage: example [name]\n\
     printf 'codewhale-tui local tool ok: %s\\n' \"${1:-world}\"\n"
}

fn init_tools_dir(tools_dir: &Path, force: bool) -> Result<(PathBuf, WriteStatus, WriteStatus)> {
    std::fs::create_dir_all(tools_dir)
        .with_context(|| format!("Failed to create tools dir {}", tools_dir.display()))?;

    let readme_path = tools_dir.join("README.md");
    let readme_status = write_template_file(&readme_path, tools_readme_template(), force)?;

    let example_path = tools_dir.join("example.sh");
    let example_status = write_template_file(&example_path, tools_example_script(), force)?;

    Ok((tools_dir.to_path_buf(), readme_status, example_status))
}

fn plugins_readme_template() -> &'static str {
    "# Local plugins\n\n\
     Each Codewhale plugin bundle lives in its own subdirectory with a\n\
     versioned `plugin.toml`. User bundles live here; workspace bundles live\n\
     under `<workspace>/.codewhale/plugins/`. Both are discovered read-only,\n\
     untrusted, and disabled by default.\n\n\
     A v0.9.1 bundle layout looks like:\n\n\
     ```\n\
     plugins/\n\
       my-plugin/\n\
         plugin.toml\n\
         skills/\n\
           my-skill/SKILL.md\n\
     ```\n\n\
     Run `/plugin validate`, `/plugin show <name>`, then `/plugin enable <name>`.\n\
     Enablement opens a content- and capability-bound trust review;\n\
     confirm the displayed `/plugin trust` command to create an owner-only,\n\
     content-addressed runtime snapshot, then enable the bundle. Remote MCP\n\
     authentication must name environment sources; never store secret values\n\
     in `plugin.toml`.\n\n\
     v0.9.1 activates only declarative Skills and MCP servers through their\n\
     existing engines. Commands, agents, hooks, LSP, native extensions,\n\
     filesystem grants, and lifecycle mutation are inventoried but inactive.\n\
     There is no marketplace, install, update, ambient compatibility scan, or\n\
     automatic trust surface in this release.\n"
}

fn plugin_example_manifest_template() -> &'static str {
    "schema_version = 1\n\n\
     [plugin]\n\
     name = \"example\"\n\
     version = \"0.1.0\"\n\
     description = \"Starter Codewhale plugin bundle\"\n\n\
     [skills]\n\
     path = \"skills\"\n"
}

fn plugin_example_skill_template() -> &'static str {
    "---\n\
     name: hello\n\
     description: Explain that the example plugin bundle is active.\n\
     ---\n\n\
     Tell the user this instruction came from the namespaced\n\
     `example:hello` plugin skill. Do not perform side effects.\n"
}

fn init_plugins_dir(
    plugins_dir: &Path,
    force: bool,
) -> Result<(
    PathBuf,
    PathBuf,
    PathBuf,
    WriteStatus,
    WriteStatus,
    WriteStatus,
)> {
    std::fs::create_dir_all(plugins_dir)
        .with_context(|| format!("Failed to create plugins dir {}", plugins_dir.display()))?;

    let readme_path = plugins_dir.join("README.md");
    let readme_status = write_template_file(&readme_path, plugins_readme_template(), force)?;

    let manifest_path = plugins_dir.join("example").join("plugin.toml");
    ensure_parent_dir(&manifest_path)?;
    let manifest_status =
        write_template_file(&manifest_path, plugin_example_manifest_template(), force)?;

    let skill_path = plugins_dir
        .join("example")
        .join("skills")
        .join("hello")
        .join("SKILL.md");
    ensure_parent_dir(&skill_path)?;
    let skill_status = write_template_file(&skill_path, plugin_example_skill_template(), force)?;

    Ok((
        readme_path,
        manifest_path,
        skill_path,
        readme_status,
        manifest_status,
        skill_status,
    ))
}

/// Resolve the user-supplied CORS origins for `codewhale serve --http`.
///
/// Sources, in priority order (later sources extend earlier ones):
/// 1. `--cors-origin URL` flags (repeatable)
/// 2. `CODEWHALE_CORS_ORIGINS` env var (comma-separated),
///    then `DEEPSEEK_CORS_ORIGINS` as an alias
/// 3. `[runtime_api] cors_origins = [...]` in `config.toml`
///
/// The runtime API always allows the built-in dev defaults
/// (localhost:3000, localhost:1420, tauri://localhost). User entries are
/// appended on top — empty strings are skipped, and duplicates are deduped
/// while preserving first-seen order. Whalescale#255 / #561.
fn resolve_cors_origins(config: &Config, flag_origins: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        if !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    };
    for o in flag_origins {
        push(o);
    }
    if let Ok(env_value) =
        std::env::var("CODEWHALE_CORS_ORIGINS").or_else(|_| std::env::var("DEEPSEEK_CORS_ORIGINS"))
    {
        for piece in env_value.split(',') {
            push(piece);
        }
    }
    if let Some(rt) = &config.runtime_api
        && let Some(list) = &rt.cors_origins
    {
        for o in list {
            push(o);
        }
    }
    out
}

fn deepseek_home_dir() -> PathBuf {
    codewhale_config::codewhale_home().unwrap_or_else(|_| {
        dirs::home_dir().map_or_else(|| PathBuf::from(".codewhale"), |h| h.join(".codewhale"))
    })
}

/// Resolve the default tools directory. Mirrors `default_skills_dir` shape.
fn default_tools_dir() -> PathBuf {
    deepseek_home_dir().join("tools")
}

/// Resolve the default plugins directory.
fn default_plugins_dir() -> PathBuf {
    deepseek_home_dir().join("plugins")
}

/// Default location for crash/offline-queue checkpoints managed by the TUI.
fn default_checkpoints_dir() -> PathBuf {
    deepseek_home_dir().join("sessions").join("checkpoints")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanPlan {
    targets: Vec<PathBuf>,
}

fn collect_clean_targets(checkpoints_dir: &Path) -> CleanPlan {
    // Every `*.json` file in the checkpoints directory is checkpoint state:
    // per-session crash checkpoints (`<session_id>.json`), the legacy
    // single-slot checkpoint (`latest.json`), and the offline input queue
    // (`offline_queue.json`). Non-JSON files and subdirectories are left
    // alone.
    let mut targets: Vec<PathBuf> = std::fs::read_dir(checkpoints_dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default();
    targets.sort();
    CleanPlan { targets }
}

fn execute_clean_plan(plan: &CleanPlan) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::with_capacity(plan.targets.len());
    for path in &plan.targets {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        removed.push(path.clone());
    }
    Ok(removed)
}

fn run_setup(
    config: &Config,
    workspace: &Path,
    args: SetupArgs,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    if args.status {
        return run_setup_status(config, workspace, plugins);
    }
    if args.clean {
        return run_setup_clean(&default_checkpoints_dir(), args.force);
    }

    use crate::palette;
    use colored::Colorize;

    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;

    let any_explicit = args.mcp || args.skills || args.tools || args.plugins;
    let run_mcp = args.mcp || args.all || !any_explicit;
    let run_skills = args.skills || args.all || !any_explicit;
    let run_tools = args.tools || args.all;
    let run_plugins = args.plugins || args.all;

    println!(
        "{}",
        "Codewhale Setup".truecolor(aqua_r, aqua_g, aqua_b).bold()
    );
    println!("{}", "==============".truecolor(sky_r, sky_g, sky_b));
    println!("Workspace: {}", crate::utils::display_path(workspace));

    if run_mcp {
        let mcp_path = config.mcp_config_path();
        let status = init_mcp_config(&mcp_path, args.force)?;
        match status {
            WriteStatus::Created => {
                println!("  ✓ Created MCP config at {}", mcp_path.display());
            }
            WriteStatus::Overwritten => {
                println!("  ✓ Overwrote MCP config at {}", mcp_path.display());
            }
            WriteStatus::SkippedExists => {
                println!("  · MCP config already exists at {}", mcp_path.display());
            }
        }
        println!(
            "    Next: edit the file, then run `codewhale mcp list` or `codewhale mcp tools`."
        );
    }

    if run_skills {
        let skills_dir = if args.local {
            workspace.join("skills")
        } else {
            config.skills_dir()
        };
        let (skill_path, status) = init_skills_dir(&skills_dir, args.force)?;
        match status {
            WriteStatus::Created => {
                println!("  ✓ Created example skill at {}", skill_path.display());
            }
            WriteStatus::Overwritten => {
                println!("  ✓ Overwrote example skill at {}", skill_path.display());
            }
            WriteStatus::SkippedExists => {
                println!(
                    "  · Example skill already exists at {}",
                    skill_path.display()
                );
            }
        }
        if args.local {
            println!(
                "    Local skills dir enabled for this workspace: {}",
                crate::utils::display_path(&skills_dir)
            );
        } else {
            println!(
                "    Skills dir: {}",
                crate::utils::display_path(&skills_dir)
            );
        }
        println!("    Next: run the TUI and use `/skills` then `/skill getting-started`.");
    }

    if run_tools {
        let tools_dir = default_tools_dir();
        let (dir, readme_status, example_status) = init_tools_dir(&tools_dir, args.force)?;
        report_write_status("Tools README", &dir.join("README.md"), readme_status);
        report_write_status("Example tool", &dir.join("example.sh"), example_status);
        println!("    Tools dir: {}", crate::utils::display_path(&dir));
        println!("    Next: drop scripts here; surface them via skills/MCP when ready.");
    }

    if run_plugins {
        let plugins_dir = default_plugins_dir();
        let (readme_path, manifest_path, skill_path, readme_status, manifest_status, skill_status) =
            init_plugins_dir(&plugins_dir, args.force)?;
        report_write_status("Plugins README", &readme_path, readme_status);
        report_write_status("Example plugin manifest", &manifest_path, manifest_status);
        report_write_status("Example plugin skill", &skill_path, skill_status);
        println!(
            "    Plugins dir: {}",
            crate::utils::display_path(&plugins_dir)
        );
        println!("    Next: run `/plugin validate`, review `example`, then trust and enable it.");
    }

    let sandbox = crate::sandbox::get_platform_sandbox();
    if let Some(kind) = sandbox {
        println!("  ✓ Sandbox available: {kind}");
    } else {
        println!("  · Sandbox not available on this platform (best-effort only).");
    }

    Ok(())
}

fn report_write_status(label: &str, path: &Path, status: WriteStatus) {
    match status {
        WriteStatus::Created => {
            println!("  ✓ Created {label} at {}", path.display());
        }
        WriteStatus::Overwritten => {
            println!("  ✓ Overwrote {label} at {}", path.display());
        }
        WriteStatus::SkippedExists => {
            println!("  · {label} already exists at {}", path.display());
        }
    }
}

/// Source of the resolved API key, used only by static doctor/setup reports.
///
/// These reports must not migrate a legacy secret store or acquire a
/// write-capable credential handle just to label a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiKeySource {
    Env,
    Config,
    Keyring,
    OAuth,
    ExternalConsent,
    NoAuth,
    Missing,
}

fn resolve_api_key_source(config: &Config) -> ApiKeySource {
    let provider = config.api_provider();
    let auth_mode = config.auth_mode_for_provider(provider);
    if crate::config::auth_mode_disables_api_key(auth_mode.as_deref()) {
        return ApiKeySource::NoAuth;
    }
    let custom_endpoint = config.provider_uses_custom_endpoint(provider);
    if !custom_endpoint && provider == crate::config::ApiProvider::OpenaiCodex {
        if crate::oauth::credentials_from_env().is_some() {
            return ApiKeySource::Env;
        }
        return config
            .external_credential_consent_status(provider)
            .filter(|status| status.route_state == "active")
            .map_or(ApiKeySource::Missing, |_| ApiKeySource::ExternalConsent);
    }
    if !custom_endpoint
        && provider == crate::config::ApiProvider::Xai
        && auth_mode
            .as_deref()
            .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
    {
        return config
            .external_credential_consent_status(provider)
            .filter(|status| status.route_state == "active")
            .map_or(ApiKeySource::OAuth, |_| ApiKeySource::ExternalConsent);
    }
    if std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .is_some()
    {
        match std::env::var("DEEPSEEK_API_KEY_SOURCE").ok().as_deref() {
            Some("config") => return ApiKeySource::Config,
            Some("keyring") if !custom_endpoint => return ApiKeySource::Keyring,
            _ => {}
        }
    }

    let provider_config_key = config
        .provider_config()
        .and_then(|entry| entry.api_key.as_ref())
        .is_some_and(|k| !k.trim().is_empty());
    let root_deepseek_key = (matches!(
        provider,
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN
    ) || (provider == crate::config::ApiProvider::Custom
        && config.uses_legacy_literal_custom_route()))
        && config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty());

    if provider_config_key || root_deepseek_key {
        ApiKeySource::Config
    } else if configured_provider_env_key_source(config).is_some() {
        ApiKeySource::Env
    } else if !config.should_skip_secret_store_for_provider(provider)
        && crate::config::provider_secret_store_api_key_read_only(config, provider).is_some()
    {
        ApiKeySource::Keyring
    } else if provider_env_key_source_for_config(config).is_some() {
        ApiKeySource::Env
    } else {
        ApiKeySource::Missing
    }
}

fn provider_env_key_source_for_config(config: &Config) -> Option<String> {
    configured_provider_env_key_source(config).or_else(|| {
        (!config.should_skip_secret_store_for_provider(config.api_provider()))
            .then(|| provider_env_key_source(config.api_provider()).map(str::to_string))
            .flatten()
    })
}

fn configured_provider_env_key_source(config: &Config) -> Option<String> {
    config
        .provider_config()
        .and_then(|entry| entry.api_key_env.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
        .map(str::to_string)
}

fn provider_env_key_source(provider: crate::config::ApiProvider) -> Option<&'static str> {
    provider
        .env_vars()
        .iter()
        .copied()
        .find(|var| std::env::var(var).is_ok_and(|value| !value.trim().is_empty()))
}

fn provider_env_vars_label(provider: crate::config::ApiProvider) -> String {
    provider.env_vars_label()
}

fn provider_config_table_key(provider: crate::config::ApiProvider) -> &'static str {
    provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
        .unwrap_or("deepseek_cn")
}

fn provider_auth_hint(provider: crate::config::ApiProvider) -> String {
    if provider == crate::config::ApiProvider::OpenaiCodex {
        "see docs/PROVIDERS.md for ChatGPT/Codex OAuth setup".to_string()
    } else {
        format!(
            "codewhale auth set --provider {} --api-key \"...\"",
            provider.as_str()
        )
    }
}

fn count_dir_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(std::result::Result::ok).count())
        .unwrap_or(0)
}

fn skills_count_for(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    crate::skills::SkillRegistry::discover(dir).len()
}

fn run_setup_status(
    config: &Config,
    workspace: &Path,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    use crate::palette;
    use colored::Colorize;

    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;
    let (red_r, red_g, red_b) = palette::WHALE_ERROR_RGB;

    println!(
        "{}",
        "Codewhale Status".truecolor(aqua_r, aqua_g, aqua_b).bold()
    );
    println!("{}", "===============".truecolor(sky_r, sky_g, sky_b));
    println!("workspace: {}", workspace.display());

    match resolve_api_key_source(config) {
        ApiKeySource::Env => {
            let env_vars = provider_env_key_source_for_config(config)
                .unwrap_or_else(|| provider_env_vars_label(config.api_provider()));
            println!(
                "  {} api_key: set via {env_vars}",
                "✓".truecolor(aqua_r, aqua_g, aqua_b)
            );
        }
        ApiKeySource::Keyring => println!(
            "  {} api_key: set via OS keyring",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Config => println!(
            "  {} api_key: set via config",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::OAuth => println!(
            "  {} oauth: Codewhale-owned storage selected (availability not probed)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::ExternalConsent => println!(
            "  {} oauth: external read-only consent configured (credential file not probed)",
            "·".dimmed()
        ),
        ApiKeySource::NoAuth => println!(
            "  {} api_key: disabled for this route",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Missing => {
            let provider = config.api_provider();
            let provider_identity = config.provider_identity_for(provider);
            let env_var = config
                .provider_config()
                .and_then(|entry| entry.api_key_env.clone())
                .unwrap_or_else(|| provider_env_vars_label(provider));
            let login_hint = if provider == crate::config::ApiProvider::OpenaiCodex {
                provider_auth_hint(provider)
            } else {
                format!("codewhale auth set --provider {provider_identity} --api-key \"...\"")
            };
            let config_location = if provider == crate::config::ApiProvider::Custom
                && config.uses_legacy_literal_custom_route()
            {
                "root `api_key`".to_string()
            } else if provider == crate::config::ApiProvider::Custom {
                format!("`[providers.{provider_identity}].api_key`")
            } else {
                format!(
                    "`[providers.{}].api_key`",
                    provider_config_table_key(provider)
                )
            };
            println!(
                "  {} api_key: missing  (set {env_var} or {config_location} in ~/.codewhale/config.toml; or run `{login_hint}`)",
                "✗".truecolor(red_r, red_g, red_b),
            );
        }
    }
    println!(
        "  · base_url: {}",
        crate::client::redact_url_for_display(&config.deepseek_base_url())
    );
    let model = config
        .default_text_model
        .clone()
        .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string());
    println!("  · default_text_model: {model}");
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    println!("  · default_mode: {default_mode} ({default_mode_source})");

    let mcp_path = config.mcp_config_path();
    let project_mcp_path = crate::mcp::workspace_mcp_config_path(workspace);
    let mcp_count =
        match crate::mcp::load_config_with_workspace_and_plugins(&mcp_path, workspace, plugins) {
            Ok(cfg) => cfg.servers.len(),
            Err(_) => 0,
        };
    let mcp_present = if mcp_path.exists() { "" } else { "  (missing)" };
    let project_mcp_present = if project_mcp_path.exists() {
        ""
    } else {
        "  (missing)"
    };
    println!(
        "  · mcp servers: {mcp_count} from {}{mcp_present} + {}{project_mcp_present}",
        mcp_path.display(),
        project_mcp_path.display()
    );

    let skills_dir = config.skills_dir();
    println!(
        "  · skills: {} at {}",
        skills_count_for(&skills_dir),
        crate::utils::display_path(&skills_dir)
    );

    let tools_dir = default_tools_dir();
    let tools_present = if tools_dir.exists() {
        ""
    } else {
        "  (missing — run `setup --tools`)"
    };
    println!(
        "  · tools: {} entries at {}{tools_present}",
        if tools_dir.exists() {
            count_dir_entries(&tools_dir)
        } else {
            0
        },
        crate::utils::display_path(&tools_dir)
    );

    let plugins_dir = default_plugins_dir();
    let plugins_present = if plugins_dir.exists() {
        ""
    } else {
        "  (missing — run `setup --plugins`)"
    };
    println!(
        "  · plugins: {} entries at {}{plugins_present}",
        if plugins_dir.exists() {
            count_dir_entries(&plugins_dir)
        } else {
            0
        },
        crate::utils::display_path(&plugins_dir)
    );

    let sandbox = crate::sandbox::get_platform_sandbox();
    match sandbox {
        Some(kind) => println!(
            "  {} sandbox: {kind}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        None => println!(
            "  {} sandbox: unavailable (commands run best-effort)",
            "!".truecolor(sky_r, sky_g, sky_b)
        ),
    }

    println!("  {} {}", "·".dimmed(), dotenv_status_line(workspace));

    println!();
    println!("Run `codewhale doctor --json` for a machine-readable check.");
    Ok(())
}

fn dotenv_status_line(workspace: &Path) -> String {
    let dotenv = workspace.join(".env");
    if dotenv.exists() {
        return format!(
            ".env present at {} (literal provider credentials only)",
            dotenv.display()
        );
    }

    if workspace.join(".env.example").exists() {
        return ".env not present in workspace (run `cp .env.example .env` and edit)".to_string();
    }

    ".env not present in workspace".to_string()
}

fn run_setup_clean(checkpoints_dir: &Path, force: bool) -> Result<()> {
    use colored::Colorize;

    if !checkpoints_dir.exists() {
        println!(
            "Nothing to clean — checkpoints dir does not exist: {}",
            checkpoints_dir.display()
        );
        return Ok(());
    }

    let plan = collect_clean_targets(checkpoints_dir);
    if plan.targets.is_empty() {
        println!(
            "Nothing to clean — no checkpoint files in {}",
            checkpoints_dir.display()
        );
        return Ok(());
    }

    if !force {
        println!(
            "Would remove {} checkpoint file(s) (use --force to apply):",
            plan.targets.len()
        );
        for path in &plan.targets {
            println!("  · {}", path.display());
        }
        return Ok(());
    }

    let removed = execute_clean_plan(&plan)?;
    println!("{}", "Cleaned checkpoints:".bold());
    for path in &removed {
        println!("  ✓ {}", path.display());
    }
    Ok(())
}

fn run_session_diagnostics(args: SessionDiagnosticsArgs) -> Result<()> {
    let contents = std::fs::read_to_string(&args.path).with_context(|| {
        format!(
            "read session diagnostic JSONL from {}",
            crate::utils::display_path(&args.path)
        )
    })?;
    let summary = crate::session_diagnostics::analyze_session_failure_jsonl(&contents);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "{}",
            crate::session_diagnostics::format_redacted_failure_summary(&summary)
        );
    }
    Ok(())
}

/// Local endpoints require an explicit opt-in because an HTTP request can wake
/// a desktop-managed daemon (notably Ollama.app). Hosted endpoints preserve the
/// historical `doctor` connectivity check.
fn doctor_should_probe_api(
    provider: crate::config::ApiProvider,
    base_url: &str,
    probe_local: bool,
) -> bool {
    let local = provider.is_self_hosted() || crate::config::base_url_uses_local_host(base_url);
    !local || probe_local
}

/// Doctor must never turn credential inspection into a refresh/write path.
/// OAuth connectivity is exercised by an ordinary user request instead;
/// doctor limits itself to non-mutating readiness inspection.
fn doctor_should_probe_auth(config: &Config) -> bool {
    let provider = config.api_provider();
    if provider == crate::config::ApiProvider::OpenaiCodex
        && !config.provider_uses_custom_endpoint(provider)
    {
        return false;
    }
    let auth_mode = config.auth_mode_for_provider(provider);
    if provider == crate::config::ApiProvider::Xai
        && auth_mode
            .as_deref()
            .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
    {
        return false;
    }
    !(provider == crate::config::ApiProvider::Moonshot
        && auth_mode
            .as_deref()
            .is_some_and(crate::config::auth_mode_uses_kimi_imported_token))
}

/// Run system diagnostics
async fn run_doctor(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
    probe_local: bool,
    plugins: &crate::plugins::PluginRegistry,
) {
    use crate::palette;
    use colored::Colorize;

    let (accent_r, accent_g, accent_b) = palette::WHALE_HUMAN_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;
    let (red_r, red_g, red_b) = palette::WHALE_ERROR_RGB;

    println!(
        "{}",
        "codewhale Doctor"
            .truecolor(accent_r, accent_g, accent_b)
            .bold()
    );
    println!("{}", "==================".truecolor(sky_r, sky_g, sky_b));
    println!();

    // Version info
    println!("{}", "Version Information:".bold());
    println!("  codewhale-tui: {}", env!("DEEPSEEK_BUILD_VERSION"));
    println!("  rust: {}", rustc_version());
    println!();

    println!("{}", "Updates:".bold());
    let current_version = env!("CARGO_PKG_VERSION");
    println!("  · current: v{current_version}");
    match codewhale_release::latest_release_tag_async(codewhale_release::ReleaseChannel::Stable)
        .await
    {
        Ok(latest_tag) => {
            match codewhale_release::compare_release_versions(current_version, &latest_tag) {
                Ok(std::cmp::Ordering::Less) => {
                    println!(
                        "  {} latest: {latest_tag}",
                        "!".truecolor(sky_r, sky_g, sky_b)
                    );
                    println!("    Update available. Run `codewhale update` to install.");
                }
                Ok(std::cmp::Ordering::Equal) => {
                    println!(
                        "  {} latest: {latest_tag}",
                        "✓".truecolor(aqua_r, aqua_g, aqua_b)
                    );
                    println!("    Already up to date.");
                }
                Ok(std::cmp::Ordering::Greater) => {
                    println!("  {} latest: {latest_tag}", "·".dimmed());
                    println!("    Current build is newer than the latest published release.");
                }
                Err(err) => {
                    println!(
                        "  {} latest: {latest_tag}",
                        "!".truecolor(sky_r, sky_g, sky_b)
                    );
                    println!("    Version comparison failed: {err}");
                }
            }
        }
        Err(err) => {
            println!(
                "  {} latest release check failed: {err}",
                "!".truecolor(sky_r, sky_g, sky_b)
            );
            println!("    Run `codewhale update --check` to retry.");
        }
    }
    println!();

    // Configuration summary
    println!("{}", "Configuration:".bold());
    let config_path = config_path_override
        .map(PathBuf::from)
        .or_else(|| codewhale_config::resolve_config_path(None).ok())
        .unwrap_or_else(|| {
            codewhale_config::codewhale_home()
                .unwrap_or_else(|_| PathBuf::from(".codewhale"))
                .join("config.toml")
        });

    if config_path.exists() {
        println!(
            "  {} config.toml found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&config_path)
        );
    } else {
        println!(
            "  {} config.toml not found at {} (using defaults/env)",
            "!".truecolor(sky_r, sky_g, sky_b),
            crate::utils::display_path(&config_path)
        );
    }
    println!("  workspace: {}", crate::utils::display_path(workspace));
    println!("  {}", doctor_search_provider_line(config));

    // State root (v0.8.44)
    println!();
    println!("{}", "State Root:".bold());
    let (code_home, legacy_home) = doctor_state_roots();
    let active_root = if code_home.exists() {
        &code_home
    } else if legacy_home.exists() {
        &legacy_home
    } else {
        &code_home
    };
    println!("  active: {}", crate::utils::display_path(active_root));
    if active_root != &code_home {
        println!(
            "  note: legacy {} found; start Codewhale once to trigger safe migration where available.",
            crate::utils::display_path(&legacy_home)
        );
    }
    if legacy_home.exists() && code_home.exists() {
        println!(
            "  dual roots: {} (primary) + {} (legacy)",
            crate::utils::display_path(&code_home),
            crate::utils::display_path(&legacy_home)
        );
    }
    let legacy_state_report = doctor_legacy_state_report(&code_home, &legacy_home);
    let session_recovery = doctor_session_recovery_report(
        &code_home,
        &legacy_home,
        codewhale_config::codewhale_home_is_explicit(),
    );
    print_doctor_legacy_state_report(
        &legacy_state_report,
        &session_recovery,
        (aqua_r, aqua_g, aqua_b),
        (sky_r, sky_g, sky_b),
    );

    let (setup_state, setup_source) = doctor_setup_state(config, workspace);
    print_doctor_setup_report(
        config,
        workspace,
        &setup_state,
        setup_source,
        (aqua_r, aqua_g, aqua_b),
        (sky_r, sky_g, sky_b),
    );

    // Check API keys
    println!();
    println!("{}", "API Keys:".bold());

    // Per-provider state: env + config file only (no values printed).
    // Keep doctor/status prompt-free even for unsigned rebuilt binaries.
    let dispatcher_api_key_source = std::env::var("DEEPSEEK_API_KEY_SOURCE").ok();
    for provider in crate::config::ApiProvider::all().iter().copied() {
        let slot = provider.as_str();
        let in_env = provider.env_vars().iter().any(|var| {
            std::env::var(var)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_some()
        });
        let injected_runtime_key = matches!(
            dispatcher_api_key_source.as_deref(),
            Some("keyring" | "env" | "cli")
        );
        let in_config = config
            .provider_config_for(provider)
            .and_then(|entry| entry.api_key.as_ref())
            .is_some_and(|v| !v.trim().is_empty())
            || (matches!(provider, crate::config::ApiProvider::Deepseek)
                && !injected_runtime_key
                && config
                    .api_key
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty()));
        let icon = if in_env || in_config {
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        } else {
            "·".dimmed()
        };
        println!(
            "  {} {slot}: env={}, config={}",
            icon,
            if in_env { "yes" } else { "no" },
            if in_config { "yes" } else { "no" }
        );
    }
    println!("  · credential precedence: ~/.codewhale/config.toml, OS keyring, then env");
    println!();
    println!(
        "{}",
        "External credential consent (configuration only):".bold()
    );
    for line in doctor_external_credential_consent_lines(config) {
        println!("  {line}");
    }

    let api_key_source = resolve_api_key_source(config);
    let has_api_key = if !matches!(
        api_key_source,
        ApiKeySource::Missing | ApiKeySource::ExternalConsent
    ) {
        let source_label = match api_key_source {
            ApiKeySource::Config => "config.toml",
            ApiKeySource::Keyring => "OS keyring",
            ApiKeySource::Env => "environment",
            ApiKeySource::OAuth => "non-mutating OAuth readiness",
            ApiKeySource::ExternalConsent => "external consent (not probed)",
            ApiKeySource::NoAuth => "no-auth route",
            ApiKeySource::Missing
                if matches!(
                    config.api_provider(),
                    crate::config::ApiProvider::Sglang
                        | crate::config::ApiProvider::Vllm
                        | crate::config::ApiProvider::Ollama
                ) =>
            {
                "optional local auth"
            }
            ApiKeySource::Missing => "unknown source",
        };
        println!(
            "  {} active provider key resolved from {source_label}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        );
        true
    } else {
        println!(
            "  {} active provider key not configured",
            "✗".truecolor(red_r, red_g, red_b)
        );
        println!(
            "    Run 'codewhale auth set --provider <name>' to save a key to ~/.codewhale/config.toml."
        );
        false
    };

    // API connectivity test
    println!();
    println!("{}", "API Connectivity:".bold());
    let api_target = doctor_api_target(config);
    println!("  · provider: {}", api_target.provider);
    println!(
        "  · base_url: {}",
        crate::client::redact_url_for_display(&api_target.base_url)
    );
    println!("  · model: {}", api_target.model);
    let tls_status = doctor_tls_status(config);
    if !tls_status.certificate_verification {
        println!("  ! {}", tls_status.message);
        println!("    Prefer SSL_CERT_FILE with a trusted custom CA bundle when possible.");
    }
    let strict_tool_mode = doctor_strict_tool_mode_status(config);
    let strict_icon = match strict_tool_mode.status {
        "ready" => "✓".truecolor(aqua_r, aqua_g, aqua_b),
        "fallback_non_beta" | "custom_endpoint" => "!".truecolor(sky_r, sky_g, sky_b),
        _ => "·".dimmed(),
    };
    println!(
        "  {} strict_tool_mode: {}",
        strict_icon, strict_tool_mode.message
    );
    if let Some(recommended) = strict_tool_mode.recommended_base_url.as_ref() {
        println!("    Use `base_url = \"{recommended}\"` for DeepSeek strict schemas.");
    }
    let capability = crate::config::provider_capability(config.api_provider(), &api_target.model);
    if let Some(alias) = capability.alias_deprecation.as_ref() {
        println!(
            "  ! model alias {} retires {}; switch to {}",
            alias.alias, alias.retirement_date, alias.replacement
        );
    }
    if has_api_key
        && doctor_should_probe_auth(config)
        && doctor_should_probe_api(config.api_provider(), &api_target.base_url, probe_local)
    {
        print!("  {} Testing connection...", "·".dimmed());
        use std::io::Write;
        std::io::stdout().flush().ok();

        // Resolve a credential through the diagnostic-only store first, then
        // probe with an in-memory clone. Constructing the normal client from
        // the original config could otherwise trigger its legacy secret-store
        // migration while a user merely asks doctor to test connectivity.
        let connectivity_result = match config.with_read_only_api_key_for_diagnostic() {
            Ok(diagnostic_config) => test_api_connectivity(&diagnostic_config).await,
            Err(error) => Err(error),
        };
        match connectivity_result {
            Ok(()) => {
                println!(
                    "\r  {} API connection successful",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b)
                );
            }
            Err(e) => {
                let error_msg = e.to_string();
                println!(
                    "\r  {} API connection failed",
                    "✗".truecolor(red_r, red_g, red_b)
                );
                if error_msg.contains("401") || error_msg.contains("Unauthorized") {
                    println!(
                        "    Invalid API key. Check `codewhale auth status`, DEEPSEEK_API_KEY, or config.toml"
                    );
                    if matches!(api_key_source, ApiKeySource::Keyring) {
                        println!(
                            "    The rejected key came from the OS keyring via the dispatcher."
                        );
                        println!(
                            "    Run `codewhale auth status` to inspect config/keyring/env sources."
                        );
                    } else if matches!(api_key_source, ApiKeySource::Env) {
                        println!(
                            "    The rejected key came from DEEPSEEK_API_KEY; no saved config key is present."
                        );
                        println!(
                            "    Run `codewhale auth set --provider deepseek` to save a config key that overrides stale env."
                        );
                    }
                } else if error_msg.contains("403") || error_msg.contains("Forbidden") {
                    println!(
                        "    API key lacks permissions. Verify key is active at platform.deepseek.com"
                    );
                } else if error_msg.contains("timeout") || error_msg.contains("Timeout") {
                    for line in doctor_timeout_recovery_lines(config) {
                        println!("    {line}");
                    }
                } else if error_msg.contains("dns") || error_msg.contains("resolve") {
                    println!("    DNS resolution failed. Check your network connection");
                } else if error_msg.contains("connect") {
                    println!("    Connection failed. Check firewall settings or try again");
                } else {
                    println!("    Error: {error_msg}");
                }
            }
        }
    } else if has_api_key && !doctor_should_probe_auth(config) {
        println!(
            "  {} Live OAuth connectivity not checked by non-mutating doctor",
            "·".dimmed()
        );
        println!(
            "    Doctor never refreshes or rewrites credentials; exercise the route with a normal request."
        );
    } else if has_api_key {
        println!(
            "  {} Live connectivity not checked for this local endpoint",
            "·".dimmed()
        );
        println!(
            "    Run `codewhale doctor --probe-local` to opt in; the request may start a local service."
        );
    } else {
        println!("  {} Skipped (no API key configured)", "·".dimmed());
    }

    // MCP configuration
    println!();
    println!("{}", "MCP Servers (configuration only):".bold());
    println!("  · Static check only; no server process was started.");
    let features = config.features();
    if features.enabled(Feature::Mcp) {
        println!(
            "  {} MCP feature flag enabled",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        );
    } else {
        println!(
            "  {} MCP feature flag disabled",
            "!".truecolor(sky_r, sky_g, sky_b)
        );
    }

    let mcp_config_path = config.mcp_config_path();
    let project_mcp_config_path = crate::mcp::workspace_mcp_config_path(workspace);
    if mcp_config_path.exists() {
        println!(
            "  {} MCP config found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&mcp_config_path)
        );
    } else {
        println!(
            "  {} MCP config not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&mcp_config_path)
        );
    }
    if project_mcp_config_path.exists() {
        println!(
            "  {} Project MCP config found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&project_mcp_config_path)
        );
    } else {
        println!(
            "  {} Project MCP config not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&project_mcp_config_path)
        );
    }

    match crate::mcp::load_config_with_workspace_and_plugins(&mcp_config_path, workspace, plugins) {
        Ok(cfg) if cfg.servers.is_empty() => {
            println!("  {} 0 merged server(s) configured", "·".dimmed());
            if !mcp_config_path.exists() && !project_mcp_config_path.exists() {
                println!("    Run `codewhale mcp init` or add `.codewhale/mcp.json`.");
            }
        }
        Ok(cfg) => {
            println!(
                "  {} {} merged server(s) configured",
                "·".dimmed(),
                cfg.servers.len()
            );
            for (name, server) in &cfg.servers {
                let status = doctor_check_mcp_server(server);
                let icon = match &status {
                    McpServerDoctorStatus::Ok(detail) => {
                        format!(
                            "  {} {name}: configuration valid; {}",
                            "✓".truecolor(aqua_r, aqua_g, aqua_b),
                            detail
                        )
                    }
                    McpServerDoctorStatus::Warning(detail) => {
                        format!(
                            "  {} {name}: configuration warning; {}",
                            "!".truecolor(sky_r, sky_g, sky_b),
                            detail
                        )
                    }
                    McpServerDoctorStatus::Error(detail) => {
                        format!(
                            "  {} {name}: configuration invalid; {}",
                            "✗".truecolor(red_r, red_g, red_b),
                            detail
                        )
                    }
                };
                println!("{icon}");
                if !server.is_enabled() {
                    println!("      disabled; live health not checked");
                } else {
                    println!(
                        "      process/protocol/backend: not checked; `codewhale mcp validate` explicitly starts and initializes configured servers"
                    );
                }
            }
        }
        Err(err) => {
            println!(
                "  {} MCP config parse error: {}",
                "✗".truecolor(red_r, red_g, red_b),
                err
            );
        }
    }

    // Skills configuration
    println!();
    println!("{}", "Skills:".bold());
    let global_skills_dir = config.skills_dir();
    let agents_skills_dir = workspace.join(".agents").join("skills");
    let local_skills_dir = workspace.join("skills");
    let agents_global_skills_dir = crate::skills::agents_global_skills_dir();
    // #432: cross-tool skill discovery dirs. Presence is reported here
    // even though they sit lower in the precedence chain so users can
    // see at a glance whether a `.opencode/skills/`, `.claude/skills/`,
    // `.cursor/skills/`, or global agentskills.io directory is contributing
    // to the merged catalogue.
    let opencode_skills_dir = workspace.join(".opencode").join("skills");
    let claude_skills_dir = workspace.join(".claude").join("skills");
    let selected_skills_dir = if agents_skills_dir.exists() {
        agents_skills_dir.clone()
    } else if local_skills_dir.exists() {
        local_skills_dir.clone()
    } else if config.skills_dir.is_none()
        && let Some(global_agents) = agents_global_skills_dir.as_ref()
        && global_agents.exists()
    {
        global_agents.clone()
    } else {
        global_skills_dir.clone()
    };

    let describe_dir = |dir: &Path| -> usize {
        std::fs::read_dir(dir)
            .map(|entries| entries.filter_map(std::result::Result::ok).count())
            .unwrap_or(0)
    };

    if local_skills_dir.exists() {
        println!(
            "  {} local skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&local_skills_dir),
            describe_dir(&local_skills_dir)
        );
    } else {
        println!(
            "  {} local skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&local_skills_dir)
        );
    }

    if agents_skills_dir.exists() {
        println!(
            "  {} .agents skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&agents_skills_dir),
            describe_dir(&agents_skills_dir)
        );
    } else {
        println!(
            "  {} .agents skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&agents_skills_dir)
        );
    }

    if let Some(agents_global_skills_dir) = agents_global_skills_dir.as_ref() {
        if agents_global_skills_dir.exists() {
            println!(
                "  {} global .agents skills dir found at {} ({} items)",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(agents_global_skills_dir),
                describe_dir(agents_global_skills_dir)
            );
        } else {
            println!(
                "  {} global .agents skills dir not found at {}",
                "·".dimmed(),
                crate::utils::display_path(agents_global_skills_dir)
            );
        }
    }

    if global_skills_dir.exists() {
        println!(
            "  {} global skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&global_skills_dir),
            describe_dir(&global_skills_dir)
        );
    } else {
        println!(
            "  {} global skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&global_skills_dir)
        );
    }

    // #432: only print interop dirs when they're populated — empty
    // .opencode/.claude folders are common and would just clutter
    // the report with false-positive "absent" lines.
    if opencode_skills_dir.exists() {
        println!(
            "  {} .opencode skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&opencode_skills_dir),
            describe_dir(&opencode_skills_dir)
        );
    }
    if claude_skills_dir.exists() {
        println!(
            "  {} .claude skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&claude_skills_dir),
            describe_dir(&claude_skills_dir)
        );
    }

    println!(
        "  {} selected skills dir: {}",
        "·".dimmed(),
        crate::utils::display_path(&selected_skills_dir)
    );
    if !agents_skills_dir.exists()
        && !local_skills_dir.exists()
        && !agents_global_skills_dir
            .as_ref()
            .is_some_and(|dir| dir.exists())
        && !global_skills_dir.exists()
    {
        println!("    Run `codewhale setup --skills` (or add --local for ./skills).");
    }

    // Tools directory
    println!();
    println!("{}", "Tools:".bold());
    let tools_dir = default_tools_dir();
    if tools_dir.exists() {
        let count = count_dir_entries(&tools_dir);
        println!(
            "  {} tools dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&tools_dir),
            count
        );
    } else {
        println!(
            "  {} tools dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&tools_dir)
        );
        println!("    Run `codewhale setup --tools` to scaffold a starter dir.");
    }

    // Plugins directory
    println!();
    println!("{}", "Plugins:".bold());
    let plugins_dir = default_plugins_dir();
    if plugins_dir.exists() {
        let count = count_dir_entries(&plugins_dir);
        println!(
            "  {} plugins dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&plugins_dir),
            count
        );
    } else {
        println!(
            "  {} plugins dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&plugins_dir)
        );
        println!("    Run `codewhale setup --plugins` to scaffold a starter dir.");
    }

    // Storage surfaces (#422 / #440 / #500)
    println!();
    println!("{}", "Storage:".bold());
    if let Some(spillover_root) = crate::tools::truncate::spillover_root() {
        let (present, count) = if spillover_root.is_dir() {
            (true, count_dir_entries(&spillover_root))
        } else {
            (false, 0)
        };
        if present {
            println!(
                "  {} tool-output spillover at {} ({} file{})",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(&spillover_root),
                count,
                if count == 1 { "" } else { "s" }
            );
        } else {
            println!(
                "  {} tool-output spillover dir not yet created at {}",
                "·".dimmed(),
                crate::utils::display_path(&spillover_root)
            );
        }
    }
    let stash = crate::composer_stash::diagnostic_stash_report();
    if let Some(stash_path) = stash.path.as_ref() {
        if let Some(error) = stash.error.as_deref() {
            println!(
                "  {} composer stash was not inspected at {}: {error}",
                "!".truecolor(sky_r, sky_g, sky_b),
                crate::utils::display_path(stash_path),
            );
        } else if stash.present {
            println!(
                "  {} composer stash at {} ({} parked draft{})",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(stash_path),
                stash.count,
                if stash.count == 1 { "" } else { "s" }
            );
        } else {
            println!(
                "  {} composer stash empty (Ctrl+G or Ctrl+S in the composer to park a draft)",
                "·".dimmed()
            );
        }
    } else if let Some(error) = stash.error.as_deref() {
        println!(
            "  {} composer stash was not inspected: {error}",
            "!".truecolor(sky_r, sky_g, sky_b),
        );
    }

    // Tool dependencies — probe external binaries that individual
    // tools rely on (Python for code_execution, pdftotext for PDF
    // reading) so users see explicit ✓/✗ rather than the tool failing
    // at execution time with "program not found". New in v0.8.31.
    println!();
    println!("{}", "Tool Dependencies:".bold());

    match crate::dependencies::resolve_python_interpreter() {
        Some(name) => println!(
            "  {} Python: {} → code_execution tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            name
        ),
        None => {
            println!(
                "  {} Python: not found (tried {:?})",
                "✗".truecolor(red_r, red_g, red_b),
                crate::dependencies::PYTHON_CANDIDATES,
            );
            println!("    code_execution tool is NOT advertised to the model on this install.");
            println!("    Install Python 3 and ensure one of those names is on PATH:");
            match std::env::consts::OS {
                "macos" => {
                    println!("      brew install python@3.12   (or download from python.org)")
                }
                "linux" => println!(
                    "      sudo apt install python3    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install Python.Python.3   (or download from python.org)")
                }
                other => println!("      install Python 3 for {other} from python.org"),
            }
        }
    }

    match crate::dependencies::resolve_node() {
        Some(_) => println!(
            "  {} Node.js: present → js_execution tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!(
                "  {} Node.js: not found (tried `node`)",
                "✗".truecolor(red_r, red_g, red_b),
            );
            println!("    js_execution tool is NOT advertised to the model on this install.");
            println!("    Install Node 18+ and ensure `node` is on PATH:");
            match std::env::consts::OS {
                "macos" => println!("      brew install node   (or download from nodejs.org)"),
                "linux" => println!(
                    "      sudo apt install nodejs    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install OpenJS.NodeJS   (or download from nodejs.org)")
                }
                other => println!("      install Node.js for {other} from nodejs.org"),
            }
        }
    }

    match crate::dependencies::resolve_pandoc() {
        Some(_) => println!(
            "  {} pandoc: present → pandoc_convert tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!("  {} pandoc: not found (optional)", "·".dimmed(),);
            println!(
                "    pandoc_convert tool is NOT advertised to the model. Install pandoc to enable:"
            );
            match std::env::consts::OS {
                "macos" => println!("      brew install pandoc"),
                "linux" => println!(
                    "      sudo apt install pandoc    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install JohnMacFarlane.Pandoc")
                }
                other => println!("      install pandoc for {other} from pandoc.org"),
            }
        }
    }

    match crate::dependencies::resolve_tesseract() {
        Some(_) => {
            if cfg!(target_os = "macos") {
                println!(
                    "  {} OCR: macOS Vision + tesseract available → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
            } else {
                println!(
                    "  {} tesseract: present → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
            }
        }
        None => {
            if cfg!(target_os = "macos") {
                println!(
                    "  {} OCR: macOS Vision available → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
                println!(
                    "    tesseract not found (optional; install only for alternate OCR packs)."
                );
            } else {
                println!("  {} tesseract: not found (optional)", "·".dimmed(),);
                println!(
                    "    image_ocr tool is NOT advertised to the model. Install tesseract to enable:"
                );
                match std::env::consts::OS {
                    "macos" => println!("      brew install tesseract"),
                    "linux" => println!(
                        "      sudo apt install tesseract-ocr    (Debian/Ubuntu) — or your distro's equivalent"
                    ),
                    "windows" => println!("      winget install UB-Mannheim.TesseractOCR"),
                    other => {
                        println!("      install tesseract for {other} from tesseract-ocr.github.io")
                    }
                }
            }
        }
    }

    // PDF reader: pure-Rust `pdf-extract` is the v0.8.32 default, so
    // `pdftotext` is no longer required for `read_file` to handle PDFs.
    // We still surface its presence (a) so users with column-heavy PDFs
    // know they can opt in via `prefer_external_pdftotext = true`, and
    // (b) so users who *did* opt in get a clean signal when the binary
    // is missing rather than discovering it on the next PDF read.
    let prefer_external = crate::settings::Settings::load_read_only()
        .map(|s| s.prefer_external_pdftotext)
        .unwrap_or(false);
    match crate::dependencies::resolve_pdftotext() {
        Some(_) => {
            if prefer_external {
                println!(
                    "  {} pdftotext: available → read_file routes PDFs through Poppler (prefer_external_pdftotext = true)",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
            } else {
                println!(
                    "  {} pdftotext: available (optional — pure-Rust extractor is the default in v0.8.32)",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
                println!(
                    "    Set `prefer_external_pdftotext = true` in settings.toml for column-heavy PDFs."
                );
            }
        }
        None => {
            if prefer_external {
                println!(
                    "  {} pdftotext: not found, but `prefer_external_pdftotext = true` is set → PDF reads will return `binary_unavailable`",
                    "✗".truecolor(red_r, red_g, red_b),
                );
                println!(
                    "    Either install Poppler or unset `prefer_external_pdftotext` to fall back to the bundled pure-Rust extractor."
                );
                match std::env::consts::OS {
                    "macos" => println!("    Install via: brew install poppler"),
                    "linux" => println!(
                        "    Install via: sudo apt install poppler-utils   (Debian/Ubuntu)"
                    ),
                    "windows" => println!(
                        "    Install Poppler for Windows from https://blog.alivate.com.au/poppler-windows/"
                    ),
                    _ => {}
                }
            } else {
                println!(
                    "  {} pdftotext: not found (optional — pure-Rust extractor is the default in v0.8.32)",
                    "·".dimmed(),
                );
                println!(
                    "    Install Poppler only if you want to opt into pdftotext for column-heavy PDFs."
                );
            }
        }
    }

    // Terminal-quirk overrides currently active. Mirrors the env
    // signals checked by `Settings::apply_env_overrides` so users
    // can see at a glance which a11y/compat overrides fired.
    println!();
    println!("{}", "Terminal Quirks:".bold());
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term_program_lc = term_program.to_ascii_lowercase();
    let mut any_quirk = false;
    if matches!(term_program.as_str(), "vscode" | "ghostty") {
        println!(
            "  {} TERM_PROGRAM={} → low_motion + fancy_animations=false (auto)",
            "•".truecolor(sky_r, sky_g, sky_b),
            term_program
        );
        any_quirk = true;
    }
    if term_program == "Termius"
        || std::env::var_os("SSH_CLIENT").is_some_and(|v| !v.is_empty())
        || std::env::var_os("SSH_TTY").is_some_and(|v| !v.is_empty())
    {
        println!(
            "  {} SSH/Termius session → low_motion + fancy_animations=false (auto, #1433)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if term_program_lc.contains("ptyxis")
        || std::env::var_os("PTYXIS_VERSION").is_some_and(|v| !v.is_empty())
    {
        println!(
            "  {} Ptyxis detected → synchronized_output=off (auto, v0.8.31)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if crate::settings::detected_legacy_windows_console_host() {
        println!(
            "  {} legacy Windows console host → low_motion + fancy_animations=false + bracketed_paste=false + synchronized_output=off (auto)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if !any_quirk {
        println!(
            "  {} no env-driven terminal-quirk overrides active",
            "·".dimmed()
        );
    }

    // Platform and sandbox checks
    println!();
    println!("{}", "Platform:".bold());
    println!("  OS: {}", std::env::consts::OS);
    println!("  Arch: {}", std::env::consts::ARCH);

    let sandbox = crate::sandbox::get_platform_sandbox();
    if let Some(kind) = sandbox {
        println!(
            "  {} sandbox available: {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            kind
        );
    } else {
        println!(
            "  {} sandbox not available (commands run best-effort)",
            "!".truecolor(sky_r, sky_g, sky_b)
        );
    }

    println!();
    println!(
        "{}",
        "All checks complete!"
            .truecolor(aqua_r, aqua_g, aqua_b)
            .bold()
    );
}

const DOCTOR_LEGACY_STATE_ITEMS: &[&str] = &[
    "sessions",
    "tasks",
    "skills",
    "slop_ledger",
    "trophies",
    "catalog",
    "review-receipts",
    "config.toml",
    "settings.toml",
    "mcp.json",
];
const DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT: usize = 20;
const DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorLegacyStateStatus {
    PrimaryOnly,
    LegacyOnly,
    Both,
    Absent,
}

impl DoctorLegacyStateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryOnly => "primary_only",
            Self::LegacyOnly => "legacy_only",
            Self::Both => "both",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone)]
struct DoctorLegacyStateEntry {
    name: &'static str,
    primary_path: PathBuf,
    legacy_path: PathBuf,
    primary_present: bool,
    legacy_present: bool,
    status: DoctorLegacyStateStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorSessionRecoveryStatus {
    Isolated,
    NoLegacySessions,
    MigrationPending,
    MigrationIncomplete,
    MigrationComplete,
    ScanFailed,
}

impl DoctorSessionRecoveryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::NoLegacySessions => "no_legacy_sessions",
            Self::MigrationPending => "migration_pending",
            Self::MigrationIncomplete => "migration_incomplete",
            Self::MigrationComplete => "migration_complete",
            Self::ScanFailed => "scan_failed",
        }
    }
}

#[derive(Debug, Clone)]
struct DoctorRecoverableSessionEntry {
    name: PathBuf,
    source_path: PathBuf,
    destination_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DoctorSessionRecoveryReport {
    status: DoctorSessionRecoveryStatus,
    primary_sessions_path: PathBuf,
    legacy_sessions_path: PathBuf,
    codewhale_home_is_explicit: bool,
    legacy_session_file_count: usize,
    already_present_file_count: usize,
    recoverable_file_count: usize,
    /// Bounded filename/path sample; the total is `recoverable_file_count`.
    recoverable: Vec<DoctorRecoverableSessionEntry>,
    error: Option<String>,
}

impl DoctorSessionRecoveryReport {
    fn needs_attention(&self) -> bool {
        matches!(
            self.status,
            DoctorSessionRecoveryStatus::MigrationPending
                | DoctorSessionRecoveryStatus::MigrationIncomplete
                | DoctorSessionRecoveryStatus::ScanFailed
        )
    }
}

fn doctor_legacy_state_status(
    primary_present: bool,
    legacy_present: bool,
) -> DoctorLegacyStateStatus {
    match (primary_present, legacy_present) {
        (true, false) => DoctorLegacyStateStatus::PrimaryOnly,
        (false, true) => DoctorLegacyStateStatus::LegacyOnly,
        (true, true) => DoctorLegacyStateStatus::Both,
        (false, false) => DoctorLegacyStateStatus::Absent,
    }
}

fn doctor_state_roots() -> (PathBuf, PathBuf) {
    let code_home =
        codewhale_config::codewhale_home().unwrap_or_else(|_| PathBuf::from("~/.codewhale"));
    let legacy_home = if codewhale_config::codewhale_home_is_explicit() {
        code_home.join(codewhale_config::LEGACY_APP_DIR)
    } else {
        codewhale_config::legacy_deepseek_home().unwrap_or_else(|_| PathBuf::from("~/.deepseek"))
    };
    (code_home, legacy_home)
}

fn doctor_legacy_state_report(
    primary_root: &Path,
    legacy_root: &Path,
) -> Vec<DoctorLegacyStateEntry> {
    DOCTOR_LEGACY_STATE_ITEMS
        .iter()
        .copied()
        .map(|name| {
            let primary_path = primary_root.join(name);
            let legacy_path = legacy_root.join(name);
            let primary_present = primary_path.exists();
            let legacy_present = legacy_path.exists();
            let status = doctor_legacy_state_status(primary_present, legacy_present);
            DoctorLegacyStateEntry {
                name,
                primary_path,
                legacy_path,
                primary_present,
                legacy_present,
                status,
            }
        })
        .collect()
}

/// Compare legacy and primary session filenames without opening session files.
///
/// This is deliberately separate from `SessionManager::default_location()`:
/// constructing the manager can trigger the additive legacy migration, while
/// doctor must remain a read-only diagnostic. Session history is stored as
/// top-level JSON files. Directories (including `checkpoints`) and symlinks
/// observed during the scan are ignored, so the diagnostic does not
/// intentionally traverse checkpoint internals or link targets. These checks
/// are best-effort observations, not a race-free no-follow guarantee.
/// A matching filename is only a regular-file counterpart check: doctor does
/// not parse or compare session descriptors.
fn doctor_session_recovery_report(
    primary_root: &Path,
    legacy_root: &Path,
    codewhale_home_is_explicit: bool,
) -> DoctorSessionRecoveryReport {
    let primary_sessions_path = primary_root.join("sessions");
    let legacy_sessions_path = legacy_root.join("sessions");
    let mut report = DoctorSessionRecoveryReport {
        status: DoctorSessionRecoveryStatus::NoLegacySessions,
        primary_sessions_path,
        legacy_sessions_path,
        codewhale_home_is_explicit,
        legacy_session_file_count: 0,
        already_present_file_count: 0,
        recoverable_file_count: 0,
        recoverable: Vec::new(),
        error: None,
    };

    if codewhale_home_is_explicit {
        report.status = DoctorSessionRecoveryStatus::Isolated;
        return report;
    }

    let legacy_root_is_present =
        match doctor_session_directory_is_safe(legacy_root, "legacy state root") {
            Ok(present) => present,
            Err(error) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(error);
                return report;
            }
        };
    if !legacy_root_is_present {
        return report;
    }
    if let Err(error) = doctor_session_directory_is_safe(primary_root, "primary state root") {
        report.status = DoctorSessionRecoveryStatus::ScanFailed;
        report.error = Some(error);
        return report;
    }

    let legacy_sessions_are_present = match doctor_session_directory_is_safe(
        &report.legacy_sessions_path,
        "legacy sessions root",
    ) {
        Ok(present) => present,
        Err(error) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(error);
            return report;
        }
    };
    if !legacy_sessions_are_present {
        return report;
    }
    let primary_sessions_are_present = match doctor_session_directory_is_safe(
        &report.primary_sessions_path,
        "primary sessions root",
    ) {
        Ok(present) => present,
        Err(error) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(error);
            return report;
        }
    };

    let entries = match std::fs::read_dir(&report.legacy_sessions_path) {
        Ok(entries) => entries,
        Err(err) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(format!(
                "could not inspect legacy session filenames at {}: {err}",
                crate::utils::display_path(&report.legacy_sessions_path)
            ));
            return report;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect an entry under {}: {err}",
                    crate::utils::display_path(&report.legacy_sessions_path)
                ));
                return report;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect legacy session entry metadata under {}: {err}",
                    crate::utils::display_path(&report.legacy_sessions_path)
                ));
                return report;
            }
        };
        if !file_type.is_file() || entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        report.legacy_session_file_count += 1;
        let name = PathBuf::from(entry.file_name());
        let destination_path = report.primary_sessions_path.join(&name);
        match std::fs::symlink_metadata(&destination_path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                report.already_present_file_count += 1;
            }
            Ok(metadata) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                let shape = if metadata.file_type().is_symlink() {
                    "destination session entry is a symlink"
                } else {
                    "destination session entry is not a regular file"
                };
                report.error = Some(format!(
                    "could not inspect destination session metadata at {}: {shape}",
                    crate::utils::display_path(&destination_path)
                ));
                return report;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                report.recoverable_file_count += 1;
                record_doctor_recoverable_session(
                    &mut report.recoverable,
                    DoctorRecoverableSessionEntry {
                        source_path: entry.path(),
                        destination_path,
                        name,
                    },
                );
            }
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect destination metadata at {}: {err}",
                    crate::utils::display_path(&destination_path)
                ));
                return report;
            }
        }
    }

    report.status = if report.legacy_session_file_count == 0 {
        DoctorSessionRecoveryStatus::NoLegacySessions
    } else if report.recoverable_file_count == 0 {
        DoctorSessionRecoveryStatus::MigrationComplete
    } else if primary_sessions_are_present {
        DoctorSessionRecoveryStatus::MigrationIncomplete
    } else {
        DoctorSessionRecoveryStatus::MigrationPending
    };
    report
}

/// Validate a session-state directory from observed metadata.
///
/// `doctor` only compares top-level filenames. It rejects a state-root or
/// sessions-root symlink observed during inspection rather than using it for a
/// recovery suggestion. This is a best-effort observation, not a race-free
/// no-follow guarantee. Missing paths are normal on a fresh install and are
/// reported as `false`.
fn doctor_session_directory_is_safe(path: &Path, label: &str) -> std::result::Result<bool, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "could not inspect {label} at {}: {error}",
                crate::utils::display_path(path)
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "could not inspect {label} at {}: path is a symlink",
            crate::utils::display_path(path)
        ));
    }
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "could not inspect {label} at {}: path is not a directory",
            crate::utils::display_path(path)
        ));
    }
    Ok(true)
}

/// Keep the report bounded while preserving a deterministic, lexical sample.
/// `read_dir` order is platform- and filesystem-dependent, so retaining the
/// first entries encountered would make the JSON and human receipts drift.
fn record_doctor_recoverable_session(
    recoverable: &mut Vec<DoctorRecoverableSessionEntry>,
    entry: DoctorRecoverableSessionEntry,
) {
    let insert_at = recoverable
        .binary_search_by(|existing| existing.name.cmp(&entry.name))
        .unwrap_or_else(|index| index);
    if recoverable.len() == DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT
        && insert_at == recoverable.len()
    {
        return;
    }
    recoverable.insert(insert_at, entry);
    if recoverable.len() > DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT {
        recoverable.pop();
    }
}

fn legacy_state_needs_attention(entry: &DoctorLegacyStateEntry) -> bool {
    entry.name != "sessions"
        && matches!(
            entry.status,
            DoctorLegacyStateStatus::LegacyOnly | DoctorLegacyStateStatus::Both
        )
}

fn print_doctor_legacy_state_report(
    report: &[DoctorLegacyStateEntry],
    session_recovery: &DoctorSessionRecoveryReport,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    let attention: Vec<_> = report
        .iter()
        .filter(|entry| legacy_state_needs_attention(entry))
        .collect();
    if attention.is_empty()
        && !session_recovery.needs_attention()
        && session_recovery.status != DoctorSessionRecoveryStatus::Isolated
    {
        println!(
            "  {} legacy state: no known .deepseek entries need migration",
            "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
        );
    } else if !attention.is_empty() {
        println!(
            "  {} legacy state needs review:",
            "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
        );
        for entry in attention {
            match entry.status {
                DoctorLegacyStateStatus::LegacyOnly => {
                    println!(
                        "    {} {} exists but {} is missing",
                        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                        crate::utils::display_path(&entry.legacy_path),
                        crate::utils::display_path(&entry.primary_path),
                    );
                }
                DoctorLegacyStateStatus::Both => {
                    println!(
                        "    {} {} exists alongside primary {}; legacy data may still need review",
                        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                        crate::utils::display_path(&entry.legacy_path),
                        crate::utils::display_path(&entry.primary_path),
                    );
                }
                DoctorLegacyStateStatus::PrimaryOnly | DoctorLegacyStateStatus::Absent => {}
            }
        }
        println!(
            "    Start Codewhale once to trigger safe migration where available, then rerun `codewhale doctor`."
        );
    }

    print_doctor_session_recovery_report(session_recovery, ok_rgb, warn_rgb);
}

fn print_doctor_session_recovery_report(
    report: &DoctorSessionRecoveryReport,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    match report.status {
        DoctorSessionRecoveryStatus::Isolated => {
            println!(
                "  {} legacy sessions: ambient ~/.deepseek/sessions was not inspected because CODEWHALE_HOME is set",
                "·".dimmed()
            );
            println!(
                "    This preserves the explicit home boundary. To inspect the default home, use a separate shell with CODEWHALE_HOME unset and rerun `codewhale doctor`."
            );
        }
        DoctorSessionRecoveryStatus::NoLegacySessions => {
            println!(
                "  {} legacy sessions: no top-level session JSON files found",
                "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
            );
        }
        DoctorSessionRecoveryStatus::MigrationComplete => {
            println!(
                "  {} legacy sessions: all {} filename(s) have regular-file counterparts under {}; descriptor contents were not compared and legacy originals remain preserved",
                "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2),
                report.legacy_session_file_count,
                crate::utils::display_path(&report.primary_sessions_path),
            );
        }
        DoctorSessionRecoveryStatus::MigrationPending
        | DoctorSessionRecoveryStatus::MigrationIncomplete => {
            let label = if report.status == DoctorSessionRecoveryStatus::MigrationIncomplete {
                "migration is incomplete"
            } else {
                "migration has not completed"
            };
            println!(
                "  {} legacy sessions: {label}; {} recoverable file(s) are absent from {}",
                "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                report.recoverable_file_count,
                crate::utils::display_path(&report.primary_sessions_path),
            );
            for entry in report
                .recoverable
                .iter()
                .take(DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT)
            {
                println!(
                    "    {} {} -> {}",
                    "·".dimmed(),
                    crate::utils::display_path(&entry.source_path),
                    crate::utils::display_path(&entry.destination_path),
                );
            }
            if report.recoverable_file_count > DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT {
                println!(
                    "    · {} more filename(s); `codewhale doctor --json` includes a bounded metadata-only sample",
                    report.recoverable_file_count - DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT
                );
            }
            println!("    Safe recovery:");
            println!(
                "      1. Back up {} and {} (if present).",
                crate::utils::display_path(&report.legacy_sessions_path),
                crate::utils::display_path(&report.primary_sessions_path),
            );
            println!(
                "      2. Close other Codewhale processes, then run `codewhale sessions`; migration adds only missing files, never overwrites primary files, and leaves legacy originals in place."
            );
            println!(
                "      3. Rerun `codewhale doctor`. If filenames remain, keep both backups and report only the listed source/destination names."
            );
        }
        DoctorSessionRecoveryStatus::ScanFailed => {
            println!(
                "  {} legacy sessions: recovery diagnostic could not complete",
                "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
            );
            if let Some(error) = report.error.as_deref() {
                println!("    {error}");
            }
            println!(
                "    Keep both session directories unchanged, back them up, fix path permissions or shape, and rerun `codewhale doctor` before attempting migration."
            );
        }
    }
    if report.status != DoctorSessionRecoveryStatus::Isolated {
        println!(
            "    Doctor inspected filenames and filesystem metadata only; it did not read chat contents, traverse checkpoints, or modify session files."
        );
    }
}

fn doctor_session_recovery_json(report: &DoctorSessionRecoveryReport) -> serde_json::Value {
    use serde_json::json;

    let recoverable: Vec<_> = report
        .recoverable
        .iter()
        .take(DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT)
        .map(|entry| {
            json!({
                "name": entry.name.display().to_string(),
                "source_path": entry.source_path.display().to_string(),
                "destination_path": entry.destination_path.display().to_string(),
            })
        })
        .collect();

    json!({
        "status": report.status.as_str(),
        "needs_attention": report.needs_attention(),
        "read_only": true,
        "chat_contents_read": false,
        "checkpoint_internals_scanned": false,
        "session_descriptors_compared": false,
        "counterpart_check": "top_level_filename_and_regular_file_only",
        "codewhale_home_is_explicit": report.codewhale_home_is_explicit,
        "legacy_sessions_path": report.legacy_sessions_path.display().to_string(),
        "primary_sessions_path": report.primary_sessions_path.display().to_string(),
        "legacy_session_file_count": report.legacy_session_file_count,
        "already_present_file_count": report.already_present_file_count,
        "recoverable_file_count": report.recoverable_file_count,
        "recoverable_files": recoverable,
        "recoverable_files_truncated": report.recoverable_file_count > report.recoverable.len(),
        "error": report.error,
        "recovery_command": if report.needs_attention() && report.status != DoctorSessionRecoveryStatus::ScanFailed {
            Some("codewhale sessions")
        } else {
            None
        },
    })
}

fn doctor_legacy_state_json(
    primary_root: &Path,
    legacy_root: &Path,
    report: &[DoctorLegacyStateEntry],
    session_recovery: &DoctorSessionRecoveryReport,
) -> serde_json::Value {
    use serde_json::json;

    let legacy_only = report
        .iter()
        .filter(|entry| entry.status == DoctorLegacyStateStatus::LegacyOnly)
        .count();
    let both = report
        .iter()
        .filter(|entry| entry.status == DoctorLegacyStateStatus::Both)
        .count();
    let entries: Vec<_> = report
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "primary_path": entry.primary_path.display().to_string(),
                "legacy_path": entry.legacy_path.display().to_string(),
                "primary_present": entry.primary_present,
                "legacy_present": entry.legacy_present,
                "status": entry.status.as_str(),
            })
        })
        .collect();

    json!({
        "primary_root": primary_root.display().to_string(),
        "legacy_root": legacy_root.display().to_string(),
        "needs_attention": report.iter().any(legacy_state_needs_attention) || session_recovery.needs_attention(),
        "legacy_only_count": legacy_only,
        "dual_present_count": both,
        "session_recovery": doctor_session_recovery_json(session_recovery),
        "entries": entries,
    })
}

fn doctor_setup_state(
    config: &Config,
    workspace: &Path,
) -> (codewhale_config::SetupState, &'static str) {
    if let Ok(Some(state)) = codewhale_config::SetupState::load() {
        return (state, "persisted");
    }

    (
        codewhale_config::SetupState::derive_inherited(&doctor_inherited_setup_facts(
            config, workspace,
        )),
        "derived",
    )
}

fn doctor_inherited_setup_facts(
    config: &Config,
    workspace: &Path,
) -> codewhale_config::InheritedConfigFacts {
    let user_constitution = codewhale_config::UserConstitution::load().ok();
    let user_constitution_validity = user_constitution.as_ref().map_or(
        codewhale_config::ConstitutionValidity::Unknown,
        codewhale_config::UserConstitutionLoad::validity,
    );
    let has_user_constitution = user_constitution
        .as_ref()
        .is_some_and(|loaded| !matches!(loaded, codewhale_config::UserConstitutionLoad::Missing));
    let has_expert_override = codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(Path::new(crate::prompts::CONSTITUTION_OVERRIDE_FILE)))
        .is_some_and(|path| path.exists());

    codewhale_config::InheritedConfigFacts {
        language: None,
        has_provider_route: !config.default_model().trim().is_empty(),
        has_credentials_or_local_runtime: doctor_has_credentials_or_local_runtime(config),
        trust_chosen: !crate::tui::onboarding::needs_trust(workspace),
        has_expert_override,
        has_user_constitution,
        user_constitution_validity,
    }
}

fn doctor_has_credentials_or_local_runtime(config: &Config) -> bool {
    if resolve_api_key_source(config) != ApiKeySource::Missing {
        return true;
    }

    matches!(
        config.api_provider(),
        crate::config::ApiProvider::Sglang
            | crate::config::ApiProvider::Vllm
            | crate::config::ApiProvider::Ollama
    )
}

fn print_doctor_setup_report(
    config: &Config,
    workspace: &Path,
    state: &codewhale_config::SetupState,
    source: &str,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    let first_run_ready = state.first_run_ready();
    let update_ready = state.update_ready(crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION);
    let operate_ready = state.operate_ready();
    let first_run_icon = if first_run_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };
    let update_icon = if update_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };
    let operate_icon = if operate_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };

    println!();
    println!("{}", "Setup State:".bold());
    println!("  · source: {source}");
    println!(
        "  {first_run_icon} first-run: {}",
        doctor_ready_label(first_run_ready)
    );
    println!(
        "  {update_icon} update checkpoint {}: {}",
        crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        doctor_ready_label(update_ready)
    );
    println!(
        "  {operate_icon} operate/fleet: {}",
        doctor_ready_label(operate_ready)
    );
    println!(
        "  · constitution autonomy: {} (guidance only)",
        doctor_constitution_autonomy_preference_id()
    );
    println!(
        "  · runtime posture: {}",
        doctor_runtime_posture_line(config, workspace)
    );
    let consistency = doctor_setup_consistency(state, source);
    if consistency["status"] == "inconsistent" {
        let issues = consistency["issues"]
            .as_array()
            .map(|issues| {
                issues
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!(
            "  {} consistency: half-applied setup detected ({issues}) — {}",
            "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
            consistency["repair"].as_str().unwrap_or("/setup"),
        );
    }
    println!(
        "  · next actions: /constitution (standing law), /setup report (readiness), /setup provider or /provider setup <name> (provider credentials), /model (route), /config (runtime posture), /setup fleet (Operate/Fleet readiness), /fleet setup (explicit profile authoring), /setup hotbar (optional shortcuts), /setup tools (Tools/MCP readiness), /setup remote (remote runtime on-ramp), /setup persistence (path review)"
    );
    for step in codewhale_config::SetupStep::ALL {
        let entry = state.steps.get(&step);
        let required = entry.is_some_and(|entry| entry.required);
        let version = entry.and_then(|entry| entry.version.as_deref());
        let result = entry.and_then(|entry| entry.result.as_deref());
        let required_label = if required { "required" } else { "optional" };
        let version_label = version.unwrap_or("unversioned");
        let result_label = result.unwrap_or("no result");
        println!(
            "    · {}: {} ({required_label}, {version_label}, {result_label})",
            setup_step_id(step),
            setup_status_id(state.status(step))
        );
    }
}

fn doctor_ready_label(ready: bool) -> &'static str {
    if ready { "ready" } else { "needs action" }
}

/// Detect half-applied setup persistence (#3410).
///
/// The setup transaction writes `constitution.json` and `setup_state.json`
/// together, so a persisted state that points at a user-global constitution
/// which is missing or unusable on disk means a write was interrupted or a
/// file was removed out-of-band. Stale `.tmp*` files in `$CODEWHALE_HOME`
/// are the other fingerprint of an interrupted atomic write.
fn doctor_setup_consistency(
    state: &codewhale_config::SetupState,
    source: &str,
) -> serde_json::Value {
    use serde_json::json;

    let mut issues: Vec<&'static str> = Vec::new();

    if source == "persisted"
        && matches!(
            state.constitution_source,
            codewhale_config::ConstitutionSource::UserGlobal
        )
    {
        match codewhale_config::UserConstitution::load() {
            Ok(codewhale_config::UserConstitutionLoad::Missing) => {
                issues.push("setup_state_points_at_missing_user_constitution");
            }
            Ok(codewhale_config::UserConstitutionLoad::Empty) => {
                issues.push("user_constitution_empty");
            }
            Ok(codewhale_config::UserConstitutionLoad::Invalid(_)) => {
                issues.push("user_constitution_invalid");
            }
            Ok(codewhale_config::UserConstitutionLoad::Unreadable(_)) | Err(_) => {
                issues.push("user_constitution_unreadable");
            }
            Ok(codewhale_config::UserConstitutionLoad::Loaded(_)) => {}
        }
    }

    if doctor_home_has_stale_setup_temp_files() {
        issues.push("stale_setup_temp_files_in_codewhale_home");
    }

    json!({
        "status": if issues.is_empty() { "consistent" } else { "inconsistent" },
        "issues": issues,
        "repair": "/constitution to rebuild standing law, /setup to re-run the checkpoint",
    })
}

fn doctor_home_has_stale_setup_temp_files() -> bool {
    let Ok(home) = codewhale_config::codewhale_home() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(&home) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().starts_with(".tmp")
            && entry.file_type().is_ok_and(|kind| kind.is_file())
    })
}

fn doctor_constitution_autonomy_preference() -> codewhale_config::AutonomyPreference {
    codewhale_config::UserConstitution::load()
        .ok()
        .and_then(|load| {
            load.constitution()
                .map(|constitution| constitution.autonomy_preference)
        })
        .unwrap_or(codewhale_config::AutonomyPreference::Unspecified)
}

fn doctor_constitution_autonomy_preference_id() -> &'static str {
    autonomy_preference_id(doctor_constitution_autonomy_preference())
}

fn autonomy_preference_id(preference: codewhale_config::AutonomyPreference) -> &'static str {
    match preference {
        codewhale_config::AutonomyPreference::Unspecified => "unspecified",
        codewhale_config::AutonomyPreference::Cautious => "cautious",
        codewhale_config::AutonomyPreference::Balanced => "balanced",
        codewhale_config::AutonomyPreference::Autonomous => "autonomous",
    }
}

fn doctor_runtime_default_mode() -> (String, &'static str) {
    match crate::settings::Settings::load_read_only() {
        Ok(settings) => (settings.default_mode, "settings"),
        Err(_) => (crate::settings::Settings::default().default_mode, "default"),
    }
}

fn doctor_runtime_posture_line(config: &Config, workspace: &Path) -> String {
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    let approval = config.approval_policy.as_deref().unwrap_or("on-request");
    let approval_source = if config.approval_policy.is_some() {
        "config"
    } else {
        "default"
    };
    let allow_shell = config.interactive_allow_shell();
    let allow_shell_source = if config.allow_shell.is_some() {
        "config"
    } else {
        "interactive default"
    };
    let sandbox = config.sandbox_mode.as_deref().unwrap_or("mode-derived");
    let sandbox_source = if config.sandbox_mode.is_some() {
        "config"
    } else {
        "default"
    };
    let network = config
        .network
        .as_ref()
        .map_or("prompt", |policy| policy.default.as_str());
    let network_source = if config.network.is_some() {
        "config"
    } else {
        "default"
    };
    let trust = if crate::tui::onboarding::needs_trust(workspace) {
        "workspace not elevated"
    } else {
        "workspace trusted"
    };

    format!(
        "default_mode={default_mode} ({default_mode_source}), approval_policy={approval} ({approval_source}), allow_shell={allow_shell} ({allow_shell_source}), sandbox={sandbox} ({sandbox_source}), network.default={network} ({network_source}), trust={trust}"
    )
}

fn doctor_operate_fleet_report_json(config: &Config, workspace: &Path) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    // Doctor reports configured routing posture only. In particular it must
    // never consume an external-file grant merely to label Fleet readiness.
    let auth_source = resolve_api_key_source(config);
    let has_credentials_or_local = doctor_auth_present_or_local(provider, auth_source);
    let subagents_enabled = config.subagents_enabled_for_provider(provider);
    let disabled_reason = if subagents_enabled {
        None
    } else {
        Some(
            config
                .subagents_disabled_reason()
                .unwrap_or("disabled for active provider"),
        )
    };
    let max_subagents = config.max_subagents_for_provider(provider);
    let launch_concurrency = config.launch_concurrency_for_provider(provider);
    let max_admitted = config.max_admitted_subagents_for_provider(provider);
    let max_spawn_depth = config.subagent_max_spawn_depth_for_provider(provider);
    let roster = crate::fleet::roster::FleetRoster::load(&config.fleet_config(), workspace);
    let mut built_in_members = 0usize;
    let mut config_members = 0usize;
    let mut personal_members = 0usize;
    let mut workspace_members = 0usize;
    for member in roster.members() {
        match member.origin {
            crate::fleet::roster::ProfileOrigin::BuiltIn => built_in_members += 1,
            crate::fleet::roster::ProfileOrigin::Config => config_members += 1,
            crate::fleet::roster::ProfileOrigin::Personal => personal_members += 1,
            crate::fleet::roster::ProfileOrigin::Workspace => workspace_members += 1,
        }
    }
    let roster_members = roster.members().len();
    let custom_members = config_members + personal_members + workspace_members;
    let roster_ready = roster_members > 0;
    let runtime_ready =
        subagents_enabled && max_subagents > 0 && launch_concurrency > 0 && max_spawn_depth > 0;

    json!({
        "ready": has_credentials_or_local && runtime_ready && roster_ready,
        "provider": {
            "id": config.provider_identity_for(provider),
            "auth": {
                "present_or_local": has_credentials_or_local,
                "source": doctor_api_key_source_label(auth_source),
            },
        },
        "worker_runtime": {
            "ready": runtime_ready,
            "enabled": subagents_enabled,
            "disabled_reason": disabled_reason,
            "max_subagents": max_subagents,
            "launch_concurrency": launch_concurrency,
            "max_admitted": max_admitted,
            "max_spawn_depth": max_spawn_depth,
            "host_enforced_workflow_receipts": true,
        },
        "roster": {
            "ready": roster_ready,
            "total": roster_members,
            "built_in": built_in_members,
            "config": config_members,
            "personal": personal_members,
            "workspace": workspace_members,
            "custom": custom_members,
            "starter_roster_available": built_in_members > 0,
            "readiness_rule": "built-in starter roster or custom roster",
        },
        "concurrency": {
            "launch_concurrency": launch_concurrency,
            "max_subagents": max_subagents,
            "max_admitted": max_admitted,
            "plan_limit_probed": false,
        },
    })
}

fn doctor_provider_model_report_json(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    let auth_source = resolve_api_key_source(config);
    let auth_present_or_local = doctor_auth_present_or_local(provider, auth_source);
    let credential_help = provider.credential_help();

    json!({
        "provider": {
            "id": config.provider_identity_for(provider),
            "display": provider.display_name(),
        },
        "model": {
            "resolved": config.default_model(),
        },
        "auth": {
            "present_or_local": auth_present_or_local,
            "source": doctor_api_key_source_label(auth_source),
            "env_vars": provider.env_vars(),
            "credential_mode": credential_help.acquisition.as_str(),
            "credential_url": credential_help.credential_url,
            "credential_docs_url": credential_help.docs_url,
            "credential_guidance": credential_help.guidance,
            "oauth_only": credential_help.acquisition
                == codewhale_config::provider::CredentialAcquisition::OAuth,
        },
        "health": {
            "live_validation": false,
            "next_action": if auth_present_or_local {
                "/model"
            } else {
                "/setup provider or /provider setup <name>"
            },
        },
    })
}

fn doctor_auth_present_or_local(
    provider: crate::config::ApiProvider,
    auth_source: ApiKeySource,
) -> bool {
    !matches!(
        auth_source,
        ApiKeySource::Missing | ApiKeySource::ExternalConsent
    ) || matches!(
        provider,
        crate::config::ApiProvider::Sglang
            | crate::config::ApiProvider::Vllm
            | crate::config::ApiProvider::Ollama
    )
}

fn doctor_external_credential_consent_statuses(
    config: &Config,
) -> Vec<codewhale_config::ExternalCredentialConsentStatus> {
    [
        crate::config::ApiProvider::OpenaiCodex,
        crate::config::ApiProvider::Xai,
    ]
    .into_iter()
    .filter_map(|provider| config.external_credential_consent_status(provider))
    .collect()
}

fn doctor_external_credential_consent_lines(config: &Config) -> Vec<String> {
    doctor_external_credential_consent_statuses(config)
        .into_iter()
        .flat_map(|status| {
            let mut lines = vec![
                format!(
                    "{}: access={}, provider={}, source={}, owner={}, path={}, version={}, state={}, ambient_path_changed={}",
                    status.provider,
                    status.access.as_str(),
                    status.provider,
                    status.source.as_str(),
                    status.owner,
                    codewhale_config::quote_os_path(&status.path),
                    status.consent_version,
                    status.route_state,
                    status.ambient_path_changed,
                ),
                format!("  semantics: {}", status.semantics),
                format!("  revoke: {}", status.revoke_command),
            ];
            if let Some(warning) = status.ambient_path_warning() {
                lines.push(format!("  {warning}"));
            }
            lines
        })
        .collect()
}

fn doctor_external_credential_consent_json(config: &Config) -> serde_json::Value {
    serde_json::Value::Array(
        doctor_external_credential_consent_statuses(config)
            .into_iter()
            .map(|status| {
                serde_json::json!({
                    "provider": status.provider,
                    "access": status.access.as_str(),
                    "source": status.source.as_str(),
                    "owner": status.owner,
                    "path": codewhale_config::quote_os_path(&status.path),
                    "consent_version": status.consent_version,
                    "scope_valid": status.scope_valid,
                    "ambient_path_changed": status.ambient_path_changed,
                    "ambient_path_warning": status.ambient_path_warning(),
                    "route_state": status.route_state,
                    "semantics": status.semantics,
                    "revoke_command": status.revoke_command,
                })
            })
            .collect(),
    )
}

fn doctor_setup_report_json(config: &Config, workspace: &Path) -> serde_json::Value {
    use serde_json::json;

    let (state, source) = doctor_setup_state(config, workspace);
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    let approval_policy = config.approval_policy.as_deref().unwrap_or("on-request");
    let approval_policy_source = if config.approval_policy.is_some() {
        "config"
    } else {
        "default"
    };
    let allow_shell = config.interactive_allow_shell();
    let allow_shell_source = if config.allow_shell.is_some() {
        "config"
    } else {
        "interactive_default"
    };
    let sandbox_mode = config.sandbox_mode.as_deref().unwrap_or("mode-derived");
    let sandbox_mode_source = if config.sandbox_mode.is_some() {
        "config"
    } else {
        "default"
    };
    let network_default = config
        .network
        .as_ref()
        .map_or("prompt", |policy| policy.default.as_str());
    let network_source = if config.network.is_some() {
        "config"
    } else {
        "default"
    };
    let workspace_trusted = !crate::tui::onboarding::needs_trust(workspace);
    let steps: Vec<_> = codewhale_config::SetupStep::ALL
        .into_iter()
        .map(|step| {
            let entry = state.steps.get(&step);
            json!({
                "step": setup_step_id(step),
                "status": setup_status_id(state.status(step)),
                "required": entry.is_some_and(|entry| entry.required),
                "version": entry.and_then(|entry| entry.version.clone()),
                "result": entry.and_then(|entry| entry.result.clone()),
            })
        })
        .collect();

    json!({
        "source": source,
        "schema_version": state.schema_version,
        "inherited": state.inherited,
        "checkpoint_version": crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        "first_run_ready": state.first_run_ready(),
        "update_ready": state.update_ready(crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION),
        "operate_ready": state.operate_ready(),
        "constitution": {
            "choice": constitution_choice_id(state.constitution_choice),
            "source": constitution_source_id(state.constitution_source),
            "validity": constitution_validity_id(state.constitution_validity),
            "checkpoint_completed_for": state.constitution_checkpoint_completed_for.clone(),
            "language": state.constitution_language.clone(),
            "preview_hash_present": state.constitution_preview_hash.is_some(),
            "preview_version": state.constitution_preview_version,
            "autonomy_preference": doctor_constitution_autonomy_preference_id(),
        },
        "runtime_posture_source": runtime_posture_source_id(state.runtime_posture_source),
        "runtime_posture": {
            "source": runtime_posture_source_id(state.runtime_posture_source),
            "default_mode": {
                "value": default_mode,
                "source": default_mode_source,
            },
            "approval_policy": {
                "value": approval_policy,
                "source": approval_policy_source,
            },
            "allow_shell": {
                "value": allow_shell,
                "source": allow_shell_source,
            },
            "sandbox_mode": {
                "value": sandbox_mode,
                "source": sandbox_mode_source,
            },
            "network_default": {
                "value": network_default,
                "source": network_source,
            },
            "workspace_trust": {
                "trusted": workspace_trusted,
                "source": "workspace",
            },
        },
        "provider_model": doctor_provider_model_report_json(config),
        "operate_fleet": doctor_operate_fleet_report_json(config, workspace),
        "consistency": doctor_setup_consistency(&state, source),
        "next_actions": {
            "constitution": "/constitution",
            "setup_report": "/setup report",
            "provider_model": "/setup provider, /provider setup <name>, or /model",
            "runtime_posture": "/config",
            "operate_fleet": "/setup fleet (readiness), /fleet setup (explicit profile authoring)",
            "hotbar": "/setup hotbar",
            "tools_mcp": "/setup tools",
            "remote_runtime": "/setup remote",
            "persistence": "/setup persistence",
        },
        "steps": steps,
    })
}

fn setup_step_id(step: codewhale_config::SetupStep) -> &'static str {
    match step {
        codewhale_config::SetupStep::Language => "language",
        codewhale_config::SetupStep::ProviderModel => "provider_model",
        codewhale_config::SetupStep::TrustSandbox => "trust_sandbox",
        codewhale_config::SetupStep::ToolsMcp => "tools_mcp",
        codewhale_config::SetupStep::Hotbar => "hotbar",
        codewhale_config::SetupStep::RemoteRuntime => "remote_runtime",
        codewhale_config::SetupStep::Persistence => "persistence",
        codewhale_config::SetupStep::Constitution => "constitution",
        codewhale_config::SetupStep::OperateFleet => "operate_fleet",
        codewhale_config::SetupStep::Verification => "verification",
    }
}

fn setup_status_id(status: codewhale_config::StepStatus) -> &'static str {
    match status {
        codewhale_config::StepStatus::NotStarted => "not_started",
        codewhale_config::StepStatus::Recommended => "recommended",
        codewhale_config::StepStatus::Optional => "optional",
        codewhale_config::StepStatus::Deferred => "deferred",
        codewhale_config::StepStatus::InProgress => "in_progress",
        codewhale_config::StepStatus::Verified => "verified",
        codewhale_config::StepStatus::NeedsAction => "needs_action",
        codewhale_config::StepStatus::Failed => "failed",
        codewhale_config::StepStatus::Skipped => "skipped",
    }
}

fn constitution_choice_id(choice: codewhale_config::ConstitutionChoice) -> &'static str {
    match choice {
        codewhale_config::ConstitutionChoice::Unset => "unset",
        codewhale_config::ConstitutionChoice::Bundled => "bundled",
        codewhale_config::ConstitutionChoice::GuidedCustom => "guided_custom",
        codewhale_config::ConstitutionChoice::ExpertOverride => "expert_override",
        codewhale_config::ConstitutionChoice::Deferred => "deferred",
    }
}

fn constitution_source_id(source: codewhale_config::ConstitutionSource) -> &'static str {
    match source {
        codewhale_config::ConstitutionSource::Bundled => "bundled",
        codewhale_config::ConstitutionSource::UserGlobal => "user_global",
        codewhale_config::ConstitutionSource::ExpertOverride => "expert_override",
    }
}

fn constitution_validity_id(validity: codewhale_config::ConstitutionValidity) -> &'static str {
    match validity {
        codewhale_config::ConstitutionValidity::Unknown => "unknown",
        codewhale_config::ConstitutionValidity::Valid => "valid",
        codewhale_config::ConstitutionValidity::Invalid => "invalid",
        codewhale_config::ConstitutionValidity::Empty => "empty",
        codewhale_config::ConstitutionValidity::Unreadable => "unreadable",
    }
}

fn runtime_posture_source_id(source: codewhale_config::RuntimePostureSource) -> &'static str {
    match source {
        codewhale_config::RuntimePostureSource::Unset => "unset",
        codewhale_config::RuntimePostureSource::Inherited => "inherited",
        codewhale_config::RuntimePostureSource::Confirmed => "confirmed",
    }
}

/// Machine-readable counterpart to `run_doctor`. Skips the live API call so it
/// is safe to run in CI and from non-interactive scripts.
fn run_doctor_json(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    use serde_json::json;

    let config_path = config_path_override
        .map(PathBuf::from)
        .or_else(|| codewhale_config::resolve_config_path(None).ok())
        .unwrap_or_else(|| {
            codewhale_config::codewhale_home()
                .unwrap_or_else(|_| PathBuf::from(".codewhale"))
                .join("config.toml")
        });

    let api_key_state = match resolve_api_key_source(config) {
        ApiKeySource::Env => "env",
        ApiKeySource::Config => "config",
        ApiKeySource::Keyring => "keyring",
        ApiKeySource::OAuth => "oauth",
        ApiKeySource::ExternalConsent => "external_consent",
        ApiKeySource::NoAuth => "none",
        ApiKeySource::Missing => "missing",
    };

    let mcp_config_path = config.mcp_config_path();
    let project_mcp_config_path = crate::mcp::workspace_mcp_config_path(workspace);
    let mcp_present = mcp_config_path.exists();
    let project_mcp_present = project_mcp_config_path.exists();
    let mcp_summary = match crate::mcp::load_config_with_workspace_and_plugins(
        &mcp_config_path,
        workspace,
        plugins,
    ) {
        Ok(cfg) => {
            let servers: Vec<serde_json::Value> = cfg
                .servers
                .iter()
                .map(|(name, server)| doctor_mcp_server_json(name, server))
                .collect();
            json!({
                "config_path": mcp_config_path.display().to_string(),
                "present": mcp_present,
                "project_config_path": project_mcp_config_path.display().to_string(),
                "project_present": project_mcp_present,
                "probe_scope": "configuration",
                "live_health_checked": false,
                "servers": servers,
            })
        }
        Err(err) => json!({
            "config_path": mcp_config_path.display().to_string(),
            "present": mcp_present,
            "project_config_path": project_mcp_config_path.display().to_string(),
            "project_present": project_mcp_present,
            "probe_scope": "configuration",
            "live_health_checked": false,
            "servers": [],
            "error": err.to_string(),
        }),
    };

    let global_skills_dir = config.skills_dir();
    let agents_skills_dir = workspace.join(".agents").join("skills");
    let local_skills_dir = workspace.join("skills");
    let agents_global_skills_dir = crate::skills::agents_global_skills_dir();
    // #432: cross-tool skill discovery dirs surface in the JSON
    // report so external dashboards can see whether any
    // `.opencode/skills/`, `.claude/skills/`, `.cursor/skills/`, or
    // global agentskills.io content is contributing to the merged catalogue.
    let opencode_skills_dir = workspace.join(".opencode").join("skills");
    let claude_skills_dir = workspace.join(".claude").join("skills");
    let selected_skills_dir = if agents_skills_dir.exists() {
        agents_skills_dir.clone()
    } else if local_skills_dir.exists() {
        local_skills_dir.clone()
    } else if config.skills_dir.is_none()
        && let Some(global_agents) = agents_global_skills_dir.as_ref()
        && global_agents.exists()
    {
        global_agents.clone()
    } else {
        global_skills_dir.clone()
    };
    let agents_global_summary = agents_global_skills_dir
        .as_ref()
        .map(|path| {
            json!({
                "path": path.display().to_string(),
                "present": path.exists(),
                "count": skills_count_for(path),
            })
        })
        .unwrap_or_else(|| {
            json!({
                "path": null,
                "present": false,
                "count": 0,
            })
        });

    let tools_dir = default_tools_dir();
    let plugins_dir = default_plugins_dir();

    // Memory feature state (#489). Operators ask "is memory on?" and
    // "where does it live?" — surface both here so the question can be
    // answered without booting the TUI. Both inputs are checked: the
    // config flag and the env-var override that the runtime would
    // honour. (The dedicated `Config::memory_enabled()` accessor lives
    // on the memory-MVP branch (#518); this duplicates the same logic
    // until the two PRs land and it can be replaced with a single
    // method call.)
    let memory_path = config.memory_path();
    let memory_enabled_env = std::env::var("CODEWHALE_MEMORY")
        .or_else(|_| std::env::var("DEEPSEEK_MEMORY"))
        .ok()
        .map(|raw| {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "on" | "true" | "yes" | "y" | "enabled"
            )
        })
        .unwrap_or(false);
    let memory_summary = json!({
        // The MVP feature is opt-in by default; this defaults to false
        // on branches without the [memory] section in `Config`.
        "enabled": memory_enabled_env,
        "path": memory_path.display().to_string(),
        "file_present": memory_path.exists(),
    });
    let api_target = doctor_api_target(config);
    let strict_tool_mode = doctor_strict_tool_mode_status(config);
    let tls_status = doctor_tls_status(config);
    let (code_home, legacy_home) = doctor_state_roots();
    let legacy_state_report = doctor_legacy_state_report(&code_home, &legacy_home);
    let session_recovery = doctor_session_recovery_report(
        &code_home,
        &legacy_home,
        codewhale_config::codewhale_home_is_explicit(),
    );

    let stash = crate::composer_stash::diagnostic_stash_report();
    let report = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "config_path": config_path.display().to_string(),
        "config_present": config_path.exists(),
        "workspace": workspace.display().to_string(),
        "legacy_state": doctor_legacy_state_json(
            &code_home,
            &legacy_home,
            &legacy_state_report,
            &session_recovery,
        ),
        "setup": doctor_setup_report_json(config, workspace),
        "api_key": {
            "source": api_key_state,
        },
        "external_credentials": doctor_external_credential_consent_json(config),
        "base_url": crate::client::redact_url_for_display(&api_target.base_url),
        "default_text_model": api_target.model,
        "route": doctor_route_report(config),
        "strict_tool_mode": {
            "enabled": strict_tool_mode.enabled,
            "status": strict_tool_mode.status,
            "function_strict_sent": strict_tool_mode.function_strict_sent,
            "message": strict_tool_mode.message,
            "recommended_base_url": strict_tool_mode.recommended_base_url,
        },
        "tls": {
            "certificate_verification": tls_status.certificate_verification,
            "insecure_skip_tls_verify": tls_status.insecure_skip_tls_verify,
            "provider": tls_status.provider,
            "message": tls_status.message,
        },
        "search_provider": doctor_search_provider_json(config),
        "memory": memory_summary,
        "mcp": mcp_summary,
        "skills": {
            "selected": selected_skills_dir.display().to_string(),
            "global": {
                "path": global_skills_dir.display().to_string(),
                "present": global_skills_dir.exists(),
                "count": skills_count_for(&global_skills_dir),
            },
            "agents": {
                "path": agents_skills_dir.display().to_string(),
                "present": agents_skills_dir.exists(),
                "count": skills_count_for(&agents_skills_dir),
            },
            "agents_global": agents_global_summary,
            "local": {
                "path": local_skills_dir.display().to_string(),
                "present": local_skills_dir.exists(),
                "count": skills_count_for(&local_skills_dir),
            },
            "opencode": {
                "path": opencode_skills_dir.display().to_string(),
                "present": opencode_skills_dir.exists(),
                "count": skills_count_for(&opencode_skills_dir),
            },
            "claude": {
                "path": claude_skills_dir.display().to_string(),
                "present": claude_skills_dir.exists(),
                "count": skills_count_for(&claude_skills_dir),
            },
        },
        "tools": {
            "path": tools_dir.display().to_string(),
            "present": tools_dir.exists(),
            "count": if tools_dir.exists() { count_dir_entries(&tools_dir) } else { 0 },
        },
        "plugins": {
            "path": plugins_dir.display().to_string(),
            "present": plugins_dir.exists(),
            "count": if plugins_dir.exists() { count_dir_entries(&plugins_dir) } else { 0 },
        },
        "storage": {
            "spillover": {
                "path": crate::tools::truncate::spillover_root()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                "present": crate::tools::truncate::spillover_root()
                    .is_some_and(|p| p.is_dir()),
                "count": crate::tools::truncate::spillover_root()
                    .filter(|p| p.is_dir())
                    .map(|p| count_dir_entries(&p))
                    .unwrap_or(0),
            },
            "stash": {
                "path": stash
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                "present": stash.present,
                "count": stash.count,
                "error": stash.error,
            },
        },
        "sandbox": match crate::sandbox::get_platform_sandbox() {
            Some(kind) => json!({"available": true, "kind": kind.to_string()}),
            None => json!({"available": false, "kind": null}),
        },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "api_connectivity": {
            "checked": false,
            "note": "Skipped in --json mode; run `codewhale doctor` for a live check.",
        },
        "capability": provider_capability_report(config),
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_doctor_context_json(config: &Config, workspace: &Path) -> Result<()> {
    let report = crate::context_report::build_headless_context_report(config, workspace);
    println!("{}", crate::context_report::context_report_json(&report));
    Ok(())
}

/// Build the `capability` section for the machine-readable doctor report.
///
/// Returns a JSON value with the resolved provider, resolved model, context
/// window, max output, thinking support, cache telemetry support, and request
/// payload mode.
fn provider_capability_report(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    let model = config.default_model();

    let cap = crate::config::provider_capability(provider, &model);
    let alias_deprecation = config.active_deepseek_alias_deprecation();

    json!({
        "resolved_provider": config.provider_identity_for(provider),
        "resolved_model": cap.resolved_model,
        "context_window": cap.context_window,
        "max_output": cap.max_output,
        "thinking_supported": cap.thinking_supported,
        "cache_telemetry_supported": cap.cache_telemetry_supported,
        "request_payload_mode": serde_json::to_value(cap.request_payload_mode).unwrap_or_default(),
        "alias_deprecation": alias_deprecation,
    })
}

fn doctor_route_report(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let target = doctor_api_target(config);
    let provider = config.api_provider();
    let redacted_base_url = crate::client::redact_url_for_display(&target.base_url);
    let context_window = crate::route_runtime::resolve_runtime_route(
        config,
        provider,
        Some(&target.model),
    )
    .ok()
    .map(|route| {
        json!({
            "tokens": route.context_window.tokens,
            "source": route.context_window.source.label(),
        })
    })
    .unwrap_or_else(|| {
        json!({
            "tokens": crate::config::provider_capability(provider, &target.model).context_window,
            "source": crate::route_runtime::ContextWindowSource::Fallback.label(),
        })
    });

    json!({
        "provider": target.provider,
        "provider_source": doctor_provider_source(config),
        "provider_config_table": doctor_provider_config_table(config, provider),
        "model": target.model,
        "wire_protocol": doctor_wire_protocol(provider),
        "base_url": {
            "redacted": redacted_base_url,
            "class": doctor_base_url_class(provider, &target.base_url),
            "fingerprint": crate::utils::redacted_identifier_for_log(&target.base_url),
        },
        "auth": {
            "scheme": doctor_auth_scheme(config),
            "source": doctor_api_key_source_label(resolve_api_key_source(config)),
        },
        "context_window": context_window,
    })
}

fn doctor_provider_config_table(config: &Config, provider: crate::config::ApiProvider) -> String {
    if provider != crate::config::ApiProvider::Custom {
        return provider_config_table_key(provider).to_string();
    }
    if config.uses_legacy_literal_custom_route() {
        "root (legacy literal custom)".to_string()
    } else {
        format!("providers.{}", config.provider_identity_for(provider))
    }
}

fn doctor_provider_source(config: &Config) -> &'static str {
    if config
        .provider
        .as_ref()
        .is_some_and(|provider| !provider.trim().is_empty())
    {
        "config"
    } else {
        "default"
    }
}

fn doctor_wire_protocol(provider: crate::config::ApiProvider) -> &'static str {
    match provider
        .metadata()
        .map(|metadata| metadata.wire())
        .unwrap_or(codewhale_config::provider::WireFormat::ChatCompletions)
    {
        codewhale_config::provider::WireFormat::ChatCompletions => "chat_completions",
        codewhale_config::provider::WireFormat::Responses => "responses",
        codewhale_config::provider::WireFormat::AnthropicMessages => "anthropic_messages",
    }
}

fn doctor_base_url_class(provider: crate::config::ApiProvider, base_url: &str) -> &'static str {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    if normalized.starts_with("http://localhost")
        || normalized.starts_with("http://127.0.0.1")
        || normalized.starts_with("http://[::1]")
    {
        return "local";
    }
    if normalized
        == provider
            .default_base_url()
            .trim_end_matches('/')
            .to_ascii_lowercase()
    {
        "default"
    } else {
        "custom"
    }
}

fn doctor_auth_scheme(config: &Config) -> &'static str {
    let provider = config.api_provider();
    if crate::config::auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref())
    {
        "none"
    } else if provider == crate::config::ApiProvider::Anthropic {
        "x-api-key"
    } else if provider == crate::config::ApiProvider::XiaomiMimo
        && (doctor_xiaomi_mimo_base_url_uses_token_plan(&config.deepseek_base_url())
            || config
                .deepseek_api_key_read_only()
                .ok()
                .is_some_and(|key| key.trim_start().starts_with("tp-")))
    {
        "api-key"
    } else if matches!(
        provider,
        crate::config::ApiProvider::Sglang
            | crate::config::ApiProvider::Vllm
            | crate::config::ApiProvider::Ollama
    ) && config.deepseek_api_key_read_only().is_err()
    {
        "none"
    } else {
        "bearer"
    }
}

fn doctor_xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/');
    [
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL,
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL,
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL,
    ]
    .iter()
    .any(|candidate| normalized.eq_ignore_ascii_case(candidate.trim_end_matches('/')))
}

fn doctor_api_key_source_label(source: ApiKeySource) -> &'static str {
    match source {
        ApiKeySource::Env => "env",
        ApiKeySource::Config => "config",
        ApiKeySource::Keyring => "keyring",
        ApiKeySource::OAuth => "oauth",
        ApiKeySource::ExternalConsent => "external_consent",
        ApiKeySource::NoAuth => "none",
        ApiKeySource::Missing => "missing",
    }
}

fn doctor_search_provider_line(config: &Config) -> String {
    let search_provider = config.search_provider_resolution();
    let switch_hint = if matches!(
        (search_provider.provider, search_provider.source),
        (
            crate::config::SearchProvider::DuckDuckGo,
            crate::config::SearchProviderSource::Default
        )
    ) {
        "; set [search] provider = \"bing\" | \"tavily\" | \"bocha\" to switch"
    } else {
        ""
    };

    format!(
        "search_provider: {} (source: {}{})",
        search_provider.provider.as_str(),
        search_provider.source.as_str(),
        switch_hint
    )
}

fn doctor_search_provider_json(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let search_provider = config.search_provider_resolution();
    json!({
        "provider": search_provider.provider.as_str(),
        "source": search_provider.source.as_str(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorApiTarget {
    provider: String,
    base_url: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorStrictToolModeStatus {
    enabled: bool,
    status: &'static str,
    function_strict_sent: bool,
    message: String,
    recommended_base_url: Option<String>,
}

fn doctor_api_target(config: &Config) -> DoctorApiTarget {
    let provider = config.api_provider();
    DoctorApiTarget {
        provider: config.provider_identity_for(provider),
        base_url: config.deepseek_base_url(),
        model: config.default_model(),
    }
}

fn doctor_strict_tool_mode_status(config: &Config) -> DoctorStrictToolModeStatus {
    if !config.strict_tool_mode.unwrap_or(false) {
        return DoctorStrictToolModeStatus {
            enabled: false,
            status: "disabled",
            function_strict_sent: false,
            message: "disabled".to_string(),
            recommended_base_url: None,
        };
    }

    let target = doctor_api_target(config);
    match known_deepseek_base_url_kind(&target.base_url) {
        Some(DeepSeekBaseUrlKind::Beta) => DoctorStrictToolModeStatus {
            enabled: true,
            status: "ready",
            function_strict_sent: true,
            message: "enabled; DeepSeek strict schemas use the beta endpoint".to_string(),
            recommended_base_url: None,
        },
        Some(DeepSeekBaseUrlKind::NonBeta) => {
            let recommended = recommended_strict_base_url(config, &target.base_url);
            DoctorStrictToolModeStatus {
                enabled: true,
                status: "fallback_non_beta",
                function_strict_sent: false,
                message:
                    "enabled, but function.strict is stripped for this non-beta DeepSeek endpoint"
                        .to_string(),
                recommended_base_url: Some(recommended.to_string()),
            }
        }
        None => DoctorStrictToolModeStatus {
            enabled: true,
            status: "custom_endpoint",
            function_strict_sent: true,
            message: "enabled; function.strict will be sent to this custom endpoint".to_string(),
            recommended_base_url: None,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorTlsStatus {
    certificate_verification: bool,
    insecure_skip_tls_verify: bool,
    provider: String,
    message: String,
}

fn doctor_tls_status(config: &Config) -> DoctorTlsStatus {
    let provider = config.provider_identity_for(config.api_provider());
    let insecure_skip_tls_verify = config.insecure_skip_tls_verify();
    let message = if insecure_skip_tls_verify {
        format!(
            "TLS certificate verification cannot be disabled for provider {provider}; use SSL_CERT_FILE with a trusted custom CA bundle"
        )
    } else {
        "TLS certificate verification enabled".to_string()
    };
    DoctorTlsStatus {
        certificate_verification: true,
        insecure_skip_tls_verify,
        provider,
        message,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeepSeekBaseUrlKind {
    Beta,
    NonBeta,
}

fn known_deepseek_base_url_kind(base_url: &str) -> Option<DeepSeekBaseUrlKind> {
    let normalized = base_url.trim_end_matches('/');
    if normalized.eq_ignore_ascii_case("https://api.deepseek.com/beta")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com/beta")
    {
        Some(DeepSeekBaseUrlKind::Beta)
    } else if normalized.eq_ignore_ascii_case("https://api.deepseek.com")
        || normalized.eq_ignore_ascii_case("https://api.deepseek.com/v1")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com/v1")
    {
        Some(DeepSeekBaseUrlKind::NonBeta)
    } else {
        None
    }
}

fn recommended_strict_base_url(_config: &Config, _base_url: &str) -> &'static str {
    crate::config::DEFAULT_DEEPSEEK_BASE_URL
}

fn doctor_timeout_recovery_lines(config: &Config) -> Vec<String> {
    let target = doctor_api_target(config);
    let mut lines = vec![format!(
        "Connection timed out while reaching {}.",
        target.base_url
    )];

    match config.api_provider() {
        crate::config::ApiProvider::Deepseek
            if target.base_url.contains("api.deepseek.com")
                && !target.base_url.contains("api.deepseeki.com") =>
        {
            lines.push(
                "If this is a custom DeepSeek-compatible endpoint, set its HTTPS base URL in ~/.codewhale/config.toml and rerun `codewhale doctor`."
                    .to_string(),
            );
        }
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN => {
            lines.push(
                "If this is a custom DeepSeek-compatible endpoint, confirm it serves `/v1/models` and `/v1/chat/completions` over HTTPS."
                    .to_string(),
            );
        }
        _ => {
            lines.push(
                "Confirm the configured provider endpoint is reachable and OpenAI-compatible for `/v1/models` and `/v1/chat/completions`."
                    .to_string(),
            );
        }
    }

    lines.push(
        "Run `codewhale doctor --json` and include `base_url`, `default_text_model`, and `api_connectivity` when filing an issue."
            .to_string(),
    );
    lines
}

fn run_execpolicy_command(command: ExecpolicyCommand) -> Result<()> {
    match command.command {
        ExecpolicySubcommand::Check(cmd) => cmd.run(),
    }
}

fn run_features_command(config: &Config, command: FeaturesCli) -> Result<()> {
    match command.command {
        FeaturesSubcommand::List => {
            print!("{}", render_feature_table(&config.features()));
            Ok(())
        }
    }
}

async fn run_models(config: &Config, args: ModelsArgs) -> Result<()> {
    use crate::client::DeepSeekClient;

    let client = DeepSeekClient::new(config)?;
    let mut models = client.list_models().await?;
    models.sort_by(|a, b| a.id.cmp(&b.id));

    if args.json {
        println!("{}", serde_json::to_string_pretty(&models)?);
        return Ok(());
    }

    if models.is_empty() {
        println!("No models returned by the API.");
        return Ok(());
    }

    let default_model = config.default_model();

    println!("Available models (default: {default_model})");
    for model in models {
        let marker = if model.id == default_model { "*" } else { " " };
        if let Some(owner) = model.owned_by {
            println!("{marker} {} ({owner})", model.id);
        } else {
            println!("{marker} {}", model.id);
        }
    }

    Ok(())
}

async fn run_speech(config: &Config, args: SpeechArgs) -> Result<()> {
    use crate::client::{DeepSeekClient, SpeechSynthesisRequest};
    use crate::config::ApiProvider;
    use crate::tools::speech::{
        DEFAULT_VOICE, SPEECH_MODEL_EXAMPLES, combine_speech_instructions,
        default_speech_output_name, describe_speech_voice, encode_voice_clone_sample_data_uri,
        infer_speech_model, normalize_speech_format,
    };

    let SpeechArgs {
        text,
        output,
        output_dir,
        model,
        voice,
        instruction,
        voice_prompt,
        clone_voice,
        format,
        json: json_output,
    } = args;

    if config.api_provider() != ApiProvider::XiaomiMimo {
        bail!(
            "`speech` requires provider = \"xiaomi-mimo\" (current: {}). Run with `--provider xiaomi-mimo` or set it in config.",
            config.api_provider().as_str()
        );
    }

    if text.trim().is_empty() {
        bail!("Speech text cannot be empty");
    }
    let voice_is_data_uri = voice
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| value.starts_with("data:audio/"));
    if clone_voice.is_some() && voice.is_some() {
        bail!("Use either --clone-voice or --voice for cloned voice data, not both");
    }
    let model = infer_speech_model(
        model.as_deref(),
        clone_voice.is_some() || voice_is_data_uri,
        voice_prompt.is_some(),
    );
    let model_lower = model.to_ascii_lowercase();
    if !model_lower.contains("tts") {
        bail!(
            "speech requires a TTS model (examples: {}); got {model}",
            SPEECH_MODEL_EXAMPLES.join(", ")
        );
    }
    let is_voice_design = model_lower.contains("voicedesign");
    let is_voice_clone = model_lower.contains("voiceclone");

    let instruction = combine_speech_instructions(instruction, voice_prompt);
    if is_voice_design
        && instruction
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        bail!(
            "mimo-v2.5-tts-voicedesign requires --voice-prompt or --instruction to describe the voice"
        );
    }

    let voice = if let Some(clone_path) = clone_voice {
        Some(encode_voice_clone_sample_data_uri(&clone_path)?)
    } else if is_voice_design {
        None
    } else if let Some(value) = voice.filter(|value| !value.trim().is_empty()) {
        Some(value)
    } else if is_voice_clone {
        bail!("mimo-v2.5-tts-voiceclone requires --clone-voice <mp3|wav> or --voice <data-uri>");
    } else {
        Some(DEFAULT_VOICE.to_string())
    };
    let format = normalize_speech_format(&format).with_context(|| {
        format!("Unsupported speech format '{format}' (allowed: wav, mp3, pcm16)")
    })?;
    let output = output.unwrap_or_else(|| {
        output_dir
            .or_else(|| config.speech_output_dir())
            .unwrap_or_default()
            .join(default_speech_output_name(&format))
    });

    let client = DeepSeekClient::new(config)?;
    let response = client
        .synthesize_speech(SpeechSynthesisRequest {
            model: model.clone(),
            text,
            instruction,
            audio_format: format.clone(),
            voice,
        })
        .await?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory {}", parent.display()))?;
    }
    std::fs::write(&output, &response.audio_bytes)
        .with_context(|| format!("Failed to write audio file {}", output.display()))?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "speech",
                "success": true,
                "model": response.model,
                "format": response.audio_format,
                "output": output.display().to_string(),
                "bytes": response.audio_bytes.len(),
                "voice": response.voice.as_deref().map(describe_speech_voice),
                "transcript": response.transcript,
            }))?
        );
    } else {
        println!(
            "Generated speech: {} ({} bytes, model: {}, format: {})",
            output.display(),
            response.audio_bytes.len(),
            response.model,
            response.audio_format
        );
    }

    Ok(())
}

#[cfg(test)]
mod speech_cli_tests {
    use super::*;
    use crate::tools::speech::{
        default_speech_output_name, infer_speech_model, normalize_speech_format,
    };

    #[test]
    fn normalizes_documented_speech_formats() {
        assert_eq!(normalize_speech_format("WAV").as_deref(), Some("wav"));
        assert_eq!(normalize_speech_format("pcm16").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("pcm").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("flac"), None);
    }

    #[test]
    fn default_speech_output_tracks_requested_format() {
        assert_eq!(
            PathBuf::from(default_speech_output_name("mp3")),
            PathBuf::from("speech.mp3")
        );
        assert_eq!(
            PathBuf::from("audio").join(default_speech_output_name("pcm")),
            PathBuf::from("audio").join("speech.pcm16")
        );
    }

    #[test]
    fn speech_command_parses_cli_passthrough_smoke() {
        let cli = Cli::try_parse_from([
            "codewhale-tui",
            "speech",
            "hello",
            "--model",
            "tts",
            "--format",
            "pcm",
            "--output-dir",
            "audio",
            "--voice",
            "Mia",
        ])
        .expect("speech command parses");

        let Some(Commands::Speech(args)) = cli.command else {
            panic!("expected speech command");
        };
        assert_eq!(args.text, "hello");
        assert_eq!(
            infer_speech_model(args.model.as_deref(), false, false),
            "mimo-v2.5-tts"
        );
        assert_eq!(
            normalize_speech_format(&args.format).as_deref(),
            Some("pcm16")
        );
        assert_eq!(args.output_dir, Some(PathBuf::from("audio")));
        assert_eq!(args.voice.as_deref(), Some("Mia"));
    }
}

/// Test API connectivity by making a minimal request
async fn test_api_connectivity(config: &Config) -> Result<()> {
    use crate::client::DeepSeekClient;
    use crate::models::{ContentBlock, Message, MessageRequest};

    let client = DeepSeekClient::new(config)?;
    let model = client.model().to_string();

    // Minimal request: single word prompt, 1 max token
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: 1,
        system: None,
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: None,
        stream: Some(false),
        temperature: None,
        top_p: None,
    };

    // Use tokio timeout to catch hanging requests
    let timeout_duration = std::time::Duration::from_secs(15);
    match tokio::time::timeout(timeout_duration, client.create_message(request)).await {
        Ok(Ok(_response)) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => anyhow::bail!("Request timeout after 15 seconds"),
    }
}

fn rustc_version() -> String {
    let Some(mut cmd) = crate::dependencies::RustC::command() else {
        return "unknown".to_string();
    };
    let Ok(output) = cmd.arg("--version").output() else {
        return "unknown".to_string();
    };
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// List saved sessions
fn sessions_resume_command() -> &'static str {
    "codewhale resume"
}

fn list_sessions(limit: usize, search: Option<String>) -> Result<()> {
    use crate::palette;
    use colored::Colorize;
    use session_manager::{SessionManager, format_session_line};

    let (action_r, action_g, action_b) = palette::WHALE_ACTION_RGB;
    let (human_r, human_g, human_b) = palette::WHALE_HUMAN_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;

    let manager = SessionManager::default_location()?;

    let sessions = if let Some(query) = search {
        manager.search_sessions(&query)?
    } else {
        manager.list_sessions()?
    };

    if sessions.is_empty() {
        println!("{}", "No sessions found.".truecolor(sky_r, sky_g, sky_b));
        println!(
            "Start a new session with: {}",
            "codewhale".truecolor(human_r, human_g, human_b)
        );
        return Ok(());
    }

    println!(
        "{}",
        "Saved Sessions"
            .truecolor(action_r, action_g, action_b)
            .bold()
    );
    println!("{}", "==============".truecolor(sky_r, sky_g, sky_b));
    println!();

    for (i, session) in sessions.iter().take(limit).enumerate() {
        let line = format_session_line(session);
        if i == 0 {
            println!("  {} {}", "*".truecolor(aqua_r, aqua_g, aqua_b), line);
        } else {
            println!("    {line}");
        }
    }

    let total = sessions.len();
    if total > limit {
        println!();
        println!(
            "  {} more session(s). Use --limit to show more.",
            total - limit
        );
    }

    println!();
    println!(
        "Resume with: {} {}",
        sessions_resume_command().truecolor(action_r, action_g, action_b),
        "<session-id>".dimmed()
    );
    println!(
        "Continue latest in this workspace: {}",
        "codewhale --continue".truecolor(action_r, action_g, action_b)
    );

    Ok(())
}

/// Initialize a new project with AGENTS.md
fn init_project() -> Result<()> {
    use crate::palette;
    use colored::Colorize;
    use project_context::create_default_agents_md;

    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;
    let (red_r, red_g, red_b) = palette::WHALE_ERROR_RGB;

    let workspace = std::env::current_dir()?;
    let agents_path = workspace.join("AGENTS.md");

    if agents_path.exists() {
        println!(
            "{} AGENTS.md already exists at {}",
            "!".truecolor(sky_r, sky_g, sky_b),
            agents_path.display()
        );
        return Ok(());
    }

    match create_default_agents_md(&workspace) {
        Ok(path) => {
            println!(
                "{} Created {}",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                path.display()
            );
            println!();
            println!("Edit this file to customize how the AI agent works with your project.");
            println!("The instructions will be loaded automatically when you run codewhale.");
        }
        Err(e) => {
            println!(
                "{} Failed to create AGENTS.md: {}",
                "✗".truecolor(red_r, red_g, red_b),
                e
            );
        }
    }

    Ok(())
}

fn resolve_workspace(cli: &Cli) -> PathBuf {
    cli.workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn load_config_from_cli(cli: &Cli) -> Result<Config> {
    load_config_from_cli_with_effective_profile(cli).map(|(config, _)| config)
}

fn effective_config_profile(cli: &Cli) -> Option<String> {
    cli.profile
        .clone()
        .or_else(|| std::env::var("CODEWHALE_PROFILE").ok())
        .or_else(|| std::env::var("DEEPSEEK_PROFILE").ok())
}

fn load_config_from_cli_with_effective_profile(cli: &Cli) -> Result<(Config, Option<String>)> {
    let profile = effective_config_profile(cli);
    let mut config = Config::load(cli.config.clone(), profile.as_deref())?;
    cli.feature_toggles.apply(&mut config)?;
    Ok((config, profile))
}

fn read_api_key_from_stdin() -> Result<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        bail!("No API key provided. Pass --api-key or pipe one via stdin.");
    }
    let mut buffer = String::new();
    stdin.read_to_string(&mut buffer)?;
    let api_key = buffer.trim().to_string();
    if api_key.is_empty() {
        bail!("No API key provided via stdin.");
    }
    Ok(api_key)
}

fn run_login(api_key: Option<String>) -> Result<()> {
    let api_key = match api_key {
        Some(key) => key,
        None => read_api_key_from_stdin()?,
    };
    let saved = config::save_api_key(&api_key)?;
    println!("Saved API key to {}", saved.describe());
    Ok(())
}

fn run_logout() -> Result<()> {
    config::clear_api_key()?;
    println!("Cleared saved API key.");
    Ok(())
}

async fn run_xai_device_auth(config_path: Option<&Path>) -> Result<()> {
    let pending = xai_oauth::device_code_login().await?;
    let activation = xai_oauth::activate_device_login(pending, config_path, None)?;
    println!(
        "xAI OAuth is ready; activated {} via {}",
        codewhale_config::quote_os_path(&activation.auth_path),
        codewhale_config::quote_os_path(&activation.config_path)
    );
    Ok(())
}

fn resolve_session_id(session_id: Option<String>, last: bool, workspace: &Path) -> Result<String> {
    if last {
        return latest_session_id_for_workspace(workspace)?.ok_or_else(|| {
            anyhow!(
                "No saved sessions found for workspace {}. Use `codewhale sessions` to list all sessions, or `codewhale resume <SESSION_ID>` to resume one explicitly.",
                workspace.display()
            )
        });
    }
    if let Some(id) = session_id {
        return Ok(id);
    }
    pick_session_id()
}

fn latest_session_id_for_workspace(workspace: &Path) -> std::io::Result<Option<String>> {
    let manager = SessionManager::default_location()?;
    Ok(manager
        .get_latest_session_for_workspace(workspace)?
        .map(|session| session.id))
}

fn fork_session(
    config: &Config,
    session_id: Option<String>,
    last: bool,
    workspace: &Path,
) -> Result<String> {
    let manager = SessionManager::default_location()?;
    let saved = if last {
        let Some(meta) = manager.get_latest_session_for_workspace(workspace)? else {
            bail!(
                "No saved sessions found for workspace {}.",
                workspace.display()
            );
        };
        manager.load_session(&meta.id)?
    } else {
        let id = resolve_session_id(session_id, false, workspace)?;
        manager.load_session_by_prefix(&id)?
    };
    let saved_provider_identity = saved
        .metadata
        .model_provider_id
        .as_deref()
        .filter(|identity| !identity.trim().is_empty())
        .unwrap_or(&saved.metadata.model_provider);
    let provider_identity = config
        .resolve_persisted_provider_identity(
            Some(&saved.metadata.model_provider),
            saved.metadata.model_provider_id.as_deref(),
        )
        .map_err(anyhow::Error::msg)
        .with_context(|| {
            format!(
                "saved session provider '{}' is unavailable; fork will not fall back",
                saved_provider_identity
            )
        })?;

    let system_prompt = saved
        .system_prompt
        .as_ref()
        .map(|text| SystemPrompt::Text(text.clone()));
    let mut forked = create_saved_session(
        &saved.messages,
        &saved.metadata.model,
        &saved.metadata.workspace,
        saved.metadata.total_tokens,
        system_prompt.as_ref(),
    );
    forked.metadata.set_model_provider_route(
        provider_identity.provider.as_str(),
        provider_identity.persisted_id(),
    );
    forked.metadata.copy_cost_from(&saved.metadata);
    forked.metadata.mark_forked_from(&saved.metadata);
    manager.save_session(&forked)?;

    let source_title = saved.metadata.title.trim();
    let source_label = if source_title.is_empty() {
        "session".to_string()
    } else {
        format!("\"{source_title}\"")
    };
    println!(
        "Forked {source_label} ({source_id}) → new session {new_id}",
        source_id = truncate_id(&saved.metadata.id),
        new_id = truncate_id(&forked.metadata.id),
    );

    Ok(forked.metadata.id)
}

fn pick_session_id() -> Result<String> {
    let manager = SessionManager::default_location()?;
    let sessions = manager.list_sessions()?;
    if sessions.is_empty() {
        bail!("No saved sessions found.");
    }

    println!("Select a session to resume:");
    for (idx, session) in sessions.iter().enumerate() {
        println!("  {:>2}. {} ({})", idx + 1, session.title, session.id);
    }
    print!("Enter a number (or press Enter to cancel): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        bail!("No session selected.");
    }
    let idx: usize = input
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid input"))?;
    let session = sessions
        .get(idx.saturating_sub(1))
        .ok_or_else(|| anyhow::anyhow!("Selection out of range"))?;
    Ok(session.id.clone())
}

async fn run_review(config: &Config, args: ReviewArgs) -> Result<()> {
    use crate::client::DeepSeekClient;

    let diff = collect_diff(&args)?;
    if diff.trim().is_empty() {
        bail!("No diff to review.");
    }
    validate_review_receipt_args(&args)?;
    if args.check_receipt {
        return run_review_receipt_check(&diff, &args);
    }

    let model = resolve_review_model(config, args.model.as_deref());
    let route = resolve_cli_exec_route(config, &model, &diff, args.model.is_none()).await?;
    let execution_config = config_for_cli_route(config, &route);
    let route_provider = execution_config.provider_identity_for(route.provider);
    let model = route.model.clone();
    let reasoning_effort = route
        .reasoning_effort
        .and_then(|effort| cli_reasoning_effort_value(&execution_config, &model, effort));

    let system = SystemPrompt::Text(
        "You are a senior code reviewer. Focus on bugs, risks, behavioral regressions, and missing tests. \
Provide findings ordered by severity with file references, then open questions, then a brief summary."
            .to_string(),
    );
    let user_prompt =
        format!("Review the following diff and provide feedback:\n\n{diff}\n\nEnd of diff.");

    let client = DeepSeekClient::new(&execution_config)?;
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: user_prompt,
                cache_control: None,
            }],
        }],
        max_tokens: 4096,
        system: Some(system),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(false),
        temperature: Some(0.2),
        top_p: Some(0.9),
    };

    let response = client.create_message(request).await?;
    let mut output = String::new();
    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            output.push_str(&text);
        }
    }
    let receipt = if args.write_receipt {
        let parsed_output = crate::tools::review::ReviewOutput::from_str(&output);
        let receipt = crate::tools::review::build_review_receipt(
            review_target_label(&args),
            &diff,
            &route_provider,
            &model,
            &parsed_output,
            &output,
            Vec::new(),
        );
        let path =
            crate::tools::review::write_review_receipt(&receipt, args.receipt_path.as_deref())?;
        Some((path, receipt))
    } else {
        None
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "review",
                "provider": route_provider,
                "model": model,
                "success": true,
                "content": output,
                "receipt_path": receipt
                    .as_ref()
                    .map(|(path, _)| path.display().to_string()),
                "receipt": receipt.as_ref().map(|(_, receipt)| receipt),
            }))?
        );
    } else {
        println!("{output}");
        if let Some((path, _)) = receipt {
            eprintln!("Review receipt written: {}", path.display());
        }
    }
    Ok(())
}

fn resolve_review_model(config: &Config, explicit_model: Option<&str>) -> String {
    explicit_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| config.default_model())
}

fn validate_review_receipt_args(args: &ReviewArgs) -> Result<()> {
    if args.receipt_path.is_some() && !args.write_receipt && !args.check_receipt {
        bail!("--receipt-path requires --write-receipt or --check-receipt");
    }
    if args.write_receipt && args.check_receipt {
        bail!("--write-receipt and --check-receipt are mutually exclusive");
    }
    Ok(())
}

fn run_review_receipt_check(diff: &str, args: &ReviewArgs) -> Result<()> {
    let (path, receipt) = if let Some(path) = args.receipt_path.as_ref() {
        (
            path.clone(),
            crate::tools::review::read_review_receipt(path)
                .with_context(|| format!("failed to read review receipt {}", path.display()))?,
        )
    } else {
        crate::tools::review::latest_review_receipt_for_diff(diff)?.ok_or_else(|| {
            anyhow!(
                "No review receipt found for the current diff. Run `codewhale review --write-receipt` first, or pass --receipt-path."
            )
        })?
    };
    let validation =
        crate::tools::review::validate_review_receipt_for_diff(diff, &receipt, Some(path.clone()));

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "review_receipt_check",
                "success": validation.passed,
                "validation": review_receipt_validation_public_json(&validation),
            }))?
        );
    } else if validation.passed {
        println!("Review receipt valid: {}", path.display());
    }

    if !validation.passed {
        bail!("Review receipt check failed: {}", validation.reason);
    }
    Ok(())
}

fn review_receipt_validation_public_json(
    validation: &crate::tools::review::ReviewReceiptValidation,
) -> serde_json::Value {
    let unresolved_risk = validation.unresolved_risk.as_ref();
    serde_json::json!({
        "passed": validation.passed,
        "status": review_receipt_validation_status(validation),
        "diff_fingerprint": validation.diff_fingerprint.as_str(),
        "receipt_fingerprint": validation.receipt_fingerprint.as_deref(),
        "unresolved": unresolved_risk.is_some_and(|risk| risk.unresolved),
        "risk_level": unresolved_risk.map(|risk| risk.level.as_str()),
    })
}

fn review_receipt_validation_status(
    validation: &crate::tools::review::ReviewReceiptValidation,
) -> &'static str {
    if validation.passed {
        "valid"
    } else if validation
        .receipt_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint != validation.diff_fingerprint.as_str())
    {
        "diff_mismatch"
    } else if validation
        .unresolved_risk
        .as_ref()
        .is_some_and(|risk| risk.unresolved)
    {
        "unresolved_risk"
    } else if validation
        .reason
        .starts_with("unsupported review receipt schema version")
    {
        "unsupported_schema"
    } else if validation.reason.starts_with("review receipt check ") {
        "check_failed"
    } else {
        "invalid"
    }
}

/// `codewhale pr <N>` (#451) — fetch a GitHub PR via `gh`, format
/// title + body + diff as the composer's first message, and launch
/// the interactive TUI. Falls back gracefully if `gh` is missing.
async fn run_pr(
    cli: &Cli,
    config: &Config,
    number: u32,
    repo: Option<&str>,
    checkout: bool,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    if !is_command_available("gh") {
        bail!(
            "`gh` CLI not found on PATH. Install GitHub CLI \
             (https://cli.github.com) and authenticate (`gh auth login`) \
             so `codewhale pr <N>` can fetch PR metadata and the diff."
        );
    }

    let view = run_gh_pr_view(number, repo)?;
    let diff = run_gh_pr_diff(number, repo)?;

    if checkout {
        match run_gh_pr_checkout(number, repo) {
            Ok(()) => eprintln!("Checked out PR #{number} into the current workspace."),
            Err(err) => eprintln!(
                "warning: gh pr checkout #{number} failed ({err}). Continuing without checkout."
            ),
        }
    }

    let prompt = format_pr_prompt(number, &view, &diff);
    let resume_session_id = if cli.continue_session {
        let workspace = resolve_workspace(cli);
        latest_session_id_for_workspace(&workspace).ok().flatten()
    } else {
        cli.resume.clone()
    };
    run_interactive(
        cli,
        config,
        resume_session_id,
        Some(tui::InitialInput::Prefill(prompt)),
        plugin_registry,
    )
    .await
}

/// Return true if `name` resolves to an executable on the current `PATH`.
///
/// Walks `$PATH` directly instead of probing with `--version`. The
/// previous implementation invoked `Command::new(name).arg("--version")`,
/// which fails on the Ubuntu CI runner because `/bin/sh` is `dash` —
/// `dash --version` exits with status 2 ("invalid option") even though
/// `sh` is plainly on PATH. macOS happens to ship bash as `sh`, which
/// does honor `--version`, so the bug was invisible locally and only
/// surfaced in CI logs.
///
/// Windows: also checks the `.exe` extension when `name` doesn't have
/// one, matching the platform's PATHEXT lookup behavior for the common
/// case.
fn is_command_available(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            // PATHEXT gives `.exe`/`.cmd`/`.bat` etc. priority — we only
            // probe `.exe` because that's the case that actually trips
            // up the negative case (`gh` resolves as `gh.exe`).
            if candidate.extension().is_none() && candidate.with_extension("exe").is_file() {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, Default)]
struct GhPullRequest {
    title: String,
    body: String,
    base: String,
    head: String,
    url: String,
}

fn run_gh_pr_view(number: u32, repo: Option<&str>) -> Result<GhPullRequest> {
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("pr").arg("view").arg(number.to_string());
    if let Some(r) = repo {
        cmd.arg("--repo").arg(r);
    }
    cmd.arg("--json")
        .arg("title,body,baseRefName,headRefName,url");
    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gh pr view`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh pr view #{number} failed: {stderr}");
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("gh pr view returned non-JSON output: {e}"))?;
    let pick = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(GhPullRequest {
        title: pick("title"),
        body: pick("body"),
        base: pick("baseRefName"),
        head: pick("headRefName"),
        url: pick("url"),
    })
}

fn run_gh_pr_diff(number: u32, repo: Option<&str>) -> Result<String> {
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("pr").arg("diff").arg(number.to_string());
    if let Some(r) = repo {
        cmd.arg("--repo").arg(r);
    }
    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gh pr diff`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh pr diff #{number} failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_gh_pr_checkout(number: u32, repo: Option<&str>) -> Result<()> {
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("pr").arg("checkout").arg(number.to_string());
    if let Some(r) = repo {
        cmd.arg("--repo").arg(r);
    }
    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gh pr checkout`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh pr checkout #{number} failed: {stderr}");
    }
    Ok(())
}

/// Format the PR review prompt that lands in the composer. Caps the
/// diff at 200 KiB so a massive PR doesn't blow the model's context
/// window before the user even hits Enter — they can always ask the
/// model to fetch more via `gh pr diff #N` from inside the session.
fn format_pr_prompt(number: u32, view: &GhPullRequest, diff: &str) -> String {
    const MAX_DIFF_BYTES: usize = 200 * 1024;
    let diff_section = if diff.len() > MAX_DIFF_BYTES {
        let cut = (0..=MAX_DIFF_BYTES)
            .rev()
            .find(|&i| diff.is_char_boundary(i))
            .unwrap_or(0);
        format!(
            "{}\n\n[…diff truncated at {} KiB; ask me to fetch more if needed]\n",
            &diff[..cut],
            MAX_DIFF_BYTES / 1024
        )
    } else {
        diff.to_string()
    };
    let body = if view.body.trim().is_empty() {
        "(no description)".to_string()
    } else {
        view.body.trim().to_string()
    };
    let title = if view.title.trim().is_empty() {
        format!("(PR #{number})")
    } else {
        view.title.trim().to_string()
    };
    let branches = match (view.base.is_empty(), view.head.is_empty()) {
        (false, false) => format!("{} ← {}", view.base, view.head),
        (false, true) => view.base.clone(),
        (true, false) => view.head.clone(),
        _ => "(unknown)".to_string(),
    };
    format!(
        "Review PR #{number} — {title}\n\
         \n\
         URL: {url}\n\
         Branches: {branches}\n\
         \n\
         ## Description\n\
         \n\
         {body}\n\
         \n\
         ## Diff\n\
         \n\
         ```diff\n\
         {diff_section}\n\
         ```\n",
        url = if view.url.is_empty() {
            "(unavailable)"
        } else {
            view.url.as_str()
        },
    )
}

fn collect_diff(args: &ReviewArgs) -> Result<String> {
    let mut cmd = crate::dependencies::Git::command()
        .ok_or_else(|| anyhow::anyhow!("git not found on PATH"))?;
    cmd.arg("diff");
    if args.staged {
        cmd.arg("--cached");
    }
    if let Some(base) = &args.base {
        cmd.arg(format!("{base}...HEAD"));
    }
    if let Some(path) = &args.path {
        cmd.arg("--").arg(path);
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git diff. Is git installed? ({e})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr.trim());
    }
    let mut diff = String::from_utf8_lossy(&output.stdout).to_string();
    if diff.len() > args.max_chars {
        diff = crate::utils::truncate_with_ellipsis(&diff, args.max_chars, "\n...[truncated]\n");
    }
    Ok(diff)
}

fn review_target_label(args: &ReviewArgs) -> String {
    let mut label = if args.staged {
        "staged".to_string()
    } else if let Some(base) = args
        .base
        .as_deref()
        .map(str::trim)
        .filter(|base| !base.is_empty())
    {
        format!("base:{base}")
    } else {
        "working-tree".to_string()
    };
    if let Some(path) = &args.path {
        label.push(' ');
        label.push_str(path.to_string_lossy().as_ref());
    }
    label
}

fn run_apply(args: ApplyArgs) -> Result<()> {
    let patch = if let Some(path) = args.patch_file {
        std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read patch {}: {}", path.display(), e))?
    } else {
        read_patch_from_stdin()?
    };
    if patch.trim().is_empty() {
        bail!("Patch is empty.");
    }

    let mut tmp = NamedTempFile::new()?;
    tmp.write_all(patch.as_bytes())?;
    let tmp_path = tmp.path().to_path_buf();

    let output = crate::dependencies::Git::command()
        .ok_or_else(|| anyhow::anyhow!("git not found on PATH"))?
        .arg("apply")
        .arg("--whitespace=nowarn")
        .arg(&tmp_path)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git apply: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git apply failed: {}", stderr.trim());
    }
    println!("Applied patch successfully.");
    Ok(())
}

fn read_patch_from_stdin() -> Result<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        bail!("No patch file provided and stdin is empty.");
    }
    let mut buffer = String::new();
    stdin.read_to_string(&mut buffer)?;
    Ok(buffer)
}

async fn run_mcp_command(
    config: &Config,
    workspace: &Path,
    command: McpCommand,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    let config_path = config.mcp_config_path();
    match command {
        McpCommand::Init { force } => {
            let status = init_mcp_config(&config_path, force)?;
            match status {
                WriteStatus::Created => {
                    println!("Created MCP config at {}", config_path.display());
                }
                WriteStatus::Overwritten => {
                    println!("Overwrote MCP config at {}", config_path.display());
                }
                WriteStatus::SkippedExists => {
                    println!(
                        "MCP config already exists at {} (use --force to overwrite)",
                        config_path.display()
                    );
                }
            }
            println!("Edit the file, then run `codewhale mcp list` or `codewhale mcp tools`.");
            Ok(())
        }
        McpCommand::List => {
            let cfg = crate::mcp::load_config_with_workspace_and_plugins(
                &config_path,
                workspace,
                plugins,
            )?;
            if cfg.servers.is_empty() {
                println!(
                    "No MCP servers configured in {} or {}",
                    config_path.display(),
                    crate::mcp::workspace_mcp_config_path(workspace).display()
                );
                return Ok(());
            }
            println!("MCP servers ({}):", cfg.servers.len());
            for (name, server) in cfg.servers {
                let status = if server.enabled && !server.disabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let auth_status = crate::mcp::oauth::auth_status_for_server(&name, &server).await;
                let auth = if auth_status == crate::mcp::oauth::McpAuthStatus::Unsupported {
                    String::new()
                } else {
                    format!(
                        " auth={}",
                        auth_status
                            .to_string()
                            .to_ascii_lowercase()
                            .replace(' ', "-")
                    )
                };
                let args = if server.args.is_empty() {
                    "".to_string()
                } else {
                    format!(" {}", server.args.join(" "))
                };
                let cmd_str = if let Some(cmd) = server.command {
                    format!("{cmd}{args}")
                } else if let Some(url) = server.url {
                    url
                } else {
                    "unknown".to_string()
                };
                let required = if server.required { " required" } else { "" };
                println!("  - {name} [{status}{required}{auth}] {cmd_str}");
            }
            Ok(())
        }
        McpCommand::Connect { server } => {
            let mut pool = McpPool::from_config_path_with_workspace_and_plugins(
                &config_path,
                workspace,
                std::sync::Arc::new(plugins.clone()),
            )?;
            if let Some(name) = server {
                if let Err(err) = pool.get_or_connect(&name).await {
                    if crate::mcp::oauth::error_looks_auth_required(&err) {
                        let hint = crate::mcp::oauth::auth_required_login_hint(&name);
                        return Err(err).context(hint);
                    }
                    return Err(err);
                }
                println!("Connected to MCP server: {name}");
            } else {
                let errors = pool.connect_all().await;
                if errors.is_empty() {
                    println!("Connected to all configured MCP servers.");
                } else {
                    for (name, err) in errors {
                        eprintln!("Failed to connect {name}: {err:#}");
                        if crate::mcp::oauth::error_looks_auth_required(&err) {
                            eprintln!("  {}", crate::mcp::oauth::auth_required_login_hint(&name));
                        }
                    }
                }
            }
            Ok(())
        }
        McpCommand::Tools { server } => {
            let mut pool = McpPool::from_config_path_with_workspace_and_plugins(
                &config_path,
                workspace,
                std::sync::Arc::new(plugins.clone()),
            )?;
            if let Some(name) = server {
                let conn = match pool.get_or_connect(&name).await {
                    Ok(conn) => conn,
                    Err(err) => {
                        if crate::mcp::oauth::error_looks_auth_required(&err) {
                            let hint = crate::mcp::oauth::auth_required_login_hint(&name);
                            return Err(err).context(hint);
                        }
                        return Err(err);
                    }
                };
                if conn.tools().is_empty() {
                    println!("No tools found for MCP server: {name}");
                } else {
                    println!("Tools for {name}:");
                    for tool in conn.tools() {
                        println!(
                            "  - {}{}",
                            tool.name,
                            crate::mcp::format_mcp_tool_description(tool.description.as_deref())
                        );
                    }
                }
            } else {
                let errors = pool.connect_all().await;
                for (name, err) in errors {
                    eprintln!("Failed to connect {name}: {err:#}");
                    if crate::mcp::oauth::error_looks_auth_required(&err) {
                        eprintln!("  {}", crate::mcp::oauth::auth_required_login_hint(&name));
                    }
                }
                let tools = pool.all_tools();
                if tools.is_empty() {
                    println!("No MCP tools discovered.");
                } else {
                    println!("MCP tools:");
                    for (name, tool) in tools {
                        println!(
                            "  - {}{}",
                            name,
                            crate::mcp::format_mcp_tool_description(tool.description.as_deref())
                        );
                    }
                }
            }
            Ok(())
        }
        McpCommand::Add {
            name,
            command,
            url,
            transport,
            bearer_token_env_var,
            oauth_client_id,
            oauth_resource,
            scopes,
            args,
        } => {
            if command.is_none() && url.is_none() {
                bail!("Provide either --command or --url for `mcp add`.");
            }
            if let Some(transport) = transport.as_deref()
                && !transport.trim().eq_ignore_ascii_case("sse")
            {
                bail!("Unsupported MCP transport '{transport}'. Supported values: sse");
            }
            let added_server = McpServerConfig {
                command,
                args,
                env: std::collections::HashMap::new(),
                cwd: None,
                url,
                transport,
                connect_timeout: None,
                execute_timeout: None,
                read_timeout: None,
                disabled: false,
                enabled: true,
                required: false,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                headers: std::collections::HashMap::new(),
                env_headers: std::collections::HashMap::new(),
                bearer_token_env_var,
                scopes,
                oauth: oauth_client_id.map(|client_id| McpServerOAuthConfig {
                    client_id: Some(client_id),
                }),
                oauth_resource,
                reviewed_plugin: None,
            };
            let can_suggest_oauth = added_server.url.is_some()
                && added_server.bearer_token_env_var.is_none()
                && added_server
                    .headers
                    .keys()
                    .all(|key| !key.trim().eq_ignore_ascii_case("authorization"))
                && added_server
                    .env_headers
                    .keys()
                    .all(|key| !key.trim().eq_ignore_ascii_case("authorization"));
            let mut cfg = load_mcp_config(&config_path)?;
            cfg.servers.insert(name.clone(), added_server.clone());
            save_mcp_config(&config_path, &cfg)?;
            println!("Added MCP server '{name}' in {}", config_path.display());
            if can_suggest_oauth
                && crate::mcp::oauth::oauth_login_support(&added_server)
                    .await
                    .is_ok_and(|support| support.is_some())
            {
                println!(
                    "OAuth is available for '{name}'. Run `codewhale mcp login {name}` to authenticate."
                );
            }
            Ok(())
        }
        McpCommand::Login { name, scopes } => {
            let cfg = crate::mcp::load_config_with_workspace_and_plugins(
                &config_path,
                workspace,
                plugins,
            )?;
            let server = cfg
                .servers
                .get(&name)
                .ok_or_else(|| anyhow!("MCP server '{name}' not found"))?;
            let explicit_scopes = (!scopes.is_empty()).then_some(scopes);
            crate::mcp::oauth::perform_oauth_login_for_server(
                &name,
                server,
                explicit_scopes,
                config.mcp_oauth_callback_port,
                config.mcp_oauth_callback_url.as_deref(),
            )
            .await?;
            println!("Stored OAuth credentials for MCP server '{name}'.");
            Ok(())
        }
        McpCommand::Logout { name } => {
            let cfg = crate::mcp::load_config_with_workspace_and_plugins(
                &config_path,
                workspace,
                plugins,
            )?;
            let server = cfg
                .servers
                .get(&name)
                .ok_or_else(|| anyhow!("MCP server '{name}' not found"))?;
            if crate::mcp::oauth::delete_oauth_tokens_for_server(&name, server)? {
                println!("Deleted stored OAuth credentials for MCP server '{name}'.");
            } else {
                println!("No stored OAuth credentials found for MCP server '{name}'.");
            }
            Ok(())
        }
        McpCommand::Remove { name } => {
            let mut cfg = load_mcp_config(&config_path)?;
            if cfg.servers.remove(&name).is_none() {
                bail!("MCP server '{name}' not found");
            }
            save_mcp_config(&config_path, &cfg)?;
            println!("Removed MCP server '{name}'");
            Ok(())
        }
        McpCommand::Enable { name } => {
            let mut cfg = load_mcp_config(&config_path)?;
            let server = cfg
                .servers
                .get_mut(&name)
                .ok_or_else(|| anyhow!("MCP server '{name}' not found"))?;
            server.enabled = true;
            server.disabled = false;
            save_mcp_config(&config_path, &cfg)?;
            println!("Enabled MCP server '{name}'");
            Ok(())
        }
        McpCommand::Disable { name } => {
            let mut cfg = load_mcp_config(&config_path)?;
            let server = cfg
                .servers
                .get_mut(&name)
                .ok_or_else(|| anyhow!("MCP server '{name}' not found"))?;
            server.enabled = false;
            server.disabled = true;
            save_mcp_config(&config_path, &cfg)?;
            println!("Disabled MCP server '{name}'");
            Ok(())
        }
        McpCommand::Validate => {
            let mut pool = McpPool::from_config_path_with_workspace_and_plugins(
                &config_path,
                workspace,
                std::sync::Arc::new(plugins.clone()),
            )?;
            let errors = pool.connect_all().await;
            if errors.is_empty() {
                println!("MCP config is valid. All enabled servers connected.");
                return Ok(());
            }
            eprintln!("MCP validation failed:");
            for (name, err) in errors {
                eprintln!("  - {name}: {err:#}");
            }
            bail!("one or more MCP servers failed validation");
        }
        McpCommand::AddSelf { name, workspace } => {
            let exe_path = std::env::current_exe()
                .map_err(|e| anyhow!("Cannot resolve current binary path: {e}"))?;
            let exe_str = exe_path.to_string_lossy().to_string();

            let mut args = vec!["serve".to_string(), "--mcp".to_string()];
            if let Some(ref ws) = workspace {
                args.push("--workspace".to_string());
                args.push(ws.clone());
            }

            let mut cfg = load_mcp_config(&config_path)?;
            if cfg.servers.contains_key(&name) {
                bail!(
                    "MCP server '{name}' already exists in {}. Use `codewhale mcp remove {name}` first, or choose a different --name.",
                    config_path.display()
                );
            }
            cfg.servers.insert(
                name.clone(),
                McpServerConfig {
                    command: Some(exe_str.clone()),
                    args,
                    env: std::collections::HashMap::new(),
                    cwd: None,
                    url: None,
                    transport: None,
                    connect_timeout: None,
                    execute_timeout: None,
                    read_timeout: None,
                    disabled: false,
                    enabled: true,
                    required: false,
                    enabled_tools: Vec::new(),
                    disabled_tools: Vec::new(),
                    headers: std::collections::HashMap::new(),
                    env_headers: std::collections::HashMap::new(),
                    bearer_token_env_var: None,
                    scopes: Vec::new(),
                    oauth: None,
                    oauth_resource: None,
                    reviewed_plugin: None,
                },
            );
            save_mcp_config(&config_path, &cfg)?;
            println!(
                "Registered Codewhale as MCP server '{name}' in {}",
                config_path.display()
            );
            println!("  command: {exe_str}");
            println!(
                "  args:    serve --mcp{}",
                workspace.map_or(String::new(), |ws| format!(" --workspace {ws}"))
            );
            println!();
            println!("Tip: Use `codewhale mcp validate` to test the connection.");
            println!("     Use `codewhale serve --http` for the HTTP/SSE runtime API instead.");
            Ok(())
        }
    }
}

fn load_mcp_config(path: &Path) -> Result<McpConfig> {
    if !path.exists() {
        return Ok(McpConfig::default());
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read MCP config {}: {}", path.display(), e))?;
    let cfg: McpConfig = serde_json::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse MCP config {}; file contents were omitted",
            codewhale_config::quote_os_path(path)
        )
    })?;
    Ok(cfg)
}

/// Diagnostic status for an MCP server entry.
#[derive(Debug)]
enum McpServerDoctorStatus {
    Ok(String),
    Warning(String),
    Error(String),
}

impl McpServerDoctorStatus {
    fn legacy_status(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok",
            Self::Warning(_) => "warning",
            Self::Error(_) => "error",
        }
    }

    fn configuration_status(&self) -> &'static str {
        match self {
            Self::Ok(_) => "valid",
            Self::Warning(_) => "warning",
            Self::Error(_) => "invalid",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Ok(detail) | Self::Warning(detail) | Self::Error(detail) => detail,
        }
    }
}

/// Inspect command availability without starting the configured MCP server.
fn doctor_mcp_command_status(server: &McpServerConfig) -> Result<McpCommandAvailability> {
    static_mcp_command_availability(server)
}

fn doctor_mcp_server_json(name: &str, server: &McpServerConfig) -> serde_json::Value {
    use serde_json::json;

    let status = doctor_check_mcp_server(server);
    json!({
        "name": name,
        "enabled": server.enabled && !server.disabled,
        // Compatibility field retained for existing doctor JSON consumers.
        // Its scope is now explicit in `checks.configuration` below.
        "status": status.legacy_status(),
        "detail": status.detail(),
        "check_scope": "configuration",
        "checks": {
            "configuration": {
                "status": status.configuration_status(),
                "detail": status.detail(),
            },
            "command": {
                "status": doctor_mcp_command_status(server)
                    .map(McpCommandAvailability::as_str)
                    .unwrap_or("invalid_environment"),
            },
            "process_reachable": {
                "status": "not_checked",
            },
            "protocol_initialized": {
                "status": "not_checked",
            },
            "backend_tool_health": {
                "status": "not_checked",
            },
        },
    })
}

/// Check an MCP server config entry for common issues.
fn doctor_check_mcp_server(server: &McpServerConfig) -> McpServerDoctorStatus {
    // No command or URL — incomplete entry.
    if server.command.is_none() && server.url.is_none() {
        return McpServerDoctorStatus::Error("no command or url configured".to_string());
    }

    // URL-based server — just report the URL.
    if let Some(ref url) = server.url {
        return McpServerDoctorStatus::Ok(format!("HTTP/SSE server at {url}"));
    }

    // Command-based: validate command path exists.
    let cmd = server.command.as_deref().unwrap_or("");
    if cmd.is_empty() {
        return McpServerDoctorStatus::Error("empty command".to_string());
    }

    let command_availability = match doctor_mcp_command_status(server) {
        Ok(McpCommandAvailability::Missing) => {
            return McpServerDoctorStatus::Error(format!("command not found: {cmd}"));
        }
        Err(error) => {
            return McpServerDoctorStatus::Error(format!(
                "invalid MCP stdio environment: {error:#}"
            ));
        }
        Ok(status) => status,
    };

    let cmd_path = Path::new(cmd);
    // Also accept Unix-style `/` prefix on Windows, where Path::is_absolute()
    // requires a drive letter.
    let is_absolute = cmd_path.is_absolute() || cmd.starts_with('/');

    if server.cwd.is_none() {
        if is_relative_stdio_path_arg(cmd) {
            return McpServerDoctorStatus::Warning(format!(
                "stdio server uses relative command \"{cmd}\" without cwd; set cwd so headless exec and UI status checks resolve the same path"
            ));
        }
        if let Some(arg) = server
            .args
            .iter()
            .find(|arg| is_relative_stdio_path_arg(arg))
        {
            return McpServerDoctorStatus::Warning(format!(
                "stdio server uses relative path argument \"{arg}\" without cwd; set cwd so headless exec and UI status checks resolve the same path"
            ));
        }
    }

    if command_availability == McpCommandAvailability::NotChecked {
        return McpServerDoctorStatus::Warning(format!(
            "stdio command availability could not be confirmed without starting \"{cmd}\""
        ));
    }

    // Detect self-hosted DeepSeek server entries.
    let is_self_hosted = server
        .args
        .windows(2)
        .any(|w| w[0] == "serve" && w[1] == "--mcp");

    let args_str = server.args.join(" ");
    if is_self_hosted {
        if is_absolute {
            McpServerDoctorStatus::Ok(format!("self-hosted MCP server ({cmd} {args_str})"))
        } else {
            McpServerDoctorStatus::Warning(format!(
                "self-hosted MCP server uses relative command \"{cmd}\" — consider using an absolute path"
            ))
        }
    } else {
        McpServerDoctorStatus::Ok(format!(
            "stdio server ({cmd}{})",
            if args_str.is_empty() {
                String::new()
            } else {
                format!(" {args_str}")
            }
        ))
    }
}

fn save_mcp_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let rendered = serde_json::to_string_pretty(cfg)
        .map_err(|e| anyhow!("Failed to serialize MCP config: {e}"))?;
    crate::utils::write_atomic(path, rendered.as_bytes())
        .map_err(|e| anyhow!("Failed to write MCP config {}: {}", path.display(), e))?;
    Ok(())
}

fn run_sandbox_command(args: SandboxArgs) -> Result<()> {
    use crate::sandbox::{CommandSpec, SandboxManager};

    let SandboxCommand::Run {
        policy,
        network,
        writable_root,
        exclude_tmpdir,
        exclude_slash_tmp,
        cwd,
        timeout_ms,
        command,
    } = args.command;

    let policy = parse_sandbox_policy(
        &policy,
        network,
        writable_root,
        exclude_tmpdir,
        exclude_slash_tmp,
    )?;
    let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let timeout = Duration::from_millis(timeout_ms.clamp(1000, 600_000));

    let (program, args) = command
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("Command is required"))?;
    let spec =
        CommandSpec::program(program, args.to_vec(), cwd.clone(), timeout).with_policy(policy);
    let manager = SandboxManager::new();
    let exec_env = manager.prepare(&spec);

    let mut cmd = Command::new(exec_env.program());
    cmd.args(exec_env.args())
        .current_dir(&exec_env.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run command: {e}"))?;
    let stdout_handle = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("stdout unavailable"))?;
    let stderr_handle = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("stderr unavailable"))?;

    let timeout = exec_env.timeout;
    let stdout_thread = std::thread::spawn(move || {
        let mut reader = stdout_handle;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut reader = stderr_handle;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    if let Some(status) = child.wait_timeout(timeout)? {
        let stdout = stdout_thread.join().unwrap_or_default();
        let stderr = stderr_thread.join().unwrap_or_default();
        let stderr_str = String::from_utf8_lossy(&stderr);
        let exit_code = status.code().unwrap_or(-1);
        let sandbox_type = exec_env.sandbox_type;
        let sandbox_denied = SandboxManager::was_denied(sandbox_type, exit_code, &stderr_str);

        if !stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&stdout));
        }
        if !stderr.is_empty() {
            eprint!("{stderr_str}");
        }
        if sandbox_denied {
            eprintln!(
                "{}",
                SandboxManager::denial_message(sandbox_type, &stderr_str)
            );
        }

        if !status.success() {
            bail!("Command failed with exit code {exit_code}");
        }
    } else {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Command timed out after {}ms", timeout.as_millis());
    }
    Ok(())
}

fn parse_sandbox_policy(
    policy: &str,
    network: bool,
    writable_root: Vec<PathBuf>,
    exclude_tmpdir: bool,
    exclude_slash_tmp: bool,
) -> Result<crate::sandbox::SandboxPolicy> {
    use crate::sandbox::SandboxPolicy;

    match policy {
        "danger-full-access" => Ok(SandboxPolicy::DangerFullAccess),
        "read-only" => Ok(SandboxPolicy::ReadOnly),
        "external-sandbox" => Ok(SandboxPolicy::ExternalSandbox {
            network_access: network,
        }),
        "workspace-write" => Ok(SandboxPolicy::WorkspaceWrite {
            writable_roots: writable_root,
            network_access: network,
            exclude_tmpdir,
            exclude_slash_tmp,
        }),
        other => bail!("Unknown sandbox policy: {other}"),
    }
}

fn should_use_alt_screen(_cli: &Cli, _config: &Config) -> bool {
    true
}

fn should_use_mouse_capture(cli: &Cli, config: &Config, use_alt_screen: bool) -> bool {
    let terminal_emulator = std::env::var("TERMINAL_EMULATOR").ok();
    let wt_session = std::env::var("WT_SESSION").ok().filter(|s| !s.is_empty());
    let conemu_pid = std::env::var("ConEmuPID").ok().filter(|s| !s.is_empty());
    should_use_mouse_capture_with(
        cli,
        config,
        use_alt_screen,
        terminal_emulator.as_deref(),
        wt_session.as_deref(),
        conemu_pid.as_deref(),
    )
}

fn should_use_mouse_capture_with(
    cli: &Cli,
    config: &Config,
    use_alt_screen: bool,
    terminal_emulator: Option<&str>,
    wt_session: Option<&str>,
    conemu_pid: Option<&str>,
) -> bool {
    if !use_alt_screen || cli.no_mouse_capture {
        return false;
    }
    if cli.mouse_capture {
        return true;
    }
    config
        .tui
        .as_ref()
        .and_then(|tui| tui.mouse_capture)
        .unwrap_or_else(|| default_mouse_capture_enabled(terminal_emulator, wt_session, conemu_pid))
}

/// Whether to enable terminal mouse capture by default for this platform/host.
///
/// On Windows the default depends on the host: Windows Terminal (which sets
/// `WT_SESSION`) and ConEmu/Cmder (which set `ConEmuPID`) handle mouse-mode
/// reporting cleanly, so default-on there gives users in-app text selection
/// and keeps the application's selection clamped to the transcript area
/// (#1169). Legacy conhost (CMD without either env var) stays default-off
/// because its mouse-mode reporting can leak SGR escape sequences as raw
/// text into the composer (#878 / #898).
///
/// Off elsewhere only for JetBrains' JediTerm, which advertises mouse
/// support but forwards the same SGR escape sequences as raw input. The
/// user can still opt back in with `[tui] mouse_capture = true` in
/// `~/.codewhale/config.toml` or `--mouse-capture`.
fn default_mouse_capture_enabled(
    terminal_emulator: Option<&str>,
    wt_session: Option<&str>,
    conemu_pid: Option<&str>,
) -> bool {
    if cfg!(windows) {
        return wt_session.is_some() || conemu_pid.is_some();
    }
    if matches!(terminal_emulator, Some(t) if t.eq_ignore_ascii_case("JetBrains-JediTerm")) {
        return false;
    }
    true
}

/// A loadable crash-recovery checkpoint candidate: session content, file
/// age, and which slot it came from (per-session file or the legacy single
/// slot).
struct RecentCheckpoint {
    session: session_manager::SavedSession,
    age: std::time::Duration,
    source: session_manager::CheckpointSource,
}

const CHECKPOINT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Load all recent crash-recovery checkpoints, pruning stale ones first.
///
/// Candidates are the per-session checkpoint files plus the legacy
/// single-slot `checkpoints/latest.json` (compatibility read). Files older
/// than 24 hours are removed; unreadable files are skipped. The result is
/// sorted most recent first.
fn load_recent_checkpoints(manager: &session_manager::SessionManager) -> Vec<RecentCheckpoint> {
    let refs = manager.list_checkpoints().unwrap_or_default();
    let mut recent = Vec::new();
    for checkpoint_ref in refs {
        let Ok(age) = std::time::SystemTime::now().duration_since(checkpoint_ref.modified) else {
            continue;
        };
        if age > CHECKPOINT_MAX_AGE {
            let _ = match &checkpoint_ref.source {
                session_manager::CheckpointSource::Session(id) => {
                    manager.clear_session_checkpoint(id)
                }
                session_manager::CheckpointSource::Legacy => manager.clear_legacy_checkpoint(),
            };
            continue;
        }
        let loaded = match &checkpoint_ref.source {
            session_manager::CheckpointSource::Session(id) => manager.load_session_checkpoint(id),
            session_manager::CheckpointSource::Legacy => manager.load_legacy_checkpoint(),
        };
        let Ok(Some(session)) = loaded else {
            continue;
        };
        recent.push(RecentCheckpoint {
            session,
            age,
            source: checkpoint_ref.source,
        });
    }
    // `list_checkpoints` sorts newest-first already; keep it explicit here so
    // selection does not silently depend on the manager's ordering.
    recent.sort_by_key(|c| c.age);
    recent
}

fn checkpoint_age_label(age: std::time::Duration) -> String {
    if age.as_secs() < 60 {
        format!("{}s ago", age.as_secs())
    } else if age.as_secs() < 3600 {
        format!("{}m ago", age.as_secs() / 60)
    } else {
        format!("{}h ago", age.as_secs() / 3600)
    }
}

/// Check for a crash-recovery checkpoint and return the session ID if explicit
/// recovery was requested *and* the checkpoint belongs to the current
/// workspace.
///
/// Candidates are all per-session checkpoint files plus the legacy
/// single-slot `checkpoints/latest.json`; each must be younger than 24 hours
/// **and its workspace must match the resolved launch workspace after
/// canonicalisation** — the newest matching candidate wins. If no candidate
/// matches, a one-line notice points at `codewhale sessions`, and nothing is
/// auto-loaded: another workspace's checkpoint file is never touched (it may
/// belong to a live session there).
fn recover_interrupted_checkpoint_for_resume(launch_workspace: &Path) -> Option<String> {
    let manager = session_manager::SessionManager::default_location().ok()?;
    let candidates = load_recent_checkpoints(&manager);
    if candidates.is_empty() {
        return None;
    }

    // Refuse to silently restore a session from another workspace. Compare
    // against the resolved launch workspace, not the shell cwd, so callers
    // using `--workspace` cannot accidentally recover a checkpoint from the
    // directory their shell happened to be in.
    let (matching, mismatched): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|candidate| {
        session_manager::workspace_scope_matches(
            &candidate.session.metadata.workspace,
            launch_workspace,
        )
    });

    let Some(best) = matching.into_iter().next() else {
        if let Some(newest) = mismatched.first() {
            eprintln!(
                "Note: an interrupted session from another workspace ({}) is \
                 available. Run `codewhale sessions` to list saved sessions. Starting \
                 fresh in {}.",
                newest.session.metadata.workspace.display(),
                launch_workspace.display(),
            );
        }
        return None;
    };

    let session_id = best.session.metadata.id.clone();

    // Persist the checkpoint as a regular session so the TUI can load it by
    // id — unless a newer regular session file for the same id already
    // exists (e.g. `--continue` ran before and the session advanced since).
    // A stale checkpoint must never overwrite newer durable session state.
    if !saved_session_is_newer(&manager, &best.session)
        && manager.save_session(&best.session).is_err()
    {
        return None;
    }

    match &best.source {
        session_manager::CheckpointSource::Session(id) => {
            // Consume the per-session checkpoint now that it is recovered.
            let _ = manager.clear_session_checkpoint(id);
        }
        session_manager::CheckpointSource::Legacy => {
            // Migrate the legacy slot to a per-session file (never
            // overwriting an existing one) and leave `latest.json` in place
            // so an older binary can still find it; its writer is already
            // gone and the file ages out within 24 hours.
            let _ = manager.write_session_checkpoint_if_absent(&best.session);
        }
    }

    let age_str = checkpoint_age_label(best.age);
    eprintln!("Recovered interrupted session ({age_str}). Use --fresh to start fresh.",);

    Some(session_id)
}

/// Whether a regular session file for the checkpoint's id already exists and
/// is at least as recent as the checkpoint. When it is, persisting the
/// checkpoint over it would replace newer durable state with older in-flight
/// state.
fn saved_session_is_newer(
    manager: &session_manager::SessionManager,
    checkpoint: &session_manager::SavedSession,
) -> bool {
    manager
        .load_session(&checkpoint.metadata.id)
        .is_ok_and(|existing| existing.metadata.updated_at >= checkpoint.metadata.updated_at)
}

/// Preserve an interrupted checkpoint on a normal fresh launch without
/// attaching it to the new TUI instance. This keeps "open another codewhale in
/// the same folder" from re-entering the previous in-flight session while still
/// leaving an explicit resume path.
///
/// Only the newest recent checkpoint drives the notice. The legacy
/// single-slot file is persisted as a regular session and consumed (today's
/// behavior for that slot); per-session checkpoint files are persisted but
/// left in place — they may belong to a live session in another terminal,
/// and `--continue` reads them directly.
fn preserve_interrupted_checkpoint_for_explicit_resume(launch_workspace: &Path) {
    let Some(manager) = session_manager::SessionManager::default_location().ok() else {
        return;
    };
    let Some(newest) = load_recent_checkpoints(&manager).into_iter().next() else {
        return;
    };

    let session_workspace = newest.session.metadata.workspace.clone();
    if !saved_session_is_newer(&manager, &newest.session) {
        let _ = manager.save_session(&newest.session);
    }
    if newest.source == session_manager::CheckpointSource::Legacy {
        let _ = manager.clear_legacy_checkpoint();
    }

    let age_str = checkpoint_age_label(newest.age);
    if session_manager::workspace_scope_matches(&session_workspace, launch_workspace) {
        eprintln!(
            "Found an in-flight session snapshot ({age_str}). Starting a new \
             session. Run `codewhale --continue` to resume it."
        );
    } else {
        eprintln!(
            "Note: an interrupted session from another workspace ({}) is \
             available. Run `codewhale sessions` to list saved sessions. Starting \
             fresh in {}.",
            session_workspace.display(),
            launch_workspace.display(),
        );
    }
}

/// Load project-level config from `$WORKSPACE/.codewhale/config.toml`, with
/// legacy `$WORKSPACE/.deepseek/config.toml` fallback, then apply its fields as
/// overrides on top of the global config (#485).
/// Only explicitly set fields in the project file are applied; everything
/// else falls back to the global value.
#[cfg(test)]
fn merge_project_config(config: &mut Config, workspace: &Path) {
    merge_project_config_with_approval_baseline(config, workspace, None);
}

/// Apply project config while evaluating approval tightening against the
/// user's effective interactive baseline. `Config::approval_policy` remains
/// authoritative when present; the saved TUI posture is used only when the
/// root config leaves approval unset.
fn merge_project_config_with_approval_baseline(
    config: &mut Config,
    workspace: &Path,
    saved_permission_posture: Option<&str>,
) {
    // When the workspace is the user's home directory, the project-scope
    // config file is also the global config file. Skip the merge to avoid
    // redundant processing and a misleading "project-scope config key
    // ignored" warning on every launch from ~.
    if let Some(home) = effective_home_dir()
        && let (Ok(w), Ok(h)) = (
            std::fs::canonicalize(workspace),
            std::fs::canonicalize(&home),
        )
        && w == h
    {
        return;
    }

    // v0.8.44: prefer .codewhale/config.toml, fall back to .deepseek/
    let path = workspace
        .join(codewhale_config::CODEWHALE_APP_DIR)
        .join("config.toml");
    let raw = match read_project_config_file(&path) {
        Ok(Some(r)) => r,
        Ok(None) => {
            let legacy = workspace
                .join(codewhale_config::LEGACY_APP_DIR)
                .join("config.toml");
            match read_project_config_file(&legacy) {
                Ok(Some(r)) => r,
                Ok(None) => return,
                Err(err) => {
                    eprintln!(
                        "warning: failed to read project-scope config {}: {err}",
                        legacy.display()
                    );
                    return;
                }
            }
        }
        Err(err) => {
            eprintln!(
                "warning: failed to read project-scope config {}: {err}",
                path.display()
            );
            return;
        }
    };
    let project: toml::Value = match toml::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let table = match project.as_table() {
        Some(t) => t,
        None => return,
    };

    // #417: dangerous keys are denied at project scope. A malicious
    // `<workspace>/.deepseek/config.toml` could otherwise:
    // * `api_key` / `base_url` / `provider` — exfiltrate prompts to a
    //   look-alike endpoint by swapping the user's credentials and
    //   target host with project-controlled values.
    // * `mcp_config_path` — point the loader at an MCP config that
    //   spawns arbitrary stdio servers under the user's identity.
    // * `mcp_oauth_callback_*` — choose local OAuth redirect listener
    //   behavior for user-owned MCP credentials.
    //
    // The overlay path is non-interactive; users can't visually
    // confirm a rogue project config is hijacking these. We surface
    // a stderr warning on first encounter so a user who *did* expect
    // the override has a chance to notice the deny instead of silent
    // discard.
    const DENY_AT_PROJECT_SCOPE: &[&str] = &[
        "api_key",
        "base_url",
        "provider",
        "mcp_config_path",
        "mcp_oauth_callback_port",
        "mcp_oauth_callback_url",
    ];
    for key in DENY_AT_PROJECT_SCOPE {
        if table.contains_key(*key) {
            eprintln!(
                "warning: project-scope config key `{key}` is ignored — \
                 set it in `~/.codewhale/config.toml` instead. \
                 (See #417 for the deny-list rationale.)"
            );
        }
    }

    // String fields a project may legitimately override (model,
    // approval/sandbox tightening, notes path, reasoning effort).
    for (key, field) in [
        ("model", &mut config.default_text_model),
        ("reasoning_effort", &mut config.reasoning_effort),
        ("notes_path", &mut config.notes_path),
    ] {
        if let Some(v) = table.get(key).and_then(toml::Value::as_str)
            && !v.is_empty()
        {
            *field = Some(v.to_string());
        }
    }

    if let Some(v) = table.get("approval_policy").and_then(toml::Value::as_str)
        && !v.is_empty()
    {
        let saved_approval_baseline =
            crate::config::approval_policy_baseline_from_permission_posture(
                saved_permission_posture,
            );
        let approval_baseline = config
            .approval_policy
            .as_deref()
            .or(saved_approval_baseline);
        if codewhale_config::project_approval_policy_is_allowed(approval_baseline, v) {
            config.approval_policy = Some(v.to_string());
        } else {
            eprintln!(
                "warning: project-scope `approval_policy = \"{v}\"` is ignored — \
                 project config can only tighten the user's approval policy. \
                 (See #417.)"
            );
        }
    }

    if let Some(v) = table.get("sandbox_mode").and_then(toml::Value::as_str)
        && !v.is_empty()
    {
        if codewhale_config::project_sandbox_mode_is_allowed(config.sandbox_mode.as_deref(), v) {
            config.sandbox_mode = Some(v.to_string());
        } else {
            eprintln!(
                "warning: project-scope `sandbox_mode = \"{v}\"` is ignored — \
                 project config can only tighten the user's sandbox mode. \
                 (See #417.)"
            );
        }
    }

    // Numeric / bool fields that benefit from per-project overrides.
    if let Some(v) = table.get("max_subagents").and_then(toml::Value::as_integer)
        && v > 0
    {
        config.max_subagents = Some((v as usize).clamp(1, crate::config::MAX_SUBAGENTS));
    }
    if let Some(v) = table.get("allow_shell").and_then(toml::Value::as_bool) {
        if v {
            eprintln!(
                "warning: project-scope `allow_shell = true` is ignored — \
                 enable shell from user config for this workspace instead. \
                 (See #417.)"
            );
        } else {
            config.allow_shell = Some(false);
        }
    }

    if table.contains_key("instructions") {
        eprintln!(
            "warning: project-scope `instructions` is ignored — \
             configure instruction files from user config instead. \
             (See #417.)"
        );
    }
}

fn read_project_config_file(path: &Path) -> io::Result<Option<String>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "project-scope config must not be a symlink",
        ));
    }
    if !file_type.is_file() {
        return Ok(None);
    }

    let mut file = open_project_config_file(path)?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(Some(raw))
}

#[cfg(unix)]
fn open_project_config_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_project_config_file(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn merge_user_workspace_config(
    config: &mut Config,
    config_path: Option<PathBuf>,
    workspace: &Path,
) {
    if config.managed_config_path.is_some() || config.requirements_path.is_some() {
        return;
    }
    let allow_shell_before = config.allow_shell;
    let allow_shell_from_env = std::env::var_os("CODEWHALE_ALLOW_SHELL").is_some()
        || std::env::var_os("DEEPSEEK_ALLOW_SHELL").is_some();
    let Some(path) = crate::config::resolve_load_config_path(config_path) else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        return;
    };
    merge_user_workspace_config_from_doc(config, &doc, workspace);
    if allow_shell_from_env {
        config.allow_shell = allow_shell_before;
    }
}

fn merge_user_workspace_config_from_doc(config: &mut Config, doc: &toml::Value, workspace: &Path) {
    for table_name in ["workspace", "projects"] {
        let Some(entries) = doc.get(table_name).and_then(toml::Value::as_table) else {
            continue;
        };
        for (raw_path, entry) in entries {
            if !workspace_config_path_matches(raw_path, workspace) {
                continue;
            }
            if let Some(allow_shell) = entry.get("allow_shell").and_then(toml::Value::as_bool) {
                config.allow_shell = Some(allow_shell);
            }
        }
    }
}

fn workspace_config_path_matches(raw_path: &str, workspace: &Path) -> bool {
    let configured = crate::config::expand_path(raw_path);
    let configured = configured.canonicalize().unwrap_or(configured);
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    paths_equal_for_config(&configured, &workspace)
}

#[cfg(windows)]
fn paths_equal_for_config(left: &Path, right: &Path) -> bool {
    normalize_windows_config_path_for_compare(left)
        == normalize_windows_config_path_for_compare(right)
}

#[cfg(not(windows))]
fn paths_equal_for_config(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(windows)]
fn normalize_windows_config_path_for_compare(path: &Path) -> String {
    normalize_windows_config_path_str(&path.to_string_lossy())
}

#[cfg(any(windows, test))]
fn normalize_windows_config_path_str(path: &str) -> String {
    let mut normalized = path.replace('/', "\\");
    if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
        normalized = format!("\\\\{rest}");
    } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
        normalized = rest.to_string();
    }
    while normalized.len() > 3 && normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized.to_ascii_lowercase()
}

fn interactive_tui_allow_shell(yolo: bool, config: &Config) -> bool {
    yolo || config.interactive_allow_shell()
}

async fn run_interactive(
    cli: &Cli,
    config: &Config,
    resume_session_id: Option<String>,
    initial_input: Option<tui::InitialInput>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Merge project-level config from $WORKSPACE/.codewhale/config.toml
    // or legacy $WORKSPACE/.deepseek/config.toml
    // unless --no-project-config was passed (#485).
    let mut merged_config = config.clone();
    merge_user_workspace_config(&mut merged_config, cli.config.clone(), &workspace);
    if !cli.no_project_config {
        let saved_permission_posture = crate::settings::Settings::load_persisted()
            .ok()
            .and_then(|settings| settings.permission_posture);
        merge_project_config_with_approval_baseline(
            &mut merged_config,
            &workspace,
            saved_permission_posture.as_deref(),
        );
    }
    let config = &merged_config;

    if !cli.skip_onboarding {
        match crate::config::ensure_config_file_exists(cli.config.clone()) {
            Ok(Some(path)) => logging::info(format!(
                "Created first-run config file at {}",
                path.display()
            )),
            Ok(None) => {}
            Err(err) => logging::warn(format!("Failed to create first-run config file: {err}")),
        }
    }

    // v0.8.44: migrate config from ~/.deepseek/ to ~/.codewhale/ on first
    // launch. Non-fatal — existing installs keep working either way.
    match codewhale_config::migrate_config_if_needed() {
        Ok(Some(migration)) => {
            eprintln!("{}", migration.user_notice());
        }
        Ok(None) => {}
        Err(err) => logging::warn(format!("Config migration skipped: {err}")),
    }

    let model = config.default_model();
    let provider = config.api_provider();
    let max_subagents = cli.max_subagents.map_or_else(
        || config.max_subagents_for_provider(provider),
        |value| value.clamp(1, MAX_SUBAGENTS),
    );
    let use_alt_screen = should_use_alt_screen(cli, config);
    let use_mouse_capture = should_use_mouse_capture(cli, config, use_alt_screen);
    let use_bracketed_paste = crate::settings::Settings::load()
        .map(|s| s.effective_bracketed_paste())
        .unwrap_or_else(|_| !crate::settings::detected_legacy_windows_console_host());

    // Auto-install bundled system skills (e.g. skill-creator) on first launch.
    // Errors are non-fatal: log a warning and continue.
    let skills_dir = config.skills_dir();
    if let Err(e) = crate::skills::install_system_skills(&skills_dir) {
        logging::warn(format!("Failed to install system skills: {e}"));
    }

    startup_trace::mark("interactive_config");

    // Seed ProviderLake from the secret-free Models.dev disk cache before any
    // picker/inventory read, then kick a best-effort background refresh (#4187).
    // Failures are quiet: bundled catalog rows always remain available.
    crate::models_dev_live::maybe_load_persisted_cache();
    crate::models_dev_live::spawn_background_refresh();

    // Boot janitors — snapshot prune (7-day default), spillover prune
    // (#422), and managed-session cleanup (v0.8.44) — are best-effort disk
    // hygiene. On a large ~/.codewhale they were the dominant startup cost
    // (a git object walk plus thousands of stat/read calls), so they run on
    // a blocking worker while the TUI brings up its first frame (#3757).
    // All three were already documented as non-fatal.
    let snapshots = config.snapshots_config();
    let janitor_snapshots_enabled = snapshots.enabled;
    let janitor_max_age = snapshots.max_age();
    let janitor_workspace = workspace.clone();
    // Session cleanup races session restore: skip it entirely when a session
    // is being resumed/continued this launch (the just-resumed session could
    // be pruned before its first save bumps `updated_at`). It runs next
    // clean launch. When we do run it, exclude the explicit resume id too.
    let janitor_resume_id = resume_session_id.clone();
    let janitor_skip_session_cleanup = resume_session_id.is_some() || cli.continue_session;
    tokio::task::spawn_blocking(move || {
        if janitor_snapshots_enabled {
            session_manager::prune_workspace_snapshots(&janitor_workspace, janitor_max_age);
        }

        match crate::tools::truncate::prune_older_than(crate::tools::truncate::SPILLOVER_MAX_AGE) {
            Ok(0) => {}
            Ok(n) => tracing::debug!(
                target: "spillover",
                "boot prune removed {n} spillover file(s)"
            ),
            Err(err) => tracing::warn!(
                target: "spillover",
                ?err,
                "spillover prune skipped on boot"
            ),
        }

        if !janitor_skip_session_cleanup
            && let Ok(manager) = session_manager::SessionManager::default_location()
        {
            let _ = manager.cleanup_old_sessions_keeping(janitor_resume_id.as_deref());
        }
    });

    // The `deepseek` launcher forwards `--yolo` to this binary via the
    // DEEPSEEK_YOLO env var (config.yolo), not as a CLI flag. Honour either.
    let yolo = cli.yolo || config.yolo.unwrap_or(false);

    tui::run_tui(
        config,
        tui::TuiOptions {
            model,
            workspace,
            config_path: cli.config.clone(),
            config_profile: effective_config_profile(cli),
            allow_shell: interactive_tui_allow_shell(yolo, config),
            use_alt_screen,
            use_mouse_capture,
            use_bracketed_paste,
            skills_dir,
            memory_path: config.memory_path(),
            notes_path: config.notes_path(),
            mcp_config_path: config.mcp_config_path(),
            use_memory: config.memory_enabled(),
            start_in_agent_mode: yolo,
            skip_onboarding: cli.skip_onboarding,
            yolo, // YOLO mode auto-approves all tool executions
            resume_session_id,
            initial_input,
            max_subagents,
        },
        plugin_registry,
    )
    .await
}

#[derive(Debug)]
struct CliAutoRoute {
    provider: crate::config::ApiProvider,
    model: String,
    reasoning_effort: Option<crate::tui::app::ReasoningEffort>,
    auto_model: bool,
}

fn cli_reasoning_effort_value(
    config: &Config,
    model: &str,
    effort: crate::tui::app::ReasoningEffort,
) -> Option<String> {
    effort
        .api_value_for_route(config.api_provider(), &config.deepseek_base_url(), model)
        .map(str::to_string)
}

fn normalize_cli_reasoning_effort(value: &str) -> Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "inherit" | "parent" | "same" | "current" | "default" | "unset"
    ) {
        return Ok(None);
    }
    crate::tui::app::ReasoningEffort::parse_strict(trimmed)
        .map(|effort| Some(effort.as_setting().to_string()))
        .map_err(anyhow::Error::msg)
}

fn config_for_cli_route(config: &Config, route: &CliAutoRoute) -> Config {
    let mut execution_config = config.clone();
    execution_config.provider = Some(config.provider_identity_for(route.provider));
    execution_config.set_provider_model_override(route.provider, Some(route.model.clone()));
    if matches!(
        route.provider,
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN
    ) {
        execution_config.default_text_model = Some(route.model.clone());
    }
    execution_config
}

async fn resolve_cli_auto_route(
    config: &Config,
    model: &str,
    prompt: &str,
) -> Result<CliAutoRoute> {
    if model.trim().eq_ignore_ascii_case("auto") {
        let selection =
            model_routing::resolve_auto_route_with_inventory(config, prompt, "", "auto", "auto")
                .await?;
        Ok(CliAutoRoute {
            provider: selection.provider,
            model: selection.model,
            reasoning_effort: selection.reasoning_effort,
            auto_model: true,
        })
    } else {
        if let Some(selection) = model_routing::resolve_explicit_route_with_inventory(config, model)
        {
            return Ok(CliAutoRoute {
                provider: selection.provider,
                model: selection.model,
                reasoning_effort: selection.reasoning_effort,
                auto_model: false,
            });
        }

        let candidate_providers = model_routing::explicit_route_candidate_providers(config, model);
        if !candidate_providers.is_empty() && !candidate_providers.contains(&config.api_provider())
        {
            let providers = candidate_providers
                .iter()
                .map(|provider| provider.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "model `{model}` is available from configured provider route(s): {providers}. \
                 Pass `--provider <provider>` with `--model {model}` to choose one explicitly. \
                 In the TUI, use `/provider`, `/model`, or `/setup` to resolve the route before sending."
            );
        }

        // When --model is not `auto`, fall back to the reasoning_effort
        // declared in the user's config.toml. The previous hard-coded `None`
        // silently dropped the user's setting on every non-auto-route exec
        // call, which (for example) prevented vllm + Qwen3 users from
        // disabling thinking via `reasoning_effort = "off"` and caused
        // 30+ second SSE idle timeouts on trivial prompts.
        Ok(CliAutoRoute {
            provider: config.api_provider(),
            model: model.to_string(),
            reasoning_effort: config
                .reasoning_effort()
                .map(crate::tui::app::ReasoningEffort::from_setting),
            auto_model: false,
        })
    }
}

async fn resolve_cli_exec_route(
    config: &Config,
    model: &str,
    prompt: &str,
    force_configured_route: bool,
) -> Result<CliAutoRoute> {
    if force_configured_route && !model.trim().eq_ignore_ascii_case("auto") {
        return Ok(CliAutoRoute {
            provider: config.api_provider(),
            model: model.to_string(),
            reasoning_effort: config
                .reasoning_effort()
                .map(crate::tui::app::ReasoningEffort::from_setting),
            auto_model: false,
        });
    }
    resolve_cli_auto_route(config, model, prompt).await
}

fn should_force_configured_exec_route(
    resuming: bool,
    explicit_provider: Option<&str>,
    explicit_model: Option<&str>,
) -> bool {
    // A configured/default model belongs to the configured provider route.
    // Cross-provider inventory inference is reserved for an explicit model
    // override without an explicit provider. Resume remains route-authoritative
    // even when its model is overridden because it restores the saved provider.
    resuming || explicit_provider.is_some() || explicit_model.is_none()
}

async fn run_one_shot(
    config: &Config,
    model: &str,
    prompt: &str,
    force_configured_route: bool,
) -> Result<()> {
    use crate::client::DeepSeekClient;
    use crate::models::{ContentBlock, Message, MessageRequest};

    let route = resolve_cli_exec_route(config, model, prompt, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let client = DeepSeekClient::new(&execution_config)?;
    let reasoning_effort = route
        .reasoning_effort
        .and_then(|effort| cli_reasoning_effort_value(&execution_config, &route.model, effort));

    let request = MessageRequest {
        model: route.model,
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: 4096,
        system: None,
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(false),
        temperature: None,
        top_p: None,
    };

    let response = client.create_message(request).await?;

    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            println!("{text}");
        }
    }

    Ok(())
}

async fn run_one_shot_json(
    config: &Config,
    model: &str,
    prompt: &str,
    force_configured_route: bool,
) -> Result<()> {
    use crate::client::DeepSeekClient;
    use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

    let route = resolve_cli_exec_route(config, model, prompt, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let provider = execution_config.provider_identity_for(route.provider);
    let client = DeepSeekClient::new(&execution_config)?;
    let model = route.model.clone();
    let reasoning_effort = route
        .reasoning_effort
        .and_then(|effort| cli_reasoning_effort_value(&execution_config, &model, effort));
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: 4096,
        system: Some(SystemPrompt::Text(
            "You are a coding assistant. Give concise, actionable responses.".to_string(),
        )),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(false),
        temperature: Some(0.2),
        top_p: Some(0.9),
    };

    let response = client.create_message(request).await?;
    let mut output = String::new();
    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            output.push_str(&text);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&one_shot_exec_json_receipt(provider, model, output,))?
    );
    Ok(())
}

fn one_shot_exec_json_receipt(
    provider: String,
    model: String,
    output: String,
) -> serde_json::Value {
    serde_json::json!({
        "mode": "one-shot",
        "provider": provider,
        "model": model,
        "success": true,
        "output": output
    })
}

fn exec_stream_provider_route(
    identity: &crate::config::ProviderIdentity,
) -> (String, Option<String>) {
    let provider = identity.provider.as_str().to_string();
    let provider_id = if identity.provider == crate::config::ApiProvider::Custom {
        identity.exact_id.clone()
    } else {
        None
    };
    (provider, provider_id)
}

#[derive(serde::Serialize)]
struct ExecStreamMeta {
    receipt_kind: &'static str,
    provider: String,
    /// Exact configured provider-table id, when one selected the route.
    /// `None` deliberately distinguishes the legacy idless root custom route
    /// from literal `[providers.custom]`, whose exact id is `"custom"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
    model: String,
    route_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_hit_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_miss_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_write_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_tokens: Option<u32>,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_count: Option<u32>,
    approval_posture: String,
    sandbox_posture: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    binary_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_sha256: Option<String>,
    prompt_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_catalog_sha256: Option<String>,
    input_analysis: ExecStreamInputAnalysis,
    visible_final_answer_chars: usize,
    session_id: String,
    resume_command: String,
    workspace: String,
    message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    termination_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_category: Option<String>,
}

#[derive(Debug, Default, Clone, serde::Serialize, PartialEq, Eq)]
struct ExecStreamInputAnalysis {
    estimated_request_tokens: usize,
    estimated_message_content_tokens: usize,
    estimated_system_tokens: usize,
    estimated_framing_tokens: usize,
    user_message_count: usize,
    assistant_message_count: usize,
    tool_message_count: usize,
    tool_use_count: usize,
    tool_result_count: usize,
    text_chars: usize,
    thinking_chars: usize,
    tool_use_input_chars: usize,
    tool_result_chars: usize,
    text_estimated_tokens: usize,
    thinking_estimated_tokens: usize,
    tool_use_input_estimated_tokens: usize,
    tool_result_estimated_tokens: usize,
}

#[derive(serde::Serialize)]
#[serde(tag = "type")]
// Keep receipts flat for stable JSONL consumers. Boxing the whole tool_result
// payload would introduce a nested object and break the stream schema.
#[allow(clippy::large_enum_variant)]
enum ExecStreamEvent {
    #[serde(rename = "content")]
    Content { content: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        id: String,
        input: serde_json::Value,
        started_at: String,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        id: String,
        name: String,
        output: String,
        status: String,
        started_at: String,
        completed_at: String,
        duration_ms: u64,
        side_effect_status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_category: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        artifact: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_metadata: Option<serde_json::Value>,
    },
    #[serde(rename = "sandbox_denied")]
    SandboxDenied {
        tool_id: String,
        tool_name: String,
        reason: String,
        outcome: String,
    },
    #[serde(rename = "workflow_event")]
    WorkflowEvent {
        run_id: String,
        event: serde_json::Value,
    },
    #[serde(rename = "session_capture")]
    SessionCapture { content: String },
    #[serde(rename = "metadata")]
    Metadata { meta: Box<ExecStreamMeta> },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "error")]
    Error { error: String },
}

fn exec_sandbox_elevation_authorized(
    allow_sandbox_elevation: bool,
    explicit_sandbox: Option<&str>,
) -> bool {
    allow_sandbox_elevation
        || explicit_sandbox.is_some_and(|policy| policy.eq_ignore_ascii_case("danger-full-access"))
}

fn emit_exec_stream_event(event: &ExecStreamEvent) -> Result<()> {
    println!("{}", serde_json::to_string(&exec_stream_value(event)?)?);
    Ok(())
}

fn exec_stream_value(event: &ExecStreamEvent) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(event)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".to_string(), serde_json::json!(1));
        object.insert(
            "schema".to_string(),
            serde_json::json!("codewhale.exec-stream"),
        );
    }
    Ok(value)
}

fn tool_error_receipt_category(error: &crate::tools::spec::ToolError) -> &'static str {
    use crate::tools::spec::ToolError;
    match error {
        ToolError::InvalidInput { .. } => "invalid_input",
        ToolError::MissingField { .. } => "missing_field",
        ToolError::PathEscape { .. } => "path_escape",
        ToolError::ExecutionFailed { .. } => "execution_failed",
        ToolError::Timeout { .. } => "timeout",
        ToolError::Cancelled { .. } => "cancelled",
        ToolError::NotAvailable { .. } => "not_available",
        ToolError::PermissionDenied { .. } => "permission_denied",
    }
}

fn tool_artifact_receipt(metadata: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let object = metadata?.as_object()?;
    let mut artifact = serde_json::Map::new();
    for key in [
        "artifact_id",
        "artifact_path",
        "artifact_relative_path",
        "artifact_byte_size",
        "spillover_path",
        "content_digest",
        "original_byte_count",
        "retained_head_bytes",
        "retained_tail_bytes",
    ] {
        if let Some(value) = object.get(key) {
            artifact.insert(key.to_string(), value.clone());
        }
    }
    (!artifact.is_empty()).then_some(serde_json::Value::Object(artifact))
}

fn current_binary_sha256() -> Option<String> {
    let bytes = std::fs::read(std::env::current_exe().ok()?).ok()?;
    Some(format!("sha256:{}", crate::hashing::sha256_hex(&bytes)))
}

async fn run_workflow_tool_command(
    cli: &Cli,
    args: WorkflowToolArgs,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    match run_workflow_tool_command_inner(cli, args, plugin_registry).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = emit_exec_stream_event(&ExecStreamEvent::Error {
                error: format!("{error:#}"),
            });
            exit_workflow_tool_failure();
        }
    }
}

async fn run_workflow_tool_command_inner(
    cli: &Cli,
    args: WorkflowToolArgs,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    use crate::tools::spec::ToolSpec;

    if args.approval_source != "explicit-workflow-command" {
        bail!("workflow-tool requires --approval-source explicit-workflow-command");
    }
    let input: serde_json::Value = serde_json::from_str(&args.input_json)
        .context("--input-json must be a valid Workflow tool input object")?;
    if !input.is_object() {
        bail!("--input-json must be a JSON object");
    }
    if !input
        .get("action")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|action| action.eq_ignore_ascii_case("run"))
    {
        bail!("workflow-tool accepts only action=run");
    }

    let workspace = resolve_workspace(cli);
    let mut config = load_config_from_cli(cli)?;
    merge_user_workspace_config(&mut config, cli.config.clone(), &workspace);
    if let Ok(env_url) =
        std::env::var("CODEWHALE_BASE_URL").or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
    {
        let trimmed = env_url.trim();
        if !trimmed.is_empty() {
            config.base_url = Some(trimmed.to_string());
        }
    }

    let model = resolve_exec_model(&config, None);
    let route = resolve_cli_exec_route(
        &config,
        &model,
        "Run a checked-in Workflow through the host runtime",
        true,
    )
    .await?;
    let execution_config = config_for_cli_route(&config, &route);
    let route_identity = execution_config
        .active_provider_identity(route.provider)
        .map_err(anyhow::Error::msg)
        .context("workflow terminal route lost its exact provider identity")?;
    let (route_provider, route_provider_id) = exec_stream_provider_route(&route_identity);
    let workflow_input_sha256 = format!(
        "sha256:{}",
        crate::hashing::sha256_hex(&serde_json::to_vec(&input)?)
    );
    let tool_id = format!("workflow_host_{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let tool_started = Instant::now();
    let tool_started_at = chrono::Utc::now().to_rfc3339();

    emit_exec_stream_event(&ExecStreamEvent::ToolUse {
        name: "workflow".to_string(),
        id: tool_id.clone(),
        input: input.clone(),
        started_at: tool_started_at.clone(),
    })?;

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(1024);
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let event_forwarder = tokio::spawn(forward_direct_workflow_events(event_rx, stop_rx));
    let (tool, context) = match build_direct_workflow_tool(
        &execution_config,
        &route,
        &workspace,
        event_tx,
        plugin_registry,
    )
    .await
    {
        Ok(built) => built,
        Err(err) => {
            let _ = stop_tx.send(());
            let _ = event_forwarder.await;
            exit_workflow_tool_error(&tool_id, err.to_string());
        }
    };

    let result = tool.execute(input, &context).await;
    drop(tool);
    let _ = stop_tx.send(());
    event_forwarder
        .await
        .context("workflow event forwarder task failed")??;

    let result = match result {
        Ok(result) => result,
        Err(err) => {
            let error = err.to_string();
            exit_workflow_tool_error(&tool_id, error);
        }
    };

    let workflow_status =
        direct_workflow_status(&result.content).unwrap_or_else(|| "unknown".to_string());
    let completed = result.success && workflow_status == "completed";
    emit_exec_stream_event(&ExecStreamEvent::ToolResult {
        id: tool_id,
        name: "workflow".to_string(),
        output: result.content.clone(),
        status: if completed { "success" } else { "error" }.to_string(),
        started_at: tool_started_at,
        completed_at: chrono::Utc::now().to_rfc3339(),
        duration_ms: u64::try_from(tool_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        side_effect_status: result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("side_effect_status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        error_category: (!completed).then(|| "tool_error".to_string()),
        truncated: result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("truncated"))
            .and_then(serde_json::Value::as_bool),
        artifact: tool_artifact_receipt(result.metadata.as_ref()),
        result_metadata: result.metadata.clone(),
    })?;
    emit_exec_stream_event(&ExecStreamEvent::Metadata {
        meta: Box::new(ExecStreamMeta {
            receipt_kind: "terminal",
            provider: route_provider,
            provider_id: route_provider_id,
            // No parent/operator model call occurs on this host-owned path;
            // child model/provider usage remains attributable in typed task
            // receipts rather than being misreported as one root model.
            model: "host-workflow".to_string(),
            route_source: "host_workflow".to_string(),
            input_tokens: None,
            output_tokens: None,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            prompt_cache_write_tokens: None,
            reasoning_tokens: None,
            duration_ms: u64::try_from(tool_started.elapsed().as_millis()).unwrap_or(u64::MAX),
            retry_count: None,
            approval_posture: "explicit_workflow_command".to_string(),
            sandbox_posture: "configured".to_string(),
            binary_sha256: current_binary_sha256(),
            config_sha256: None,
            prompt_sha256: workflow_input_sha256,
            tool_catalog_sha256: None,
            input_analysis: ExecStreamInputAnalysis::default(),
            visible_final_answer_chars: result.content.chars().count(),
            session_id: String::new(),
            resume_command: String::new(),
            workspace: workspace.display().to_string(),
            message_count: 0,
            status: Some(workflow_status.clone()),
            termination_reason: Some(if completed { "resolved" } else { "tool_error" }.to_string()),
            error_category: (!completed).then(|| "tool".to_string()),
        }),
    })?;
    if !completed {
        let error = format!("workflow run ended with terminal status {workflow_status}");
        emit_exec_stream_event(&ExecStreamEvent::Error {
            error: error.clone(),
        })?;
        exit_workflow_tool_failure();
    }
    emit_exec_stream_event(&ExecStreamEvent::Done)?;
    Ok(())
}

fn exit_workflow_tool_failure() -> ! {
    let _ = io::stdout().flush();
    std::process::exit(1)
}

fn exit_workflow_tool_error(tool_id: &str, error: String) -> ! {
    let now = chrono::Utc::now().to_rfc3339();
    let _ = emit_exec_stream_event(&ExecStreamEvent::ToolResult {
        id: tool_id.to_string(),
        name: "workflow".to_string(),
        output: error.clone(),
        status: "error".to_string(),
        started_at: now.clone(),
        completed_at: now,
        duration_ms: 0,
        side_effect_status: "unknown".to_string(),
        error_category: Some("execution_failed".to_string()),
        truncated: None,
        artifact: None,
        result_metadata: None,
    });
    let _ = emit_exec_stream_event(&ExecStreamEvent::Error { error });
    exit_workflow_tool_failure()
}

async fn initialize_direct_workflow_mcp_pool(
    config: &Config,
    workspace: &Path,
    network_policy: Option<crate::network_policy::NetworkPolicyDecider>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Option<(
    std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    Vec<(String, String)>,
)> {
    if !config.features().enabled(Feature::Mcp) {
        return None;
    }
    let mut pool = crate::mcp::McpPool::from_config_path_with_workspace_and_plugins(
        &config.mcp_config_path(),
        workspace,
        plugin_registry,
    )
    .unwrap_or_else(|error| {
        tracing::debug!("No MCP config for direct Workflow runtime: {error:#}");
        crate::mcp::McpPool::new(crate::mcp::McpConfig::default())
    });
    if let Some(policy) = network_policy {
        pool = pool.with_network_policy(policy);
    }
    let failures = pool
        .connect_all()
        .await
        .into_iter()
        .map(|(server, error)| (server, format!("{error:#}")))
        .collect();
    Some((std::sync::Arc::new(tokio::sync::Mutex::new(pool)), failures))
}

async fn build_direct_workflow_tool(
    config: &Config,
    route: &CliAutoRoute,
    workspace: &Path,
    event_tx: tokio::sync::mpsc::Sender<crate::core::events::Event>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<(
    crate::tools::workflow::WorkflowTool,
    crate::tools::ToolContext,
)> {
    use std::sync::Arc;

    use crate::client::DeepSeekClient;
    use crate::core::authority::shell_policy_for_mode;
    use crate::fleet::roster::FleetRoster;
    use crate::tools::AgentToolSurfaceOptions;
    use crate::tools::goal::new_shared_goal_state;
    use crate::tools::subagent::{SubAgentRuntime, new_shared_subagent_manager_with_timeout};
    use crate::tools::todo::new_shared_todo_list;
    use crate::tui::app::AppMode;

    let provider = config.api_provider();
    if !config.subagents_enabled_for_provider(provider) {
        bail!(
            "Workflow dispatch requires sub-agents for provider {} ({})",
            provider.as_str(),
            config
                .subagents_disabled_reason()
                .unwrap_or("provider-specific sub-agent configuration disabled it")
        );
    }

    let yolo = config.yolo.unwrap_or(false);
    let mode = if yolo {
        AppMode::Yolo
    } else {
        AppMode::Operate
    };
    let allow_shell = yolo || config.allow_shell();
    let shell_policy = shell_policy_for_mode(mode, allow_shell);
    let trusted = crate::workspace_trust::WorkspaceTrust::load_for(workspace);
    let mut context = crate::tools::ToolContext::with_auto_approve(
        workspace.to_path_buf(),
        yolo,
        config.notes_path(),
        config.mcp_config_path(),
        yolo,
    )
    .with_features(config.features())
    .with_skills_config(
        config.skills_dir(),
        config.skills_config().scan_codewhale_only(),
    )
    .with_plugin_registry(std::sync::Arc::clone(&plugin_registry))
    .with_shell_policy(shell_policy)
    .with_trusted_external_paths(trusted.paths().to_vec())
    .with_elevated_sandbox_policy(workflow_host_sandbox_policy(config, mode, workspace));
    let network_policy = config.network.clone().map(|network| {
        crate::network_policy::NetworkPolicyDecider::with_default_audit(network.into_runtime())
    });
    if let Some(policy) = network_policy.as_ref() {
        context = context.with_network_policy(policy.clone());
    }
    if config.memory_enabled() {
        context.memory_path = Some(config.memory_path());
    }
    context.search_provider = config.search_provider();
    context.search_api_key = config
        .search
        .as_ref()
        .and_then(|search| search.api_key.clone());
    context.search_base_url = config
        .search
        .as_ref()
        .and_then(|search| search.base_url.clone());
    if let Some(backend) = crate::sandbox::backend::create_backend(config)? {
        context = context.with_sandbox_backend(Arc::from(backend));
    }

    let max_subagents = config.max_subagents_for_provider(provider);
    let manager = new_shared_subagent_manager_with_timeout(
        workspace.to_path_buf(),
        max_subagents,
        config
            .max_admitted_subagents_for_provider(provider)
            .max(max_subagents),
        Duration::from_secs(config.subagent_heartbeat_timeout_secs_for_provider(provider)),
        config.launch_concurrency_for_provider(provider),
        config.subagent_token_budget_for_provider(provider),
    );
    let roster = Arc::new(FleetRoster::load(&config.fleet_config(), workspace));
    let mut role_models = roster.model_overrides();
    role_models.extend(config.subagent_model_overrides());

    let features = config.features();
    let mut surface = AgentToolSurfaceOptions::new(shell_policy);
    surface.apply_patch_enabled = features.enabled(Feature::ApplyPatch);
    surface.web_search_enabled = features.enabled(Feature::WebSearch);
    surface.memory_tool_enabled = config.memory_enabled() && !config.moraine_fallback();
    surface.vision_config = features
        .enabled(Feature::VisionModel)
        .then(|| config.vision_model_config())
        .flatten();
    surface.speech_output_dir = config.speech_output_dir();
    surface.goal_state = Some(new_shared_goal_state());

    let client = DeepSeekClient::new(config)?;
    let reasoning_effort = route
        .reasoning_effort
        .and_then(|effort| cli_reasoning_effort_value(config, &route.model, effort));
    let mcp_pool = if let Some((pool, failures)) =
        initialize_direct_workflow_mcp_pool(config, workspace, network_policy, plugin_registry)
            .await
    {
        for (server, error) in failures {
            tracing::warn!(
                server = %server,
                error = %error,
                "direct Workflow runtime could not connect MCP server"
            );
        }
        Some(pool)
    } else {
        None
    };
    let runtime = SubAgentRuntime::new(
        client,
        route.model.clone(),
        context.clone(),
        allow_shell,
        Some(event_tx),
        manager.clone(),
    )
    .with_locale_tag(
        crate::localization::resolve_locale(
            &crate::settings::Settings::load_persisted()
                .unwrap_or_default()
                .locale,
        )
        .tag(),
    )
    .with_role_models(role_models)
    .with_api_config(config.clone())
    .with_fleet_roster(roster)
    .with_auto_model(route.auto_model)
    .with_reasoning_effort(reasoning_effort, route.auto_model)
    .with_agent_tool_surface_options(surface)
    .with_max_spawn_depth(config.subagent_max_spawn_depth_for_provider(provider))
    .with_step_api_timeout(Duration::from_secs(
        config.subagent_api_timeout_secs_for_provider(provider),
    ))
    .with_speech_output_dir(config.speech_output_dir())
    .with_mcp_pool(mcp_pool)
    .with_todos(new_shared_todo_list())
    .with_parent_mode(mode);

    Ok((
        crate::tools::workflow::WorkflowTool::new(manager, runtime).with_explicit_cli_approval(),
        context,
    ))
}

fn workflow_host_sandbox_policy(
    config: &Config,
    mode: crate::tui::app::AppMode,
    workspace: &Path,
) -> crate::sandbox::SandboxPolicy {
    use crate::sandbox::SandboxPolicy;

    match config.sandbox_mode.as_deref() {
        Some("read-only") => SandboxPolicy::ReadOnly,
        Some("workspace-write") => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![workspace.to_path_buf()],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        },
        Some("danger-full-access") => SandboxPolicy::DangerFullAccess,
        Some("external-sandbox") => SandboxPolicy::ExternalSandbox {
            network_access: true,
        },
        _ => crate::core::authority::sandbox_policy_for_mode(mode, workspace),
    }
}

async fn forward_direct_workflow_events(
    mut event_rx: tokio::sync::mpsc::Receiver<crate::core::events::Event>,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    loop {
        tokio::select! {
            biased;
            event = event_rx.recv() => match event {
                Some(event) => emit_direct_workflow_event(event)?,
                None => return Ok(()),
            },
            _ = &mut stop_rx => {
                while let Ok(event) = event_rx.try_recv() {
                    emit_direct_workflow_event(event)?;
                }
                return Ok(());
            }
        }
    }
}

fn emit_direct_workflow_event(event: crate::core::events::Event) -> Result<()> {
    if let crate::core::events::Event::WorkflowUi { run_id, event } = event {
        emit_exec_stream_event(&ExecStreamEvent::WorkflowEvent { run_id, event })?;
    }
    Ok(())
}

fn direct_workflow_status(content: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()?
        .get("status")?
        .as_str()
        .map(str::to_ascii_lowercase)
}

fn exec_stream_input_analysis(
    messages: &[Message],
    system: Option<&SystemPrompt>,
) -> ExecStreamInputAnalysis {
    let mut analysis = ExecStreamInputAnalysis {
        estimated_request_tokens: crate::compaction::estimate_input_tokens_conservative(
            messages, system,
        ),
        estimated_message_content_tokens: crate::compaction::estimate_tokens(messages),
        estimated_system_tokens: exec_stream_estimate_system_tokens(system),
        estimated_framing_tokens: messages.len().saturating_mul(12).saturating_add(48),
        ..ExecStreamInputAnalysis::default()
    };

    for message in messages {
        match message.role.as_str() {
            "user" => analysis.user_message_count += 1,
            "assistant" => analysis.assistant_message_count += 1,
            "tool" => analysis.tool_message_count += 1,
            _ => {}
        }

        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    exec_stream_add_text_estimate(
                        text,
                        &mut analysis.text_chars,
                        &mut analysis.text_estimated_tokens,
                    );
                }
                ContentBlock::Thinking { thinking, .. } => {
                    exec_stream_add_text_estimate(
                        thinking,
                        &mut analysis.thinking_chars,
                        &mut analysis.thinking_estimated_tokens,
                    );
                }
                ContentBlock::ToolUse { input, .. } | ContentBlock::ServerToolUse { input, .. } => {
                    analysis.tool_use_count += 1;
                    exec_stream_add_json_estimate(
                        input,
                        &mut analysis.tool_use_input_chars,
                        &mut analysis.tool_use_input_estimated_tokens,
                    );
                }
                ContentBlock::ToolResult {
                    content,
                    content_blocks,
                    ..
                } => {
                    analysis.tool_result_count += 1;
                    exec_stream_add_text_estimate(
                        content,
                        &mut analysis.tool_result_chars,
                        &mut analysis.tool_result_estimated_tokens,
                    );
                    if let Some(blocks) = content_blocks {
                        exec_stream_add_json_estimate(
                            blocks,
                            &mut analysis.tool_result_chars,
                            &mut analysis.tool_result_estimated_tokens,
                        );
                    }
                }
                ContentBlock::ToolSearchToolResult { content, .. }
                | ContentBlock::CodeExecutionToolResult { content, .. } => {
                    analysis.tool_result_count += 1;
                    exec_stream_add_json_estimate(
                        content,
                        &mut analysis.tool_result_chars,
                        &mut analysis.tool_result_estimated_tokens,
                    );
                }
                ContentBlock::ImageUrl { .. } => {}
            }
        }
    }

    analysis
}

fn exec_stream_add_text_estimate(text: &str, chars: &mut usize, tokens: &mut usize) {
    *chars = chars.saturating_add(text.chars().count());
    *tokens = tokens.saturating_add(crate::compaction::estimate_text_tokens_conservative(text));
}

fn exec_stream_add_json_estimate<T: serde::Serialize>(
    value: &T,
    chars: &mut usize,
    tokens: &mut usize,
) {
    let text = serde_json::to_string(value).unwrap_or_default();
    exec_stream_add_text_estimate(&text, chars, tokens);
}

fn exec_stream_estimate_system_tokens(system: Option<&SystemPrompt>) -> usize {
    match system {
        Some(SystemPrompt::Text(text)) => {
            crate::compaction::estimate_text_tokens_conservative(text)
        }
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| crate::compaction::estimate_text_tokens_conservative(&block.text))
            .sum(),
        None => 0,
    }
}

fn exec_saved_session_line(session_id: &str) -> String {
    format!("session: {}", truncate_id(session_id))
}

fn exec_resumed_session_line(session_id: &str) -> String {
    format!("resumed session: {}", truncate_id(session_id))
}

fn exec_stream_session_ref(session_id: &str) -> String {
    crate::utils::redacted_identifier_for_log(session_id)
}

fn exec_stream_resume_hint(session_id: &str) -> String {
    if session_id.trim().is_empty() {
        String::new()
    } else {
        "codewhale exec --resume <redacted-session-id>".to_string()
    }
}

#[derive(Clone, Copy)]
struct PersistedProviderRoute<'a> {
    kind: &'a str,
    id: Option<&'a str>,
}

fn persist_exec_session(
    messages: &[Message],
    model: &str,
    provider_route: PersistedProviderRoute<'_>,
    workspace: &Path,
    system_prompt: &Option<SystemPrompt>,
    session_id: Option<&str>,
    total_tokens: u64,
) -> Result<String> {
    let manager =
        SessionManager::default_location().context("could not open session manager for save")?;
    let mut saved = if let Some(id) = session_id.filter(|id| !id.trim().is_empty()) {
        match manager.load_session(id) {
            Ok(existing) => session_manager::update_session(
                existing,
                messages,
                total_tokens,
                system_prompt.as_ref(),
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                session_manager::create_saved_session_with_id_and_mode(
                    id.to_string(),
                    messages,
                    model,
                    workspace,
                    total_tokens,
                    system_prompt.as_ref(),
                    Some("exec"),
                )
            }
            Err(err) => return Err(err).context("could not load existing exec session"),
        }
    } else {
        session_manager::create_saved_session_with_mode(
            messages,
            model,
            workspace,
            total_tokens,
            system_prompt.as_ref(),
            Some("exec"),
        )
    };
    stamp_exec_session_metadata(
        &mut saved,
        model,
        provider_route.kind,
        provider_route.id,
        workspace,
    );
    let id = saved.metadata.id.clone();
    manager
        .save_session(&saved)
        .context("could not save exec session")?;
    Ok(id)
}

fn stamp_exec_session_metadata(
    saved: &mut session_manager::SavedSession,
    model: &str,
    model_provider_kind: &str,
    model_provider_id: Option<&str>,
    workspace: &Path,
) {
    saved.metadata.model = model.to_string();
    saved
        .metadata
        .set_model_provider_route(model_provider_kind, model_provider_id);
    saved.metadata.workspace = workspace.to_path_buf();
    saved.metadata.mode = Some("exec".to_string());
}

#[derive(serde::Serialize)]
struct ExecToolEntry {
    name: String,
    success: bool,
    output: String,
}

#[derive(serde::Serialize)]
struct ExecOutcome {
    kind: String,
    outcome: String,
    tool_name: String,
    reason: String,
}

#[derive(serde::Serialize, Default)]
struct ExecSummary {
    mode: String,
    provider: String,
    model: String,
    prompt: String,
    output: String,
    tools: Vec<ExecToolEntry>,
    outcomes: Vec<ExecOutcome>,
    status: Option<String>,
    termination_reason: Option<String>,
    error_category: Option<String>,
    error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn run_exec_agent(
    config: &Config,
    model: &str,
    prompt: &str,
    workspace: PathBuf,
    max_subagents: usize,
    auto_approve: bool,
    allow_sandbox_elevation: bool,
    explicit_sandbox: Option<&str>,
    trust_mode: bool,
    json_output: bool,
    resume_session: Option<session_manager::SavedSession>,
    force_configured_route: bool,
    output_format: ExecOutputFormat,
    max_turns: u32,
    allowed_tools: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    append_system_prompt: Option<String>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    use crate::compaction::CompactionConfig;
    use crate::core::engine::{EngineConfig, spawn_engine};
    use crate::core::events::Event;
    use crate::core::ops::Op;
    use crate::tools::plan::new_shared_plan_state;
    use crate::tools::todo::new_shared_todo_list;
    use crate::tui::app::AppMode;

    let route = resolve_cli_exec_route(config, model, prompt, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let auto_model = route.auto_model;
    let effective_provider = route.provider;
    let effective_model = route.model;
    let validated_route = crate::route_runtime::resolve_runtime_route(
        &execution_config,
        effective_provider,
        Some(&effective_model),
    )
    .map_err(anyhow::Error::msg)?
    .validate()
    .map_err(anyhow::Error::msg)?;
    let effective_provider_name = validated_route.identity.key.clone();
    let effective_provider_id = validated_route.identity.exact_id.clone();
    let (effective_provider_kind, effective_stream_provider_id) =
        exec_stream_provider_route(&validated_route.identity);
    let route_source = if auto_model {
        "auto_resolver"
    } else {
        "explicit_or_configured"
    }
    .to_string();
    let exec_started = Instant::now();
    let prompt_sha256 = format!("sha256:{}", crate::hashing::sha256_hex(prompt.as_bytes()));
    let binary_sha256 = current_binary_sha256();
    let approval_posture = if auto_approve { "auto_tools" } else { "ask" }.to_string();
    let sandbox_posture = explicit_sandbox.unwrap_or("configured_default").to_string();
    let active_route_limits =
        crate::route_budget::known_route_limits(validated_route.candidate.limits());
    let max_subagents = if max_subagents == config.max_subagents_for_provider(config.api_provider())
    {
        execution_config
            .max_subagents_for_provider(effective_provider)
            .clamp(1, MAX_SUBAGENTS)
    } else {
        max_subagents
    };
    let effective_reasoning_effort = route
        .reasoning_effort
        .and_then(|effort| cli_reasoning_effort_value(&execution_config, &effective_model, effort));

    let settings = crate::settings::Settings::load().unwrap_or_default();
    let auto_compact_enabled = if crate::settings::Settings::auto_compact_explicitly_configured() {
        settings.auto_compact
    } else {
        crate::route_budget::auto_compact_default_for_route(
            effective_provider,
            &effective_model,
            active_route_limits,
        )
    };
    let compaction = CompactionConfig {
        enabled: auto_compact_enabled,
        model: effective_model.clone(),
        effective_context_window: Some(crate::route_budget::route_context_window_tokens(
            effective_provider,
            &effective_model,
            active_route_limits,
        )),
        token_threshold: crate::route_budget::compaction_threshold_for_route_at_percent(
            effective_provider,
            &effective_model,
            active_route_limits,
            settings.auto_compact_threshold_percent,
        ),
        ..Default::default()
    };

    let network_policy = execution_config.network.clone().map(|toml_cfg| {
        crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
    });

    let lsp_config = execution_config
        .lsp
        .clone()
        .map(crate::config::LspConfigToml::into_runtime);
    let engine_config = EngineConfig {
        model: effective_model.clone(),
        active_route_limits,
        workspace: workspace.clone(),
        plugin_registry: Some(plugin_registry),
        allow_shell: auto_approve || execution_config.allow_shell(),
        trust_mode,
        notes_path: execution_config.notes_path(),
        mcp_config_path: execution_config.mcp_config_path(),
        skills_dir: execution_config.skills_dir(),
        skills_scan_codewhale_only: execution_config.skills_config().scan_codewhale_only(),
        instructions: {
            let mut instrs: Vec<crate::prompts::InstructionSource> = execution_config
                .instructions_paths()
                .into_iter()
                .map(Into::into)
                .collect();
            if let Some(ref extra) = append_system_prompt {
                instrs.push(crate::prompts::InstructionSource::Inline {
                    name: "cli:append-system-prompt".into(),
                    content: extra.clone(),
                });
            }
            instrs
        },
        project_context_pack_enabled: execution_config.project_context_pack_enabled(),
        translation_enabled: false,
        show_thinking: settings.show_thinking,
        max_steps: max_turns,
        max_subagents,
        max_admitted_subagents: execution_config
            .max_admitted_subagents_for_provider(effective_provider)
            .max(max_subagents),
        launch_concurrency: execution_config.launch_concurrency_for_provider(effective_provider),
        subagents_enabled: execution_config.subagents_enabled_for_provider(effective_provider),
        features: execution_config.features(),
        auto_review_policy: execution_config.auto_review_policy(),
        compaction: compaction.clone(),
        todos: new_shared_todo_list(),
        plan_state: new_shared_plan_state(),
        goal_state: crate::tools::goal::new_shared_goal_state(),
        max_spawn_depth: execution_config.subagent_max_spawn_depth_for_provider(effective_provider),
        subagent_token_budget: execution_config
            .subagent_token_budget_for_provider(effective_provider),
        network_policy,
        snapshots_enabled: execution_config.snapshots_config().enabled,
        snapshots_max_workspace_bytes: execution_config
            .snapshots_config()
            .max_workspace_gb
            .saturating_mul(1024 * 1024 * 1024),
        lsp_config,
        runtime_services: crate::tools::spec::RuntimeToolServices::default(),
        subagent_model_overrides: execution_config.subagent_model_overrides(),
        fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::load(
            &execution_config.fleet_config(),
            &workspace,
        )),
        subagent_api_timeout: std::time::Duration::from_secs(
            execution_config.subagent_api_timeout_secs_for_provider(effective_provider),
        ),
        stream_chunk_timeout: std::time::Duration::from_secs(
            execution_config.stream_chunk_timeout_secs(),
        ),
        subagent_heartbeat_timeout: std::time::Duration::from_secs(
            execution_config.subagent_heartbeat_timeout_secs_for_provider(effective_provider),
        ),
        prefer_bwrap: execution_config.prefer_bwrap.unwrap_or(false),
        memory_enabled: execution_config.memory_enabled(),
        moraine_fallback: execution_config.moraine_fallback(),
        memory_path: execution_config.memory_path(),
        speech_output_dir: execution_config.speech_output_dir(),
        vision_config: execution_config.vision_model_config(),
        strict_tool_mode: execution_config.strict_tool_mode.unwrap_or(false),
        goal_objective: None,
        goal_token_budget: None,
        goal_status: crate::tools::goal::GoalStatus::Active,
        allowed_tools: allowed_tools.clone(),
        disallowed_tools: disallowed_tools.clone(),
        hook_executor: None,
        locale_tag: crate::localization::resolve_locale(&settings.locale)
            .tag()
            .to_string(),
        workshop: config.workshop.clone(),
        search_provider: execution_config.search_provider(),
        search_api_key: execution_config
            .search
            .as_ref()
            .and_then(|s| s.api_key.clone()),
        search_base_url: execution_config
            .search
            .as_ref()
            .and_then(|s| s.base_url.clone()),
        tools_always_load: execution_config.tools_always_load(),
        tools: execution_config.tools.clone(),
        verbosity: execution_config.verbosity.clone(),
        workspace_follow_symlinks: settings.workspace_follow_symlinks,
        exec_policy_engine: execution_config.exec_policy_engine.clone(),
        terminal_chrome_enabled: false,
    };

    let engine_handle = spawn_engine(engine_config, &execution_config);
    let mode = if auto_approve {
        AppMode::Yolo
    } else {
        AppMode::Agent
    };

    let resuming_session = resume_session.is_some();
    let mut loaded_session_id = None;
    if let Some(saved) = resume_session {
        let saved_id = saved.metadata.id.clone();
        if saved.metadata.workspace != workspace && output_format == ExecOutputFormat::Text {
            eprintln!(
                "Warning: session {} was created in a different workspace ({}). Resuming anyway.",
                truncate_id(&saved_id),
                saved.metadata.workspace.display(),
            );
        }

        engine_handle
            .send(Op::SyncSession {
                session_id: Some(saved_id.clone()),
                messages: saved.messages,
                system_prompt: saved.system_prompt.map(SystemPrompt::Text),
                system_prompt_override: false,
                model: saved.metadata.model,
                workspace: saved.metadata.workspace,
                mode,
            })
            .await?;
        loaded_session_id = Some(saved_id.clone());
        if output_format == ExecOutputFormat::Text && !json_output {
            eprintln!("{}", exec_resumed_session_line(&saved_id));
        }
    }

    engine_handle
        .send(Op::SendMessage {
            content: prompt.to_string(),
            mode,
            route: Box::new(validated_route.into_resolved()),
            compaction: Box::new(compaction.clone()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            allowed_tools: allowed_tools.clone(),
            dynamic_tools: Vec::new(),
            hook_executor: None,
            reasoning_effort: effective_reasoning_effort,
            reasoning_effort_auto: auto_model,
            auto_model,
            allow_shell: auto_approve || execution_config.allow_shell(),
            trust_mode,
            auto_approve,
            translation_enabled: false,
            show_thinking: settings.show_thinking,
            approval_mode: if auto_approve {
                crate::tui::approval::ApprovalMode::Bypass
            } else {
                execution_config
                    .approval_policy
                    .as_deref()
                    .and_then(crate::tui::approval::ApprovalMode::from_config_value)
                    .unwrap_or_default()
            },
            verbosity: execution_config.verbosity.clone(),
            provenance: crate::core::ops::UserInputProvenance::ExternalUser,
        })
        .await?;

    let mut summary = ExecSummary {
        mode: "agent".to_string(),
        provider: effective_provider_name.clone(),
        model: effective_model.clone(),
        prompt: prompt.to_string(),
        ..ExecSummary::default()
    };
    let can_elevate_sandbox =
        exec_sandbox_elevation_authorized(allow_sandbox_elevation, explicit_sandbox);
    let mut sandbox_denied = false;
    let mut approval_required = false;
    let mut tool_error_seen = false;
    let mut last_error_category = None;
    let mut reported_sandbox_contract = false;

    let should_persist_session = resuming_session || output_format == ExecOutputFormat::StreamJson;
    let mut latest_session_id = loaded_session_id;
    let mut latest_messages: Vec<Message> = Vec::new();
    let mut latest_system_prompt: Option<SystemPrompt> = None;
    let mut latest_model = effective_model;
    let mut latest_workspace = workspace.clone();
    let mut tool_starts: HashMap<String, (Instant, String)> = HashMap::new();

    let mut stdout = io::stdout();
    let mut ends_with_newline = false;
    loop {
        let event = {
            let mut rx = engine_handle.rx_event.write().await;
            rx.recv().await
        };

        let Some(event) = event else {
            break;
        };

        match event {
            Event::MessageDelta { content, .. } => {
                summary.output.push_str(&content);
                if output_format == ExecOutputFormat::StreamJson {
                    emit_exec_stream_event(&ExecStreamEvent::Content { content })?;
                } else if !json_output {
                    print!("{content}");
                    stdout.flush()?;
                }
                ends_with_newline = summary.output.ends_with('\n');
            }
            Event::MessageComplete { .. }
                if output_format == ExecOutputFormat::Text
                    && !json_output
                    && !ends_with_newline =>
            {
                println!();
            }
            Event::ThinkingDelta { .. } => {
                // Exec stream-json intentionally omits reasoning deltas; the
                // TUI transcript retains its existing Activity Detail surface.
            }
            Event::ToolCallStarted { id, name, input } => {
                let started_at = chrono::Utc::now().to_rfc3339();
                tool_starts.insert(id.clone(), (Instant::now(), started_at.clone()));
                if output_format == ExecOutputFormat::StreamJson {
                    emit_exec_stream_event(&ExecStreamEvent::ToolUse {
                        name,
                        id,
                        input,
                        started_at,
                    })?;
                } else if !json_output {
                    let summary = summarize_tool_args(&input);
                    if let Some(summary) = summary {
                        eprintln!("tool: {name} ({summary})");
                    } else {
                        eprintln!("tool: {name}");
                    }
                }
            }
            Event::ToolCallComplete {
                id, name, result, ..
            } => {
                let (duration_ms, started_at) = tool_starts
                    .remove(&id)
                    .map(|(started, timestamp)| {
                        (
                            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                            timestamp,
                        )
                    })
                    .unwrap_or_else(|| (0, chrono::Utc::now().to_rfc3339()));
                let receipt_name = name.clone();
                match result {
                    Ok(output) => {
                        tool_error_seen |= !output.success;
                        summary.tools.push(ExecToolEntry {
                            name: name.clone(),
                            success: output.success,
                            output: output.content.clone(),
                        });
                        if output_format == ExecOutputFormat::StreamJson {
                            emit_exec_stream_event(&ExecStreamEvent::ToolResult {
                                id,
                                name: receipt_name,
                                output: output.content,
                                status: if output.success {
                                    "success".to_string()
                                } else {
                                    "error".to_string()
                                },
                                started_at,
                                completed_at: chrono::Utc::now().to_rfc3339(),
                                duration_ms,
                                side_effect_status: output
                                    .metadata
                                    .as_ref()
                                    .and_then(|metadata| metadata.get("side_effect_status"))
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("unknown")
                                    .to_string(),
                                error_category: (!output.success).then(|| {
                                    output
                                        .metadata
                                        .as_ref()
                                        .and_then(|metadata| metadata.get("error_category"))
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("tool_reported_failure")
                                        .to_string()
                                }),
                                truncated: output
                                    .metadata
                                    .as_ref()
                                    .and_then(|metadata| metadata.get("truncated"))
                                    .and_then(serde_json::Value::as_bool),
                                artifact: tool_artifact_receipt(output.metadata.as_ref()),
                                result_metadata: output.metadata,
                            })?;
                        } else if !json_output {
                            if name == "exec_shell" && !output.content.trim().is_empty() {
                                eprintln!("tool {name} completed");
                                eprintln!(
                                    "--- stdout/stderr ---\n{}\n---------------------",
                                    output.content
                                );
                            } else {
                                eprintln!(
                                    "tool {name} completed: {}",
                                    summarize_tool_output(&output.content)
                                );
                            }
                        }
                    }
                    Err(err) => {
                        tool_error_seen = true;
                        let error_text = err.to_string();
                        summary.tools.push(ExecToolEntry {
                            name: name.clone(),
                            success: false,
                            output: error_text.clone(),
                        });
                        if output_format == ExecOutputFormat::StreamJson {
                            emit_exec_stream_event(&ExecStreamEvent::ToolResult {
                                id,
                                name: receipt_name,
                                output: error_text,
                                status: "error".to_string(),
                                started_at,
                                completed_at: chrono::Utc::now().to_rfc3339(),
                                duration_ms,
                                side_effect_status: "not_started_or_unknown".to_string(),
                                error_category: Some(tool_error_receipt_category(&err).to_string()),
                                truncated: None,
                                artifact: None,
                                result_metadata: None,
                            })?;
                        } else if !json_output {
                            eprintln!("tool {name} failed: {err}");
                        }
                    }
                }
            }
            Event::AgentSpawned { id, prompt, .. }
                if output_format == ExecOutputFormat::Text && !json_output =>
            {
                eprintln!("sub-agent {id} spawned: {}", summarize_tool_output(&prompt));
            }
            Event::AgentProgress { id, status, .. }
                if output_format == ExecOutputFormat::Text && !json_output =>
            {
                eprintln!("sub-agent {id}: {status}");
            }
            Event::AgentComplete { id, result }
                if output_format == ExecOutputFormat::Text && !json_output =>
            {
                eprintln!(
                    "sub-agent {id} completed: {}",
                    summarize_tool_output(&result)
                );
            }
            Event::AgentSpawned { .. }
            | Event::AgentProgress { .. }
            | Event::AgentComplete { .. } => {}
            Event::WorkflowUi { run_id, event }
                if output_format == ExecOutputFormat::StreamJson =>
            {
                emit_exec_stream_event(&ExecStreamEvent::WorkflowEvent { run_id, event })?;
            }
            Event::ApprovalRequired { id, .. } => {
                if auto_approve {
                    let _ = engine_handle.approve_tool_call(id).await;
                } else {
                    approval_required = true;
                    let _ = engine_handle.deny_tool_call(id).await;
                }
            }
            Event::ElevationRequired {
                tool_id,
                tool_name,
                denial_reason,
                ..
            } => {
                if can_elevate_sandbox {
                    let policy = crate::sandbox::SandboxPolicy::DangerFullAccess;
                    let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                } else {
                    sandbox_denied = true;
                    approval_required = true;
                    summary.outcomes.push(ExecOutcome {
                        kind: "sandbox_denied".to_string(),
                        outcome: "approval_required".to_string(),
                        tool_name: tool_name.clone(),
                        reason: denial_reason.clone(),
                    });
                    if !reported_sandbox_contract {
                        eprintln!(
                            "sandbox denied {tool_name}: {denial_reason}; --auto approves tools but does not elevate sandbox access — use --sandbox danger-full-access or --allow-sandbox-elevation to opt in"
                        );
                        reported_sandbox_contract = true;
                    }
                    if output_format == ExecOutputFormat::StreamJson {
                        emit_exec_stream_event(&ExecStreamEvent::SandboxDenied {
                            tool_id: tool_id.clone(),
                            tool_name,
                            reason: denial_reason,
                            outcome: "approval_required".to_string(),
                        })?;
                    }
                    let _ = engine_handle.deny_tool_call(tool_id).await;
                }
            }
            Event::Error {
                envelope,
                recoverable: _,
            } => {
                last_error_category = Some(envelope.category);
                summary.error_category = Some(envelope.category.to_string());
                summary.error = Some(envelope.message.clone());
                if output_format == ExecOutputFormat::StreamJson {
                    emit_exec_stream_event(&ExecStreamEvent::Error {
                        error: envelope.message,
                    })?;
                } else if !json_output {
                    eprintln!("error: {}", envelope.message);
                }
            }
            Event::TurnComplete {
                status,
                error,
                usage,
                tool_catalog,
                ..
            } => {
                summary.status = Some(format!("{status:?}").to_lowercase());
                if error.is_some() {
                    summary.error = error;
                }
                if sandbox_denied
                    && summary.error.is_none()
                    && matches!(status, crate::core::events::TurnOutcomeStatus::Failed)
                {
                    summary.error = Some(
                        "exec turn failed after sandbox denial; explicit sandbox elevation was not authorized"
                            .to_string(),
                    );
                }
                if last_error_category.is_none() {
                    last_error_category = summary
                        .error
                        .as_deref()
                        .map(crate::error_taxonomy::classify_error_message);
                    summary.error_category =
                        last_error_category.map(|category| category.to_string());
                }
                let termination_reason = crate::core::termination::classify_turn_termination(
                    status,
                    last_error_category,
                    tool_error_seen,
                    approval_required,
                );
                summary.termination_reason = Some(termination_reason.as_str().to_string());
                let saved_session_id = if should_persist_session && !latest_messages.is_empty() {
                    match persist_exec_session(
                        &latest_messages,
                        &latest_model,
                        PersistedProviderRoute {
                            kind: effective_provider.as_str(),
                            id: effective_provider_id.as_deref(),
                        },
                        &latest_workspace,
                        &latest_system_prompt,
                        latest_session_id.as_deref(),
                        u64::from(usage.input_tokens) + u64::from(usage.output_tokens),
                    ) {
                        Ok(id) => {
                            if output_format == ExecOutputFormat::Text && !json_output {
                                eprintln!("{}", exec_saved_session_line(&id));
                            }
                            Some(id)
                        }
                        Err(err) => {
                            if output_format == ExecOutputFormat::Text && !json_output {
                                eprintln!("warning: failed to save exec session: {err}");
                            }
                            latest_session_id.clone()
                        }
                    }
                } else {
                    latest_session_id.clone()
                };

                if output_format == ExecOutputFormat::StreamJson {
                    if let Some(id) = saved_session_id.as_ref() {
                        emit_exec_stream_event(&ExecStreamEvent::SessionCapture {
                            content: exec_stream_session_ref(id),
                        })?;
                    }
                    emit_exec_stream_event(&ExecStreamEvent::Metadata {
                        meta: Box::new(ExecStreamMeta {
                            receipt_kind: "terminal",
                            provider: effective_provider_kind.clone(),
                            provider_id: effective_stream_provider_id.clone(),
                            model: latest_model.clone(),
                            route_source: route_source.clone(),
                            input_tokens: Some(usage.input_tokens),
                            output_tokens: Some(usage.output_tokens),
                            prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
                            prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
                            prompt_cache_write_tokens: usage.prompt_cache_write_tokens,
                            reasoning_tokens: usage.reasoning_tokens,
                            duration_ms: u64::try_from(exec_started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                            retry_count: None,
                            approval_posture: approval_posture.clone(),
                            sandbox_posture: sandbox_posture.clone(),
                            binary_sha256: binary_sha256.clone(),
                            config_sha256: None,
                            prompt_sha256: prompt_sha256.clone(),
                            tool_catalog_sha256: tool_catalog.as_ref().and_then(|catalog| {
                                serde_json::to_vec(catalog).ok().map(|bytes| {
                                    format!("sha256:{}", crate::hashing::sha256_hex(&bytes))
                                })
                            }),
                            input_analysis: exec_stream_input_analysis(
                                &latest_messages,
                                latest_system_prompt.as_ref(),
                            ),
                            visible_final_answer_chars: summary.output.chars().count(),
                            resume_command: saved_session_id
                                .as_deref()
                                .map(exec_stream_resume_hint)
                                .unwrap_or_default(),
                            session_id: saved_session_id
                                .as_deref()
                                .map(exec_stream_session_ref)
                                .unwrap_or_default(),
                            workspace: latest_workspace.display().to_string(),
                            message_count: latest_messages.len(),
                            status: summary.status.clone(),
                            termination_reason: summary.termination_reason.clone(),
                            error_category: summary.error_category.clone(),
                        }),
                    })?;
                    emit_exec_stream_event(&ExecStreamEvent::Done)?;
                }
                let _ = engine_handle.send(Op::Shutdown).await;
                break;
            }
            Event::SessionUpdated {
                session_id,
                messages,
                system_prompt,
                model,
                workspace,
            } => {
                latest_session_id = Some(session_id);
                latest_messages = messages;
                latest_system_prompt = system_prompt;
                latest_model = model;
                latest_workspace = workspace;
            }
            // #3027: surface the engine's max-steps notice in text mode so a
            // --max-turns run that stops early says why instead of going quiet.
            Event::Status { message }
                if output_format == ExecOutputFormat::Text
                    && !json_output
                    && message.contains("Reached maximum steps") =>
            {
                eprintln!("{message}");
            }
            _ => {}
        }
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }

    if let Some(error) = summary.error.as_ref()
        && !error.trim().is_empty()
    {
        bail!("exec turn failed: {error}");
    }

    if matches!(
        summary.status.as_deref(),
        Some("failed" | "canceled" | "interrupted")
    ) {
        let status = summary.status.as_deref().unwrap_or("unknown");
        bail!("exec turn ended with status {status}");
    }

    Ok(())
}

#[cfg(test)]
mod serve_bind_host_tests {
    use super::*;

    #[test]
    fn http_defaults_to_loopback() {
        assert_eq!(
            resolve_serve_bind_host(false, None),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
                mobile_rebound_to_lan: false,
            }
        );
    }

    #[test]
    fn mobile_default_rebinds_to_lan_with_warning_flag() {
        assert_eq!(
            resolve_serve_bind_host(true, None),
            ServeBindHost {
                host: "0.0.0.0".to_string(),
                mobile_rebound_to_lan: true,
            }
        );
    }

    #[test]
    fn mobile_respects_explicit_loopback_host() {
        assert_eq!(
            resolve_serve_bind_host(true, Some("127.0.0.1".to_string())),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
                mobile_rebound_to_lan: false,
            }
        );
    }

    #[test]
    fn http_and_mobile_are_mutually_exclusive() {
        let err = validate_serve_mode_selection(false, true, true, false, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("--http and --mobile are mutually exclusive")
        );
    }

    #[test]
    fn web_is_a_distinct_loopback_runtime_mode() {
        assert!(validate_serve_mode_selection(false, false, false, true, false).unwrap());
        let err = validate_serve_mode_selection(false, true, false, true, false).unwrap_err();
        assert!(err.to_string().contains("--web is mutually exclusive"));
        assert_eq!(
            resolve_serve_bind_host(false, None),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
                mobile_rebound_to_lan: false,
            }
        );
    }
}

#[cfg(test)]
mod doctor_legacy_state_tests {
    use super::*;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use tempfile::TempDir;

    struct EnvVarRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarRestore {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = env::var_os(key);
            unsafe {
                env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    fn roots(tmp: &TempDir) -> (PathBuf, PathBuf) {
        (tmp.path().join(".codewhale"), tmp.path().join(".deepseek"))
    }

    fn entry<'a>(report: &'a [DoctorLegacyStateEntry], name: &str) -> &'a DoctorLegacyStateEntry {
        report
            .iter()
            .find(|entry| entry.name == name)
            .expect("legacy state entry should exist")
    }

    #[test]
    fn doctor_legacy_state_report_marks_unmigrated_legacy_entries() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        fs::create_dir_all(legacy_root.join("tasks")).expect("legacy tasks");
        fs::create_dir_all(&primary_root).expect("primary root");
        fs::write(legacy_root.join("config.toml"), "api_key = 'old'").expect("legacy config");

        let report = doctor_legacy_state_report(&primary_root, &legacy_root);
        let session_recovery = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(
            entry(&report, "sessions").status,
            DoctorLegacyStateStatus::LegacyOnly
        );
        assert_eq!(
            entry(&report, "config.toml").status,
            DoctorLegacyStateStatus::LegacyOnly
        );
        assert_eq!(
            entry(&report, "skills").status,
            DoctorLegacyStateStatus::Absent
        );

        let json =
            doctor_legacy_state_json(&primary_root, &legacy_root, &report, &session_recovery);
        assert_eq!(json["needs_attention"], true);
        assert_eq!(json["legacy_only_count"], 3);
        assert_eq!(json["dual_present_count"], 0);
    }

    #[test]
    fn doctor_legacy_state_report_marks_dual_present_entries() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(primary_root.join("sessions")).expect("primary sessions");
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        fs::write(primary_root.join("mcp.json"), "{}").expect("primary mcp");
        fs::write(legacy_root.join("mcp.json"), "{}").expect("legacy mcp");

        let report = doctor_legacy_state_report(&primary_root, &legacy_root);
        let session_recovery = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(
            entry(&report, "sessions").status,
            DoctorLegacyStateStatus::Both
        );
        assert_eq!(
            entry(&report, "mcp.json").status,
            DoctorLegacyStateStatus::Both
        );

        let json =
            doctor_legacy_state_json(&primary_root, &legacy_root, &report, &session_recovery);
        assert_eq!(json["needs_attention"], true);
        assert_eq!(json["legacy_only_count"], 0);
        assert_eq!(json["dual_present_count"], 2);
    }

    #[test]
    fn doctor_legacy_state_report_is_clear_when_only_primary_exists() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(primary_root.join("sessions")).expect("primary sessions");
        fs::write(primary_root.join("settings.toml"), "default_mode = 'ask'")
            .expect("primary settings");

        let report = doctor_legacy_state_report(&primary_root, &legacy_root);
        let session_recovery = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(
            entry(&report, "sessions").status,
            DoctorLegacyStateStatus::PrimaryOnly
        );
        assert!(!report.iter().any(legacy_state_needs_attention));

        let json =
            doctor_legacy_state_json(&primary_root, &legacy_root, &report, &session_recovery);
        assert_eq!(json["needs_attention"], false);
        assert_eq!(json["legacy_only_count"], 0);
        assert_eq!(json["dual_present_count"], 0);
    }

    #[test]
    fn doctor_legacy_state_report_is_clear_when_neither_root_exists() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);

        let report = doctor_legacy_state_report(&primary_root, &legacy_root);
        let session_recovery = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert!(
            report
                .iter()
                .all(|entry| entry.status == DoctorLegacyStateStatus::Absent)
        );
        assert!(!report.iter().any(legacy_state_needs_attention));

        let json =
            doctor_legacy_state_json(&primary_root, &legacy_root, &report, &session_recovery);
        assert_eq!(json["needs_attention"], false);
        assert_eq!(json["legacy_only_count"], 0);
        assert_eq!(json["dual_present_count"], 0);
    }

    #[test]
    fn doctor_reports_incomplete_session_migration_without_mutating_files() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        let primary_sessions = primary_root.join("sessions");
        let legacy_sessions = legacy_root.join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(legacy_sessions.join("checkpoints")).expect("legacy checkpoints");
        fs::write(primary_sessions.join("already-there.json"), b"primary")
            .expect("primary session");
        fs::write(legacy_sessions.join("already-there.json"), b"legacy")
            .expect("legacy matching session");
        fs::write(
            legacy_sessions.join("recover-me.json"),
            b"not parsed by doctor",
        )
        .expect("legacy recoverable session");
        fs::write(
            legacy_sessions.join("checkpoints").join("latest.json"),
            b"checkpoint not inspected",
        )
        .expect("legacy checkpoint");

        let legacy_before = fs::read(legacy_sessions.join("recover-me.json"))
            .expect("read legacy fixture before diagnostic");
        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(
            report.status,
            DoctorSessionRecoveryStatus::MigrationIncomplete
        );
        assert_eq!(report.legacy_session_file_count, 2);
        assert_eq!(report.already_present_file_count, 1);
        assert_eq!(report.recoverable_file_count, 1);
        assert_eq!(report.recoverable.len(), 1);
        assert_eq!(report.recoverable[0].name, PathBuf::from("recover-me.json"));
        assert!(
            !primary_sessions.join("recover-me.json").exists(),
            "doctor must not copy a recoverable session"
        );
        assert_eq!(
            fs::read(legacy_sessions.join("recover-me.json"))
                .expect("legacy file remains after diagnostic"),
            legacy_before,
            "doctor must not rewrite or delete the legacy source"
        );

        let json = doctor_session_recovery_json(&report);
        assert_eq!(json["needs_attention"], true);
        assert_eq!(json["read_only"], true);
        assert_eq!(json["chat_contents_read"], false);
        assert_eq!(json["checkpoint_internals_scanned"], false);
        assert_eq!(json["recoverable_file_count"], 1);
        assert_eq!(json["recovery_command"], "codewhale sessions");
        assert_eq!(json["recoverable_files"][0]["name"], "recover-me.json");
        let serialized = json.to_string();
        assert!(
            !serialized.contains("not parsed by doctor"),
            "the report must not expose session contents"
        );
        assert!(
            !serialized.contains("checkpoint not inspected"),
            "the report must not expose checkpoint contents"
        );
    }

    #[test]
    fn doctor_treats_preserved_legacy_sessions_as_complete_by_filename() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        let primary_sessions = primary_root.join("sessions");
        let legacy_sessions = legacy_root.join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::write(primary_sessions.join("same-name.json"), b"primary").expect("primary session");
        fs::write(legacy_sessions.join("same-name.json"), b"legacy").expect("legacy session");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(
            report.status,
            DoctorSessionRecoveryStatus::MigrationComplete
        );
        assert!(!report.needs_attention());
        assert_eq!(report.recoverable_file_count, 0);
        assert!(report.recoverable.is_empty());
        assert_eq!(report.already_present_file_count, 1);
        let json = doctor_session_recovery_json(&report);
        assert_eq!(json["session_descriptors_compared"], false);
        assert_eq!(
            json["counterpart_check"],
            "top_level_filename_and_regular_file_only"
        );
    }

    #[test]
    fn doctor_bounds_recoverable_session_filename_samples() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        let legacy_sessions = legacy_root.join("sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        for index in 0..DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT {
            fs::write(
                legacy_sessions.join(format!("late-{index:03}.json")),
                b"fixture",
            )
            .expect("legacy session fixture");
        }
        fs::write(legacy_sessions.join("early-000.json"), b"fixture")
            .expect("earliest legacy session fixture");
        fs::write(legacy_sessions.join("early-001.json"), b"fixture")
            .expect("second earliest legacy session fixture");
        let total = DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT + 2;

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);
        let json = doctor_session_recovery_json(&report);

        assert_eq!(report.recoverable_file_count, total);
        assert_eq!(
            report.recoverable.len(),
            DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT
        );
        assert_eq!(
            json["recoverable_files"].as_array().map(Vec::len),
            Some(DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT)
        );
        assert_eq!(
            report.recoverable.first().map(|entry| entry.name.as_path()),
            Some(Path::new("early-000.json")),
            "the bounded sample must not depend on read_dir order"
        );
        assert_eq!(
            report.recoverable.last().map(|entry| entry.name.as_path()),
            Some(Path::new("late-097.json")),
            "the bounded sample must retain the lexical prefix"
        );
        assert_eq!(json["recoverable_files_truncated"], true);
    }

    #[test]
    fn doctor_session_recovery_fails_closed_on_an_unreadable_path_shape() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(&legacy_root).expect("legacy root");
        fs::write(legacy_root.join("sessions"), b"not a directory")
            .expect("invalid legacy sessions path");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(report.needs_attention());
        assert!(report.error.as_deref().is_some_and(|error| {
            error.contains("legacy sessions root") && error.contains("not a directory")
        }));
    }

    #[test]
    fn doctor_session_recovery_rejects_a_non_directory_legacy_state_root() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::write(&legacy_root, b"not a state directory").expect("invalid legacy root");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(report.error.as_deref().is_some_and(|error| {
            error.contains("legacy state root") && error.contains("not a directory")
        }));
    }

    #[test]
    fn doctor_session_recovery_rejects_a_non_directory_primary_state_root() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        fs::write(&primary_root, b"not a state directory").expect("invalid primary root");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(report.error.as_deref().is_some_and(|error| {
            error.contains("primary state root") && error.contains("not a directory")
        }));
    }

    #[test]
    fn doctor_session_recovery_rejects_a_non_directory_primary_sessions_root() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        fs::create_dir_all(&primary_root).expect("primary root");
        fs::write(primary_root.join("sessions"), b"not a sessions directory")
            .expect("invalid primary sessions path");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(report.error.as_deref().is_some_and(|error| {
            error.contains("primary sessions root") && error.contains("not a directory")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn doctor_session_recovery_rejects_a_symlinked_legacy_sessions_root() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        let external_sessions = tmp.path().join("external-sessions");
        fs::create_dir_all(&external_sessions).expect("external sessions");
        fs::write(
            external_sessions.join("must-not-be-enumerated.json"),
            b"session contents must stay unread",
        )
        .expect("external session fixture");
        fs::create_dir_all(&legacy_root).expect("legacy root");
        symlink(&external_sessions, legacy_root.join("sessions"))
            .expect("symlinked legacy sessions root");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, false);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(report.needs_attention());
        assert_eq!(report.legacy_session_file_count, 0);
        assert!(report.recoverable.is_empty());
        assert!(
            report
                .error
                .as_deref()
                .is_some_and(|error| error.contains("legacy sessions root")
                    && error.contains("path is a symlink"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn doctor_session_recovery_rejects_symlinked_primary_root_and_sessions_root() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        let external_primary = tmp.path().join("external-primary");
        fs::create_dir_all(external_primary.join("sessions")).expect("external primary");
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        symlink(&external_primary, &primary_root).expect("symlinked primary root");

        let root_report = doctor_session_recovery_report(&primary_root, &legacy_root, false);
        assert_eq!(root_report.status, DoctorSessionRecoveryStatus::ScanFailed);
        assert!(root_report.error.as_deref().is_some_and(|error| {
            error.contains("primary state root") && error.contains("path is a symlink")
        }));

        fs::remove_file(&primary_root).expect("remove primary root symlink");
        fs::create_dir_all(&primary_root).expect("primary root");
        symlink(&external_primary, primary_root.join("sessions"))
            .expect("symlinked primary sessions root");

        let sessions_report = doctor_session_recovery_report(&primary_root, &legacy_root, false);
        assert_eq!(
            sessions_report.status,
            DoctorSessionRecoveryStatus::ScanFailed
        );
        assert!(sessions_report.error.as_deref().is_some_and(|error| {
            error.contains("primary sessions root") && error.contains("path is a symlink")
        }));
    }

    #[test]
    fn explicit_codewhale_home_skips_session_recovery_scan() {
        let tmp = TempDir::new().expect("tempdir");
        let (primary_root, legacy_root) = roots(&tmp);
        fs::create_dir_all(legacy_root.join("sessions")).expect("legacy sessions");
        fs::write(legacy_root.join("sessions").join("ambient.json"), b"legacy")
            .expect("legacy session");

        let report = doctor_session_recovery_report(&primary_root, &legacy_root, true);

        assert_eq!(report.status, DoctorSessionRecoveryStatus::Isolated);
        assert!(report.codewhale_home_is_explicit);
        assert_eq!(report.legacy_session_file_count, 0);
        assert_eq!(report.recoverable_file_count, 0);
        assert!(report.recoverable.is_empty());
        assert!(!report.needs_attention());
    }

    #[test]
    fn doctor_state_roots_ignore_ambient_legacy_home_when_codewhale_home_is_explicit() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let explicit_home = tmp.path().join("isolated-codewhale");
        let ambient_legacy = tmp.path().join(".deepseek");
        fs::create_dir_all(&ambient_legacy).expect("ambient legacy root");
        fs::write(
            ambient_legacy.join("config.toml"),
            "provider = 'deepseek'\n",
        )
        .expect("ambient legacy config");
        let _home = EnvVarRestore::set("HOME", tmp.path());
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", &explicit_home);

        let (primary_root, legacy_root) = doctor_state_roots();
        let report = doctor_legacy_state_report(&primary_root, &legacy_root);
        let session_recovery = doctor_session_recovery_report(
            &primary_root,
            &legacy_root,
            codewhale_config::codewhale_home_is_explicit(),
        );

        assert_eq!(primary_root, explicit_home);
        assert_eq!(
            legacy_root,
            primary_root.join(codewhale_config::LEGACY_APP_DIR)
        );
        assert!(
            report
                .iter()
                .all(|entry| entry.status == DoctorLegacyStateStatus::Absent),
            "doctor must not report ambient legacy state when CODEWHALE_HOME is explicit"
        );
        assert!(!report.iter().any(legacy_state_needs_attention));
        assert_eq!(
            session_recovery.status,
            DoctorSessionRecoveryStatus::Isolated
        );
        assert!(session_recovery.recoverable.is_empty());
    }
}

#[cfg(test)]
mod doctor_setup_state_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn prepare_env(tmp: &TempDir) -> (crate::test_support::EnvVarGuard, PathBuf) {
        let codewhale_home = tmp.path().join(".codewhale");
        fs::create_dir_all(&codewhale_home).expect("codewhale home");
        (
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str()),
            codewhale_home,
        )
    }

    fn provider_step(report: &serde_json::Value) -> &serde_json::Value {
        report["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .find(|step| step["step"] == "provider_model")
            .expect("provider/model step")
    }

    #[test]
    fn doctor_setup_consistency_flags_missing_user_constitution() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let state = codewhale_config::SetupState {
            constitution_source: codewhale_config::ConstitutionSource::UserGlobal,
            ..Default::default()
        };
        state.save().expect("persist setup state");

        let report = doctor_setup_report_json(&Config::default(), &workspace);

        assert_eq!(report["source"], "persisted");
        assert_eq!(report["consistency"]["status"], "inconsistent");
        let issues = report["consistency"]["issues"].to_string();
        assert!(
            issues.contains("setup_state_points_at_missing_user_constitution"),
            "{issues}"
        );
    }

    #[test]
    fn doctor_setup_consistency_flags_stale_temp_files() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, codewhale_home) = prepare_env(&tmp);
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(codewhale_home.join(".tmpAbC123"), b"orphaned atomic write")
            .expect("stale temp file");

        let report = doctor_setup_report_json(&Config::default(), &workspace);

        assert_eq!(report["consistency"]["status"], "inconsistent");
        let issues = report["consistency"]["issues"].to_string();
        assert!(
            issues.contains("stale_setup_temp_files_in_codewhale_home"),
            "{issues}"
        );
    }

    #[test]
    fn doctor_setup_consistency_reports_consistent_for_clean_home() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let report = doctor_setup_report_json(&Config::default(), &workspace);

        assert_eq!(report["consistency"]["status"], "consistent");
        assert_eq!(
            report["consistency"]["issues"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0
        );
    }

    #[test]
    fn doctor_setup_report_json_derives_state_without_sidecar() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let report = doctor_setup_report_json(&Config::default(), &workspace);

        assert_eq!(report["source"], "derived");
        assert_eq!(report["inherited"], true);
        assert_eq!(report["next_actions"]["constitution"], "/constitution");
        assert_eq!(report["next_actions"]["setup_report"], "/setup report");
        assert_eq!(
            report["next_actions"]["provider_model"],
            "/setup provider, /provider setup <name>, or /model"
        );
        assert_eq!(report["next_actions"]["runtime_posture"], "/config");
        assert_eq!(
            report["next_actions"]["operate_fleet"],
            "/setup fleet (readiness), /fleet setup (explicit profile authoring)"
        );
        assert_eq!(report["next_actions"]["hotbar"], "/setup hotbar");
        assert_eq!(report["next_actions"]["tools_mcp"], "/setup tools");
        assert_eq!(report["next_actions"]["remote_runtime"], "/setup remote");
        assert_eq!(report["next_actions"]["persistence"], "/setup persistence");
        assert_eq!(
            report["checkpoint_version"],
            crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION
        );
        assert_eq!(report["update_ready"], false);
        assert_eq!(report["operate_ready"], false);
        assert_eq!(
            report["operate_fleet"]["concurrency"]["plan_limit_probed"],
            false
        );
        assert_eq!(
            report["operate_fleet"]["roster"]["readiness_rule"],
            "built-in starter roster or custom roster"
        );
        assert_eq!(report["provider_model"]["provider"]["id"], "deepseek");
        assert_eq!(report["provider_model"]["provider"]["display"], "DeepSeek");
        assert_eq!(
            report["provider_model"]["model"]["resolved"],
            crate::config::DEFAULT_TEXT_MODEL
        );
        assert_eq!(report["provider_model"]["auth"]["source"], "missing");
        assert_eq!(
            report["provider_model"]["auth"]["credential_url"],
            "https://platform.deepseek.com/api_keys"
        );
        assert_eq!(
            report["provider_model"]["auth"]["credential_mode"],
            "api_key"
        );
        assert_eq!(
            report["provider_model"]["auth"]["env_vars"][0],
            "DEEPSEEK_API_KEY"
        );
        assert_eq!(report["provider_model"]["health"]["live_validation"], false);
        assert_eq!(report["constitution"]["source"], "bundled");
        assert_eq!(report["constitution"]["autonomy_preference"], "unspecified");
        assert_eq!(report["runtime_posture"]["source"], "unset");
        assert_eq!(report["runtime_posture"]["default_mode"]["value"], "agent");
        assert_eq!(
            report["runtime_posture"]["approval_policy"]["value"],
            "on-request"
        );
        assert_eq!(report["runtime_posture"]["allow_shell"]["value"], true);
        assert_eq!(
            report["runtime_posture"]["sandbox_mode"]["value"],
            "mode-derived"
        );
        assert_eq!(
            report["runtime_posture"]["network_default"]["value"],
            "prompt"
        );
        assert_eq!(provider_step(&report)["status"], "needs_action");
    }

    #[test]
    fn doctor_setup_provider_model_json_covers_cn_codex_and_local_matrix() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _codex_key = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _codex_legacy_key = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let codex_auth_path = tmp.path().join("external-codex-auth.json");
        let codex_auth_raw = serde_json::json!({
            "tokens": {
                "access_token": crate::test_support::future_test_jwt("doctor"),
                "account_id": "acct-doctor-read-only",
                "refresh_token": "must-never-be-used",
                "unknown": {"preserve": true}
            }
        })
        .to_string();
        fs::write(&codex_auth_path, &codex_auth_raw).expect("Codex auth trap fixture");
        let _codex_auth =
            crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &codex_auth_path);
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let cn_config = Config {
            provider: Some("deepseek-cn".to_string()),
            ..Config::default()
        };
        let cn_report = doctor_setup_report_json(&cn_config, &workspace);
        assert_eq!(cn_report["provider_model"]["provider"]["id"], "deepseek-cn");
        assert_eq!(
            cn_report["provider_model"]["provider"]["display"],
            "DeepSeek (legacy alias)"
        );
        assert_eq!(
            cn_report["provider_model"]["auth"]["env_vars"][0],
            "DEEPSEEK_API_KEY"
        );
        assert_eq!(
            cn_report["provider_model"]["auth"]["credential_url"],
            "https://platform.deepseek.com/api_keys"
        );
        assert_eq!(cn_report["provider_model"]["auth"]["oauth_only"], false);
        assert_eq!(
            cn_report["provider_model"]["health"]["live_validation"],
            false
        );

        let codex_config = Config {
            provider: Some("openai-codex".to_string()),
            ..Config::default()
        };
        crate::external_credentials::reset_side_effect_trap();
        let codex_report = doctor_setup_report_json(&codex_config, &workspace);
        assert_eq!(
            codex_report["provider_model"]["provider"]["id"],
            crate::config::ApiProvider::OpenaiCodex.as_str()
        );
        assert!(codex_report["provider_model"]["auth"]["credential_url"].is_null());
        assert_eq!(
            codex_report["provider_model"]["auth"]["credential_mode"],
            "oauth"
        );
        assert_eq!(codex_report["provider_model"]["auth"]["oauth_only"], true);
        assert_eq!(
            codex_report["provider_model"]["health"]["next_action"],
            "/setup provider or /provider setup <name>"
        );
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (0, 0),
            "doctor must not stat or read external credentials without consent"
        );

        let mut consent = codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            codex_auth_path.clone(),
        );
        let codex_read_only = Config {
            provider: Some("openai-codex".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai_codex: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(consent.clone()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let changed_ambient_path = tmp.path().join("new-ambient-codex-auth.json");
        let _changed_codex_auth =
            crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &changed_ambient_path);
        crate::external_credentials::reset_side_effect_trap();
        let codex_read_only_report = doctor_setup_report_json(&codex_read_only, &workspace);
        assert_eq!(
            codex_read_only_report["provider_model"]["auth"]["present_or_local"],
            false
        );
        assert_eq!(
            codex_read_only_report["provider_model"]["auth"]["source"],
            "external_consent"
        );
        let status_json = doctor_external_credential_consent_json(&codex_read_only);
        let codex_status = status_json
            .as_array()
            .and_then(|rows| rows.first())
            .expect("Codex structural status");
        assert_eq!(codex_status["access"], "read_only");
        assert_eq!(codex_status["provider"], "openai-codex");
        assert_eq!(codex_status["source"], "codex_cli");
        assert_eq!(codex_status["route_state"], "active");
        assert_eq!(codex_status["ambient_path_changed"], true);
        assert!(
            codex_status["ambient_path_warning"]
                .as_str()
                .is_some_and(|warning| warning.contains("remains pinned"))
        );
        assert_eq!(
            codex_status["revoke_command"],
            "codewhale auth external-revoke --provider openai-codex"
        );
        let human = doctor_external_credential_consent_lines(&codex_read_only).join("\n");
        assert!(human.contains("path="), "{human}");
        assert!(human.contains("version=1"), "{human}");
        assert!(human.contains("no refresh, identity-provider or discovery requests"));
        assert!(human.contains("normal requests to the explicitly selected provider"));
        assert!(human.contains("consent remains pinned"), "{human}");
        assert!(
            human.contains(&codewhale_config::quote_os_path(&codex_auth_path)),
            "{human}"
        );
        assert!(!human.contains(&changed_ambient_path.display().to_string()));
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 0, 0, 0),
            "doctor consent status is structural and must not inspect the file"
        );
        assert_eq!(
            fs::read_to_string(&codex_auth_path).expect("unchanged Codex auth fixture"),
            codex_auth_raw
        );

        consent.access = codewhale_config::ExternalCredentialAccess::Managed;
        let codex_managed = Config {
            provider: Some("openai-codex".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai_codex: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(consent),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        crate::external_credentials::reset_side_effect_trap();
        let codex_managed_report = doctor_setup_report_json(&codex_managed, &workspace);
        assert_eq!(
            codex_managed_report["provider_model"]["auth"]["present_or_local"],
            false
        );
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (0, 0),
            "unsupported managed mode must fail before external I/O"
        );
        assert_eq!(
            fs::read_to_string(&codex_auth_path).expect("unchanged managed auth fixture"),
            codex_auth_raw
        );

        let local_config = Config {
            provider: Some("ollama".to_string()),
            ..Config::default()
        };
        let local_report = doctor_setup_report_json(&local_config, &workspace);
        assert_eq!(local_report["provider_model"]["provider"]["id"], "ollama");
        assert_eq!(
            local_report["provider_model"]["auth"]["present_or_local"],
            true
        );
        assert!(local_report["provider_model"]["auth"]["credential_url"].is_null());
        assert_eq!(
            local_report["provider_model"]["auth"]["credential_mode"],
            "local_optional"
        );
        assert_eq!(local_report["provider_model"]["auth"]["oauth_only"], false);
        assert_eq!(
            local_report["provider_model"]["health"]["next_action"],
            "/model"
        );

        let kimi_config = Config {
            provider: Some("moonshot".to_string()),
            ..Config::default()
        };
        let kimi_report = doctor_setup_report_json(&kimi_config, &workspace);
        assert_eq!(
            kimi_report["provider_model"]["auth"]["credential_url"],
            "https://platform.kimi.ai/console/api-keys"
        );
        assert_eq!(
            kimi_report["provider_model"]["auth"]["credential_docs_url"],
            "https://platform.kimi.ai/docs/overview"
        );
        assert_eq!(
            kimi_report["provider_model"]["auth"]["credential_mode"],
            "api_key"
        );
        assert!(
            kimi_report["provider_model"]["auth"]["credential_guidance"]
                .as_str()
                .is_some_and(|guidance| guidance.contains("OAuth is not available"))
        );
    }

    #[test]
    fn doctor_setup_report_json_uses_persisted_state() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let mut state = codewhale_config::SetupState::default();
        state.set_step(
            codewhale_config::SetupStep::Language,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            ),
        );
        state.set_step(
            codewhale_config::SetupStep::ProviderModel,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            )
            .with_result("deepseek/deepseek-chat"),
        );
        state.set_step(
            codewhale_config::SetupStep::TrustSandbox,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            ),
        );
        state
            .complete_constitution_checkpoint(
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
                codewhale_config::ConstitutionChoice::Bundled,
            )
            .set_step(
                codewhale_config::SetupStep::Constitution,
                codewhale_config::StepEntry::new(
                    codewhale_config::StepStatus::Verified,
                    true,
                    crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
                ),
            );
        state.runtime_posture_source = codewhale_config::RuntimePostureSource::Confirmed;
        state.save().expect("persist setup state");
        codewhale_config::UserConstitution {
            autonomy_preference: codewhale_config::AutonomyPreference::Balanced,
            ..Default::default()
        }
        .save()
        .expect("persist user constitution");
        let config = Config {
            approval_policy: Some("never".to_string()),
            allow_shell: Some(false),
            sandbox_mode: Some("read-only".to_string()),
            network: Some(crate::config::NetworkPolicyToml {
                default: "deny".to_string(),
                ..Default::default()
            }),
            ..Config::default()
        };

        let report = doctor_setup_report_json(&config, &workspace);

        assert_eq!(report["source"], "persisted");
        assert_eq!(report["first_run_ready"], true);
        assert_eq!(report["update_ready"], true);
        assert_eq!(report["operate_ready"], false);
        assert_eq!(report["constitution"]["choice"], "bundled");
        assert_eq!(
            report["constitution"]["checkpoint_completed_for"],
            crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION
        );
        assert_eq!(report["constitution"]["autonomy_preference"], "balanced");
        assert_eq!(report["runtime_posture_source"], "confirmed");
        assert_eq!(report["runtime_posture"]["source"], "confirmed");
        assert_eq!(
            report["runtime_posture"]["approval_policy"]["value"],
            "never"
        );
        assert_eq!(
            report["runtime_posture"]["approval_policy"]["source"],
            "config"
        );
        assert_eq!(report["runtime_posture"]["allow_shell"]["value"], false);
        assert_eq!(report["runtime_posture"]["allow_shell"]["source"], "config");
        assert_eq!(
            report["runtime_posture"]["sandbox_mode"]["value"],
            "read-only"
        );
        assert_eq!(
            report["runtime_posture"]["sandbox_mode"]["source"],
            "config"
        );
        assert_eq!(
            report["runtime_posture"]["network_default"]["value"],
            "deny"
        );
        assert_eq!(
            report["runtime_posture"]["network_default"]["source"],
            "config"
        );
        assert_eq!(provider_step(&report)["result"], "deepseek/deepseek-chat");
    }

    #[test]
    fn doctor_setup_report_json_fails_closed_without_operate_receipts() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let mut state = codewhale_config::SetupState::default();
        state.set_step(
            codewhale_config::SetupStep::Language,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            ),
        );
        state.set_step(
            codewhale_config::SetupStep::ProviderModel,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                true,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            ),
        );
        state.runtime_posture_source = codewhale_config::RuntimePostureSource::Confirmed;
        state.complete_constitution_checkpoint(
            crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            codewhale_config::ConstitutionChoice::Bundled,
        );
        state.set_step(
            codewhale_config::SetupStep::OperateFleet,
            codewhale_config::StepEntry::new(
                codewhale_config::StepStatus::Verified,
                false,
                crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
            )
            .with_result(
                "provider=ready, runtime=ready, roster=ready, concurrency=plan limit not probed",
            ),
        );
        state.save().expect("persist setup state");

        let report = doctor_setup_report_json(&Config::default(), &workspace);

        assert_eq!(report["first_run_ready"], true);
        assert_eq!(report["operate_ready"], false);
        assert_eq!(
            report["operate_fleet"]["concurrency"]["plan_limit_probed"],
            false
        );
        assert!(
            report["operate_fleet"]["roster"]["built_in"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        let operate_step = report["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .find(|step| step["step"] == "operate_fleet")
            .expect("operate/fleet step");
        assert_eq!(operate_step["status"], "verified");
        assert!(
            operate_step["result"]
                .as_str()
                .is_some_and(|result| result.contains("plan limit not probed"))
        );
    }
}

#[cfg(test)]
mod doctor_endpoint_tests {
    use super::*;

    #[test]
    fn doctor_api_target_reports_default_endpoint() {
        let config = Config::default();

        let target = doctor_api_target(&config);

        assert_eq!(target.provider, "deepseek");
        assert_eq!(target.base_url, crate::config::DEFAULT_DEEPSEEK_BASE_URL);
        assert_eq!(target.model, crate::config::DEFAULT_TEXT_MODEL);
    }

    #[test]
    fn doctor_api_target_routes_deepseek_cn_alias_to_beta_endpoint() {
        let config = Config {
            provider: Some("deepseek-cn".to_string()),
            ..Default::default()
        };

        let target = doctor_api_target(&config);

        assert_eq!(target.provider, "deepseek-cn");
        assert_eq!(target.base_url, crate::config::DEFAULT_DEEPSEEKCN_BASE_URL);
        assert_eq!(target.base_url, crate::config::DEFAULT_DEEPSEEK_BASE_URL);
        assert_eq!(target.model, crate::config::DEFAULT_TEXT_MODEL);
    }

    #[test]
    fn strict_tool_mode_doctor_reports_disabled_by_default() {
        let config = Config::default();

        let status = doctor_strict_tool_mode_status(&config);

        assert!(!status.enabled);
        assert_eq!(status.status, "disabled");
        assert!(!status.function_strict_sent);
        assert!(status.recommended_base_url.is_none());
    }

    #[test]
    fn doctor_known_base_urls_are_ascii_case_insensitive() {
        assert!(doctor_xiaomi_mimo_base_url_uses_token_plan(
            "HTTPS://TOKEN-PLAN-CN.XIAOMIMIMO.COM/V1/"
        ));
        assert_eq!(
            known_deepseek_base_url_kind("HTTPS://API.DEEPSEEK.COM/BETA/"),
            Some(DeepSeekBaseUrlKind::Beta)
        );
        assert_eq!(
            known_deepseek_base_url_kind("HTTPS://API.DEEPSEEK.COM/V1/"),
            Some(DeepSeekBaseUrlKind::NonBeta)
        );
    }

    #[test]
    fn strict_tool_mode_doctor_accepts_default_beta_endpoint() {
        let config = Config {
            strict_tool_mode: Some(true),
            ..Default::default()
        };

        let status = doctor_strict_tool_mode_status(&config);

        assert!(status.enabled);
        assert_eq!(status.status, "ready");
        assert!(status.function_strict_sent);
        assert!(status.message.contains("beta endpoint"));
        assert!(status.recommended_base_url.is_none());
    }

    #[test]
    fn strict_tool_mode_doctor_warns_for_non_beta_deepseek_endpoint() {
        let config = Config {
            strict_tool_mode: Some(true),
            base_url: Some("https://api.deepseek.com".to_string()),
            ..Default::default()
        };

        let status = doctor_strict_tool_mode_status(&config);

        assert_eq!(status.status, "fallback_non_beta");
        assert!(!status.function_strict_sent);
        assert_eq!(
            status.recommended_base_url.as_deref(),
            Some(crate::config::DEFAULT_DEEPSEEK_BASE_URL)
        );
    }

    #[test]
    fn strict_tool_mode_doctor_accepts_deepseek_cn_alias_default_endpoint() {
        let config = Config {
            provider: Some("deepseek-cn".to_string()),
            strict_tool_mode: Some(true),
            ..Default::default()
        };

        let status = doctor_strict_tool_mode_status(&config);

        assert_eq!(status.status, "ready");
        assert!(status.function_strict_sent);
        assert!(status.message.contains("beta endpoint"));
        assert!(status.recommended_base_url.is_none());
    }

    #[test]
    fn strict_tool_mode_doctor_marks_custom_endpoint_as_forwarded() {
        let config = Config {
            provider: Some("vllm".to_string()),
            strict_tool_mode: Some(true),
            ..Default::default()
        };

        let status = doctor_strict_tool_mode_status(&config);

        assert_eq!(status.status, "custom_endpoint");
        assert!(status.function_strict_sent);
        assert!(status.message.contains("custom endpoint"));
    }

    #[test]
    fn doctor_tls_status_reports_verification_enabled_by_default() {
        let status = doctor_tls_status(&Config::default());

        assert!(status.certificate_verification);
        assert!(!status.insecure_skip_tls_verify);
        assert_eq!(status.provider, "deepseek");
        assert!(status.message.contains("enabled"));
    }

    #[test]
    fn doctor_tls_status_warns_when_active_provider_skips_verification() {
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.insecure_skip_tls_verify = Some(true);
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        let status = doctor_tls_status(&config);

        assert!(status.certificate_verification);
        assert!(status.insecure_skip_tls_verify);
        assert_eq!(status.provider, "openai");
        assert!(status.message.contains("cannot be disabled"));
        assert!(status.message.contains("SSL_CERT_FILE"));
    }

    #[test]
    fn provider_capability_report_exposes_alias_deprecation_for_deepseek_chat() {
        let mut config = Config {
            default_text_model: Some("deepseek-chat".to_string()),
            ..Default::default()
        };
        crate::config::normalize_model_config_for_test(&mut config);

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], "deepseek-v4-flash");
        assert_eq!(report["context_window"], 1_000_000);
        assert_eq!(report["thinking_supported"], true);
        assert_eq!(report["alias_deprecation"]["alias"], "deepseek-chat");
        assert_eq!(
            report["alias_deprecation"]["replacement"],
            "deepseek-v4-flash"
        );
        assert_eq!(
            report["alias_deprecation"]["retirement_utc"],
            "2026-07-24T15:59:00Z"
        );
    }

    #[test]
    fn provider_capability_report_preserves_custom_deepseek_alias_namespace() {
        let mut config = Config {
            base_url: Some("https://models.example/v1".to_string()),
            default_text_model: Some("deepseek-chat".to_string()),
            ..Default::default()
        };
        crate::config::normalize_model_config_for_test(&mut config);

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], "deepseek-chat");
        assert!(report["alias_deprecation"].is_null());
    }

    #[test]
    fn provider_capability_report_leaves_canonical_flash_alias_metadata_null() {
        let config = Config {
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        };

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], "deepseek-v4-flash");
        assert!(report["alias_deprecation"].is_null());
    }

    #[test]
    fn doctor_route_report_exposes_tokenhub_openai_compatible_route_without_secret() {
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.api_key = Some("tokenhub-secret-value".to_string());
        providers.openai.base_url = Some("https://tokenhub.tencentmaas.com/v1".to_string());
        providers.openai.model = Some("deepseek-ai/DeepSeek-V4-Pro".to_string());
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        let report = doctor_route_report(&config);
        let serialized = report.to_string();

        assert_eq!(report["provider"], "openai");
        assert_eq!(report["provider_source"], "config");
        assert_eq!(report["provider_config_table"], "openai");
        assert_eq!(report["model"], "deepseek-ai/DeepSeek-V4-Pro");
        assert_eq!(report["wire_protocol"], "chat_completions");
        assert_eq!(
            report["base_url"]["redacted"],
            "https://tokenhub.tencentmaas.com/v1"
        );
        assert_eq!(report["base_url"]["class"], "custom");
        assert_eq!(report["auth"]["scheme"], "bearer");
        assert_eq!(report["auth"]["source"], "config");
        assert!(
            report["base_url"]["fingerprint"]
                .as_str()
                .is_some_and(|value| value.starts_with("<redacted:"))
        );
        assert!(!serialized.contains("tokenhub-secret-value"));
    }

    #[test]
    fn doctor_route_report_exposes_siliconflow_cn_provider_route() {
        let mut providers = crate::config::ProvidersConfig::default();
        providers.siliconflow_cn.api_key = Some("sf-cn-secret-value".to_string());
        providers.siliconflow_cn.base_url =
            Some(crate::config::DEFAULT_SILICONFLOW_CN_BASE_URL.to_string());
        providers.siliconflow_cn.model = Some(crate::config::DEFAULT_SILICONFLOW_MODEL.to_string());
        let config = Config {
            provider: Some("siliconflow-CN".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        let report = doctor_route_report(&config);
        let serialized = report.to_string();

        assert_eq!(report["provider"], "siliconflow-CN");
        assert_eq!(report["provider_config_table"], "siliconflow_cn");
        assert_eq!(report["model"], crate::config::DEFAULT_SILICONFLOW_MODEL);
        assert_eq!(
            report["base_url"]["redacted"],
            crate::config::DEFAULT_SILICONFLOW_CN_BASE_URL
        );
        assert_eq!(report["base_url"]["class"], "default");
        assert_eq!(report["auth"]["scheme"], "bearer");
        assert_eq!(report["auth"]["source"], "config");
        assert!(!serialized.contains("sf-cn-secret-value"));
    }

    #[test]
    fn doctor_route_report_names_kimi_code_context_provenance() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("kimi-plan-secret".to_string()),
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = doctor_route_report(&config);
        let serialized = report.to_string();

        assert_eq!(report["context_window"]["tokens"], 262_144);
        assert_eq!(
            report["context_window"]["source"],
            "static Kimi Code safe floor"
        );
        assert!(!serialized.contains("kimi-plan-secret"));
    }

    #[test]
    fn doctor_search_provider_line_includes_duckduckgo_default_source_and_switch_hint() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };

        let line = doctor_search_provider_line(&Config::default());

        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") },
        }
        assert!(line.contains("search_provider: duckduckgo"));
        assert!(line.contains("source: default"));
        assert!(line.contains("[search] provider"));
        assert!(line.contains("provider = \"bing\""));
    }

    #[test]
    fn doctor_search_provider_json_reports_config_source() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };
        let config = Config {
            search: Some(crate::config::SearchConfig {
                provider: Some(crate::config::SearchProvider::DuckDuckGo),
                base_url: None,
                api_key: None,
            }),
            ..Default::default()
        };

        let report = doctor_search_provider_json(&config);

        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") },
        }
        assert_eq!(report["provider"], "duckduckgo");
        assert_eq!(report["source"], "config");
    }

    #[test]
    fn doctor_search_provider_json_reports_env_override_source() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", "tavily") };

        let report = doctor_search_provider_json(&Config::default());

        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") },
        }
        assert_eq!(report["provider"], "tavily");
        assert_eq!(report["source"], "env override");
    }

    #[test]
    fn doctor_search_provider_line_omits_switch_hint_when_bing_is_configured() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };
        let config = Config {
            search: Some(crate::config::SearchConfig {
                provider: Some(crate::config::SearchProvider::Bing),
                base_url: None,
                api_key: None,
            }),
            ..Default::default()
        };

        let line = doctor_search_provider_line(&config);

        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") },
        }
        assert!(line.contains("search_provider: bing"));
        assert!(line.contains("source: config"));
        assert!(!line.contains("[search] provider"));
    }

    #[test]
    fn timeout_recovery_keeps_default_deepseek_users_on_default_endpoint() {
        let config = Config::default();

        let text = doctor_timeout_recovery_lines(&config).join("\n");

        assert!(text.contains("api.deepseek.com"));
        assert!(text.contains("custom DeepSeek-compatible endpoint"));
        assert!(!text.contains("provider = \"deepseek-cn\""));
        assert!(text.contains("codewhale doctor --json"));
    }

    #[test]
    fn timeout_recovery_for_custom_provider_checks_openai_compatibility() {
        let config = Config {
            provider: Some("vllm".to_string()),
            ..Default::default()
        };

        let text = doctor_timeout_recovery_lines(&config).join("\n");

        assert!(text.contains("/v1/models"));
        assert!(text.contains("/v1/chat/completions"));
        assert!(!text.contains("api.deepseeki.com"));
    }
}

#[cfg(test)]
mod terminal_mode_tests {
    use super::*;
    use clap::Parser;

    fn parse_cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("CLI args should parse")
    }

    #[test]
    fn plugin_registry_discovery_is_route_independent_and_read_only() {
        let _env_lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let codewhale_home = temp.path().join("home");
        std::fs::create_dir_all(&workspace).unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let workspace_arg = workspace.to_string_lossy().into_owned();

        for route in [
            Vec::<&str>::new(),
            vec!["resume", "--last"],
            vec!["fork", "--last"],
            vec!["exec", "hello"],
            vec!["serve", "--mcp"],
        ] {
            let mut args = vec![
                "codewhale-tui".to_string(),
                "--workspace".to_string(),
                workspace_arg.clone(),
            ];
            args.extend(route.into_iter().map(str::to_string));
            let cli = Cli::try_parse_from(args).expect("route should parse");
            let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
            let registry = discovery
                .registry_for_workspace(cli.workspace.as_deref().unwrap_or(workspace.as_path()));
            assert_eq!(registry.workspace(), workspace.as_path());
            assert!(
                !codewhale_home.join("plugins/state.json").exists(),
                "startup discovery must remain read-only"
            );
        }
    }

    fn custom_exec_config(active: &str) -> Config {
        let mut custom = std::collections::HashMap::new();
        for (name, base_url, model) in [
            (
                "custom-a",
                "http://127.0.0.1:18181/v1",
                crate::config::ZAI_GLM_5_2_MODEL,
            ),
            ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
        ] {
            custom.insert(
                name.to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some(base_url.to_string()),
                    model: Some(model.to_string()),
                    api_key: Some("local-test-key".to_string()),
                    ..Default::default()
                },
            );
        }
        Config {
            provider: Some(active.to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn doctor_json_surfaces_keep_exact_named_custom_provider() {
        let config = custom_exec_config("custom-a");
        let workspace = tempfile::tempdir().expect("doctor workspace");

        let operate = doctor_operate_fleet_report_json(&config, workspace.path());
        let provider_model = doctor_provider_model_report_json(&config);
        let capability = provider_capability_report(&config);
        let route = doctor_route_report(&config);

        assert_eq!(operate["provider"]["id"], "custom-a");
        assert_eq!(provider_model["provider"]["id"], "custom-a");
        assert_eq!(capability["resolved_provider"], "custom-a");
        assert_eq!(route["provider"], "custom-a");
        assert_eq!(route["provider_config_table"], "providers.custom-a");
        let serialized = serde_json::to_string(&serde_json::json!({
            "operate": operate,
            "provider_model": provider_model,
            "capability": capability,
            "route": route,
        }))
        .expect("doctor JSON");
        assert!(!serialized.contains("local-test-key"));
    }

    fn saved_exec_session(provider: &str, model: &str) -> session_manager::SavedSession {
        let mut saved = session_manager::create_saved_session_with_mode(
            &[],
            model,
            Path::new("/tmp/exec-resume"),
            0,
            None,
            Some("exec"),
        );
        let kind = crate::config::ApiProvider::parse(provider)
            .unwrap_or(crate::config::ApiProvider::Custom)
            .as_str();
        let exact_id = (!provider
            .eq_ignore_ascii_case(crate::config::ApiProvider::Custom.as_str()))
        .then_some(provider);
        saved.metadata.set_model_provider_route(kind, exact_id);
        saved
    }

    #[test]
    fn prompt_flag_accepts_split_prompt_words_for_windows_cmd_shims() {
        let cli = parse_cli(&["codewhale", "-p", "hello", "world"]);

        assert_eq!(cli.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn prompt_flag_starts_interactive_submit_input() {
        let cli = parse_cli(&["codewhale", "-p", "read", "the", "project"]);

        assert_eq!(
            top_level_prompt_initial_input(&cli.prompt),
            Some(tui::InitialInput::Submit("read the project".to_string()))
        );
    }

    #[test]
    fn companion_binary_reports_its_own_name() {
        assert_eq!(Cli::command().get_name(), "codewhale-tui");
    }

    #[test]
    fn xai_device_auth_subcommand_parses() {
        let cli = parse_cli(&["codewhale-tui", "auth", "xai-device"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(TuiAuthArgs {
                command: TuiAuthCommand::XaiDevice
            }))
        ));
    }

    #[test]
    fn workflow_tool_internal_subcommand_parses_exact_json() {
        let cli = parse_cli(&[
            "codewhale-tui",
            "workflow-tool",
            "--approval-source",
            "explicit-workflow-command",
            "--input-json",
            r#"{"action":"run","source_path":"workflows/demo.js"}"#,
        ]);
        let Some(Commands::WorkflowTool(args)) = cli.command else {
            panic!("expected workflow-tool command");
        };
        assert!(args.input_json.contains("\"action\":\"run\""));
    }

    #[tokio::test]
    async fn direct_workflow_tool_runs_without_an_operator_model_turn() {
        use crate::tools::spec::ToolSpec;

        let workspace = tempfile::tempdir().expect("workspace");
        let config = Config {
            provider: Some("vllm".to_string()),
            mcp_config_path: Some(
                workspace
                    .path()
                    .join("missing-mcp.json")
                    .display()
                    .to_string(),
            ),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    base_url: Some("http://127.0.0.1:9/v1".to_string()),
                    model: Some("offline-test-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let route = CliAutoRoute {
            provider: crate::config::ApiProvider::Vllm,
            model: "offline-test-model".to_string(),
            reasoning_effort: None,
            auto_model: false,
        };
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
        let plugins = Arc::new(crate::plugins::PluginRegistry::empty(workspace.path()));
        let (tool, context) =
            build_direct_workflow_tool(&config, &route, workspace.path(), event_tx, plugins)
                .await
                .expect("build direct workflow runtime");

        let result = tool
            .execute(
                serde_json::json!({
                    "action": "run",
                    "script": "phase('offline'); return { ok: true };",
                    "token_budget": 1_000_000
                }),
                &context,
            )
            .await
            .expect("model-free workflow run");
        let payload: serde_json::Value =
            serde_json::from_str(&result.content).expect("workflow JSON");

        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["result"]["ok"], true);
        assert_eq!(payload["child_ids"].as_array().map(Vec::len), Some(0));
        assert_eq!(
            payload["plan_approval"]["decision"],
            "approved_explicit_cli_command"
        );
        assert!(!context.auto_approve);
        assert!(!context.trust_mode);
        assert_eq!(
            context.shell_policy,
            crate::worker_profile::ShellPolicy::None
        );
        assert!(matches!(
            context.elevated_sandbox_policy,
            Some(crate::sandbox::SandboxPolicy::WorkspaceWrite { .. })
        ));
        let mut event_types = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let crate::core::events::Event::WorkflowUi { event, .. } = event
                && let Some(kind) = event["type"].as_str()
            {
                event_types.push(kind.to_string());
            }
        }
        assert!(event_types.iter().any(|kind| kind == "run_started"));
        assert!(event_types.iter().any(|kind| kind == "run_completed"));
    }

    #[tokio::test]
    async fn direct_workflow_mcp_pool_applies_network_policy_before_connect() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mcp_path = workspace.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
                "mcpServers": {
                    "blocked": { "url": "https://blocked.invalid/mcp" }
                }
            }"#,
        )
        .expect("write MCP config");
        let config = Config {
            mcp_config_path: Some(mcp_path.display().to_string()),
            ..Default::default()
        };
        let policy = crate::network_policy::NetworkPolicyDecider::new(
            crate::network_policy::NetworkPolicy {
                default: crate::network_policy::DecisionToml::Deny,
                allow: Vec::new(),
                deny: Vec::new(),
                proxy: Vec::new(),
                proxy_fake_ip_cidrs: Vec::new(),
                audit: false,
            },
            None,
        );

        let plugins = Arc::new(crate::plugins::PluginRegistry::empty(workspace.path()));
        let (_pool, failures) =
            initialize_direct_workflow_mcp_pool(&config, workspace.path(), Some(policy), plugins)
                .await
                .expect("MCP feature enabled");
        assert_eq!(failures.len(), 1, "failures={failures:?}");
        assert_eq!(failures[0].0, "blocked");
        assert!(failures[0].1.contains("blocked by network policy"));
    }

    #[test]
    fn exec_model_resolution_uses_provider_scoped_default() {
        let _env_lock = crate::test_support::lock_test_env();
        let _codewhale_model = crate::test_support::EnvVarGuard::remove("CODEWHALE_MODEL");
        let _deepseek_model = crate::test_support::EnvVarGuard::remove("DEEPSEEK_MODEL");
        let config = Config {
            provider: Some("openrouter".to_string()),
            default_text_model: Some("deepseek/deepseek-v4-pro".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openrouter: crate::config::ProviderConfig {
                    model: Some("arcee-ai/trinity-large-thinking".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            resolve_exec_model(&config, None),
            "arcee-ai/trinity-large-thinking"
        );
        assert_eq!(
            resolve_exec_model(&config, Some("arcee-ai/trinity-large-thinking")),
            "arcee-ai/trinity-large-thinking"
        );
    }

    #[test]
    fn exec_model_resolution_prefers_codewhale_model_env_override() {
        let _env_lock = crate::test_support::lock_test_env();
        let _codewhale_model = crate::test_support::EnvVarGuard::set("CODEWHALE_MODEL", " auto ");
        let _deepseek_model =
            crate::test_support::EnvVarGuard::set("DEEPSEEK_MODEL", "stale-deepseek-model");
        let config = Config {
            default_text_model: Some("deepseek/deepseek-v4-pro".to_string()),
            ..Default::default()
        };

        assert_eq!(resolve_exec_model(&config, None), "auto");
    }

    #[test]
    fn exec_model_resolution_uses_legacy_deepseek_model_env_override() {
        let _env_lock = crate::test_support::lock_test_env();
        let _codewhale_model = crate::test_support::EnvVarGuard::remove("CODEWHALE_MODEL");
        let _deepseek_model = crate::test_support::EnvVarGuard::set("DEEPSEEK_MODEL", " auto ");
        let config = Config {
            default_text_model: Some("deepseek/deepseek-v4-pro".to_string()),
            ..Default::default()
        };

        assert_eq!(resolve_exec_model(&config, None), "auto");
    }

    #[test]
    fn exec_model_resolution_uses_provider_safe_default_for_zai() {
        let _env_lock = crate::test_support::lock_test_env();
        let _codewhale_model = crate::test_support::EnvVarGuard::remove("CODEWHALE_MODEL");
        let _deepseek_model = crate::test_support::EnvVarGuard::remove("DEEPSEEK_MODEL");
        let config = Config {
            provider: Some("zai".to_string()),
            default_text_model: Some(crate::config::DEFAULT_TEXT_MODEL.to_string()),
            ..Default::default()
        };

        assert_eq!(
            resolve_exec_model(&config, None),
            crate::config::DEFAULT_ZAI_MODEL
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn explicit_exec_model_routes_to_unique_authenticated_provider_candidate() {
        let _env_lock = crate::test_support::lock_test_env();
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let _openrouter = crate::test_support::EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config {
            provider: Some("deepseek".to_string()),
            default_text_model: Some(crate::config::DEFAULT_TEXT_MODEL.to_string()),
            ..Default::default()
        };

        let route = resolve_cli_auto_route(&config, crate::config::ZAI_GLM_5_2_MODEL, "pong")
            .await
            .expect("explicit GLM should route to the configured Z.ai provider");

        assert_eq!(route.provider, crate::config::ApiProvider::Zai);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert!(!route.auto_model);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn explicit_exec_model_reports_ambiguous_authenticated_provider_candidates() {
        let _env_lock = crate::test_support::lock_test_env();
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let _openrouter = crate::test_support::EnvVarGuard::set("OPENROUTER_API_KEY", "or-key");
        let config = Config {
            provider: Some("deepseek".to_string()),
            default_text_model: Some(crate::config::DEFAULT_TEXT_MODEL.to_string()),
            ..Default::default()
        };

        let err = resolve_cli_auto_route(&config, crate::config::ZAI_GLM_5_2_MODEL, "pong")
            .await
            .expect_err("ambiguous GLM route should ask for an explicit provider");
        let message = err.to_string();

        assert!(message.contains("model `GLM-5.2` is available"));
        assert!(message.contains("openrouter"));
        assert!(message.contains("zai"));
        assert!(message.contains("--provider"));
        assert!(message.contains("/provider"));
        assert!(message.contains("/model"));
        assert!(message.contains("/setup"));
    }

    #[test]
    fn cli_route_execution_config_stamps_routed_model_into_provider_slot() {
        let mut providers = crate::config::ProvidersConfig::default();
        providers.deepseek.model = Some("deepseek-v4-pro".to_string());
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(providers),
            ..Default::default()
        };
        let route = CliAutoRoute {
            provider: crate::config::ApiProvider::Deepseek,
            model: "deepseek-v4-flash".to_string(),
            reasoning_effort: None,
            auto_model: true,
        };

        let execution_config = config_for_cli_route(&config, &route);

        assert_eq!(execution_config.default_model(), "deepseek-v4-flash");
        assert_eq!(
            execution_config
                .provider_config_for(crate::config::ApiProvider::Deepseek)
                .and_then(|entry| entry.model.as_deref()),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn cli_route_execution_config_preserves_legacy_literal_custom_root_route() {
        let _lock = crate::test_support::lock_test_env();
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
        let config = Config {
            provider: Some("custom".to_string()),
            api_key: Some("legacy-root-key".to_string()),
            base_url: Some("http://127.0.0.1:18183/v1".to_string()),
            default_text_model: Some("legacy-model".to_string()),
            ..Default::default()
        };
        let route = CliAutoRoute {
            provider: crate::config::ApiProvider::Custom,
            model: "routed-legacy-model".to_string(),
            reasoning_effort: None,
            auto_model: false,
        };

        let execution = config_for_cli_route(&config, &route);

        assert!(execution.uses_legacy_literal_custom_route());
        assert!(
            execution
                .providers
                .as_ref()
                .is_none_or(|providers| !providers.custom.contains_key("custom"))
        );
        assert_eq!(execution.provider.as_deref(), Some("custom"));
        assert_eq!(execution.default_model(), "routed-legacy-model");
        assert_eq!(execution.deepseek_base_url(), "http://127.0.0.1:18183/v1");
        assert_eq!(execution.deepseek_api_key().unwrap(), "legacy-root-key");
        for _ in 0..2 {
            let identity = execution
                .resolve_provider_identity("custom")
                .expect("legacy identity remains repeatedly resolvable");
            assert_eq!(identity.key, "custom");
        }
        let client =
            crate::client::DeepSeekClient::new(&execution).expect("legacy execution client");
        assert_eq!(client.base_url(), "http://127.0.0.1:18183/v1");
    }

    #[test]
    fn exec_accepts_split_prompt_words_for_windows_cmd_shims() {
        let cli = parse_cli(&["codewhale", "exec", "hello", "world"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn exec_keeps_model_flag_before_split_prompt_words() {
        let cli = parse_cli(&["codewhale", "exec", "--model", "auto", "hello", "world"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.model.as_deref(), Some("auto"));
        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn exec_keeps_flags_before_split_prompt_words() {
        let cli = parse_cli(&["codewhale", "exec", "--json", "hello", "world"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert!(args.json);
        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn exec_parses_provider_flag_alongside_model() {
        // #4093: Fleet threads `--provider <id>` so a worker launches on its
        // profile-pinned provider even when the parent session is elsewhere.
        let cli = parse_cli(&[
            "codewhale",
            "exec",
            "--provider",
            "openrouter",
            "--model",
            "glm-5.2",
            "audit",
        ]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.provider.as_deref(), Some("openrouter"));
        assert_eq!(args.model.as_deref(), Some("glm-5.2"));
        assert_eq!(args.prompt, vec!["audit"]);
        // The threaded id round-trips through the provider vocabulary the exec
        // handler validates against — never a model-id sniff (EPIC #2608).
        assert_eq!(
            crate::config::ApiProvider::parse(args.provider.as_deref().unwrap()),
            Some(crate::config::ApiProvider::Openrouter)
        );
    }

    #[test]
    fn exec_provider_override_accepts_configured_custom_provider() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "lm-studio".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:1234/v1".to_string()),
                model: Some("qwen-2.5-7b".to_string()),
                api_key: Some("lm-studio".to_string()),
                ..Default::default()
            },
        );
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        };

        apply_exec_provider_override(&mut config, "lm-studio")
            .expect("configured custom provider should be accepted");

        assert_eq!(config.provider.as_deref(), Some("lm-studio"));
        assert_eq!(config.api_provider(), crate::config::ApiProvider::Custom);
    }

    #[test]
    fn exec_provider_override_prefers_exact_case_colliding_custom_key() {
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "CUSTOM".to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some("http://127.0.0.1:5678/v1".to_string()),
                        model: Some("case-model".to_string()),
                        api_key: Some("case-key".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };

        apply_exec_provider_override(&mut config, "CUSTOM")
            .expect("exact case-colliding custom provider");
        assert_eq!(config.provider.as_deref(), Some("CUSTOM"));
        assert_eq!(config.api_provider(), crate::config::ApiProvider::Custom);
        assert_eq!(
            config.provider_identity_for(crate::config::ApiProvider::Custom),
            "CUSTOM"
        );
        let route = crate::route_runtime::resolve_runtime_route(
            &config,
            crate::config::ApiProvider::Custom,
            Some("case-model"),
        )
        .expect("resolve exact case-colliding route")
        .validate()
        .expect("preflight exact case-colliding route");
        assert_eq!(route.identity.key, "CUSTOM");
        assert_eq!(route.client.base_url(), "http://127.0.0.1:5678/v1");
    }

    #[test]
    fn exec_provider_override_rejects_unknown_provider() {
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            ..Default::default()
        };

        let err = apply_exec_provider_override(&mut config, "lm-studio")
            .expect_err("unconfigured custom provider should fail closed");
        let message = err.to_string();

        assert!(message.contains("Unrecognized --provider"));
        assert!(message.contains("[providers.<name>] custom provider"));
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
    }

    #[test]
    fn exec_resume_route_matrix_preserves_or_overrides_exact_provider_deliberately() {
        let saved = saved_exec_session("custom-a", crate::config::ZAI_GLM_5_2_MODEL);

        let mut restored = custom_exec_config("custom-b");
        let model = resolve_exec_resume_route(&mut restored, &saved, false, None)
            .expect("plain resume restores saved route");
        assert_eq!(restored.provider.as_deref(), Some("custom-a"));
        assert_eq!(model, crate::config::ZAI_GLM_5_2_MODEL);

        let mut explicit_provider = custom_exec_config("custom-a");
        apply_exec_provider_override(&mut explicit_provider, "custom-b").expect("custom B");
        let model = resolve_exec_resume_route(&mut explicit_provider, &saved, true, None)
            .expect("explicit provider wins");
        assert_eq!(explicit_provider.provider.as_deref(), Some("custom-b"));
        assert_eq!(model, "model-b");

        let mut explicit_model = custom_exec_config("custom-b");
        let model =
            resolve_exec_resume_route(&mut explicit_model, &saved, false, Some("override-model"))
                .expect("explicit model keeps saved provider");
        assert_eq!(explicit_model.provider.as_deref(), Some("custom-a"));
        assert_eq!(model, "override-model");

        let mut missing = custom_exec_config("custom-b");
        missing
            .providers
            .as_mut()
            .expect("providers")
            .custom
            .remove("custom-a");
        let before = missing.provider.clone();
        let err = resolve_exec_resume_route(&mut missing, &saved, false, None)
            .expect_err("removed saved provider must fail closed");
        assert!(err.to_string().contains("will not fall back"), "{err}");
        assert_eq!(missing.provider, before);
    }

    #[tokio::test]
    async fn forced_exec_route_keeps_custom_provider_when_model_matches_builtin_catalog() {
        let config = custom_exec_config("custom-a");

        let route =
            resolve_cli_exec_route(&config, crate::config::ZAI_GLM_5_2_MODEL, "audit", true)
                .await
                .expect("forced route");
        let execution = config_for_cli_route(&config, &route);

        assert_eq!(route.provider, crate::config::ApiProvider::Custom);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert_eq!(execution.provider.as_deref(), Some("custom-a"));
    }

    #[tokio::test]
    async fn no_flag_exec_keeps_configured_named_custom_route_for_matching_builtin_model() {
        let mut config = custom_exec_config("custom-a");
        config
            .providers
            .as_mut()
            .expect("providers")
            .custom
            .get_mut("custom-a")
            .expect("custom A")
            .model = Some(crate::config::ZAI_GLM_5_2_MODEL.to_string());
        let model = resolve_exec_model(&config, None);
        let force = should_force_configured_exec_route(false, None, None);

        assert!(force, "configured/default exec route must be authoritative");
        assert!(!should_force_configured_exec_route(
            false,
            None,
            Some(crate::config::ZAI_GLM_5_2_MODEL)
        ));
        assert!(should_force_configured_exec_route(
            false,
            Some("custom-a"),
            Some(crate::config::ZAI_GLM_5_2_MODEL)
        ));
        assert!(should_force_configured_exec_route(
            true,
            None,
            Some("override-model")
        ));

        let route = resolve_cli_exec_route(&config, &model, "audit", force)
            .await
            .expect("no-flag configured route");
        let execution = config_for_cli_route(&config, &route);
        assert_eq!(route.provider, crate::config::ApiProvider::Custom);
        assert_eq!(route.model, crate::config::ZAI_GLM_5_2_MODEL);
        assert_eq!(execution.provider.as_deref(), Some("custom-a"));
    }

    #[tokio::test]
    async fn configured_review_default_keeps_named_custom_route_and_exact_receipt() {
        let mut config = custom_exec_config("custom-a");
        config
            .providers
            .as_mut()
            .expect("providers")
            .custom
            .get_mut("custom-a")
            .expect("custom A")
            .model = Some("model-a".to_string());
        config.default_text_model = Some("stale-root-deepseek-model".to_string());
        let model = resolve_review_model(&config, None);
        assert_eq!(model, "model-a");
        assert_eq!(
            resolve_review_model(&config, Some("explicit-review-model")),
            "explicit-review-model"
        );

        let route = resolve_cli_exec_route(&config, &model, "review diff", true)
            .await
            .expect("configured review route");
        let execution = config_for_cli_route(&config, &route);
        let provider = execution.provider_identity_for(route.provider);

        assert_eq!(route.provider, crate::config::ApiProvider::Custom);
        assert_eq!(provider, "custom-a");
        assert_eq!(execution.deepseek_base_url(), "http://127.0.0.1:18181/v1");
        let output = crate::tools::review::ReviewOutput::from_str("{}");
        let receipt = crate::tools::review::build_review_receipt(
            "working tree",
            "diff --git a/a b/a",
            provider,
            &route.model,
            &output,
            "{}",
            Vec::new(),
        );
        assert_eq!(receipt.provider, "custom-a");
        let serialized = serde_json::to_string(&receipt).expect("review receipt");
        assert!(!serialized.contains("127.0.0.1"));
        assert!(!serialized.contains("local-test-key"));
    }

    #[tokio::test]
    async fn configured_workflow_default_keeps_named_custom_route() {
        let config = custom_exec_config("custom-a");
        let model = config.default_model();

        let route = resolve_cli_exec_route(
            &config,
            &model,
            "Run a checked-in Workflow through the host runtime",
            true,
        )
        .await
        .expect("configured workflow route");
        let execution = config_for_cli_route(&config, &route);

        assert_eq!(route.provider, crate::config::ApiProvider::Custom);
        assert_eq!(execution.provider_identity_for(route.provider), "custom-a");
        assert_eq!(execution.deepseek_base_url(), "http://127.0.0.1:18181/v1");
        let client = crate::client::DeepSeekClient::new(&execution).expect("workflow client");
        assert_eq!(client.base_url(), "http://127.0.0.1:18181/v1");
    }

    #[test]
    fn exec_json_receipts_keep_exact_named_custom_provider() {
        let config = custom_exec_config("custom-a");
        let provider = config.provider_identity_for(crate::config::ApiProvider::Custom);
        let one_shot =
            one_shot_exec_json_receipt(provider.clone(), "model-a".to_string(), "done".to_string());
        assert_eq!(one_shot["provider"], "custom-a");

        let agent = serde_json::to_value(ExecSummary {
            mode: "agent".to_string(),
            provider,
            model: "model-a".to_string(),
            ..ExecSummary::default()
        })
        .expect("agent exec JSON receipt");
        assert_eq!(agent["provider"], "custom-a");
        let serialized = serde_json::to_string(&agent).expect("serialize receipt");
        assert!(!serialized.contains("127.0.0.1"));
        assert!(!serialized.contains("local-test-key"));
    }

    #[test]
    fn exec_stream_provider_pair_preserves_named_literal_and_root_custom_provenance() {
        let named = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Custom,
            key: "lm-studio".to_string(),
            exact_id: Some("lm-studio".to_string()),
        };
        let literal = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: Some("custom".to_string()),
        };
        let root = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: None,
        };
        let built_in = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Deepseek,
            key: "deepseek".to_string(),
            exact_id: Some("deepseek".to_string()),
        };

        assert_eq!(
            exec_stream_provider_route(&named),
            ("custom".to_string(), Some("lm-studio".to_string()))
        );
        assert_eq!(
            exec_stream_provider_route(&literal),
            ("custom".to_string(), Some("custom".to_string()))
        );
        assert_eq!(
            exec_stream_provider_route(&root),
            ("custom".to_string(), None)
        );
        assert_eq!(
            exec_stream_provider_route(&built_in),
            ("deepseek".to_string(), None)
        );
    }

    #[test]
    fn resumed_exec_persistence_updates_provider_and_model_as_one_route() {
        let saved_a = saved_exec_session("custom-a", crate::config::ZAI_GLM_5_2_MODEL);
        let mut config = custom_exec_config("custom-a");
        apply_exec_provider_override(&mut config, "custom-b").expect("custom B");
        let model = resolve_exec_resume_route(&mut config, &saved_a, true, None)
            .expect("explicit provider route");
        let mut persisted = saved_a;
        stamp_exec_session_metadata(
            &mut persisted,
            &model,
            crate::config::ApiProvider::Custom.as_str(),
            Some("custom-b"),
            Path::new("/tmp/exec-resume"),
        );

        let mut next_config = custom_exec_config("custom-a");
        let resumed_model = resolve_exec_resume_route(&mut next_config, &persisted, false, None)
            .expect("next plain resume");

        assert_eq!(persisted.metadata.model_provider, "custom");
        assert_eq!(
            persisted.metadata.model_provider_id.as_deref(),
            Some("custom-b")
        );
        assert_eq!(persisted.metadata.model, "model-b");
        assert_eq!(next_config.provider.as_deref(), Some("custom-b"));
        assert_eq!(resumed_model, "model-b");
    }

    #[test]
    fn exec_persistence_omits_id_for_legacy_root_custom_route() {
        let mut saved = session_manager::create_saved_session_with_mode(
            &[],
            "legacy-root-model",
            Path::new("/tmp/exec-root"),
            0,
            None,
            Some("exec"),
        );
        stamp_exec_session_metadata(
            &mut saved,
            "legacy-root-model",
            crate::config::ApiProvider::Custom.as_str(),
            None,
            Path::new("/tmp/exec-root"),
        );

        assert_eq!(saved.metadata.model_provider, "custom");
        assert_eq!(saved.metadata.model_provider_id, None);
        assert!(
            !serde_json::to_string(&saved)
                .expect("serialize exec session")
                .contains("model_provider_id")
        );
    }

    #[test]
    fn exec_parses_reasoning_effort_flag_alongside_provider() {
        let cli = parse_cli(&[
            "codewhale",
            "exec",
            "--provider",
            "openrouter",
            "--model",
            "glm-5.2",
            "--reasoning-effort",
            "max",
            "audit",
        ]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.provider.as_deref(), Some("openrouter"));
        assert_eq!(args.model.as_deref(), Some("glm-5.2"));
        assert_eq!(args.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(args.prompt, vec!["audit"]);
    }

    #[test]
    fn cli_reasoning_effort_normalizes_aliases_and_rejects_typos() {
        assert_eq!(
            normalize_cli_reasoning_effort("xhigh").unwrap().as_deref(),
            Some("max")
        );
        assert_eq!(normalize_cli_reasoning_effort("default").unwrap(), None);
        assert!(normalize_cli_reasoning_effort("expensive").is_err());
    }

    #[test]
    fn exec_accepts_resume_session_flags_for_harnesses() {
        let cli = parse_cli(&[
            "codewhale",
            "exec",
            "--resume",
            "abc123",
            "--output-format",
            "stream-json",
            "follow up",
        ]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.resume.as_deref(), Some("abc123"));
        assert_eq!(args.output_format, ExecOutputFormat::StreamJson);
        assert_eq!(args.prompt, vec!["follow up"]);
    }

    #[test]
    fn exec_accepts_session_id_alias() {
        let cli = parse_cli(&["codewhale", "exec", "--session-id", "abc123", "follow up"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.session_id.as_deref(), Some("abc123"));
        assert_eq!(args.output_format, ExecOutputFormat::Text);
    }

    #[test]
    fn exec_parses_tool_gate_and_hardening_flags() {
        let cli = parse_cli(&[
            "codewhale",
            "exec",
            "--allowed-tools",
            "read_file,grep_files",
            "--disallowed-tools",
            "exec_shell",
            "--max-turns",
            "7",
            "--append-system-prompt",
            "extra rules",
            "do the thing",
        ]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(
            args.allowed_tools.as_deref(),
            Some(&["read_file".to_string(), "grep_files".to_string()][..])
        );
        assert_eq!(
            args.disallowed_tools.as_deref(),
            Some(&["exec_shell".to_string()][..])
        );
        assert_eq!(args.max_turns, Some(7));
        assert_eq!(args.append_system_prompt.as_deref(), Some("extra rules"));
        assert_eq!(args.prompt, vec!["do the thing"]);
    }

    #[test]
    fn exec_auto_does_not_authorize_sandbox_elevation() {
        let cli = parse_cli(&["codewhale", "exec", "--auto", "run it"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert!(!exec_sandbox_elevation_authorized(
            args.allow_sandbox_elevation,
            args.sandbox.as_deref()
        ));
    }

    #[test]
    fn exec_explicit_sandbox_elevation_opt_ins_authorize_retry() {
        let danger = parse_cli(&[
            "codewhale",
            "exec",
            "--auto",
            "--sandbox",
            "danger-full-access",
            "run it",
        ]);
        let Some(Commands::Exec(args)) = danger.command else {
            panic!("expected exec command");
        };
        assert!(exec_sandbox_elevation_authorized(
            args.allow_sandbox_elevation,
            args.sandbox.as_deref()
        ));

        let flag = parse_cli(&[
            "codewhale",
            "exec",
            "--auto",
            "--allow-sandbox-elevation",
            "run it",
        ]);
        let Some(Commands::Exec(args)) = flag.command else {
            panic!("expected exec command");
        };
        assert!(exec_sandbox_elevation_authorized(
            args.allow_sandbox_elevation,
            args.sandbox.as_deref()
        ));
    }

    #[test]
    fn exec_sandbox_denial_stream_event_is_typed() {
        let event = ExecStreamEvent::SandboxDenied {
            tool_id: "call_1".to_string(),
            tool_name: "exec_shell".to_string(),
            reason: "write blocked".to_string(),
            outcome: "approval_required".to_string(),
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&event).expect("serializes"))
                .expect("valid json");
        assert_eq!(value["type"], "sandbox_denied");
        assert_eq!(value["outcome"], "approval_required");
    }

    #[test]
    fn exec_help_separates_agent_mode_from_sandbox_elevation() {
        let mut cli = Cli::command();
        let help = cli
            .find_subcommand_mut("exec")
            .expect("exec command")
            .render_help()
            .to_string();
        assert!(help.contains("--auto"));
        assert!(help.contains("--sandbox"));
        assert!(help.contains("--allow-sandbox-elevation"));
        assert!(help.contains("does not change the"));
        assert!(help.contains("explicitly authorize sandbox elevation"));
    }

    #[test]
    fn exec_shell_only_tool_surface_env_sets_shell_allowlist() {
        let _env_lock = crate::test_support::lock_test_env();
        let _surface =
            crate::test_support::EnvVarGuard::set(CODEWHALE_TOOL_SURFACE_ENV, " shell-only ");

        let allowed_tools = resolve_exec_allowed_tools(None, exec_tool_surface_from_env())
            .expect("shell-only surface should set an allowlist");

        assert_eq!(
            allowed_tools,
            vec![
                "exec_shell".to_string(),
                "exec_shell_wait".to_string(),
                "exec_shell_interact".to_string(),
            ]
        );
    }

    #[test]
    fn exec_explicit_allowed_tools_override_shell_only_env() {
        let _env_lock = crate::test_support::lock_test_env();
        let _surface =
            crate::test_support::EnvVarGuard::set(CODEWHALE_TOOL_SURFACE_ENV, "shell-only");
        let explicit = vec![" Read_File ".to_string(), "GREP_FILES".to_string()];

        let allowed_tools =
            resolve_exec_allowed_tools(Some(&explicit), exec_tool_surface_from_env())
                .expect("explicit allowlist should be preserved");

        assert_eq!(
            allowed_tools,
            vec!["read_file".to_string(), "grep_files".to_string()]
        );
    }

    #[test]
    fn exec_full_tool_surface_env_leaves_allowlist_unset() {
        let _env_lock = crate::test_support::lock_test_env();
        let _surface = crate::test_support::EnvVarGuard::set(CODEWHALE_TOOL_SURFACE_ENV, "full");

        assert_eq!(
            resolve_exec_allowed_tools(None, exec_tool_surface_from_env()),
            None
        );
    }

    #[test]
    fn exec_unknown_tool_surface_env_warns_without_allowlist() {
        assert!(should_warn_unknown_exec_tool_surface("shell_onyl"));
        assert!(!should_warn_unknown_exec_tool_surface("shell-only"));
        assert!(!should_warn_unknown_exec_tool_surface("native-tools"));
        assert!(!should_warn_unknown_exec_tool_surface("full"));
        assert!(!should_warn_unknown_exec_tool_surface(" "));
        assert_eq!(parse_exec_tool_surface("shell_onyl"), None);
    }

    #[test]
    fn exec_rejects_zero_max_turns() {
        let err = Cli::try_parse_from(["codewhale", "exec", "--max-turns", "0", "hello"])
            .expect_err("max-turns must be >= 1");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn exec_accepts_continue_for_latest_workspace_session() {
        let cli = parse_cli(&["codewhale", "exec", "--continue", "follow up"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert!(args.continue_session);
    }

    #[test]
    fn sessions_footer_points_to_resume_subcommand() {
        let cli = parse_cli(&["codewhale", "resume", "abc123"]);
        let Some(Commands::Resume { session_id, last }) = cli.command else {
            panic!("expected resume command");
        };

        assert_eq!(session_id.as_deref(), Some("abc123"));
        assert!(!last);
        assert_eq!(sessions_resume_command(), "codewhale resume");
        assert!(!sessions_resume_command().contains("--resume"));
    }

    #[test]
    fn plugin_registry_initialization_precedes_dotenv_for_all_launch_paths() {
        use std::cell::Cell;

        #[derive(Clone, Copy)]
        enum Expected {
            Plain,
            Resume,
            Fork,
            Exec,
            Serve,
        }

        let cases: &[(&[&str], Expected)] = &[
            (&["codewhale"], Expected::Plain),
            (&["codewhale", "resume", "--last"], Expected::Resume),
            (&["codewhale", "fork", "--last"], Expected::Fork),
            (&["codewhale", "exec", "probe"], Expected::Exec),
            (&["codewhale", "serve", "--mcp"], Expected::Serve),
        ];

        for (args, expected) in cases {
            let phase = Cell::new(0);
            let (_cli, command) = prepare_cli_startup(
                parse_cli(args),
                || {
                    assert_eq!(phase.get(), 0, "plugin init order for {args:?}");
                    phase.set(1);
                },
                || {
                    assert_eq!(phase.get(), 1, "dotenv load order for {args:?}");
                    phase.set(2);
                },
            );

            assert_eq!(phase.get(), 2, "startup phases for {args:?}");
            let correct_variant = matches!(
                (expected, command.as_ref()),
                (Expected::Plain, None)
                    | (Expected::Resume, Some(Commands::Resume { .. }))
                    | (Expected::Fork, Some(Commands::Fork { .. }))
                    | (Expected::Exec, Some(Commands::Exec(_)))
                    | (Expected::Serve, Some(Commands::Serve(_)))
            );
            assert!(correct_variant, "unexpected command for {args:?}");
        }
    }

    #[test]
    fn workspace_dotenv_loads_only_provider_credentials_and_preserves_shell_values() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _nvidia = crate::test_support::EnvVarGuard::set("NVIDIA_API_KEY", "shell-key");
        let _home = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");
        let _config = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _shell = crate::test_support::EnvVarGuard::remove("DEEPSEEK_ALLOW_SHELL");
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            "DEEPSEEK_API_KEY=workspace-key\n\
             NVIDIA_API_KEY=repo-must-not-override-shell\n\
             CODEWHALE_HOME=./attacker-home\n\
             CODEWHALE_CONFIG_PATH=./attacker.toml\n\
             DEEPSEEK_ALLOW_SHELL=true\n",
        )
        .expect("write dotenv");

        let report = load_workspace_dotenv_credentials_from_path(&dotenv).expect("safe load");

        assert_eq!(
            std::env::var("DEEPSEEK_API_KEY").as_deref(),
            Ok("workspace-key")
        );
        assert_eq!(std::env::var("NVIDIA_API_KEY").as_deref(), Ok("shell-key"));
        assert!(std::env::var_os("CODEWHALE_HOME").is_none());
        assert!(std::env::var_os("CODEWHALE_CONFIG_PATH").is_none());
        assert!(std::env::var_os("DEEPSEEK_ALLOW_SHELL").is_none());
        assert_eq!(
            report.loaded,
            BTreeSet::from(["DEEPSEEK_API_KEY".to_string()])
        );
        assert_eq!(
            report.ignored,
            BTreeSet::from([
                "CODEWHALE_CONFIG_PATH".to_string(),
                "CODEWHALE_HOME".to_string(),
                "DEEPSEEK_ALLOW_SHELL".to_string(),
            ])
        );
    }

    #[test]
    fn workspace_dotenv_rejects_ambient_variable_substitution() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _ambient = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_JS_SECRET_LEAK_TEST",
            "ambient-secret-must-not-expand",
        );
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            "DEEPSEEK_API_KEY=${CODEWHALE_JS_SECRET_LEAK_TEST}\n",
        )
        .expect("write dotenv");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("expansion must fail closed")
            .to_string();

        assert!(error.contains("variable expansion"));
        assert!(!error.contains("ambient-secret-must-not-expand"));
        assert!(std::env::var_os("DEEPSEEK_API_KEY").is_none());
    }

    #[test]
    fn workspace_dotenv_rejects_multiline_ambient_variable_substitution() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _ambient = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_JS_SECRET_LEAK_TEST",
            "ambient-secret-must-not-expand",
        );
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            "DEEPSEEK_API_KEY=\"prefix\n$CODEWHALE_JS_SECRET_LEAK_TEST=bar\nsuffix\"\n",
        )
        .expect("write dotenv");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("multiline expansion must fail closed")
            .to_string();

        assert!(error.contains("variable expansion"));
        assert!(!error.contains("ambient-secret-must-not-expand"));
        assert!(std::env::var_os("DEEPSEEK_API_KEY").is_none());
    }

    #[test]
    fn workspace_dotenv_comment_quote_cannot_hide_later_expansion() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _ambient = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_JS_SECRET_LEAK_TEST",
            "ambient-secret-must-not-expand",
        );
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            "# unmatched quote in ignored comment: '\n\
             DEEPSEEK_API_KEY=$CODEWHALE_JS_SECRET_LEAK_TEST\n",
        )
        .expect("write dotenv");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("comment quote must not hide expansion")
            .to_string();

        assert!(error.contains("variable expansion"));
        assert!(!error.contains("ambient-secret-must-not-expand"));
        assert!(std::env::var_os("DEEPSEEK_API_KEY").is_none());
    }

    #[test]
    fn workspace_dotenv_allows_single_quoted_literal_dollar() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(&dotenv, "DEEPSEEK_API_KEY='$literal-value'\n").expect("write dotenv");

        load_workspace_dotenv_credentials_from_path(&dotenv).expect("literal dollar load");

        assert_eq!(
            std::env::var("DEEPSEEK_API_KEY").as_deref(),
            Ok("$literal-value")
        );
    }

    #[test]
    fn workspace_dotenv_parse_failure_applies_no_earlier_credentials() {
        let _lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            "DEEPSEEK_API_KEY=must-not-survive\nBROKEN=\"unterminated\n",
        )
        .expect("write dotenv");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("parse failure must be transactional")
            .to_string();

        assert!(error.contains("could not be parsed safely"), "{error}");
        assert!(!error.contains("must-not-survive"));
        assert!(std::env::var_os("DEEPSEEK_API_KEY").is_none());
    }

    #[test]
    fn workspace_dotenv_credential_allowlist_excludes_control_plane_names() {
        for provider in codewhale_config::provider::providers_sorted_for_display() {
            for key in provider.env_vars() {
                assert!(
                    is_workspace_dotenv_credential_key(key),
                    "provider credential {key} must remain supported"
                );
            }
        }
        for key in [
            "CODEWHALE_HOME",
            "CODEWHALE_CONFIG_PATH",
            "DEEPSEEK_CONFIG_PATH",
            "DEEPSEEK_PROFILE",
            "DEEPSEEK_MANAGED_CONFIG_PATH",
            "DEEPSEEK_REQUIREMENTS_PATH",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_MODEL",
            "DEEPSEEK_APPROVAL_POLICY",
            "DEEPSEEK_SANDBOX_MODE",
            "DEEPSEEK_ALLOW_SHELL",
            "DEEPSEEK_YOLO",
            "DEEPSEEK_MCP_CONFIG",
            "CODEWHALE_RUNTIME_TOKEN",
            "PATH",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
        ] {
            assert!(
                !is_workspace_dotenv_credential_key(key),
                "control-plane variable {key} must not load from a workspace"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_dotenv_does_not_follow_symbolic_links() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let external = tmp.path().join("external-credentials");
        let dotenv = tmp.path().join(".env");
        std::fs::write(&external, "DEEPSEEK_API_KEY=external-secret\n")
            .expect("write external fixture");
        symlink(&external, &dotenv).expect("create dotenv symlink");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("symlink must fail closed")
            .to_string();

        assert!(error.contains("securely open"), "{error}");
        assert!(!error.contains("external-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_dotenv_rejects_hard_links_to_external_files() {
        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let external = tmp.path().join("external-credentials");
        let dotenv = tmp.path().join(".env");
        std::fs::write(&external, "DEEPSEEK_API_KEY=external-secret\n")
            .expect("write external fixture");
        std::fs::hard_link(&external, &dotenv).expect("create dotenv hard link");

        let error = load_workspace_dotenv_credentials_from_path(&dotenv)
            .expect_err("hard link must fail closed")
            .to_string();

        assert!(error.contains("multiple filesystem links"), "{error}");
        assert!(!error.contains("external-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_dotenv_rejects_fifo_without_blocking_startup() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::sync::mpsc;
        use std::time::Duration;

        let tmp = tempfile::TempDir::new().expect("temp workspace");
        let dotenv = tmp.path().join(".env");
        let c_path = CString::new(dotenv.as_os_str().as_bytes()).expect("fifo path");
        // SAFETY: `c_path` is a live, NUL-terminated path and the requested
        // mode grants access only to the current user.
        let result = unsafe { libc::mkfifo(c_path.as_ptr(), libc::S_IRUSR | libc::S_IWUSR) };
        assert_eq!(result, 0, "mkfifo failed: {}", io::Error::last_os_error());

        let (tx, rx) = mpsc::channel();
        let worker_path = dotenv.clone();
        let worker = std::thread::spawn(move || {
            let result = load_workspace_dotenv_credentials_from_path(&worker_path)
                .map(|_| "unexpected success".to_string())
                .unwrap_or_else(|error| error.to_string());
            tx.send(result).expect("send loader result");
        });

        let error = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(error) => error,
            Err(timeout) => {
                // Release a regressed blocking reader so the test can fail
                // promptly instead of leaving a stuck process behind.
                let _writer = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&dotenv)
                    .expect("open fifo writer to release blocked reader");
                let _ = rx.recv_timeout(Duration::from_secs(1));
                worker.join().expect("join released loader");
                panic!("workspace .env FIFO blocked startup: {timeout}");
            }
        };
        worker.join().expect("join loader");

        assert!(error.contains("not a regular file"), "{error}");
    }

    #[test]
    fn exec_json_conflicts_with_stream_json_output() {
        let err = Cli::try_parse_from([
            "codewhale",
            "exec",
            "--json",
            "--output-format",
            "stream-json",
            "hello",
        ])
        .expect_err("json summary and stream-json must not mix");

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn exec_stream_events_are_json_lines() {
        let event = ExecStreamEvent::ToolResult {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            output: "line 1\nline 2".to_string(),
            status: "success".to_string(),
            started_at: "2026-07-13T00:00:00Z".to_string(),
            completed_at: "2026-07-13T00:00:01Z".to_string(),
            duration_ms: 1000,
            side_effect_status: "not_started".to_string(),
            error_category: None,
            truncated: Some(false),
            artifact: None,
            result_metadata: None,
        };

        let value = exec_stream_value(&event).expect("serializes");
        let json = serde_json::to_string(&value).expect("serializes");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["type"], "tool_result");
        assert_eq!(parsed["schema"], "codewhale.exec-stream");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["duration_ms"], 1000);
        assert_eq!(parsed["side_effect_status"], "not_started");
    }

    #[test]
    fn workflow_receipt_stream_event_is_one_json_line() {
        let event = ExecStreamEvent::WorkflowEvent {
            run_id: "workflow_1234".to_string(),
            event: serde_json::json!({
                "type": "handoff_promoted",
                "artifact_id": "workflow_1234:agent_1:review-gate:review_report",
                "gate_id": "review-gate",
                "kind": "review_report",
                "from_role": "reviewer",
                "to_role": "verifier",
                "producer_task_id": "agent_1"
            }),
        };

        let value = exec_stream_value(&event).expect("serializes");
        let json = serde_json::to_string(&value).expect("serializes");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["type"], "workflow_event");
        assert_eq!(parsed["schema"], "codewhale.exec-stream");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["run_id"], "workflow_1234");
        assert_eq!(parsed["event"]["type"], "handoff_promoted");
        assert_eq!(
            parsed["event"]["artifact_id"],
            "workflow_1234:agent_1:review-gate:review_report"
        );
        assert_eq!(parsed["event"]["gate_id"], "review-gate");
        assert_eq!(parsed["event"]["kind"], "review_report");
        assert_eq!(parsed["event"]["from_role"], "reviewer");
        assert_eq!(parsed["event"]["to_role"], "verifier");
        assert_eq!(parsed["event"]["producer_task_id"], "agent_1");
        assert!(parsed["event"].get("payload").is_none(), "{parsed}");

        let consumed = ExecStreamEvent::WorkflowEvent {
            run_id: "workflow_1234".to_string(),
            event: serde_json::json!({
                "type": "handoff_consumed",
                "artifact_id": "workflow_1234:agent_1:review-gate:review_report",
                "kind": "review_report",
                "from_role": "reviewer",
                "to_role": "verifier",
                "consumer_task_id": "agent_2"
            }),
        };
        let consumed = exec_stream_value(&consumed).expect("serializes consumed receipt");
        assert_eq!(consumed["type"], "workflow_event");
        assert_eq!(consumed["schema"], "codewhale.exec-stream");
        assert_eq!(consumed["schema_version"], 1);
        assert_eq!(consumed["event"]["type"], "handoff_consumed");
        assert_eq!(
            consumed["event"]["artifact_id"],
            "workflow_1234:agent_1:review-gate:review_report"
        );
        assert_eq!(consumed["event"]["consumer_task_id"], "agent_2");
        assert!(consumed["event"].get("payload").is_none(), "{consumed}");
    }

    #[test]
    fn exec_stream_metadata_redacts_resume_breadcrumbs() {
        let raw_session_id = "abc123fullsecret";
        let event = ExecStreamEvent::Metadata {
            meta: Box::new(ExecStreamMeta {
                receipt_kind: "terminal",
                provider: "deepseek".to_string(),
                provider_id: None,
                model: "deepseek-v4-flash".to_string(),
                route_source: "explicit_or_configured".to_string(),
                input_tokens: Some(123),
                output_tokens: Some(45),
                prompt_cache_hit_tokens: Some(10),
                prompt_cache_miss_tokens: None,
                prompt_cache_write_tokens: None,
                reasoning_tokens: Some(3),
                duration_ms: 2500,
                retry_count: None,
                approval_posture: "ask".to_string(),
                sandbox_posture: "configured_default".to_string(),
                binary_sha256: Some("sha256:binary".to_string()),
                config_sha256: None,
                prompt_sha256: "sha256:prompt".to_string(),
                tool_catalog_sha256: Some("sha256:tools".to_string()),
                input_analysis: ExecStreamInputAnalysis::default(),
                visible_final_answer_chars: 17,
                session_id: exec_stream_session_ref(raw_session_id),
                resume_command: exec_stream_resume_hint(raw_session_id),
                workspace: "/tmp/work".to_string(),
                message_count: 4,
                status: Some("completed".to_string()),
                termination_reason: Some("resolved".to_string()),
                error_category: None,
            }),
        };

        let json = serde_json::to_string(&event).expect("serializes");
        assert!(!json.contains('\n'));
        assert!(!json.contains(raw_session_id));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["type"], "metadata");
        assert_ne!(parsed["meta"]["session_id"], raw_session_id);
        assert!(
            parsed["meta"]["session_id"]
                .as_str()
                .unwrap()
                .starts_with("<redacted:")
        );
        assert_eq!(
            parsed["meta"]["resume_command"],
            "codewhale exec --resume <redacted-session-id>"
        );
        assert_eq!(parsed["meta"]["workspace"], "/tmp/work");
        assert_eq!(parsed["meta"]["message_count"], 4);
        assert_eq!(parsed["meta"]["visible_final_answer_chars"], 17);

        let capture = ExecStreamEvent::SessionCapture {
            content: exec_stream_session_ref(raw_session_id),
        };
        let capture_json = serde_json::to_string(&capture).expect("serializes");
        assert!(!capture_json.contains(raw_session_id));
        let parsed_capture: serde_json::Value =
            serde_json::from_str(&capture_json).expect("valid json");
        assert_eq!(parsed_capture["type"], "session_capture");
        assert_ne!(parsed_capture["content"], raw_session_id);
    }

    #[test]
    fn exec_stream_input_analysis_reports_prompt_composition() {
        let system = SystemPrompt::Text("system rules".to_string());
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "run tests".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "checking context".to_string(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "working".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "exec_shell".to_string(),
                        input: serde_json::json!({"command": "cargo test"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "stdout line\nstderr line".to_string(),
                    is_error: Some(false),
                    content_blocks: Some(vec![serde_json::json!({
                        "type": "text",
                        "text": "structured output"
                    })]),
                }],
            },
        ];

        let analysis = exec_stream_input_analysis(&messages, Some(&system));

        assert_eq!(analysis.user_message_count, 2);
        assert_eq!(analysis.assistant_message_count, 1);
        assert_eq!(analysis.tool_message_count, 0);
        assert_eq!(analysis.tool_use_count, 1);
        assert_eq!(analysis.tool_result_count, 1);
        assert_eq!(analysis.thinking_chars, "checking context".chars().count());
        assert!(analysis.text_chars >= "run testsworking".chars().count());
        assert!(analysis.tool_use_input_chars > 0);
        assert!(analysis.tool_result_chars >= "stdout line\nstderr line".chars().count());
        assert!(analysis.estimated_system_tokens > 0);
        assert!(analysis.estimated_message_content_tokens > 0);
        assert!(
            analysis.estimated_request_tokens
                >= analysis.estimated_system_tokens
                    + analysis.estimated_message_content_tokens
                    + analysis.estimated_framing_tokens
        );
    }

    #[test]
    fn review_receipt_check_public_json_omits_private_details() {
        let validation = crate::tools::review::ReviewReceiptValidation {
            passed: false,
            reason: "secret reason with /tmp/private/receipt.json".to_string(),
            diff_fingerprint: "sha256:current".to_string(),
            receipt_fingerprint: Some("sha256:current".to_string()),
            receipt_path: Some(PathBuf::from("/tmp/private/receipt.json")),
            unresolved_risk: Some(crate::tools::review::ReviewReceiptRisk {
                unresolved: true,
                level: "error".to_string(),
                summary: "secret summary".to_string(),
            }),
        };

        let public = review_receipt_validation_public_json(&validation);
        let encoded = serde_json::to_string(&public).expect("public json");

        assert_eq!(public["passed"], false);
        assert_eq!(public["status"], "unresolved_risk");
        assert_eq!(public["risk_level"], "error");
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("/tmp/private"));
    }

    #[test]
    fn exec_text_session_breadcrumbs_use_compact_ids() {
        let session_id = "1234567890abcdef";

        assert_eq!(exec_saved_session_line(session_id), "session: 12345678");
        assert_eq!(
            exec_resumed_session_line(session_id),
            "resumed session: 12345678"
        );
        assert!(!exec_saved_session_line(session_id).contains(session_id));
        assert!(!exec_resumed_session_line(session_id).contains(session_id));
    }

    #[test]
    fn alternate_screen_defaults_on_in_auto_mode() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    fn no_alt_screen_flag_is_accepted_but_keeps_alternate_screen() {
        let cli = parse_cli(&["codewhale", "--no-alt-screen"]);
        let config = Config::default();

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    fn config_never_is_accepted_but_keeps_alternate_screen() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: Some("never".to_string()),
                mouse_capture: None,
                terminal_probe_timeout_ms: None,
                stream_chunk_timeout_secs: None,
                status_items: None,
                osc8_links: None,
                composer_arrows_scroll: None,
                notification_condition: None,
                header_items: None,
            }),
            ..Config::default()
        };

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    #[cfg(not(windows))]
    fn mouse_capture_defaults_on_when_alternate_screen_is_active() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    #[test]
    #[cfg(windows)]
    fn mouse_capture_defaults_off_on_legacy_windows_console() {
        // Legacy conhost (no `WT_SESSION` and no `ConEmuPID`) keeps the
        // v0.8.x default-off behavior: mouse-mode reporting on legacy console
        // can leak SGR escapes into the composer.
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    // #1169: Windows Terminal sets `WT_SESSION` and handles mouse-mode
    // reporting cleanly, so default-on there gives users in-app text
    // selection (and the side-effect of clamping selection to the
    // transcript region instead of the terminal painting across the
    // sidebar via native selection).
    #[test]
    #[cfg(windows)]
    fn mouse_capture_defaults_on_in_windows_terminal() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            None,
            Some("{a3a3b3a8-aa00-0000-0000-000000000000}"),
            None,
        ));
    }

    // ConEmu/Cmder sets `ConEmuPID` and handles VT mouse-mode reporting
    // cleanly; default mouse capture on there so users get in-app scrolling.
    #[test]
    #[cfg(windows)]
    fn mouse_capture_defaults_on_in_conemu() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            None,
            None,
            Some("12345"),
        ));
    }

    #[test]
    fn no_mouse_capture_flag_disables_mouse_capture() {
        let cli = parse_cli(&["codewhale", "--no-mouse-capture"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    #[test]
    fn config_can_disable_default_mouse_capture() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(false),
                terminal_probe_timeout_ms: None,
                stream_chunk_timeout_secs: None,
                status_items: None,
                osc8_links: None,
                composer_arrows_scroll: None,
                notification_condition: None,
                header_items: None,
            }),
            ..Config::default()
        };

        assert!(!should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    #[test]
    fn mouse_capture_flag_enables_mouse_capture() {
        let cli = parse_cli(&["codewhale", "--mouse-capture"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    #[test]
    fn config_can_enable_mouse_capture() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(true),
                terminal_probe_timeout_ms: None,
                stream_chunk_timeout_secs: None,
                status_items: None,
                osc8_links: None,
                composer_arrows_scroll: None,
                notification_condition: None,
                header_items: None,
            }),
            ..Config::default()
        };

        assert!(should_use_mouse_capture_with(
            &cli, &config, true, None, None, None
        ));
    }

    #[test]
    fn mouse_capture_is_off_without_alternate_screen() {
        let cli = parse_cli(&["codewhale", "--mouse-capture"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(
            &cli, &config, false, None, None, None
        ));
    }

    // Issue #878 / #898: JetBrains JediTerm advertises mouse support but
    // forwards SGR mouse-event escapes as raw input characters, producing
    // the "input box auto-fills with garbled characters when I move the
    // mouse" failure mode in PyCharm/IDEA terminals. Default the capture
    // off when we see TERMINAL_EMULATOR=JetBrains-JediTerm; explicit
    // config / --mouse-capture still wins.

    #[test]
    fn mouse_capture_defaults_off_in_jetbrains_jediterm() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
            None,
            None,
        ));
    }

    #[test]
    fn jetbrains_default_off_is_case_insensitive() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config::default();

        // JetBrains has occasionally varied the casing across releases;
        // a case-insensitive match keeps the protection in place.
        assert!(!should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("jetbrains-jediterm"),
            None,
            None,
        ));
    }

    #[test]
    fn mouse_capture_flag_overrides_jetbrains_default() {
        let cli = parse_cli(&["codewhale", "--mouse-capture"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
            None,
            None,
        ));
    }

    #[test]
    fn config_mouse_capture_true_overrides_jetbrains_default() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(true),
                terminal_probe_timeout_ms: None,
                stream_chunk_timeout_secs: None,
                status_items: None,
                osc8_links: None,
                composer_arrows_scroll: None,
                notification_condition: None,
                header_items: None,
            }),
            ..Config::default()
        };

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
            None,
            None,
        ));
    }
}

#[cfg(test)]
mod interactive_startup_tests {
    use super::*;

    #[test]
    fn interactive_tui_defaults_agent_shell_to_approval_gated_on() {
        let default_config = Config::default();
        assert!(
            interactive_tui_allow_shell(false, &default_config),
            "interactive Agent mode should expose shell tools by default so approvals can gate commands"
        );

        let disabled = Config {
            allow_shell: Some(false),
            ..Config::default()
        };
        assert!(
            !interactive_tui_allow_shell(false, &disabled),
            "explicit allow_shell=false still hides shell tools"
        );

        assert!(
            interactive_tui_allow_shell(true, &disabled),
            "YOLO forces shell access for its no-guardrails contract"
        );
    }
}

#[cfg(test)]
mod project_config_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Write a `<workspace>/.deepseek/config.toml` and return the workspace
    /// root so the merge function can find it.
    fn workspace_with_project_config(body: &str) -> tempfile::TempDir {
        let tmp = tempdir().expect("tempdir");
        let project_dir = tmp.path().join(".deepseek");
        fs::create_dir_all(&project_dir).expect("mkdir .deepseek");
        fs::write(project_dir.join("config.toml"), body).expect("write project config");
        tmp
    }

    #[cfg(unix)]
    #[test]
    fn project_overlay_rejects_symlinked_primary_config() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let primary_dir = workspace.path().join(codewhale_config::CODEWHALE_APP_DIR);
        let legacy_dir = workspace.path().join(codewhale_config::LEGACY_APP_DIR);
        fs::create_dir_all(&primary_dir).expect("mkdir primary");
        fs::create_dir_all(&legacy_dir).expect("mkdir legacy");
        let outside_config = outside.path().join("config.toml");
        fs::write(&outside_config, "model = \"outside-model\"\n").expect("write outside config");
        fs::write(legacy_dir.join("config.toml"), "model = \"legacy-model\"\n")
            .expect("write legacy config");
        std::os::unix::fs::symlink(&outside_config, primary_dir.join("config.toml"))
            .expect("symlink project config");
        let mut config = Config {
            default_text_model: Some("base-model".to_string()),
            ..Config::default()
        };

        merge_project_config(&mut config, workspace.path());

        assert_eq!(
            config.default_text_model.as_deref(),
            Some("base-model"),
            "symlinked primary project config should stop the project overlay"
        );
    }

    fn with_home_dir<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
        }
        let result = f();
        unsafe {
            match prev_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match prev_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
        result
    }

    #[test]
    fn project_overlay_skips_when_workspace_is_home_directory() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let project_dir = tmp.path().join(codewhale_config::CODEWHALE_APP_DIR);
        fs::create_dir_all(&project_dir).expect("mkdir .codewhale");
        fs::write(
            project_dir.join("config.toml"),
            r#"model = "project-override-model""#,
        )
        .expect("write project config");

        with_home_dir(tmp.path(), || {
            let mut config = Config {
                default_text_model: Some("deepseek-v4-flash".to_string()),
                ..Config::default()
            };

            merge_project_config(&mut config, tmp.path());

            assert_eq!(
                config.default_text_model.as_deref(),
                Some("deepseek-v4-flash")
            );
        });
    }

    #[test]
    fn project_overlay_overrides_model_but_denies_provider() {
        // #417: `provider` is on the deny-list; only the `model`
        // override applies. The denied key emits a stderr warning
        // (verified by integration runs; here we assert the post-
        // merge state).
        let tmp = workspace_with_project_config(
            r#"
provider = "nvidia-nim"
model = "deepseek-ai/deepseek-v4-pro"
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.provider, None,
            "#417: project-scope `provider` must be denied"
        );
        assert_eq!(
            config.default_text_model.as_deref(),
            Some("deepseek-ai/deepseek-v4-pro"),
            "model is allowed at project scope"
        );
    }

    #[test]
    fn project_overlay_denies_dangerous_credentials_and_redirects() {
        // #417: `api_key` / `base_url` / `provider` / `mcp_config_path`
        // and MCP OAuth callback settings are all on the deny-list. A
        // malicious project must not be able to redirect prompts, hijack MCP
        // servers, or influence OAuth callback behavior via these.
        let tmp = workspace_with_project_config(
            r#"
api_key = "ATTACKER_KEY"
base_url = "https://evil.example.com"
provider = "nvidia-nim"
mcp_config_path = "/tmp/attacker-mcp.json"
mcp_oauth_callback_port = 9999
mcp_oauth_callback_url = "http://evil.example.com/callback"
"#,
        );
        let mut config = Config {
            api_key: Some("USER_KEY".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            mcp_oauth_callback_port: Some(1455),
            mcp_oauth_callback_url: Some("http://127.0.0.1:1455/callback".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.api_key.as_deref(),
            Some("USER_KEY"),
            "user api_key must survive project-config attack"
        );
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://api.deepseek.com"),
            "user base_url must survive project-config attack"
        );
        assert_eq!(
            config.provider, None,
            "project-scope provider must be denied"
        );
        assert_eq!(
            config.mcp_config_path, None,
            "project-scope mcp_config_path must be denied"
        );
        assert_eq!(
            config.mcp_oauth_callback_port,
            Some(1455),
            "project-scope mcp_oauth_callback_port must be denied"
        );
        assert_eq!(
            config.mcp_oauth_callback_url.as_deref(),
            Some("http://127.0.0.1:1455/callback"),
            "project-scope mcp_oauth_callback_url must be denied"
        );
    }

    #[test]
    fn project_overlay_overrides_approval_and_sandbox() {
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "never"
sandbox_mode = "read-only"
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(config.approval_policy.as_deref(), Some("never"));
        assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
    }

    #[test]
    fn project_overlay_denies_approval_auto_and_sandbox_danger_values() {
        // #417 value-deny: the loosest values (`approval_policy = "auto"`,
        // `sandbox_mode = "danger-full-access"`) are pure escalation.
        // Even when the user hasn't set these fields, the project
        // can't push the session to the loosest posture.
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "auto"
sandbox_mode = "danger-full-access"
model = "deepseek-v4-pro"
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.approval_policy, None,
            "project-scope `approval_policy = \"auto\"` must be denied"
        );
        assert_eq!(
            config.sandbox_mode, None,
            "project-scope `sandbox_mode = \"danger-full-access\"` must be denied"
        );
        // Non-escalation overrides on the same merge succeed —
        // the deny is per-key, not per-file.
        assert_eq!(
            config.default_text_model.as_deref(),
            Some("deepseek-v4-pro"),
            "non-escalation overrides should still apply"
        );
    }

    #[test]
    fn project_overlay_preserves_user_strict_value_when_project_tries_to_loosen() {
        // Belt-and-suspenders: if the user has `approval_policy = "never"`
        // and the project tries `approval_policy = "auto"`, the deny
        // keeps the user's strict value rather than falling through to
        // None.
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "auto"
"#,
        );
        let mut config = Config {
            approval_policy: Some("never".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.approval_policy.as_deref(),
            Some("never"),
            "user's strict approval_policy must survive a project escalation attempt"
        );
    }

    #[test]
    fn project_overlay_preserves_user_policy_when_project_tries_intermediate_loosening() {
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "on-request"
sandbox_mode = "workspace-write"
"#,
        );
        let mut config = Config {
            approval_policy: Some("never".to_string()),
            sandbox_mode: Some("read-only".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(config.approval_policy.as_deref(), Some("never"));
        assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
    }

    #[test]
    fn project_overlay_can_tighten_user_policy() {
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "never"
sandbox_mode = "read-only"
"#,
        );
        let mut config = Config {
            approval_policy: Some("on-request".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(config.approval_policy.as_deref(), Some("never"));
        assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
    }

    #[test]
    fn project_overlay_can_tighten_saved_full_access_posture() {
        let tmp = workspace_with_project_config(
            r#"
approval_policy = "on-request"
"#,
        );
        let mut config = Config::default();

        merge_project_config_with_approval_baseline(&mut config, tmp.path(), Some("full-access"));

        assert_eq!(
            config.approval_policy.as_deref(),
            Some("on-request"),
            "a project may tighten the saved Full Access baseline to Ask"
        );
    }

    #[test]
    fn project_overlay_overrides_max_subagents_and_can_disable_shell() {
        let tmp = workspace_with_project_config(
            r#"
max_subagents = 4
allow_shell = false
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(config.max_subagents, Some(4));
        assert_eq!(config.allow_shell, Some(false));
    }

    #[test]
    fn project_overlay_cannot_enable_shell() {
        let tmp = workspace_with_project_config(
            r#"
allow_shell = true
"#,
        );
        let mut config = Config {
            allow_shell: Some(false),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.allow_shell,
            Some(false),
            "project overlay must not loosen shell access"
        );
    }

    #[test]
    fn user_workspace_overlay_can_enable_shell_for_matching_workspace() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("project");
        fs::create_dir_all(&workspace).expect("mkdir workspace");
        let raw = format!(
            "[workspace.'{}']\nallow_shell = true\n",
            workspace.display()
        );
        let doc: toml::Value = toml::from_str(&raw).expect("parse config");

        let mut config = Config::default();
        merge_user_workspace_config_from_doc(&mut config, &doc, &workspace);

        assert_eq!(config.allow_shell, Some(true));
    }

    #[test]
    fn user_workspace_overlay_accepts_legacy_projects_table() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("project");
        fs::create_dir_all(&workspace).expect("mkdir workspace");
        let raw = format!("[projects.'{}']\nallow_shell = true\n", workspace.display());
        let doc: toml::Value = toml::from_str(&raw).expect("parse config");

        let mut config = Config::default();
        merge_user_workspace_config_from_doc(&mut config, &doc, &workspace);

        assert_eq!(config.allow_shell, Some(true));
    }

    #[test]
    fn user_workspace_overlay_ignores_non_matching_workspace() {
        let tmp = tempdir().expect("tempdir");
        let configured_workspace = tmp.path().join("configured");
        let active_workspace = tmp.path().join("active");
        fs::create_dir_all(&configured_workspace).expect("mkdir configured workspace");
        fs::create_dir_all(&active_workspace).expect("mkdir active workspace");
        let raw = format!(
            "[workspace.'{}']\nallow_shell = true\n",
            configured_workspace.display()
        );
        let doc: toml::Value = toml::from_str(&raw).expect("parse config");

        let mut config = Config::default();
        merge_user_workspace_config_from_doc(&mut config, &doc, &active_workspace);

        assert_eq!(config.allow_shell, None);
    }

    #[test]
    fn user_workspace_overlay_preserves_allow_shell_env_override() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("project");
        fs::create_dir_all(&workspace).expect("mkdir workspace");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            format!(
                "[workspace.'{}']\nallow_shell = true\n",
                workspace.display()
            ),
        )
        .expect("write config");

        unsafe {
            std::env::set_var("DEEPSEEK_ALLOW_SHELL", "false");
        }
        let mut config = Config {
            allow_shell: Some(false),
            ..Config::default()
        };
        merge_user_workspace_config(&mut config, Some(config_path), &workspace);
        unsafe {
            std::env::remove_var("DEEPSEEK_ALLOW_SHELL");
        }

        assert_eq!(config.allow_shell, Some(false));
    }

    #[test]
    fn user_workspace_overlay_does_not_override_managed_config() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("project");
        fs::create_dir_all(&workspace).expect("mkdir workspace");
        let config_path = tmp.path().join("config.toml");
        fs::write(
            &config_path,
            format!(
                "[workspace.'{}']\nallow_shell = true\n",
                workspace.display()
            ),
        )
        .expect("write config");

        let mut config = Config {
            allow_shell: Some(false),
            managed_config_path: Some("managed.toml".to_string()),
            ..Config::default()
        };
        merge_user_workspace_config(&mut config, Some(config_path), &workspace);

        assert_eq!(config.allow_shell, Some(false));
    }

    #[test]
    fn windows_config_path_compare_normalizes_mixed_separators() {
        assert_eq!(
            normalize_windows_config_path_str(r"C:\Users\me\repo"),
            normalize_windows_config_path_str(r"C:/Users/me/repo/")
        );
    }

    #[test]
    fn windows_config_path_compare_normalizes_verbatim_and_unc_prefixes() {
        assert_eq!(
            normalize_windows_config_path_str(r"\\?\C:\Users\me\repo"),
            normalize_windows_config_path_str(r"C:/Users/me/repo")
        );
        assert_eq!(
            normalize_windows_config_path_str(r"\\?\UNC\server\share\repo"),
            normalize_windows_config_path_str(r"\\server/share/repo/")
        );
    }

    #[test]
    fn project_overlay_clamps_max_subagents_to_safe_range() {
        let tmp = workspace_with_project_config(
            r#"
max_subagents = 500
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.max_subagents,
            Some(crate::config::MAX_SUBAGENTS),
            "should clamp to MAX_SUBAGENTS"
        );
    }

    #[test]
    fn project_overlay_ignores_negative_max_subagents() {
        let tmp = workspace_with_project_config(
            r#"
max_subagents = -3
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(config.max_subagents, None, "negative should be ignored");
    }

    #[test]
    fn project_overlay_skips_missing_config_file() {
        let tmp = tempdir().expect("tempdir");
        let mut config = Config {
            provider: Some("codewhale".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Untouched.
        assert_eq!(config.provider.as_deref(), Some("codewhale"));
    }

    #[test]
    fn project_overlay_skips_malformed_toml() {
        let tmp = workspace_with_project_config("this is not valid TOML !!");
        let mut config = Config {
            provider: Some("codewhale".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Untouched on parse error — better to fall back to global than crash.
        assert_eq!(config.provider.as_deref(), Some("codewhale"));
    }

    #[test]
    fn project_overlay_ignores_empty_string_values() {
        let tmp = workspace_with_project_config(
            r#"
provider = ""
model = ""
"#,
        );
        let mut config = Config {
            provider: Some("codewhale".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Empty strings are ignored — they're rarely a deliberate override.
        assert_eq!(config.provider.as_deref(), Some("codewhale"));
        assert_eq!(
            config.default_text_model.as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn project_overlay_ignores_project_instructions_array() {
        let tmp = workspace_with_project_config(
            r#"
instructions = ["./AGENTS.md", "./extra.md"]
"#,
        );
        let user = vec!["~/global.md".to_string()];
        let mut config = Config {
            instructions: Some(user.clone()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.instructions.as_deref(),
            Some(user.as_slice()),
            "project overlay must not replace user-owned instructions"
        );
    }

    #[test]
    fn project_overlay_empty_instructions_array_preserves_user_list() {
        let tmp = workspace_with_project_config(
            r#"
instructions = []
"#,
        );
        let user = vec!["~/global.md".to_string(), "~/team-prefs.md".to_string()];
        let mut config = Config {
            instructions: Some(user.clone()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.instructions.as_deref(),
            Some(user.as_slice()),
            "project overlay must not clear user-owned instructions"
        );
    }

    #[test]
    fn project_overlay_preserves_user_instructions_when_field_absent() {
        let tmp = workspace_with_project_config(
            r#"
provider = "deepseek"
"#,
        );
        let user = vec!["~/global.md".to_string()];
        let mut config = Config {
            instructions: Some(user.clone()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // No `instructions` key in the project file → user list intact.
        assert_eq!(
            config.instructions.as_deref(),
            Some(user.as_slice()),
            "absent project field must not clobber the user list"
        );
    }

    #[test]
    fn project_overlay_ignores_new_instructions_when_user_has_none() {
        let tmp = workspace_with_project_config(
            r#"
instructions = ["./AGENTS.md", "", "  ", "./extra.md"]
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.instructions.as_deref(),
            None,
            "project overlay must not introduce instruction paths"
        );
    }
}

#[cfg(test)]
mod doctor_mcp_tests {
    use super::*;

    fn make_server(command: Option<&str>, args: &[&str], url: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            command: command.map(String::from),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: std::collections::HashMap::new(),
            cwd: None,
            url: url.map(String::from),
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: std::collections::HashMap::new(),
            env_headers: std::collections::HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
            reviewed_plugin: None,
        }
    }

    fn write_path_only_command(dir: &Path) -> String {
        let command = "codewhale-doctor-mcp-path-only-test";
        #[cfg(windows)]
        let file_name = format!("{command}.exe");
        #[cfg(not(windows))]
        let file_name = command.to_string();
        let path = dir.join(file_name);
        std::fs::write(&path, b"test executable").expect("write path-only command");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&path)
                .expect("path-only command metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions)
                .expect("make path-only command executable");
        }
        command.to_string()
    }

    #[test]
    fn test_no_command_or_url_is_error() {
        let server = make_server(None, &[], None);
        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Error(_)
        ));
    }

    #[test]
    fn test_url_server_is_ok() {
        let server = make_server(None, &[], Some("http://localhost:3000/mcp"));
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("HTTP/SSE")),
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn test_command_server_is_ok() {
        let executable = std::env::current_exe().expect("current test executable");
        let executable = executable.to_string_lossy();
        let server = make_server(Some(&executable), &["server.js"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("stdio")),
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn doctor_uses_server_path_for_bare_command_availability() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = write_path_only_command(temp.path());
        let mut server = make_server(Some(&command), &[], None);
        server.env.insert(
            "PATH".to_string(),
            temp.path().to_string_lossy().into_owned(),
        );

        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Ok(_)
        ));
        assert_eq!(
            doctor_mcp_server_json("path-only", &server)["checks"]["command"]["status"],
            "available"
        );

        server.command = Some("codewhale-doctor-mcp-command-that-does-not-exist".to_string());
        #[cfg(not(windows))]
        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Error(detail) if detail.contains("command not found")
        ));
        #[cfg(windows)]
        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Warning(detail) if detail.contains("could not be confirmed")
        ));
        #[cfg(not(windows))]
        assert_eq!(
            doctor_mcp_server_json("missing", &server)["checks"]["command"]["status"],
            "missing"
        );
        #[cfg(windows)]
        assert_eq!(
            doctor_mcp_server_json("missing", &server)["checks"]["command"]["status"],
            "not_checked"
        );
    }

    #[test]
    fn doctor_reports_path_expansion_errors_without_leaking_values() {
        let _lock = crate::test_support::lock_test_env();
        let _missing =
            crate::test_support::EnvVarGuard::remove("CODEWHALE_DOCTOR_MCP_MISSING_PATH");
        let mut server = make_server(Some("codewhale-mcp-command"), &[], None);
        server.env.insert(
            "PATH".to_string(),
            "do-not-leak-${CODEWHALE_DOCTOR_MCP_MISSING_PATH}-also-secret".to_string(),
        );

        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Error(detail) => {
                assert!(detail.contains("CODEWHALE_DOCTOR_MCP_MISSING_PATH"));
                assert!(!detail.contains("do-not-leak"));
                assert!(!detail.contains("also-secret"));
            }
            other => panic!("Expected invalid environment error, got {other:?}"),
        }
        assert_eq!(
            doctor_mcp_server_json("invalid-env", &server)["checks"]["command"]["status"],
            "invalid_environment"
        );
    }

    #[test]
    fn test_relative_stdio_path_arg_without_cwd_warns() {
        let executable = std::env::current_exe().expect("current test executable");
        let executable = executable.to_string_lossy();
        let server = make_server(Some(&executable), &["server/mcp_server.py"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Warning(detail) => {
                assert!(detail.contains("relative path argument"));
                assert!(detail.contains("cwd"));
            }
            other => panic!("Expected Warning for relative path argument, got {other:?}"),
        }
    }

    #[test]
    fn test_relative_stdio_path_arg_with_cwd_is_ok() {
        let executable = std::env::current_exe().expect("current test executable");
        let executable = executable.to_string_lossy();
        let mut server = make_server(Some(&executable), &["server/mcp_server.py"], None);
        server.cwd = Some(PathBuf::from("/tmp/codewhale-project"));
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("stdio")),
            other => panic!("Expected Ok when cwd anchors relative path, got {other:?}"),
        }
    }

    #[test]
    fn test_self_hosted_absolute_is_ok() {
        let server = make_server(Some("/usr/local/bin/codewhale"), &["serve", "--mcp"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) | McpServerDoctorStatus::Error(detail) => {
                // On systems where the path doesn't exist, this will be Error.
                // On systems where it does, it'll be Ok. Either is valid for the test.
                assert!(
                    detail.contains("self-hosted") || detail.contains("not found"),
                    "unexpected detail: {detail}"
                );
            }
            McpServerDoctorStatus::Warning(detail) => {
                panic!("Absolute path should not warn: {detail}")
            }
        }
    }

    #[cfg(test)]
    mod mcp_auth_guidance_tests {
        #[test]
        fn mcp_auth_hint_is_actionable_for_connect_failures() {
            let hint = crate::mcp::oauth::auth_required_login_hint("nordic-mcp");
            assert_eq!(
                hint,
                "MCP server 'nordic-mcp' requires OAuth authentication. Run `codewhale mcp login nordic-mcp` to authenticate."
            );
        }
    }

    #[test]
    fn test_self_hosted_relative_is_warning() {
        #[cfg(unix)]
        let command = "sh";
        #[cfg(windows)]
        let command = "cmd";
        let server = make_server(Some(command), &["serve", "--mcp"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Warning(detail) => {
                assert!(detail.contains("relative"));
            }
            other => panic!("Expected Warning for relative path, got {other:?}"),
        }
    }

    #[test]
    fn test_empty_command_is_error() {
        let server = make_server(Some(""), &[], None);
        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Error(_)
        ));
    }

    #[test]
    fn doctor_json_separates_configuration_from_live_health() {
        let server = make_server(None, &[], Some("http://127.0.0.1:3000/mcp"));
        let report = doctor_mcp_server_json("tools-only", &server);

        assert_eq!(report["check_scope"], "configuration");
        assert_eq!(report["checks"]["configuration"]["status"], "valid");
        assert_eq!(report["checks"]["command"]["status"], "not_applicable");
        assert_eq!(
            report["checks"]["process_reachable"]["status"],
            "not_checked"
        );
        assert_eq!(
            report["checks"]["protocol_initialized"]["status"],
            "not_checked"
        );
        assert_eq!(
            report["checks"]["backend_tool_health"]["status"],
            "not_checked"
        );
        assert!(!report.to_string().contains("healthy"));
    }

    #[cfg(unix)]
    #[test]
    fn static_mcp_check_never_starts_the_configured_command() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("started");
        let script = temp.path().join("mcp-server");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .expect("write test server");
        let mut permissions = std::fs::metadata(&script)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("make script executable");

        let script = script.to_string_lossy();
        let server = make_server(Some(&script), &[], None);
        assert!(matches!(
            doctor_check_mcp_server(&server),
            McpServerDoctorStatus::Ok(_)
        ));
        assert!(!marker.exists(), "static doctor check started MCP server");
    }
}

#[cfg(test)]
mod doctor_live_probe_tests {
    use super::*;

    #[test]
    fn local_provider_probe_requires_explicit_opt_in() {
        assert!(!doctor_should_probe_api(
            crate::config::ApiProvider::Ollama,
            "http://127.0.0.1:11434/v1",
            false,
        ));
        assert!(doctor_should_probe_api(
            crate::config::ApiProvider::Ollama,
            "http://127.0.0.1:11434/v1",
            true,
        ));
    }

    #[test]
    fn custom_loopback_probe_also_requires_explicit_opt_in() {
        assert!(!doctor_should_probe_api(
            crate::config::ApiProvider::Custom,
            "http://localhost:8000/v1",
            false,
        ));
    }

    #[test]
    fn hosted_provider_preserves_the_default_live_check() {
        assert!(doctor_should_probe_api(
            crate::config::ApiProvider::Deepseek,
            "https://api.deepseek.com/beta",
            false,
        ));
    }

    #[test]
    fn oauth_routes_skip_live_probe_to_keep_doctor_non_mutating() {
        let codex = Config {
            provider: Some("openai-codex".to_string()),
            ..Config::default()
        };
        assert!(!doctor_should_probe_auth(&codex));

        let xai = Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        assert!(!doctor_should_probe_auth(&xai));
        assert!(doctor_should_probe_auth(&Config::default()));
    }
}

#[cfg(test)]
mod setup_helper_tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    #[test]
    fn init_tools_dir_creates_readme_and_example() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tools");
        let (returned_dir, readme_status, example_status) =
            init_tools_dir(&dir, false).expect("init_tools_dir should succeed");

        assert_eq!(returned_dir, dir);
        assert!(matches!(readme_status, WriteStatus::Created));
        assert!(matches!(example_status, WriteStatus::Created));
        assert!(dir.join("README.md").exists());
        assert!(dir.join("example.sh").exists());

        let readme = std::fs::read_to_string(dir.join("README.md")).unwrap();
        assert!(
            readme.contains("# name:"),
            "README must show frontmatter convention"
        );

        let example = std::fs::read_to_string(dir.join("example.sh")).unwrap();
        assert!(example.starts_with("#!/usr/bin/env sh"));
        assert!(example.contains("# name: example"));
        assert!(example.contains("# description:"));
    }

    #[test]
    fn init_tools_dir_skips_existing_without_force() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tools");
        let _ = init_tools_dir(&dir, false).unwrap();
        let (_, readme_status, example_status) = init_tools_dir(&dir, false).unwrap();
        assert!(matches!(readme_status, WriteStatus::SkippedExists));
        assert!(matches!(example_status, WriteStatus::SkippedExists));
    }

    #[test]
    fn init_tools_dir_force_overwrites() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("tools");
        let _ = init_tools_dir(&dir, false).unwrap();
        std::fs::write(dir.join("example.sh"), "stale").unwrap();
        let (_, _, example_status) = init_tools_dir(&dir, true).unwrap();
        assert!(matches!(example_status, WriteStatus::Overwritten));
        let example = std::fs::read_to_string(dir.join("example.sh")).unwrap();
        assert_ne!(example, "stale");
    }

    #[test]
    fn init_plugins_dir_creates_readme_and_example_layout() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plugins");
        let (readme_path, manifest_path, skill_path, readme_status, manifest_status, skill_status) =
            init_plugins_dir(&dir, false).unwrap();

        assert_eq!(readme_path, dir.join("README.md"));
        assert_eq!(manifest_path, dir.join("example").join("plugin.toml"));
        assert_eq!(
            skill_path,
            dir.join("example/skills/hello").join("SKILL.md")
        );
        assert!(matches!(readme_status, WriteStatus::Created));
        assert!(matches!(manifest_status, WriteStatus::Created));
        assert!(matches!(skill_status, WriteStatus::Created));
        assert!(readme_path.exists());
        assert!(manifest_path.exists());
        assert!(skill_path.exists());

        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains("schema_version = 1"));
        assert!(manifest.contains("name = \"example\""));
        let validated =
            crate::plugins::manifest::PluginManifest::validate_from_path(&manifest_path)
                .expect("scaffolded plugin should validate");
        assert_eq!(validated.inventory.skills, 1);
    }

    #[test]
    fn collect_clean_targets_finds_all_checkpoint_json_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("latest.json"), "{}").unwrap();
        std::fs::write(dir.join("offline_queue.json"), "[]").unwrap();
        // Per-session crash checkpoint files are clean targets too.
        std::fs::write(dir.join("some-session-id.json"), "{}").unwrap();
        // Non-JSON files and subdirectories are left alone.
        std::fs::write(dir.join("notes.txt"), "keep").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();

        let plan = collect_clean_targets(dir);
        assert_eq!(plan.targets.len(), 3);
        assert!(plan.targets.iter().any(|p| p.ends_with("latest.json")));
        assert!(
            plan.targets
                .iter()
                .any(|p| p.ends_with("offline_queue.json"))
        );
        assert!(
            plan.targets
                .iter()
                .any(|p| p.ends_with("some-session-id.json"))
        );
        assert!(!plan.targets.iter().any(|p| p.ends_with("notes.txt")));
    }

    #[test]
    fn execute_clean_plan_removes_files_and_returns_them() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let latest = dir.join("latest.json");
        let queue = dir.join("offline_queue.json");
        std::fs::write(&latest, "{}").unwrap();
        std::fs::write(&queue, "[]").unwrap();

        let plan = collect_clean_targets(dir);
        let removed = execute_clean_plan(&plan).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!latest.exists());
        assert!(!queue.exists());
    }

    #[test]
    fn run_setup_clean_dry_run_lists_targets_without_force() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("latest.json"), "{}").unwrap();
        run_setup_clean(dir, false).unwrap();
        // Without --force, files must remain on disk.
        assert!(dir.join("latest.json").exists());
    }

    #[test]
    fn run_setup_clean_force_removes_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("latest.json"), "{}").unwrap();
        std::fs::write(dir.join("offline_queue.json"), "[]").unwrap();
        run_setup_clean(dir, true).unwrap();
        assert!(!dir.join("latest.json").exists());
        assert!(!dir.join("offline_queue.json").exists());
    }

    #[test]
    fn run_setup_clean_handles_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("does-not-exist");
        // Should print and return Ok without error.
        run_setup_clean(&dir, true).unwrap();
        assert!(!dir.exists());
    }

    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
        }
        let result = f();
        unsafe {
            match prev_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match prev_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
        result
    }

    #[test]
    fn plain_launch_preserves_checkpoint_but_starts_fresh() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let messages = vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "in flight".to_string(),
                    cache_control: None,
                }],
            }];
            let session = create_saved_session(&messages, "test-model", &workspace, 0, None);
            let session_id = session.metadata.id.clone();
            manager.save_checkpoint(&session).expect("save checkpoint");

            preserve_interrupted_checkpoint_for_explicit_resume(&workspace);

            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_some(),
                "normal launch must leave the per-session checkpoint in place \
                 (it may belong to a live session; `--continue` consumes it)"
            );
            assert!(
                manager.load_session(&session_id).is_ok(),
                "normal launch should keep an explicit resume target"
            );
        });
    }

    #[test]
    fn plain_launch_consumes_legacy_checkpoint_after_preserving_it() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let session = create_saved_session(
                &[Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "legacy in flight".to_string(),
                        cache_control: None,
                    }],
                }],
                "test-model",
                &workspace,
                0,
                None,
            );
            let session_id = session.metadata.id.clone();
            write_legacy_checkpoint(&manager, &session);

            preserve_interrupted_checkpoint_for_explicit_resume(&workspace);

            assert!(
                manager
                    .load_legacy_checkpoint()
                    .expect("load legacy checkpoint")
                    .is_none(),
                "normal launch should consume the legacy single-slot checkpoint"
            );
            assert!(
                manager.load_session(&session_id).is_ok(),
                "normal launch should keep an explicit resume target"
            );
        });
    }

    #[test]
    fn continue_recovers_same_workspace_checkpoint() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let messages = vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "continue me".to_string(),
                    cache_control: None,
                }],
            }];
            let session = create_saved_session(&messages, "test-model", &workspace, 0, None);
            let session_id = session.metadata.id.clone();
            manager.save_checkpoint(&session).expect("save checkpoint");

            let recovered = recover_interrupted_checkpoint_for_resume(&workspace);

            assert_eq!(recovered.as_deref(), Some(session_id.as_str()));
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_none(),
                "--continue should consume the per-session checkpoint"
            );
            assert!(manager.load_session(&session_id).is_ok());
        });
    }

    /// Write a legacy single-slot checkpoint file the way pre-cutover
    /// binaries did. The current binary only reads this slot.
    fn write_legacy_checkpoint(manager: &SessionManager, session: &session_manager::SavedSession) {
        let checkpoints = manager.sessions_dir().join("checkpoints");
        std::fs::create_dir_all(&checkpoints).expect("create checkpoints dir");
        let content = serde_json::to_string_pretty(session).expect("serialize checkpoint");
        std::fs::write(checkpoints.join("latest.json"), content).expect("write legacy checkpoint");
    }

    #[test]
    fn continue_recovers_legacy_checkpoint_and_migrates_it() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let messages = vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "legacy continue".to_string(),
                    cache_control: None,
                }],
            }];
            let session = create_saved_session(&messages, "test-model", &workspace, 0, None);
            let session_id = session.metadata.id.clone();
            write_legacy_checkpoint(&manager, &session);

            let recovered = recover_interrupted_checkpoint_for_resume(&workspace);

            assert_eq!(recovered.as_deref(), Some(session_id.as_str()));
            assert!(
                manager.load_session(&session_id).is_ok(),
                "recovered legacy checkpoint must be loadable as a session"
            );
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load per-session checkpoint")
                    .is_some(),
                "legacy recovery must migrate to a per-session checkpoint file"
            );
            assert!(
                manager
                    .load_legacy_checkpoint()
                    .expect("load legacy checkpoint")
                    .is_some(),
                "legacy latest.json stays in place for one more release"
            );
        });
    }

    #[test]
    fn continue_refuses_checkpoint_from_other_workspace() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let launch_workspace = tmp.path().join("launch-workspace");
        let other_workspace = tmp.path().join("other-workspace");
        std::fs::create_dir_all(&launch_workspace).unwrap();
        std::fs::create_dir_all(&other_workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let messages = vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "belongs elsewhere".to_string(),
                    cache_control: None,
                }],
            }];
            let session = create_saved_session(&messages, "test-model", &other_workspace, 0, None);
            let session_id = session.metadata.id.clone();
            manager.save_checkpoint(&session).expect("save checkpoint");

            let recovered = recover_interrupted_checkpoint_for_resume(&launch_workspace);

            assert_eq!(recovered, None, "workspace mismatch must refuse recovery");
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_some(),
                "another workspace's checkpoint file must be left untouched"
            );
        });
    }

    #[test]
    fn continue_twice_does_not_clobber_newer_session_with_stale_legacy_checkpoint() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let stale = create_saved_session(
                &[Message {
                    role: "user".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "crash-time state".to_string(),
                        cache_control: None,
                    }],
                }],
                "test-model",
                &workspace,
                0,
                None,
            );
            let session_id = stale.metadata.id.clone();
            write_legacy_checkpoint(&manager, &stale);

            // The session advanced after the checkpoint was taken: a newer
            // regular session file exists for the same id.
            let mut advanced = stale.clone();
            advanced.messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "post-recovery progress".to_string(),
                    cache_control: None,
                }],
            });
            advanced.metadata.message_count = advanced.messages.len();
            advanced.metadata.updated_at = stale.metadata.updated_at + chrono::Duration::hours(1);
            manager.save_session(&advanced).expect("save newer session");

            let recovered = recover_interrupted_checkpoint_for_resume(&workspace);

            assert_eq!(recovered.as_deref(), Some(session_id.as_str()));
            let persisted = manager.load_session(&session_id).expect("load session");
            assert_eq!(
                persisted.messages.len(),
                advanced.messages.len(),
                "stale checkpoint content must not overwrite the newer session"
            );
        });
    }

    #[test]
    fn dotenv_status_points_to_example_when_present() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".env.example"), "DEEPSEEK_API_KEY=\n").unwrap();

        assert_eq!(
            dotenv_status_line(tmp.path()),
            ".env not present in workspace (run `cp .env.example .env` and edit)"
        );

        std::fs::write(tmp.path().join(".env"), "DEEPSEEK_API_KEY=test\n").unwrap();
        assert!(dotenv_status_line(tmp.path()).contains(".env present at"));
    }

    #[test]
    fn env_example_is_trackable_and_every_key_is_wired() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let env_example = std::fs::read_to_string(root.join(".env.example")).unwrap();
        let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();

        assert!(gitignore.contains("!.env.example"));

        let keys = documented_env_keys(&env_example);
        for required in [
            "DEEPSEEK_API_KEY",
            "NVIDIA_API_KEY",
            "NVIDIA_NIM_API_KEY",
            "ATLASCLOUD_API_KEY",
        ] {
            assert!(
                keys.contains(required),
                ".env.example is missing {required}"
            );
        }

        for key in &keys {
            assert!(
                is_workspace_dotenv_credential_key(key),
                ".env.example documents non-credential control setting {key}"
            );
        }

        let sources = [
            include_str!("config.rs"),
            include_str!("logging.rs"),
            include_str!("../../config/src/lib.rs"),
            include_str!("../../config/src/provider.rs"),
            include_str!("../../cli/src/main.rs"),
        ]
        .join("\n");

        for key in keys {
            assert!(
                sources.contains(&key),
                ".env.example documents {key}, but no source file references it"
            );
        }
    }

    fn documented_env_keys(content: &str) -> BTreeSet<String> {
        content
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                let uncommented = trimmed
                    .strip_prefix('#')
                    .map(str::trim_start)
                    .unwrap_or(trimmed);
                let (key, _) = uncommented.split_once('=')?;
                let key = key.trim();
                let is_env_key = key
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
                    && key.chars().any(|ch| ch == '_');
                is_env_key.then(|| key.to_string())
            })
            .collect()
    }

    #[test]
    fn resolve_api_key_source_reports_env_when_set() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var("DEEPSEEK_API_KEY").ok();
        let prev_source = std::env::var("DEEPSEEK_API_KEY_SOURCE").ok();
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "test-helper-value");
            std::env::remove_var("DEEPSEEK_API_KEY_SOURCE");
        }
        let cfg = Config::default();
        let source = resolve_api_key_source(&cfg);
        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY") },
        }
        match prev_source {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY_SOURCE", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY_SOURCE") },
        }
        assert_eq!(source, ApiKeySource::Env);
    }

    #[test]
    fn resolve_api_key_source_reports_dispatcher_keyring() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var("DEEPSEEK_API_KEY").ok();
        let prev_source = std::env::var("DEEPSEEK_API_KEY_SOURCE").ok();
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "test-helper-value");
            std::env::set_var("DEEPSEEK_API_KEY_SOURCE", "keyring");
        }
        let cfg = Config::default();
        let source = resolve_api_key_source(&cfg);
        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY") },
        }
        match prev_source {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY_SOURCE", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY_SOURCE") },
        }
        assert_eq!(source, ApiKeySource::Keyring);
    }

    #[test]
    fn resolve_api_key_source_reports_standalone_secret_store() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().expect("temp home");
        let codewhale_home = temp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("create codewhale home");
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        codewhale_secrets::Secrets::auto_detect()
            .set("deepseek", "standalone-secret")
            .expect("save secret");

        assert_eq!(
            resolve_api_key_source(&Config::default()),
            ApiKeySource::Keyring
        );
    }

    #[test]
    fn custom_provider_env_source_precedes_saved_secret_store() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().expect("temp home");
        let codewhale_home = temp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("create codewhale home");
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _declared_env =
            crate::test_support::EnvVarGuard::set("QA_CUSTOM_API_KEY", "declared-env-key");
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        codewhale_secrets::Secrets::auto_detect()
            .set("custom", "saved-custom-secret")
            .expect("save secret");

        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "qa-gateway".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://gateway.example.test/v1".to_string()),
                model: Some("qa-model".to_string()),
                api_key_env: Some("QA_CUSTOM_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("qa-gateway".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::Env);
        assert_eq!(
            config.deepseek_api_key().expect("custom key"),
            "declared-env-key"
        );
    }

    #[test]
    fn named_custom_provider_does_not_report_generic_secret_store() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().expect("temp home");
        let codewhale_home = temp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("create codewhale home");
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        codewhale_secrets::Secrets::auto_detect()
            .set("custom", "unrelated-custom-secret")
            .expect("save secret");

        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "qa-gateway".to_string(),
            crate::config::ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://gateway.example.test/v1".to_string()),
                model: Some("qa-model".to_string()),
                auth_mode: Some("api_key".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("qa-gateway".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::Missing);
        assert!(config.deepseek_api_key().is_err());
    }

    #[test]
    fn custom_built_in_endpoint_does_not_report_ambient_provider_key() {
        let _lock = crate::test_support::lock_test_env();
        let _openrouter =
            crate::test_support::EnvVarGuard::set("OPENROUTER_API_KEY", "ambient-key");
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openrouter.base_url = Some("https://gateway.example.test/v1".to_string());
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(providers),
            ..Config::default()
        };

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::Missing);
        assert!(config.deepseek_api_key().is_err());
    }

    #[test]
    fn auth_mode_none_reports_distinct_no_auth_source_and_scheme() {
        let _lock = crate::test_support::lock_test_env();
        let _openrouter =
            crate::test_support::EnvVarGuard::set("OPENROUTER_API_KEY", "ambient-key");
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openrouter.auth_mode = Some("none".to_string());
        providers.openrouter.api_key = Some("configured-key".to_string());
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(providers),
            ..Config::default()
        };

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::NoAuth);
        assert_eq!(doctor_api_key_source_label(ApiKeySource::NoAuth), "none");
        assert_eq!(doctor_auth_scheme(&config), "none");
        assert_eq!(config.deepseek_api_key().expect("no-auth route"), "");
    }

    #[test]
    fn resolve_api_key_source_prefers_config_over_env() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var("DEEPSEEK_API_KEY").ok();
        let prev_source = std::env::var("DEEPSEEK_API_KEY_SOURCE").ok();
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", "stale-env-key");
            std::env::remove_var("DEEPSEEK_API_KEY_SOURCE");
        }
        let cfg = Config {
            api_key: Some("fresh-config-key".to_string()),
            ..Config::default()
        };
        let source = resolve_api_key_source(&cfg);
        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY") },
        }
        match prev_source {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_API_KEY_SOURCE", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_API_KEY_SOURCE") },
        }
        assert_eq!(source, ApiKeySource::Config);
    }

    #[test]
    fn resolve_api_key_source_reports_active_provider_env_from_metadata() {
        let _guard = crate::test_support::lock_test_env();
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _anthropic_key =
            crate::test_support::EnvVarGuard::set("ANTHROPIC_API_KEY", "test-anthropic-key");
        let cfg = Config {
            provider: Some("anthropic".to_string()),
            ..Config::default()
        };

        let source = resolve_api_key_source(&cfg);

        assert_eq!(source, ApiKeySource::Env);
    }

    #[test]
    fn resolve_api_key_source_ignores_unresolved_provider_command_metadata() {
        let _guard = crate::test_support::lock_test_env();
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _openai_key = crate::test_support::EnvVarGuard::remove("OPENAI_API_KEY");
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.auth = Some(codewhale_config::ProviderAuthSourceToml {
            source: codewhale_config::AuthSourceKind::Command,
            command: vec!["secret-tool".to_string(), "lookup".to_string()],
            timeout_ms: Some(2000),
            secret_id: None,
        });
        let cfg = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Config::default()
        };

        let source = resolve_api_key_source(&cfg);

        assert_eq!(source, ApiKeySource::Missing);
        assert!(cfg.deepseek_api_key().is_err());
    }

    #[test]
    fn resolve_api_key_source_ignores_unresolved_provider_secret_metadata() {
        let _guard = crate::test_support::lock_test_env();
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _openai_key = crate::test_support::EnvVarGuard::remove("OPENAI_API_KEY");
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.auth = Some(codewhale_config::ProviderAuthSourceToml {
            source: codewhale_config::AuthSourceKind::Secret,
            command: Vec::new(),
            timeout_ms: None,
            secret_id: Some("codewhale/openai".to_string()),
        });
        let cfg = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Config::default()
        };

        let source = resolve_api_key_source(&cfg);

        assert_eq!(source, ApiKeySource::Missing);
        assert!(cfg.deepseek_api_key().is_err());
    }

    #[test]
    fn resolve_api_key_source_ignores_root_deepseek_key_for_other_provider() {
        let _guard = crate::test_support::lock_test_env();
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _openrouter_key = crate::test_support::EnvVarGuard::remove("OPENROUTER_API_KEY");
        let cfg = Config {
            provider: Some("openrouter".to_string()),
            api_key: Some("legacy-deepseek-root-key".to_string()),
            ..Config::default()
        };

        let source = resolve_api_key_source(&cfg);

        assert_eq!(source, ApiKeySource::Missing);
    }

    #[test]
    fn provider_status_helpers_use_provider_metadata() {
        assert_eq!(
            provider_env_vars_label(crate::config::ApiProvider::NvidiaNim),
            "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY / DEEPSEEK_API_KEY"
        );
        assert_eq!(
            provider_config_table_key(crate::config::ApiProvider::Anthropic),
            "anthropic"
        );
        assert_eq!(
            provider_config_table_key(crate::config::ApiProvider::SiliconflowCn),
            "siliconflow_cn"
        );
        assert!(
            provider_auth_hint(crate::config::ApiProvider::OpenaiCodex).contains("PROVIDERS.md")
        );
    }

    #[test]
    fn skills_count_for_returns_zero_for_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("nope");
        assert_eq!(skills_count_for(&dir), 0);
    }

    #[test]
    fn skills_count_for_counts_valid_skill_dirs() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("skills");
        let skill_dir = dir.join("getting-started");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: getting-started\ndescription: hi\n---\nbody",
        )
        .unwrap();
        assert_eq!(skills_count_for(&dir), 1);
    }
}

#[cfg(test)]
mod pr_prompt_tests {
    use super::*;

    fn sample_pr() -> GhPullRequest {
        GhPullRequest {
            title: "Add cool feature".to_string(),
            body: "Closes #99.\n\nAlso:\n- bullet a\n- bullet b".to_string(),
            base: "main".to_string(),
            head: "feat/cool".to_string(),
            url: "https://github.com/example/repo/pull/123".to_string(),
        }
    }

    #[test]
    fn format_pr_prompt_includes_title_url_branches_body_and_diff() {
        let prompt = format_pr_prompt(123, &sample_pr(), "diff --git a/x b/x\n+y");
        assert!(prompt.contains("Review PR #123 — Add cool feature"));
        assert!(prompt.contains("URL: https://github.com/example/repo/pull/123"));
        assert!(prompt.contains("Branches: main ← feat/cool"));
        assert!(prompt.contains("Closes #99."));
        assert!(prompt.contains("- bullet a"));
        assert!(prompt.contains("```diff"));
        assert!(prompt.contains("diff --git a/x b/x"));
    }

    #[test]
    fn format_pr_prompt_handles_empty_body_and_unknown_branches() {
        let pr = GhPullRequest {
            title: String::new(),
            body: "   ".to_string(),
            base: String::new(),
            head: String::new(),
            url: String::new(),
        };
        let prompt = format_pr_prompt(7, &pr, "(diff body)");
        // Empty title falls back to a placeholder.
        assert!(prompt.contains("(PR #7)"));
        // Empty body renders the explicit placeholder.
        assert!(prompt.contains("(no description)"));
        assert!(prompt.contains("Branches: (unknown)"));
        assert!(prompt.contains("URL: (unavailable)"));
    }

    #[test]
    fn format_pr_prompt_truncates_oversize_diff_at_a_codepoint_boundary() {
        // 300 KiB of `X` bytes with a multibyte char near the cap.
        let mut diff = "X".repeat(190 * 1024);
        diff.push_str(&"🚀".repeat(5_000));
        let prompt = format_pr_prompt(1, &sample_pr(), &diff);
        assert!(prompt.contains("[…diff truncated"));
        assert!(prompt.contains("at 200 KiB"));
        // Ensure we didn't slice mid-codepoint — the result still
        // round-trips as valid UTF-8 (it's a String, so this is by
        // construction; the test pins behaviour against silent panics
        // if the cut logic regresses).
        assert!(prompt.is_ascii() || prompt.contains('🚀'));
    }

    #[test]
    fn is_command_available_detects_present_and_absent_binaries() {
        // `sh` is part of the POSIX baseline on every Unix runner and
        // ships with `git-bash` on Windows CI. It should be present.
        // (Skip on Windows CI without git-bash because the runner
        // could legitimately lack `sh.exe`.)
        #[cfg(unix)]
        assert!(is_command_available("sh"), "POSIX `sh` should be on PATH");

        // A deliberately-implausible name to confirm the negative
        // branch — `--version` on this would exec(3) → ENOENT.
        assert!(
            !is_command_available("this-command-cannot-exist-codewhale-tui-test-ENOENT-marker"),
            "missing command should return false, not panic"
        );
    }
}
