//! Codewhale TUI library — single-binary entry point.

#![allow(clippy::uninlined_format_args)]

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
mod agy_credentials;
mod approval_log;
mod artifacts;
mod audit;
mod auto_reasoning;
mod automation_manager;
mod chatgpt_oauth;
mod child_env;
mod client;
pub mod cloud_dispatch;
mod codex_model_cache;
mod command_safety;
mod commands;
mod compaction;
mod composer_history;
mod composer_stash;
pub mod computer_meter;
mod config;
mod config_persistence;
mod config_ui;
mod context_budget;
mod context_report;
mod continual_harness;
mod core;
mod cost_status;
mod credentials;
mod deepseek_theme;
mod dependencies;
pub mod dispatch_runner;
mod doctor;
mod doctor_fix;
mod dsh_credentials;
mod elapsed;
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
mod image_attach;
mod import_claude;
mod integrations;
mod lane_control;
mod llm_client;
mod llm_response_cache;
mod localization;
mod logging;
mod lsp;
mod mcp;
mod mcp_server;
mod media_originals;
mod model_catalog;
mod model_context;
mod model_inventory;
mod model_profile;
mod model_registry;
mod model_routing;
mod models;
mod models_dev_live;
mod native_memory;
mod network_policy;
mod oauth;
mod operate;
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
mod remote_control;
mod remote_setup;
pub mod repl;
mod repo_law;
mod request_manifest;
mod request_tuning;
mod resource_telemetry;
mod retry_status;
pub mod rlm;
mod route_billing;
mod route_budget;
mod route_receipt;
mod route_runtime;
mod runtime_api;
mod runtime_chat_relay;
mod runtime_handoff;
mod runtime_log;
mod runtime_policy;
mod runtime_threads;
mod safe_label;
mod sandbox;
mod scorecard;
#[allow(dead_code)]
mod session_diagnostics;
// Acceptance matrix for #2934 / #4397. Test-only: the table documents the
// contract for reviewers and is enforced by the tests beside it, so it does
// not need to exist in a shipped binary.
#[cfg(test)]
#[path = "main/tests.rs"]
mod doctor_loader_tests;
#[cfg(test)]
mod session_control_acceptance;
#[allow(dead_code)]
mod session_manager;
mod session_peek;
mod session_projection;
mod session_resume;
pub mod session_tree;
mod settings;
mod shell_dispatcher;
mod skill_state;
mod skills;
mod snapshot;
mod startup_trace;
mod task_manager;
mod telemetry_notice;
#[cfg(test)]
mod test_support;
// TLS bootstrap and platform client builders live in codewhale-release;
// `crate::tls::*` keeps resolving for every caller.
use codewhale_release::tls;
mod todo_snapshot;
mod tool_history_repair;
mod tool_inspection;
mod tool_output_receipts;
mod tools;
mod tui;
mod turn_route_plan;
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
    is_relative_stdio_path_arg,
};
use crate::models::Role;
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};
use crate::session_manager::{SessionManager, create_saved_session, truncate_id};
use crate::tui::app::ScreenMode;
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
    version = env!("CODEWHALE_BUILD_VERSION"),
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

    /// Maximum number of concurrent sub-agents (1-128; default 64)
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

    /// Start account-owned web remote control for this interactive session.
    #[arg(long, hide = true)]
    remote_control: bool,

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
    /// Sign in to your Codewhale account (use the `codewhale` CLI).
    Login {
        /// Legacy provider-key flag: rejected with a redirect to `auth set`.
        #[arg(long, hide = true)]
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
    /// Manage local Agent fleet runs and workers (`fleet` is a compatibility alias)
    #[command(name = "fleet")]
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
    /// Inspect feature flags
    Features(FeaturesCli),
    /// Connect third-party harnesses through Codewhale (currently: DeepSeek Harness `dsh`)
    Integrations {
        #[command(subcommand)]
        command: IntegrationsCommand,
    },
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
    /// Comma-separated list of canonical tools to allow (all others denied).
    /// Names are case-insensitive: Bash, File, Git, Run, etc.
    #[arg(long, value_delimiter = ',')]
    allowed_tools: Option<Vec<String>>,
    /// Comma-separated list of tools to deny (deny wins over allow).
    #[arg(long, value_delimiter = ',')]
    disallowed_tools: Option<Vec<String>>,
    /// Maximum number of model steps before the run ends. Omitted means unlimited.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_turns: Option<u32>,
    /// Maximum number of tool calls admitted in one model turn. Omitted means unlimited.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_tool_calls: Option<u32>,
    /// Shut down when the parent closes this process's stdin. Fleet workers
    /// pass this automatically: a dead manager must not leave detached workers
    /// spending forever (R7).
    #[arg(long, default_value_t = false)]
    parent_death_watch: bool,
    /// Extra text appended to the system prompt for this run.
    #[arg(long)]
    append_system_prompt: Option<String>,
    /// Internal Fleet worker authority envelope. Non-secret, versioned JSON.
    #[arg(long, value_name = "JSON", hide = true)]
    tool_authority_json: Option<String>,
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
    /// Sign in with ChatGPT for Codex subscription access (PKCE loopback).
    #[command(name = "chatgpt")]
    Chatgpt,
    /// Revoke Codewhale-owned ChatGPT tokens. Codex CLI consent is unchanged.
    #[command(name = "chatgpt-revoke")]
    ChatgptRevoke,
}

const CODEWHALE_TOOL_SURFACE_ENV: &str = "CODEWHALE_TOOL_SURFACE";
const SHELL_ONLY_EXEC_TOOLS: &[&str] = &["bash"];

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
    /// List durable fleet runs from this workspace's ledger
    List,
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
    /// fleet run id
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
///
/// Registration is synchronous, before the spawn: a `tokio::spawn`ed task does
/// not run until the scheduler first polls it, so registering the signal
/// streams *inside* it leaves a window — unbounded under load — where SIGINT
/// still has its default disposition and kills the process outright. That is
/// the very outcome this handler exists to prevent, and it produced a real
/// terminated-by-signal exit (no code, no terminal restore, no `session_end`).
/// After this function returns, the signals are armed.
fn spawn_signal_cleanup_task() {
    let signals = TerminatingSignals::register();
    tokio::spawn(async move {
        let exit_code = signals.wait().await;
        // If we get here a fatal signal arrived. Restore the terminal
        // and exit. A second signal during cleanup re-enters this
        // path and aborts via `std::process::exit` directly.
        static CLEANED_UP: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !CLEANED_UP.swap(true, std::sync::atomic::Ordering::SeqCst) {
            #[cfg(unix)]
            crate::tools::shell::abort_pending_persistent_process_groups_for_exit();
            crate::tui::ui::emergency_restore_terminal();
            // Nothing async survives the `exit` below, so this is the last
            // chance to say how the session ended. `record_blocking` is one
            // `O_APPEND` write with no lock: taking the compaction lock here
            // would let a second Codewhale process sharing CODEWHALE_HOME hang
            // Ctrl-C, and the second-signal short-circuit below has to stay
            // reachable. A no-op unless this process was armed.
            //
            // The class is stated, not derived: `RunTerminationReason::Canceled`
            // also exits 130, so `exit_code` cannot tell a signal from an
            // Esc-cancelled turn.
            record_signal_session_end();
        }
        std::process::exit(exit_code);
    });
}

/// When this process's armed telemetry session began. Set once, at arming, and
/// read from both the ordinary teardown and the signal path.
static TELEMETRY_SESSION_START: std::sync::OnceLock<std::time::Instant> =
    std::sync::OnceLock::new();

/// Build `session_end` from what this process actually accumulated.
///
/// The exit class is read from the process-wide atomic and never derived from
/// an exit code: `RunTerminationReason::Canceled` maps to 130, the same value
/// the SIGINT path uses, so a code-based derivation would report every
/// Esc-cancelled turn as a signal.
///
/// The cold-start bucket is `None` unless the interactive event loop actually
/// began, which is what keeps it absent rather than invented on the surfaces
/// that have no event loop.
fn telemetry_session_end() -> codewhale_telemetry::Event {
    let counters = codewhale_telemetry::session_counters();
    codewhale_telemetry::Event::SessionEnd {
        duration_bucket: codewhale_telemetry::DurationBucket::from_secs(
            TELEMETRY_SESSION_START
                .get()
                .map_or(0, |start| start.elapsed().as_secs()),
        ),
        exit_class: codewhale_telemetry::exit_class(),
        cold_start_bucket: crate::startup_trace::cold_start_ms()
            .map(codewhale_telemetry::ColdStartBucket::from_millis),
        providers: counters.providers(),
        counters: counters.counters(),
        errors: counters.errors(),
        turn_wall: counters.turn_wall(),
    }
}

/// Close the session synchronously, from the signal handler.
///
/// A no-op unless this process was armed.
fn record_signal_session_end() {
    codewhale_telemetry::set_exit_class(codewhale_telemetry::ExitClass::Signal);
    codewhale_telemetry::record_blocking(telemetry_session_end());
}

/// Terminating-signal streams, registered up front and awaited later.
///
/// Splitting registration from the await is the point: the OS disposition
/// changes when `register` returns, not when the waiting task is first polled.
#[cfg(unix)]
struct TerminatingSignals {
    sigint: Option<tokio::signal::unix::Signal>,
    sigterm: Option<tokio::signal::unix::Signal>,
    sighup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl TerminatingSignals {
    /// Install the handlers. Failing to install any individual stream is
    /// non-fatal: we still want the others to work.
    fn register() -> Self {
        use tokio::signal::unix::{SignalKind, signal};
        Self {
            sigint: signal(SignalKind::interrupt()).ok(),
            sigterm: signal(SignalKind::terminate()).ok(),
            sighup: signal(SignalKind::hangup()).ok(),
        }
    }

    /// Resolve with 128 + signal number for whichever arrives first. The
    /// fallback never-resolving future keeps `select!` well-typed when a
    /// stream failed to register.
    async fn wait(mut self) -> i32 {
        tokio::select! {
            _ = async { match self.sigint.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 130,
            _ = async { match self.sigterm.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 143,
            _ = async { match self.sighup.as_mut() { Some(s) => { s.recv().await; }, None => std::future::pending::<()>().await, } } => 129,
        }
    }
}

/// Windows: `ctrl_c` covers both Ctrl+C and Ctrl+Break (CTRL_C_EVENT /
/// CTRL_BREAK_EVENT). Console-close, logoff, and shutdown events are not
/// currently routed through tokio.
#[cfg(not(unix))]
struct TerminatingSignals {
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
}

#[cfg(not(unix))]
impl TerminatingSignals {
    fn register() -> Self {
        Self {
            ctrl_c: tokio::signal::windows::ctrl_c().ok(),
        }
    }

    async fn wait(mut self) -> i32 {
        match self.ctrl_c.as_mut() {
            Some(s) => {
                s.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
        130
    }
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
    let read = || {
        ["CODEWHALE_MODEL", "DEEPSEEK_MODEL"]
            .into_iter()
            .find_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|model| model.trim().to_string())
                    .filter(|model| !model.is_empty())
            })
    };
    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
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

/// Fold the dispatcher-forwarded launch overrides (`CODEWHALE_PROVIDER` /
/// `CODEWHALE_MODEL`, set by `codewhale --provider X --model Y exec ...`)
/// into the explicit route signals `exec --resume`/`--continue` honour.
///
/// Exec-level flags win when both are present; either source counts as
/// "the user named a route for this run", so a resume must not silently
/// restore the saved provider/model over it.
fn exec_resume_route_overrides(
    exec_provider: Option<&str>,
    exec_model: Option<&str>,
    launch_provider: Option<&str>,
    launch_model: Option<&str>,
) -> (bool, Option<String>) {
    let non_empty = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let explicit_provider =
        non_empty(exec_provider).is_some() || non_empty(launch_provider).is_some();
    let explicit_model = non_empty(exec_model).or_else(|| non_empty(launch_model));
    (explicit_provider, explicit_model)
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
    /// Emit machine-readable structural JSON output (always offline)
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
    /// Opt in to probing the configured hosted provider API
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    probe_api: bool,
    /// Opt in to contacting the release service for an update check
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    check_updates: bool,
    /// Opt in to starting enabled MCP servers and checking process/protocol reachability
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    probe_mcp: bool,
    /// Opt in to a credential-free transport probe of the selected search provider
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    probe_search: bool,
    /// Plan and apply automatic repairs with consent (#5552)
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    fix: bool,
    /// Apply the planned repairs without prompting (requires --fix)
    #[arg(long, default_value_t = false, requires = "fix")]
    yes: bool,
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

    /// Output audio path. Defaults to `speech.<format>` in `--output-dir`,
    /// `[speech].output_dir`, or the current directory.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Directory for the default `speech.<format>` output file when `-o`/`--output` is omitted.
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
    #[arg(long, conflicts_with_all = ["pr", "base"])]
    staged: bool,
    /// Review GitHub pull request #N instead of a local diff (fetched via `gh`)
    #[arg(long, conflicts_with_all = ["staged", "base", "path"])]
    pr: Option<u32>,
    /// Repository in `owner/name` form for --pr. Defaults to the current
    /// workspace's `gh` config (i.e. the repo gh thinks you're in).
    #[arg(long, requires = "pr")]
    repo: Option<String>,
    /// Post the review to the pull request (one COMMENT review with inline
    /// line comments plus a summary). Requires --pr; without it the review
    /// is only printed locally.
    #[arg(long, requires = "pr")]
    post: bool,
    /// Base ref to diff against (e.g. origin/main)
    #[arg(long, conflicts_with_all = ["staged", "pr"])]
    base: Option<String>,
    /// Limit diff to a specific path
    #[arg(long)]
    path: Option<PathBuf>,
    /// Override model for this review
    #[arg(long)]
    model: Option<String>,
    /// Override the provider route for this review (e.g. `zai`, `openrouter`,
    /// `deepseek`). Non-secret identifier only — credentials still resolve
    /// from the environment/config. Use it to disambiguate a `--model` that
    /// more than one configured route offers (which otherwise hard-errors
    /// with "available from configured provider route(s): ...").
    #[arg(long)]
    provider: Option<String>,
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
    /// Bind host for HTTP server (default loopback; mobile is always loopback-only)
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
}

fn resolve_serve_bind_host(_mobile: bool, host: Option<String>) -> ServeBindHost {
    ServeBindHost {
        host: host.unwrap_or_else(|| "127.0.0.1".to_string()),
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

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum IntegrationsCommand {
    /// Official DeepSeek Harness (`dsh`) connected through Codewhale
    Dsh {
        #[command(subcommand)]
        command: DshIntegrationCommand,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum DshIntegrationCommand {
    /// Detect dsh and report the integration state without writing anything
    Status {
        /// Emit machine-readable JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show exactly what `connect`/`update` would write, without writing it
    Plan {
        #[arg(long, default_value_t = false)]
        json: bool,
        /// DSH profile the overlay targets (`web` or `headless`)
        #[arg(long, default_value = "web")]
        profile: String,
        /// Mirror Codewhale full access as DSH danger-full-access (only when Codewhale itself runs with full access)
        #[arg(long, default_value_t = false)]
        allow_full_access: bool,
        /// Record the Codewhale palette (skin) decision for the bundle profile; applied via DSH's `overrideTokens`, never through the overlay
        #[arg(long, default_value_t = false)]
        skin: bool,
    },
    /// Write the overlay and receipt under $CODEWHALE_HOME/integrations/dsh
    Connect {
        #[arg(long, default_value = "web")]
        profile: String,
        #[arg(long, default_value_t = false)]
        allow_full_access: bool,
        #[arg(long, default_value_t = false)]
        skin: bool,
        /// Confirm the disclosed plan without an interactive prompt (required when stdin is not a terminal)
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Re-derive the overlay from the current Codewhale route
    Update {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, default_value_t = false)]
        allow_full_access: bool,
        /// Turn the bundle-profile skin on/off (`--skin false`; defaults to the previous choice)
        #[arg(long)]
        skin: Option<bool>,
        /// Turn the ambient ocean scene behind the DSH web UI on/off (`--ocean false`; defaults to the previous choice, initially on; needs the skin)
        #[arg(long)]
        ocean: Option<bool>,
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Run dsh with the Codewhale overlay; extra args go to the dsh app
    Launch {
        /// Override the recorded profile (`web` or `headless`)
        #[arg(long)]
        profile: Option<String>,
        /// Print the exact command instead of running it
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Keep the overlay but refuse launches
    Disable,
    /// Allow launches again
    Enable,
    /// Delete Codewhale-owned files only; $DSH_HOME is never touched
    Remove {
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Documented DSH plugin path: install the Codewhale bundle into a dedicated `codewhale` DSH profile via `dsh plugin add` (pnpm required)
    InstallBundle {
        /// Which shipped DSH app the dedicated profile boots (`web` or `headless`)
        #[arg(long, default_value = "web")]
        app: String,
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// `dsh plugin --profile codewhale remove codewhale-dsh-bundle`, then delete only Codewhale-owned bundle files
    RemoveBundle {
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
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

/// Pre-clap seam feeding `apply_process_hardening` (#5723): resolve only the
/// *startup* sandbox posture — `CODEWHALE_SANDBOX_MODE` /
/// `DEEPSEEK_SANDBOX_MODE` first, then the config file's `sandbox_mode` key —
/// so the irreversible `PR_SET_NO_NEW_PRIVS` decision can honor a
/// `danger-full-access` launch.
///
/// This is deliberately a narrow single-key read, not a config-system
/// reorder: the full pipeline (clap flags such as `--config`/`exec
/// --sandbox`, profiles, managed and project overlays) cannot run before
/// process hardening, which must land before Tokio and any worker threads.
/// What the seam cannot see keeps the hardened default — fail-closed. The
/// one deliberate gap in the other direction: a later-resolved override that
/// *tightens* a config-file `danger-full-access` (e.g. managed requirements)
/// leaves the flag off; `CODEWHALE_NO_NEW_PRIVS=1` remains the explicit
/// override that forces it on in any posture. The env path override
/// (`CODEWHALE_CONFIG_PATH`) is honored through `resolve_load_config_path`;
/// only clap-parsed paths are invisible here.
fn resolve_startup_sandbox_mode_for_hardening() -> Option<String> {
    if let Ok(value) =
        std::env::var("CODEWHALE_SANDBOX_MODE").or_else(|_| std::env::var("DEEPSEEK_SANDBOX_MODE"))
    {
        return Some(value);
    }
    let path = crate::config::resolve_load_config_path(None)
        .ok()
        .flatten()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let doc = raw.parse::<toml::Value>().ok()?;
    doc.get("sandbox_mode")
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

/// Entry point for the single binary. Takes argv including binary name at 0,
/// parses with clap, and runs the TUI/runtime dispatch. Returns process exit
/// code for the caller to exit with.
pub fn run(args: Vec<String>) -> std::process::ExitCode {
    match run_with_args(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            for cause in err.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// Internal implementation that mirrors the old `main()` but takes explicit
/// args instead of reading `std::env::args()`. Used by `run()` and tested
/// directly.
fn run_with_args(args: Vec<String>) -> Result<()> {
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
    // The startup-posture read is the narrow seam documented above: a startup
    // resolved to danger-full-access skips PR_SET_NO_NEW_PRIVS (#5723), every
    // other outcome keeps it.
    let startup_sandbox_mode = resolve_startup_sandbox_mode_for_hardening();
    crate::sandbox::process_hardening::apply_process_hardening(startup_sandbox_mode.as_deref());

    // ── Fatal-signal terminal guard (#5424) ───────────────────────────────
    // Abort-class deaths (stack overflow, allocation failure, double panic)
    // skip the panic hook AND every Drop guard, leaving mouse capture and
    // the kitty keyboard stack leaking into the user's shell. A classic
    // sigaction handler restores the terminal and stamps a marker before
    // re-raising. Also before any threads exist.
    crate::tui::ui::fatal_signal_guard::install_fatal_signal_guard();

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

        // Telemetry, if and only if this process was armed. This hook is
        // installed before `Cli::parse()` and long before any config is
        // resolved, so it cannot consult a resolved value — but it can consult
        // a `OnceLock` that is by construction empty until resolution
        // completes. A user who never opted in panics without writing a byte
        // and without creating a directory.
        //
        // The site is allowlist-reduced and `msg` is deliberately not read: a
        // slicing panic embeds the entire string being sliced, and this tree
        // slices user and model text in dozens of places.
        codewhale_telemetry::set_exit_class(codewhale_telemetry::ExitClass::Panic);
        if let Some(site) = panic_info
            .location()
            .map(|loc| codewhale_telemetry::reduce_panic_site(loc.file(), loc.line(), loc.column()))
        {
            codewhale_telemetry::record_blocking(codewhale_telemetry::Event::Panic { site });
        }
        // Write crash dump best-effort
        if let Some(home) = crate::config::effective_home_dir() {
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
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            e.exit();
        }
    };
    // #5098: project-scope fleet agent profiles (`.codewhale/agents/*.toml`)
    // join the dispatch roster under the same trust decision as the rest of
    // project-level config — `--no-project-config` opts the layer out for
    // every roster read in this process.
    crate::fleet::roster::set_project_agent_profiles_enabled(!cli.no_project_config);
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

fn run_async_main(
    cli: Cli,
    command: Option<Commands>,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    build_runtime(command.as_ref())?.block_on(run_async_main_inner(
        cli,
        command,
        plugin_discovery,
        plugin_registry,
    ))
}

/// Build the runtime that owns every async task in this binary.
///
/// `#[tokio::main]` used to expand here, which left every worker thread on
/// tokio's 2 MiB default while only the `codewhale-main` owner thread above
/// received `CODEWHALE_MAIN_STACK_BYTES`. The engine does not run on that owner
/// thread — `core::engine::spawn_engine` hands `Engine::run` to
/// `utils::spawn_supervised`, a bare `tokio::spawn` — so the explicit stack
/// never applied where the depth actually is.
///
/// A debug-build `agent` dispatch (turn_loop -> FuturesUnordered ->
/// execute_rich_full_with_context -> AgentTool::execute -> spawn_subagent_from_input)
/// measured a stack high-water mark between 2.25 and 2.5 MiB and aborted the
/// whole process on the guard page. A Rust stack overflow is not a panic: it
/// raises SIGABRT, so `spawn_supervised`'s `catch_unwind` cannot see it and the
/// process dies with 134 mid-dispatch.
///
/// Build the runtime that owns every async task in this binary.
///
/// `command` selects the worker-count policy: read-only diagnostic commands
/// run on a small fixed pool instead of tokio's one-worker-per-CPU default
/// (see [`diagnostic_worker_count`]). Interactive sessions and servers keep
/// the default sizing unchanged.
///
/// `#[tokio::main]` used to expand here, which left every worker thread on
/// tokio's 2 MiB default while only the `codewhale-main` owner thread above
/// received `CODEWHALE_MAIN_STACK_BYTES`. The engine does not run on that owner
/// thread — `core::engine::spawn_engine` hands `Engine::run` to
/// `utils::spawn_supervised`, a bare `tokio::spawn` — so the explicit stack
/// never applied where the depth actually is.
///
/// A debug-build `agent` dispatch (turn_loop -> FuturesUnordered ->
/// execute_rich_full_with_context -> AgentTool::execute -> spawn_subagent_from_input)
/// measured a stack high-water mark between 2.25 and 2.5 MiB and aborted the
/// whole process on the guard page. A Rust stack overflow is not a panic: it
/// raises SIGABRT, so `spawn_supervised`'s `catch_unwind` cannot see it and the
/// process dies with 134 mid-dispatch.
///
/// This is behavior-identical to the old `#[tokio::main]` expansion apart from
/// the stack size, and it makes the knob greppable.
pub(crate) fn build_runtime(command: Option<&Commands>) -> Result<tokio::runtime::Runtime> {
    let mut builder = tokio_runtime_builder();
    if let Some(workers) = diagnostic_worker_count(command) {
        builder.worker_threads(workers);
    }
    builder
        .build()
        .context("Failed to build the Codewhale Tokio runtime")
}

/// Number of async workers to request from tokio.
///
/// Unset means tokio's default: one worker per CPU. That default is right for
/// interactive sessions and long-running servers, but short-lived offline
/// commands gain nothing from a full-CPU pool — they pay thread spawn, stack
/// reservation, and teardown futex traffic for capacity they never use (perf
/// attribution: pthread_create under `Builder::build` dominates init samples).
const DIAGNOSTIC_WORKER_CAP: usize = 2;

fn diagnostic_worker_count(command: Option<&Commands>) -> Option<usize> {
    let capped = match command {
        // Read-only diagnostic surfaces (doctor family).
        Some(
            Commands::Doctor(_)
            | Commands::Eval(_)
            | Commands::SessionDiagnostics(_)
            | Commands::Sessions { .. },
        ) => true,
        // Only the read-only status report; mutating setup keeps defaults.
        Some(Commands::Setup(args)) => args.status,
        _ => false,
    };
    capped.then_some(DIAGNOSTIC_WORKER_CAP)
}

fn tokio_runtime_builder() -> tokio::runtime::Builder {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .enable_all()
        .thread_stack_size(CODEWHALE_MAIN_STACK_BYTES);
    builder
}

/// Which product surface this process is serving.
///
/// A function of the parsed subcommand, never of the executable: this one
/// binary serves at least five surfaces, so `current_exe()` would label all of
/// them the same.
fn telemetry_surface(command: Option<&Commands>) -> codewhale_telemetry::Surface {
    use codewhale_telemetry::Surface;
    match command {
        None | Some(Commands::Resume { .. } | Commands::Fork { .. } | Commands::Pr { .. }) => {
            Surface::Tui
        }
        Some(Commands::Exec(_)) => Surface::Exec,
        Some(Commands::Serve(args)) => {
            if args.mcp {
                Surface::McpServer
            } else {
                Surface::Serve
            }
        }
        Some(_) => Surface::Cli,
    }
}

/// How this session was started, for `session_start`.
fn telemetry_session_source(command: Option<&Commands>) -> codewhale_telemetry::SessionSource {
    use codewhale_telemetry::SessionSource;
    match command {
        None | Some(Commands::Pr { .. }) => SessionSource::Interactive,
        Some(Commands::Resume { .. }) => SessionSource::Resume,
        Some(Commands::Fork { .. }) => SessionSource::Fork,
        Some(Commands::Serve(_)) => SessionSource::Api,
        Some(_) => SessionSource::Unknown,
    }
}

/// Read-only commands must not create telemetry state as a side effect.
fn telemetry_command_is_read_only(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(Commands::Doctor(_) | Commands::SessionDiagnostics(_) | Commands::Sessions { .. })
    ) || matches!(command, Some(Commands::Setup(args)) if args.status)
}

/// Resolve the emit predicate and arm, once, before anything can record.
///
/// This is the read that v1 of the design was missing entirely:
/// `resolve_runtime_options` had no non-test caller in this crate, so neither
/// `telemetry = false` in the config file nor `CODEWHALE_TELEMETRY=0` was ever
/// consulted by a process that would have emitted.
///
/// `CliRuntimeOverrides::default()` is correct here. The dispatcher has already
/// applied the kill-switch floor and forwarded the *resolved* value through
/// `CODEWHALE_TELEMETRY`, which `EnvRuntimeOverrides::load()` picks up — and
/// re-reading `CODEWHALE_TELEMETRY` inside the telemetry crate would fork
/// `parse_bool`, the `DEEPSEEK_TELEMETRY` alias, and the floor into a second
/// source of truth.
fn arm_telemetry_with_setup(
    config_path: Option<PathBuf>,
    surface: codewhale_telemetry::Surface,
    source: codewhale_telemetry::SessionSource,
    setup_override: Option<&codewhale_config::SetupState>,
) {
    let Ok(store) = codewhale_config::ConfigStore::load(config_path) else {
        return;
    };
    let resolved = store
        .config
        .resolve_runtime_options(&codewhale_config::CliRuntimeOverrides::default());
    let setup = if let Some(setup) = setup_override {
        setup.clone()
    } else {
        let Some(setup) = codewhale_telemetry::load_setup_state_for_decision() else {
            // An existing unreadable privacy record may contain a decline.
            // Failing closed is safer than replacing it with default-on.
            return;
        };
        setup
    };
    let codewhale_telemetry::TelemetryDecision::Enabled(consent) =
        codewhale_telemetry::decide(&resolved, &setup, surface)
    else {
        return;
    };
    codewhale_telemetry::init(consent.with_config_path(Some(store.path().to_path_buf())));
    let _ = TELEMETRY_SESSION_START.set(std::time::Instant::now());
    codewhale_telemetry::record(codewhale_telemetry::Event::SessionStart { source });
}

fn arm_telemetry(cli: &Cli, command: Option<&Commands>) {
    if telemetry_command_is_read_only(command) {
        return;
    }
    arm_telemetry_with_setup(
        cli.config.clone(),
        telemetry_surface(command),
        telemetry_session_source(command),
        None,
    );
}

/// Apply the choice made in the native TUI disclosure.
///
/// The in-memory setup state is authoritative for this process. In particular,
/// a Disable choice reaches `decide` as an opt-out even when neither durable
/// write landed, so the current launch cannot arm and any existing buffer is
/// wiped whenever the telemetry home remains reachable.
pub(crate) fn apply_tui_telemetry_decision(
    pending: &crate::telemetry_notice::PendingTelemetryNotice,
    setup: &codewhale_config::SetupState,
) {
    arm_telemetry_with_setup(
        pending.config_path.clone(),
        codewhale_telemetry::Surface::Tui,
        pending.session_source,
        Some(setup),
    );
}

/// Close the armed session and flush, bounded.
///
/// Short CLI (`config`, `doctor`, `auth`, …) records `session_end`, seals the
/// local queue within a much smaller deadline, and returns. The 3s network
/// flush is a TUI/exec concern: a hung TLS handshake must not hold
/// `codewhale config list`. A configured endpoint remains buffered for the
/// next interactive session; an explicitly empty endpoint writes its local
/// dry-run batch immediately.
async fn finish_telemetry(outcome: &Result<()>, surface: codewhale_telemetry::Surface) {
    if !codewhale_telemetry::is_armed() {
        return;
    }
    // Only escalate: the panic hook and the signal path have already spoken if
    // they ran, and a stated class must not be overwritten by an inferred one.
    if outcome.is_err()
        && codewhale_telemetry::exit_class() == codewhale_telemetry::ExitClass::Clean
    {
        codewhale_telemetry::set_exit_class(codewhale_telemetry::ExitClass::Error);
    }
    codewhale_telemetry::record(telemetry_session_end());
    if surface == codewhale_telemetry::Surface::Cli {
        let _ =
            codewhale_telemetry::persist_local_blocking(codewhale_telemetry::CLI_PERSIST_TIMEOUT);
        return;
    }
    // `shutdown_blocking` parks a thread waiting on the writer, so it goes to
    // the blocking pool, and it is bounded there. The persistence actor's
    // unbounded `let _ = task.await` next door is not a pattern to copy here: a
    // hung TLS handshake would hold the process open past the last frame.
    let _ = tokio::time::timeout(
        codewhale_telemetry::SHUTDOWN_FLUSH_TIMEOUT,
        tokio::task::spawn_blocking(|| {
            codewhale_telemetry::shutdown_blocking(codewhale_telemetry::SHUTDOWN_FLUSH_TIMEOUT)
        }),
    )
    .await;
}

async fn run_async_main_inner(
    cli: Cli,
    command: Option<Commands>,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    // Install signal handlers that restore the terminal before the process
    // exits. Without this, Ctrl+C delivered while raw mode / kitty keyboard
    // enhancement / alt-screen are active (or in the brief windows around
    // startup and teardown where they're being toggled) leaves the user's shell
    // receiving raw CSI sequences like `^[[>5u` until they run `reset` (#1583).
    //
    // Once the TUI's raw mode is engaged the terminal driver delivers Ctrl+C as
    // the byte 0x03 rather than SIGINT, so the in-TUI key handler — not this
    // handler — is what processes user interrupts during normal operation. This
    // handler exists for the gaps: pre-TUI subcommands (--version, doctor,
    // login, …), the moments around enable_raw_mode / disable_raw_mode, the
    // external-editor suspend path, and SIGTERM / SIGHUP from the OS.
    //
    // It goes up before arming and before the notice: arming is the first
    // externally observable thing this process does (it creates the telemetry
    // buffer), and the notice is the first thing that can sit waiting on a
    // human. A Ctrl-C in either window must still restore the terminal and exit
    // 130 rather than kill the process outright. Recording a `session_end` from
    // the signal path is a no-op until `arm_telemetry` runs, so installing
    // ahead of it collects nothing.
    spawn_signal_cleanup_task();

    // A due interactive disclosure belongs to the first native TUI frame. In
    // that one case arming is deferred until its decision event; every other
    // surface keeps the ordinary pre-dispatch predicate. This is what lets an
    // immediate Disable choice stop this very session without printing or
    // blocking on a shell questionnaire first.
    let surface = telemetry_surface(command.as_ref());
    let telemetry_notice_plan = if surface == codewhale_telemetry::Surface::Tui {
        crate::telemetry_notice::plan_if_due(
            cli.config.clone(),
            telemetry_session_source(command.as_ref()),
        )
    } else {
        crate::telemetry_notice::TelemetryNoticePlan::NotDue
    };
    let should_arm_before_dispatch = surface != codewhale_telemetry::Surface::Tui
        || telemetry_notice_plan.should_arm_before_tui();
    let pending_telemetry_notice = telemetry_notice_plan.into_pending();
    if should_arm_before_dispatch {
        arm_telemetry(&cli, command.as_ref());
    }
    let outcome = run_async_main_dispatch(
        cli,
        command,
        plugin_discovery,
        plugin_registry,
        pending_telemetry_notice,
    )
    .await;
    finish_telemetry(&outcome, surface).await;
    outcome
}

async fn run_async_main_dispatch(
    cli: Cli,
    command: Option<Commands>,
    plugin_discovery: Arc<crate::plugins::PluginDiscoveryContext>,
    plugin_registry: Arc<crate::plugins::PluginRegistry>,
    mut pending_telemetry_notice: Option<crate::telemetry_notice::PendingTelemetryNotice>,
) -> Result<()> {
    logging::set_verbose(cli.verbose || logging::env_requests_verbose_logging());

    // Install any user prompt overrides from the config directory before an
    // engine can compose a system prompt. The override cells are
    // first-call-wins; doing this once here keeps every downstream turn
    // consistent. Missing files are a no-op (bundled defaults). See #3638.
    crate::prompts::load_prompt_overrides_from_config_home();

    // Plugins own one read-only discovery snapshot per process. Initialize it
    // before the subcommand match so plain launch, resume, fork, exec, serve,
    // and every other runtime surface use the same plugin trust decision
    // (#3916, #4399). Discovery never enables, trusts, executes, or persists a
    // bundle.

    // Handle subcommands first
    if let Some(command) = command {
        return match command {
            Commands::Doctor(args) => {
                let config = match load_doctor_config_from_cli(&cli, &args) {
                    Ok(config) => config,
                    Err(error) if args.json => return run_doctor_json_config_error(&error),
                    Err(_) => {
                        bail!(
                            "doctor configuration validation failed; details omitted because configuration errors may contain credential material"
                        )
                    }
                };
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
                    let probes = crate::doctor::DoctorProbeRequest {
                        check_updates: args.check_updates,
                        probe_api: args.probe_api,
                        probe_local: args.probe_local,
                        probe_mcp: args.probe_mcp,
                        probe_search: args.probe_search,
                    };
                    run_doctor(
                        &config,
                        &workspace,
                        cli.config.as_deref(),
                        probes,
                        plugin_registry.as_ref(),
                    )
                    .await;
                    if args.fix {
                        let plan = crate::doctor_fix::plan_fixes(
                            &config,
                            &workspace,
                            plugin_registry.as_ref(),
                        );
                        crate::doctor_fix::print_fix_plan(&plan);
                        if !plan.is_empty() && (args.yes || crate::doctor_fix::confirm_fix(&plan)) {
                            let results = crate::doctor_fix::apply_fixes(&plan);
                            crate::doctor_fix::print_apply_results(&results);
                        }
                    }
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
                TuiAuthCommand::Chatgpt => run_chatgpt_pkce_auth(cli.config.as_deref()).await,
                TuiAuthCommand::ChatgptRevoke => run_chatgpt_pkce_revoke(cli.config.as_deref()),
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
                // #4641: `--no-project-config` skips the workspace-specific
                // `[workspace]`/`[projects]` user-config overlay so a headless
                // launch (e.g. a future Verifiers harness) sees a reproducible
                // config surface that depends only on the explicit `--config`.
                if !cli.no_project_config {
                    merge_user_workspace_config(&mut config, cli.config.clone(), &workspace);
                }
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
                let explicit_reasoning = args
                    .reasoning_effort
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(reasoning_arg) = explicit_reasoning {
                    config.reasoning_effort = normalize_cli_reasoning_effort(reasoning_arg)?;
                    config.reasoning_effort_inferred_from_legacy_alias = false;
                }
                let prompt = join_prompt_parts(&args.prompt);
                let resume_session_id = resolve_exec_resume_session_id(&args, &workspace)?;
                validate_exec_tool_authority_resume(
                    args.tool_authority_json.as_deref(),
                    resume_session_id.is_some(),
                )?;
                let resume_session = resume_session_id
                    .as_deref()
                    .map(load_exec_resume_session)
                    .transpose()?;
                let explicit_model = args
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|model| !model.is_empty());
                if resume_session.is_none() {
                    let explicit_route_override = explicit_provider.is_some()
                        || explicit_model.is_some()
                        || crate::config::explicit_launch_provider_override().is_some()
                        || crate::config::explicit_launch_model_override().is_some();
                    apply_selected_fleet_operator_for_launch(
                        &mut config,
                        &workspace,
                        explicit_route_override,
                        explicit_reasoning.is_some(),
                    )?;
                }
                // The `codewhale` dispatcher refuses `--provider`/`--model`
                // after `exec` and forwards the top-level flags as
                // `CODEWHALE_PROVIDER` / `CODEWHALE_MODEL` instead, so a
                // resume must treat those launch overrides as explicit or it
                // silently restores the saved route (cloud-agent e2e,
                // 2026-08-30).
                let (resume_explicit_provider, resume_explicit_model) = exec_resume_route_overrides(
                    explicit_provider,
                    explicit_model,
                    crate::config::explicit_launch_provider_override().as_deref(),
                    crate::config::explicit_launch_model_override().as_deref(),
                );
                let model = if let Some(saved) = resume_session.as_ref() {
                    resolve_exec_resume_route(
                        &mut config,
                        saved,
                        resume_explicit_provider,
                        resume_explicit_model.as_deref(),
                    )?
                } else {
                    resolve_exec_model(&config, explicit_model)
                };
                let force_configured_route = should_force_configured_exec_route(
                    resume_session.is_some(),
                    explicit_provider,
                    explicit_model,
                );
                // A launcher can forward `--yolo` to this binary via the
                // CODEWHALE_YOLO env var (which the config loader folds into
                // `config.yolo`), not as a CLI flag. Honour either source.
                let yolo = cli.yolo || config.yolo.unwrap_or(false);
                let env_tool_surface = exec_tool_surface_from_env();
                let needs_engine = args.auto
                    || yolo
                    || resume_session_id.is_some()
                    || args.output_format == ExecOutputFormat::StreamJson
                    || args.max_turns.is_some()
                    || args.max_tool_calls.is_some()
                    || args.allowed_tools.is_some()
                    || args.disallowed_tools.is_some()
                    || args.append_system_prompt.is_some()
                    || args.tool_authority_json.is_some()
                    || args.sandbox.is_some()
                    || args.allow_sandbox_elevation
                    || env_tool_surface.is_some();
                if needs_engine {
                    if args.parent_death_watch {
                        spawn_parent_death_watch();
                    }
                    let provider = config.api_provider();
                    let max_subagents = cli.max_subagents.map_or_else(
                        || config.max_subagents_for_provider(provider),
                        |value| value.clamp(1, MAX_SUBAGENTS),
                    );
                    let auto_mode = args.auto || yolo;
                    let max_turns = exec_max_steps(args.max_turns);
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
                        args.max_tool_calls,
                        allowed_tools,
                        disallowed_tools,
                        args.append_system_prompt.clone(),
                        args.tool_authority_json.clone(),
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
                    pending_telemetry_notice.take(),
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
            Commands::Features(command) => {
                let config = load_config_from_cli(&cli)?;
                run_features_command(&config, command)
            }
            Commands::Integrations { command } => {
                // Identity derivation is structural: credential-bearing
                // environment values never enter this path.
                let config = load_structural_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                integrations::cli::run(&config, &workspace, command)
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
                    let (mut config, config_profile) =
                        load_config_from_cli_with_effective_profile(&cli)?;
                    let explicit_route_override =
                        crate::config::explicit_launch_provider_override().is_some()
                            || crate::config::explicit_launch_model_override().is_some();
                    apply_selected_fleet_operator_for_launch(
                        &mut config,
                        &workspace,
                        explicit_route_override,
                        false,
                    )?;
                    let cors_origins = resolve_cors_origins(&config, &args.cors_origin);
                    let bind_host = resolve_serve_bind_host(args.mobile, args.host);
                    if args.web && bind_host.host != "127.0.0.1" {
                        bail!("Codewhale web is loopback-only and must bind to 127.0.0.1");
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
                    pending_telemetry_notice.take(),
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
                    pending_telemetry_notice.take(),
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
            pending_telemetry_notice.take(),
            std::sync::Arc::clone(&plugin_registry),
        )
        .await;
    }

    // Handle session resume. Plain `codewhale` starts fresh: interrupted
    // snapshots are preserved for explicit resume, but never auto-attached.
    let mut startup_notice = None;
    let resume_session_id = if cli.continue_session {
        let workspace = resolve_workspace(&cli);
        resolve_continue_session_id(
            &workspace,
            io::stdin().is_terminal() && io::stdout().is_terminal(),
        )
    } else if let Some(id) = cli.resume.clone() {
        Some(id)
    } else if !cli.fresh {
        let workspace = resolve_workspace(&cli);
        preserve_interrupted_checkpoint_for_explicit_resume(&workspace);
        // Opt-in auto-resume (#2934). Off by default, so the historical
        // "plain `codewhale` starts fresh" behaviour is unchanged unless the
        // user asked for something else. The decision never resumes an
        // archived, unreadable, or foreign-workspace session; every fallback
        // carries a receipt rather than silently starting blank.
        let (session_id, notice) = resolve_auto_resume(&workspace);
        startup_notice = notice;
        session_id
    } else {
        None
    };

    // Default: Interactive TUI
    // --yolo starts in YOLO mode (auto-approve; shell enabled)
    run_interactive_with_notice(
        &cli,
        &config,
        resume_session_id,
        None,
        startup_notice,
        pending_telemetry_notice.take(),
        plugin_registry,
    )
    .await
}

/// Resolve the opt-in auto-resume setting into a session id plus a receipt.
///
/// Deliberately scoped to the plain interactive launch. `codewhale "do X"`
/// (top-level prompt) and `codewhale exec` are not covered: silently prefixing
/// a one-shot task with a prior conversation would change what is sent to the
/// model, which is not a layout preference the user opted into.
fn resolve_auto_resume(workspace: &Path) -> (Option<String>, Option<String>) {
    use crate::session_resume::{ResumeRequest, decide_auto_resume};

    let enabled = crate::settings::Settings::load_persisted()
        .map(|settings| settings.session_auto_resume)
        .unwrap_or(false);
    if !enabled {
        return (None, None);
    }
    let Ok(manager) = SessionManager::default_location() else {
        return (None, None);
    };
    let decision = decide_auto_resume(true, &ResumeRequest::default(), workspace, &manager);
    (
        decision.session_id().map(str::to_string),
        decision.status_message(),
    )
}

fn prepare_cli_startup(
    cli: Cli,
    initialize_plugins: impl FnOnce(),
    load_dotenv: impl FnOnce(),
) -> (Cli, Option<Commands>) {
    initialize_plugins();
    let command = cli.command.clone();
    let should_load_dotenv = match command.as_ref() {
        Some(Commands::Doctor(args)) => args.probe_api || args.probe_local,
        _ => true,
    };
    if should_load_dotenv {
        load_dotenv();
    }
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
    use crate::fleet::control as fleet_control;
    use crate::fleet::executor::FleetExecutor;
    use crate::fleet::manager::{FleetManager, FleetStatusSnapshot, FleetWorkerInspection};
    use codewhale_lane::{ControlOperation, ControlSurface};
    use codewhale_protocol::fleet::{FleetAlertEventClass, FleetArtifactKind, FleetRunId};

    // Every label and every row below comes from the shared Fleet control
    // surface, so `codewhale fleet …` and `/fleet …` cannot drift in how they
    // describe the same durable ledger (#1888, #4022).
    fn print_status(status: &FleetStatusSnapshot) {
        println!("{}", fleet_control::render_fleet_status_snapshot(status));
    }

    fn print_inspection(inspection: &FleetWorkerInspection) {
        println!("{}", fleet_control::render_inspection(inspection));
    }

    fn print_artifacts(inspection: &FleetWorkerInspection) {
        println!("{}", fleet_control::render_artifacts(inspection));
    }

    /// Print one shared control receipt on the CLI surface.
    fn emit_fleet_receipt(receipt: &codewhale_lane::ControlReceipt) -> Result<()> {
        if receipt.is_error() {
            eprintln!("{}", receipt.render());
            let detail = receipt
                .failure
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| receipt.outcome.as_str().to_string());
            bail!("{}: {detail}", receipt.operation_id);
        }
        println!("{}", receipt.render());
        Ok(())
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
                .with_context(|| format!("reading Fleet log {}", path.display()))?;
            let preview: String = contents.chars().take(16 * 1024).collect();
            // Worker logs can contain captured terminal bytes (a child TUI's
            // mouse-tracking handshake, SGR, OSC). Printing them raw would
            // re-arm mouse reporting in the caller's shell and leave it
            // executing escape fragments after this command exits.
            let mut safe_preview = String::with_capacity(preview.len());
            crate::tui::osc8::strip_ansi_into(&preview, &mut safe_preview);
            print!("{safe_preview}");
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
    let provider = config.api_provider();
    let max_subagents = config.max_subagents_for_provider(provider);
    let coordination_manager = crate::tools::subagent::new_shared_subagent_manager_with_timeout(
        workspace.to_path_buf(),
        max_subagents,
        config
            .max_admitted_subagents_for_provider(provider)
            .max(max_subagents),
        Duration::from_secs(config.subagent_heartbeat_timeout_secs_for_provider(provider)),
        config.launch_concurrency_for_provider(provider),
        config.subagent_token_budget_for_provider(provider),
    );
    // Probe the durable ledger *before* opening the manager: FleetManager::open
    // creates `.codewhale/fleet.jsonl` as a side effect, so a later probe would
    // always find a ledger and the CLI would report availability differently
    // from the slash surface for the same workspace (#4022).
    let fleet_context = fleet_control::fleet_control_context(workspace);
    // Probing is not enough on its own: `FleetManager::open` *creates* the
    // ledger, and it used to run for every subcommand before this match. That
    // made `codewhale fleet status` in a ledgerless workspace print
    // "no_fleet_ledger" while simultaneously creating the file it said was
    // missing — and the next invocation then reported an empty ledger as if a
    // Fleet had existed all along. Refuse the control verbs here, before the
    // manager exists, so the CLI and `/fleet` agree and neither surface
    // conjures the store it is reporting on (#4022).
    if let Some(operation) = match &args.command {
        FleetCommand::List => Some(ControlOperation::FleetList),
        FleetCommand::Status => Some(ControlOperation::FleetStatus),
        FleetCommand::Interrupt { .. } => Some(ControlOperation::FleetInterrupt),
        FleetCommand::Resume { .. } => Some(ControlOperation::FleetResume),
        _ => None,
    } {
        let descriptor = operation.descriptor();
        let availability = descriptor.availability(ControlSurface::Cli, fleet_context);
        if !availability.is_available() {
            return emit_fleet_receipt(&codewhale_lane::ControlReceipt::unavailable(
                descriptor,
                ControlSurface::Cli,
                availability,
            ));
        }
    }

    // The configured route is the operator: fleet workers without a
    // task/profile model pin inherit the session's active model.
    let manager = FleetManager::open(workspace)?
        .with_exec_config(fleet_config.exec.clone())
        .with_fleet_config(fleet_config)
        .with_sub_agent_manager(coordination_manager)
        .with_session_model(config.default_model())
        .with_route_config(config.clone());
    match args.command {
        FleetCommand::Init => {
            println!("Fleet ledger: {}", manager.ledger_path().display());
            Ok(())
        }
        FleetCommand::Run(args) => {
            let max_workers = args.max_workers.clamp(1, 128);
            let manager =
                manager.with_stale_after(Duration::from_secs(args.stale_after_seconds.max(1)));
            let report = manager.create_run_from_task_spec_path(&args.task_spec, max_workers)?;
            println!(
                "Fleet run: {} tasks={} leased={} queued={}",
                report.run_id.0, report.task_count, report.leased, report.queued
            );
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
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
        FleetCommand::List => emit_fleet_receipt(&fleet_control::execute_fleet_control_with(
            ControlSurface::Cli,
            workspace,
            fleet_context,
            &manager,
            ControlOperation::FleetList,
            None,
        )),
        FleetCommand::Status => emit_fleet_receipt(&fleet_control::execute_fleet_control_with(
            ControlSurface::Cli,
            workspace,
            fleet_context,
            &manager,
            ControlOperation::FleetStatus,
            None,
        )),
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
            emit_fleet_receipt(&fleet_control::execute_fleet_control_with(
                ControlSurface::Cli,
                workspace,
                fleet_context,
                &manager,
                ControlOperation::FleetInterrupt,
                Some(&worker_id),
            ))
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
            emit_fleet_receipt(&fleet_control::execute_fleet_control_with(
                ControlSurface::Cli,
                workspace,
                fleet_context,
                &manager,
                ControlOperation::FleetResume,
                Some(&run_id),
            ))
        }
        FleetCommand::Stop { all } => {
            if !all {
                bail!("pass --all to stop all Fleet work");
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
     Codewhale activates declarative Skills, MCP servers, Commands, Agent\n\
     profiles, and Hooks through their existing engines. LSP, native\n\
     extensions, filesystem grants, and lifecycle mutation stay inventoried\n\
     and inactive; a mixed bundle can still activate supported components.\n\
     Marketplace catalogs, install, update, and uninstall all feed this same\n\
     disabled-and-untrusted review path; none grants automatic trust. Codewhale\n\
     does not scan other applications for ambient plugins.\n"
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
        crate::config::effective_home_dir()
            .map_or_else(|| PathBuf::from(".codewhale"), |h| h.join(".codewhale"))
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

    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_ACTION_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_ACTION_RGB;

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

    let sandbox = crate::sandbox::get_platform_sandbox_with_bwrap_preference(
        config.prefer_bwrap.unwrap_or(false),
    );
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
    ConfigDeclared,
    EnvDeclared,
    ExternalAuthDeclared,
    SecretStoreUnprobed,
    SecretStoreUnavailable,
    OAuth,
    ExternalConsent,
    NoAuth,
    LocalRuntime,
    Unknown,
}

/// What structural diagnostics can truthfully say about credential
/// availability without consulting environment values or durable stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialAvailability {
    Present,
    NotRequired,
    Unknown,
    NotProbed,
    Unavailable,
}

impl CredentialAvailability {
    fn label(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::NotRequired => "not_required",
            Self::Unknown => "unknown",
            Self::NotProbed => "not_probed",
            Self::Unavailable => "unavailable",
        }
    }

    fn certifies_ready(self) -> bool {
        matches!(self, Self::Present | Self::NotRequired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CredentialDiagnostic {
    source: ApiKeySource,
    availability: CredentialAvailability,
}

impl CredentialDiagnostic {
    const fn new(source: ApiKeySource, availability: CredentialAvailability) -> Self {
        Self {
            source,
            availability,
        }
    }
}

fn resolve_credential_diagnostic(config: &Config) -> CredentialDiagnostic {
    let provider = config.api_provider();
    let base_url = config.deepseek_base_url();
    let auth_mode = config.auth_mode_for_provider(provider);
    if crate::config::auth_mode_disables_api_key(auth_mode.as_deref()) {
        return CredentialDiagnostic::new(
            ApiKeySource::NoAuth,
            CredentialAvailability::NotRequired,
        );
    }
    if !crate::config::auth_mode_requires_api_key(auth_mode.as_deref())
        && (crate::config::provider_route_is_keyless_self_hosted(provider, &base_url)
            || crate::config::base_url_uses_local_host(&base_url))
    {
        return CredentialDiagnostic::new(
            ApiKeySource::LocalRuntime,
            CredentialAvailability::NotRequired,
        );
    }
    let custom_endpoint = config.provider_uses_custom_endpoint(provider);
    if !custom_endpoint && provider == crate::config::ApiProvider::OpenaiCodex {
        return config
            .external_credential_consent_status(provider)
            .filter(|status| status.route_state == "active")
            .map_or_else(
                || {
                    CredentialDiagnostic::new(
                        ApiKeySource::OAuth,
                        CredentialAvailability::NotProbed,
                    )
                },
                |_| {
                    CredentialDiagnostic::new(
                        ApiKeySource::ExternalConsent,
                        CredentialAvailability::NotProbed,
                    )
                },
            );
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
            .map_or_else(
                || {
                    CredentialDiagnostic::new(
                        ApiKeySource::OAuth,
                        CredentialAvailability::NotProbed,
                    )
                },
                |_| {
                    CredentialDiagnostic::new(
                        ApiKeySource::ExternalConsent,
                        CredentialAvailability::NotProbed,
                    )
                },
            );
    }
    let provider_config = config.provider_config();
    let provider_config_key_kind = provider_config
        .and_then(|entry| entry.api_key.as_deref())
        .map(crate::config::classify_config_api_key_value);
    let root_key_applies = matches!(
        provider,
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN
    ) || (provider == crate::config::ApiProvider::Custom
        && config.uses_legacy_literal_custom_route());
    let root_key_kind = root_key_applies
        .then_some(config.api_key.as_deref())
        .flatten()
        .map(crate::config::classify_config_api_key_value);

    if matches!(
        provider_config_key_kind,
        Some(crate::config::ConfigApiKeyValueKind::Literal)
    ) || matches!(
        root_key_kind,
        Some(crate::config::ConfigApiKeyValueKind::Literal)
    ) {
        CredentialDiagnostic::new(
            ApiKeySource::ConfigDeclared,
            CredentialAvailability::Present,
        )
    } else if config
        .provider_config()
        .and_then(|entry| entry.api_key_env.as_deref())
        .is_some_and(|name| !name.trim().is_empty())
    {
        CredentialDiagnostic::new(ApiKeySource::EnvDeclared, CredentialAvailability::NotProbed)
    } else if config
        .provider_config()
        .and_then(|entry| entry.auth.as_ref())
        .is_some()
    {
        CredentialDiagnostic::new(
            ApiKeySource::ExternalAuthDeclared,
            CredentialAvailability::NotProbed,
        )
    } else if matches!(
        provider_config_key_kind,
        Some(crate::config::ConfigApiKeyValueKind::SecretStoreSentinel)
    ) || matches!(
        root_key_kind,
        Some(crate::config::ConfigApiKeyValueKind::SecretStoreSentinel)
    ) {
        if config.should_skip_secret_store_for_provider(provider) {
            return CredentialDiagnostic::new(
                ApiKeySource::SecretStoreUnavailable,
                CredentialAvailability::Unavailable,
            );
        }
        // The sentinel is a declaration that runtime resolution should use
        // the secret-store layer, never a literal key. Doctor does not read it.
        CredentialDiagnostic::new(
            ApiKeySource::SecretStoreUnprobed,
            CredentialAvailability::NotProbed,
        )
    } else if !config.should_skip_secret_store_for_provider(provider) {
        // No literal config declaration was found, but this route can continue
        // through the durable store and ambient provider environment. Ordinary
        // doctor deliberately does not inspect either source.
        CredentialDiagnostic::new(
            ApiKeySource::SecretStoreUnprobed,
            CredentialAvailability::NotProbed,
        )
    } else {
        CredentialDiagnostic::new(ApiKeySource::Unknown, CredentialAvailability::Unknown)
    }
}

#[cfg(test)]
fn resolve_api_key_source(config: &Config) -> ApiKeySource {
    resolve_credential_diagnostic(config).source
}

fn provider_config_table_key(provider: crate::config::ApiProvider) -> &'static str {
    provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
        .unwrap_or("deepseek_cn")
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

    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_ACTION_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_ACTION_RGB;

    println!(
        "{}",
        "Codewhale Status".truecolor(aqua_r, aqua_g, aqua_b).bold()
    );
    println!("{}", "===============".truecolor(sky_r, sky_g, sky_b));
    println!("workspace: {}", workspace.display());

    let credential = resolve_credential_diagnostic(config);
    match credential.source {
        ApiKeySource::ConfigDeclared => println!(
            "  {} api_key: literal config value structurally present",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::EnvDeclared => println!(
            "  {} api_key: environment source declared (value not inspected)",
            "·".dimmed()
        ),
        ApiKeySource::ExternalAuthDeclared => println!(
            "  {} api_key: external auth source declared (value not inspected)",
            "·".dimmed()
        ),
        ApiKeySource::SecretStoreUnprobed => println!(
            "  {} api_key: secret store eligible (store not probed)",
            "·".dimmed()
        ),
        ApiKeySource::SecretStoreUnavailable => println!(
            "  {} api_key: secret-store sentinel declared, but this route cannot use that store",
            "!".truecolor(sky_r, sky_g, sky_b)
        ),
        ApiKeySource::OAuth => println!(
            "  {} oauth: Codewhale-owned route selected (token availability not probed)",
            "·".dimmed()
        ),
        ApiKeySource::ExternalConsent => println!(
            "  {} oauth: external read-only consent configured (credential file not probed)",
            "·".dimmed()
        ),
        ApiKeySource::NoAuth => println!(
            "  {} api_key: disabled for this route",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::LocalRuntime => println!(
            "  {} api_key: not required for this local runtime",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Unknown => println!(
            "  {} api_key: unknown (credential environment and durable stores not inspected)",
            "·".dimmed()
        ),
    }
    println!(
        "  · credential availability: {}",
        credential.availability.label()
    );
    println!(
        "  · base_url: {}",
        crate::doctor::structural_url_authority(&config.deepseek_base_url())
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

    let sandbox = crate::sandbox::get_platform_sandbox_with_bwrap_preference(
        config.prefer_bwrap.unwrap_or(false),
    );
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

/// Live API checks are explicit. Local endpoints have a separate opt-in because
/// an HTTP request can wake a desktop-managed daemon (notably Ollama.app).
fn doctor_should_probe_api(
    provider: crate::config::ApiProvider,
    base_url: &str,
    probes: crate::doctor::DoctorProbeRequest,
) -> bool {
    let local = crate::config::provider_route_is_keyless_self_hosted(provider, base_url)
        || crate::config::base_url_uses_local_host(base_url);
    probes.should_probe_api(local)
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
    probes: crate::doctor::DoctorProbeRequest,
    plugins: &crate::plugins::PluginRegistry,
) {
    use crate::palette;
    use colored::Colorize;

    let (accent_r, accent_g, accent_b) = palette::WHALE_HUMAN_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_ACTION_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_ACTION_RGB;
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
    println!("  codewhale-tui: {}", env!("CODEWHALE_BUILD_VERSION"));
    println!("  rust: {}", rustc_version());
    println!();

    println!("{}", "Updates:".bold());
    crate::doctor::print_update_report(probes).await;
    println!();

    // Configuration summary
    let doctor_paths = match crate::doctor::DoctorPathReport::resolve(config_path_override) {
        Ok(paths) => paths,
        Err(error) => {
            println!("{}", "Resolved User Paths:".bold());
            println!(
                "  {} unavailable: {error:#}",
                "✗".truecolor(red_r, red_g, red_b)
            );
            return;
        }
    };
    println!("{}", "Configuration:".bold());
    let config_path = &doctor_paths.config;

    if config_path.exists() {
        println!(
            "  {} config.toml found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(config_path)
        );
        // Secret hygiene: name the keys, never the values. Plain-text config
        // is not a secret store.
        if let Ok(raw) = std::fs::read_to_string(config_path) {
            let flagged = crate::doctor::config_credential_shaped_keys(&raw);
            if !flagged.is_empty() {
                println!(
                    "  {} credential-shaped value(s) in config.toml ({}): move them to the secret backend, then scrub the file — config.toml is plain text",
                    "!".truecolor(sky_r, sky_g, sky_b),
                    flagged.join(", ")
                );
            }
        }
    } else {
        println!(
            "  {} config.toml not found at {} (using defaults/env)",
            "!".truecolor(sky_r, sky_g, sky_b),
            crate::utils::display_path(config_path)
        );
    }
    println!("  workspace: {}", crate::utils::display_path(workspace));
    println!("  {}", doctor_search_provider_line(config));

    println!();
    println!("{}", "Resolved User Paths (read-only):".bold());
    for (label, path) in doctor_paths.entries() {
        println!("  · {label}: {}", crate::utils::display_path(path));
    }

    let secret_backend = codewhale_secrets::diagnose_secret_backend();
    println!();
    println!("{}", "Secret Backend (structural only):".bold());
    for line in crate::doctor::secret_backend_human_lines(&secret_backend) {
        println!("  · {line}");
    }

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
    print_doctor_fleet_roster_layers(config, workspace);

    // Check API keys
    println!();
    println!("{}", "API Keys:".bold());

    // Per-provider state: env + config file only (no values printed).
    // Keep doctor/status prompt-free and credential-value-free even for
    // unsigned rebuilt binaries.
    for provider in crate::config::ApiProvider::all().iter().copied() {
        let slot = provider.as_str();
        let provider_config = config.provider_config_for(provider);
        let config_declared = provider_config.is_some_and(|entry| {
            entry.api_key.as_deref().is_some_and(|key| {
                crate::config::classify_config_api_key_value(key)
                    == crate::config::ConfigApiKeyValueKind::Literal
            })
        }) || (matches!(provider, crate::config::ApiProvider::Deepseek)
            && config.api_key.as_deref().is_some_and(|key| {
                crate::config::classify_config_api_key_value(key)
                    == crate::config::ConfigApiKeyValueKind::Literal
            }));
        let env_source_declared = provider_config
            .and_then(|entry| entry.api_key_env.as_deref())
            .is_some_and(|name| !name.trim().is_empty());
        let icon = if config_declared || env_source_declared {
            "·".truecolor(aqua_r, aqua_g, aqua_b)
        } else {
            "·".dimmed()
        };
        println!(
            "  {} {slot}: env_source={}, config_source={}",
            icon,
            if env_source_declared {
                "declared (value not inspected)"
            } else {
                "not inspected"
            },
            if config_declared {
                "declared (value not inspected)"
            } else {
                "not declared"
            }
        );
    }
    println!("  · credential precedence is unchanged; doctor does not inspect credential values");
    println!();
    println!(
        "{}",
        "External credential consent (configuration only):".bold()
    );
    for line in doctor_external_credential_consent_lines(config) {
        println!("  {line}");
    }

    println!();
    println!(
        "{}",
        "DeepSeek Harness integration (read-only detection):".bold()
    );
    for line in doctor_dsh_integration_lines(config, workspace) {
        println!("  {line}");
    }

    let credential = resolve_credential_diagnostic(config);
    let source_label = match credential.source {
        ApiKeySource::ConfigDeclared => "literal config value structurally present",
        ApiKeySource::EnvDeclared => "environment source declared; value not inspected",
        ApiKeySource::ExternalAuthDeclared => {
            "external auth source declared; credential not resolved"
        }
        ApiKeySource::SecretStoreUnprobed => "secret store eligible; store not probed",
        ApiKeySource::SecretStoreUnavailable => {
            "secret-store sentinel declared, but this route cannot use that store"
        }
        ApiKeySource::OAuth => "OAuth route configured; token availability not probed",
        ApiKeySource::ExternalConsent => "external consent configured; token file not read",
        ApiKeySource::NoAuth => "no-auth route",
        ApiKeySource::LocalRuntime => "local runtime; credentials not required",
        ApiKeySource::Unknown => "unknown; credential environment and stores not inspected",
    };
    println!(
        "  {} active provider credential source: {source_label}",
        "·".dimmed()
    );
    println!(
        "  · active provider credential availability: {}",
        credential.availability.label()
    );

    // API connectivity test
    println!();
    println!("{}", "API Connectivity:".bold());
    let api_target = doctor_api_target(config);
    // Configured-vs-active honesty (DGF-01): doctor describes the route a
    // session launched NOW would resolve. It cannot see inside an already
    // running session, which keeps the route it resolved at its own launch.
    println!(
        "  · scope: configured route — what a session launched now would use; a running session keeps the route it resolved at launch (its TUI header shows the live route)"
    );
    println!("  · provider: {}", api_target.provider);
    println!(
        "  · base_url: {}",
        crate::doctor::structural_url_authority(&api_target.base_url)
    );
    match api_target.resolution {
        DoctorModelResolution::Resolved => {
            println!("  · model: {} (resolved)", api_target.model);
        }
        DoctorModelResolution::ConfiguredOnly => {
            println!(
                "  · model: {} (configured; route resolution unavailable)",
                api_target.model
            );
        }
    }
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
    if let Some(recommended) = strict_tool_mode.recommended_base_url.as_deref() {
        println!(
            "    Use the {} endpoint for DeepSeek strict schemas.",
            crate::doctor::structural_url_authority(recommended)
        );
    }
    let capability = crate::config::provider_capability(config.api_provider(), &api_target.model);
    if let Some(alias) = capability.alias_deprecation.as_ref() {
        println!(
            "  ! model alias {} retires {}; switch to {}",
            alias.alias, alias.retirement_date, alias.replacement
        );
    }
    let live_api_requested =
        doctor_should_probe_api(config.api_provider(), &api_target.base_url, probes);
    let endpoint_is_local = crate::config::provider_route_is_keyless_self_hosted(
        config.api_provider(),
        &api_target.base_url,
    ) || crate::config::base_url_uses_local_host(&api_target.base_url);
    if doctor_should_probe_auth(config) && live_api_requested {
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
                } else if crate::doctor::is_keyless_ds4_route(config) {
                    println!("    {error_msg}");
                } else {
                    println!(
                        "    Error details omitted because provider failures can contain credential material."
                    );
                }
            }
        }
    } else if !doctor_should_probe_auth(config) {
        println!(
            "  {} Live OAuth connectivity not checked by non-mutating doctor",
            "·".dimmed()
        );
        println!(
            "    Doctor never refreshes or rewrites credentials; exercise the route with a normal request."
        );
    } else {
        if endpoint_is_local {
            println!(
                "  {} Live connectivity not checked for this local endpoint",
                "·".dimmed()
            );
            println!(
                "    Run `codewhale doctor --probe-local` to opt in; the request may start a local service."
            );
        } else {
            println!(
                "  {} Live hosted connectivity not checked (offline default)",
                "·".dimmed()
            );
            println!("    Run `codewhale doctor --probe-api` to opt in.");
        }
    }

    println!();
    println!("{}", "Search Provider Reachability:".bold());
    let search_probe = crate::doctor::doctor_search_probe(config, probes).await;
    for line in crate::doctor::doctor_search_probe_lines(&search_probe) {
        println!("  {line}");
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
            if probes.should_probe_mcp() {
                println!();
                println!(
                    "  {} Live MCP probe enabled: starting enabled servers; backend tool health remains untested.",
                    "!".truecolor(sky_r, sky_g, sky_b)
                );
                match crate::mcp::McpPool::from_config_path_with_workspace_and_plugins(
                    &mcp_config_path,
                    workspace,
                    std::sync::Arc::new(plugins.clone()),
                ) {
                    Ok(mut pool) => {
                        let errors = pool.connect_all().await;
                        let failed = errors
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<std::collections::BTreeSet<_>>();
                        for (name, server) in &cfg.servers {
                            if !server.is_enabled() {
                                continue;
                            }
                            if failed.contains(name.as_str()) {
                                println!(
                                    "      {} {name}: process/protocol unreachable; error details omitted",
                                    "✗".truecolor(red_r, red_g, red_b)
                                );
                            } else {
                                println!(
                                    "      {} {name}: process reachable and protocol initialized; backend tool health not checked",
                                    "✓".truecolor(aqua_r, aqua_g, aqua_b)
                                );
                            }
                        }
                    }
                    Err(_) => println!(
                        "      {} live MCP probe could not load merged configuration; details omitted",
                        "✗".truecolor(red_r, red_g, red_b)
                    ),
                }
            } else {
                println!(
                    "    Use codewhale doctor --probe-mcp to opt in to live process/protocol checks; it may start configured servers."
                );
            }
        }
        Err(_) => {
            println!(
                "  {} MCP configuration could not be loaded; details omitted",
                "✗".truecolor(red_r, red_g, red_b)
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

    // PDF text extraction is an optional integration. Codewhale itself stays
    // a single required executable; file and web tools report a typed
    // failed `binary_unavailable` result when Poppler is not installed.
    match crate::dependencies::resolve_pdftotext() {
        Some(_) => println!(
            "  {} pdftotext: available → PDF text extraction enabled",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!(
                "  {} pdftotext: not found (optional; PDF text reads fail as `binary_unavailable`)",
                "·".dimmed(),
            );
            match std::env::consts::OS {
                "macos" => println!("    Install via: brew install poppler"),
                "linux" => {
                    println!("    Install via: sudo apt install poppler-utils   (Debian/Ubuntu)")
                }
                "windows" => println!(
                    "    Install Poppler for Windows from https://blog.alivate.com.au/poppler-windows/"
                ),
                _ => {}
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

    let sandbox = crate::sandbox::get_platform_sandbox_with_bwrap_preference(
        config.prefer_bwrap.unwrap_or(false),
    );
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
    resolve_credential_diagnostic(config)
        .availability
        .certifies_ready()
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

    let credential = resolve_credential_diagnostic(config);
    // Setup completion is persisted independently from credential probing.
    // Ordinary doctor deliberately does not read environment values or the
    // durable secret store, so `not_probed` must not erase a completed lane.
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
        "  · credential: source={}, availability={}",
        doctor_api_key_source_label(credential.source),
        credential.availability.label()
    );
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
    println!(
        "  · control socket: {}",
        doctor_control_socket_posture_line(config)
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

/// #5098: print every profile id that exists in more than one roster layer
/// so a personal/config edit that loses to project is visible without
/// opening `/fleet`.
fn print_doctor_fleet_roster_layers(config: &Config, workspace: &Path) {
    use colored::Colorize;

    let roster =
        crate::fleet::identity::load_effective_roster(&config.fleet_config(), workspace, None);
    println!();
    println!("{}", "Fleet roster layers:".bold());
    if let Some(error) = roster.load_error() {
        println!("  ! {error}");
        return;
    }
    let lines = roster.doctor_layer_lines();
    if lines.is_empty() {
        println!("  · no profile id is defined in more than one layer");
        return;
    }
    for line in lines {
        if let Some(layer) = line.strip_prefix("  ") {
            println!("      {layer}");
        } else {
            println!("  · {line}");
        }
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

/// TUI settings posture used when `config.approval_policy` is unset.
/// Doctor must surface this separately so a saved Full Access baseline is not
/// misreported as the config default `approval_policy=on-request`.
fn doctor_runtime_permission_posture() -> (String, &'static str) {
    match crate::settings::Settings::load_read_only() {
        Ok(settings) => match settings.permission_posture {
            Some(posture) => (posture, "settings"),
            None => ("unset".to_string(), "default"),
        },
        Err(_) => ("unset".to_string(), "default"),
    }
}

fn doctor_runtime_posture_line(config: &Config, workspace: &Path) -> String {
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    let (permission_posture, permission_posture_source) = doctor_runtime_permission_posture();
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
    let (telemetry_on, telemetry_source) = doctor_runtime_telemetry(config);
    let telemetry = if telemetry_on { "on" } else { "off" };

    format!(
        "default_mode={default_mode} ({default_mode_source}), permission_posture={permission_posture} ({permission_posture_source}), approval_policy={approval} ({approval_source}), allow_shell={allow_shell} ({allow_shell_source}), sandbox={sandbox} ({sandbox_source}), network.default={network} ({network_source}), telemetry={telemetry} ({telemetry_source}), trust={trust}"
    )
}

/// Doctor posture for the per-session control socket, enabled via
/// `[control_socket].enabled` (false = off, the default). Report the
/// resolved state and, when enabled, where the socket appears for the
/// running session.
fn doctor_control_socket_posture_line(config: &Config) -> String {
    let enabled = config
        .control_socket
        .as_ref()
        .is_some_and(|socket| socket.enabled);
    if enabled {
        "control_socket=on (sessions/<id>/control.sock per running session)".to_string()
    } else {
        "control_socket=off (default)".to_string()
    }
}

/// Resolved telemetry consent and where it came from (#5441).
///
/// Telemetry ships ON by default, and no posture surface reported that — a
/// user who never opted in saw nothing saying "telemetry: on (default)".
/// Truth change only: the resolution itself is [`codewhale_config`]'s.
fn doctor_runtime_telemetry(config: &Config) -> (bool, &'static str) {
    let (on, source) = codewhale_config::resolved_telemetry_consent(config.telemetry);
    (on, source.as_str())
}

fn doctor_operate_fleet_report_json(config: &Config, workspace: &Path) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    // Doctor reports configured routing posture only. In particular it must
    // never consume an external-file grant merely to label Fleet readiness.
    let credential = resolve_credential_diagnostic(config);
    let has_credentials_or_local = credential.availability.certifies_ready();
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
    let roster =
        crate::fleet::identity::load_effective_roster(&config.fleet_config(), workspace, None);
    let mut built_in_members = 0usize;
    let mut plugin_members = 0usize;
    let mut config_members = 0usize;
    let mut personal_members = 0usize;
    let mut workspace_members = 0usize;
    for member in roster.members() {
        match member.origin {
            crate::fleet::roster::ProfileOrigin::BuiltIn => built_in_members += 1,
            crate::fleet::roster::ProfileOrigin::Plugin => plugin_members += 1,
            crate::fleet::roster::ProfileOrigin::Config => config_members += 1,
            crate::fleet::roster::ProfileOrigin::Personal => personal_members += 1,
            crate::fleet::roster::ProfileOrigin::Workspace => workspace_members += 1,
        }
    }
    let roster_members = roster.members().len();
    let custom_members = plugin_members + config_members + personal_members + workspace_members;
    let roster_ready = roster.load_error().is_none() && roster_members > 0;
    let runtime_ready =
        subagents_enabled && max_subagents > 0 && launch_concurrency > 0 && max_spawn_depth > 0;
    let multi_layer: Vec<serde_json::Value> = roster
        .multi_layer_report()
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "effective": entry.effective.to_string(),
                "effective_path": entry.effective_path.display().to_string(),
                "layers": entry
                    .layers
                    .iter()
                    .map(|layer| {
                        json!({
                            "origin": layer.origin.to_string(),
                            "path": layer.source.display().to_string(),
                            "wins": layer.wins,
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "ready": has_credentials_or_local && runtime_ready && roster_ready,
        "provider": {
            "id": config.provider_identity_for(provider),
            "auth": {
                "present_or_local": has_credentials_or_local,
                "source": doctor_api_key_source_label(credential.source),
                "availability": credential.availability.label(),
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
            "error": roster.load_error(),
            "total": roster_members,
            "built_in": built_in_members,
            "config": config_members,
            "personal": personal_members,
            "workspace": workspace_members,
            "custom": custom_members,
            "starter_roster_available": built_in_members > 0,
            "readiness_rule": "built-in starter roster or custom roster",
            "multi_layer": multi_layer,
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
    let credential = resolve_credential_diagnostic(config);
    let auth_present_or_local = credential.availability.certifies_ready();
    let credential_help = provider.credential_help();
    let credential_url = credential_help
        .credential_url
        .map(crate::doctor::structural_url_authority);
    let credential_docs_url = credential_help
        .docs_url
        .map(crate::doctor::structural_url_authority);

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
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
            "env_vars": provider.env_vars(),
            "credential_mode": credential_help.acquisition.as_str(),
            "credential_url": credential_url,
            "credential_docs_url": credential_docs_url,
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

fn doctor_dsh_integration_report(
    config: &Config,
    workspace: &Path,
) -> anyhow::Result<crate::integrations::dsh::DshStatusReport> {
    use crate::integrations::dsh;
    let paths = dsh::DshPaths::from_process()?;
    let detection = dsh::detect::detect(&dsh::DetectEnv::from_process(), &dsh::ProcessRunner);
    let identity = dsh::codewhale_route_identity(config, workspace);
    dsh::compute_status(
        &paths,
        detection,
        identity,
        false,
        dsh::bundle_availability_now(),
    )
}

fn doctor_dsh_integration_lines(config: &Config, workspace: &Path) -> Vec<String> {
    match doctor_dsh_integration_report(config, workspace) {
        Ok(report) => {
            let mut lines = vec![
                format!("state: {}", report.state.label()),
                crate::integrations::dsh::status_line(&report),
                format!(
                    "owned files: {} (overlay {})",
                    crate::utils::display_path(&report.paths_root),
                    if report.overlay_present {
                        "present"
                    } else {
                        "absent"
                    }
                ),
            ];
            if !report.shadowing_namespaces.is_empty() {
                lines.push(format!(
                    "dsh settings.yaml sections that can shadow the overlay: {}",
                    report.shadowing_namespaces.join(", ")
                ));
            }
            lines
        }
        Err(error) => vec![format!("unavailable: {error}")],
    }
}

fn doctor_dsh_integration_json(config: &Config, workspace: &Path) -> serde_json::Value {
    match doctor_dsh_integration_report(config, workspace) {
        Ok(report) => serde_json::json!({
            "state": report.state.label(),
            "summary": crate::integrations::dsh::status_line(&report),
            "dsh_version": report.detection.version,
            "compatibility": report.detection.compatibility.label(),
            "overlay_present": report.overlay_present,
            "shadowing_namespaces": report.shadowing_namespaces,
        }),
        Err(error) => serde_json::json!({ "state": "unavailable", "error": error.to_string() }),
    }
}

fn doctor_external_credential_consent_statuses(
    config: &Config,
) -> Vec<codewhale_config::ExternalCredentialConsentStatus> {
    [
        crate::config::ApiProvider::OpenaiCodex,
        crate::config::ApiProvider::Xai,
        crate::config::ApiProvider::Deepseek,
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
    let (permission_posture, permission_posture_source) = doctor_runtime_permission_posture();
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
    let (telemetry_value, telemetry_source) = doctor_runtime_telemetry(config);
    let workspace_trusted = !crate::tui::onboarding::needs_trust(workspace);
    let credential = resolve_credential_diagnostic(config);
    let credential_ready = credential.availability.certifies_ready();
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
        "credential": {
            "ready": credential_ready,
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
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
            "permission_posture": {
                "value": permission_posture,
                "source": permission_posture_source,
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
            "telemetry": {
                "value": telemetry_value,
                "source": telemetry_source,
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

/// Emit a bounded, secret-redacted JSON failure when configuration cannot be
/// loaded or validated. Invalid configuration must not be forced through the
/// normal doctor report because its route/capability facts would be misleading.
fn run_doctor_json_config_error(error: &anyhow::Error) -> Result<()> {
    let safe_message = error
        .downcast_ref::<crate::config::SafeConfigDiagnostic>()
        .map(ToString::to_string);
    let report = serde_json::json!({
        "status": "error",
        "error": {
            "kind": "config_validation",
            "message": safe_message.as_deref().unwrap_or("configuration validation failed; details omitted because configuration errors may contain credential material"),
        },
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    // Keep stderr generic: the actionable, redacted error is already on
    // stdout, and Rust's Result termination must never redisclose a secret.
    bail!("doctor configuration validation failed; see JSON output")
}

/// Machine-readable counterpart to `run_doctor`. This report is always
/// structural and offline; live probe flags conflict with `--json`.
fn run_doctor_json(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    use serde_json::json;

    let doctor_paths = crate::doctor::DoctorPathReport::resolve(config_path_override)?;
    let config_path = &doctor_paths.config;
    let secret_backend = codewhale_secrets::diagnose_secret_backend();

    let credential = resolve_credential_diagnostic(config);

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
        Err(_) => json!({
            "config_path": mcp_config_path.display().to_string(),
            "present": mcp_present,
            "project_config_path": project_mcp_config_path.display().to_string(),
            "project_present": project_mcp_present,
            "probe_scope": "configuration",
            "live_health_checked": false,
            "servers": [],
            "error": "configuration_unavailable_details_omitted",
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
        "paths": doctor_paths,
        "secret_backend": secret_backend,
        "workspace": workspace.display().to_string(),
        "legacy_state": doctor_legacy_state_json(
            &code_home,
            &legacy_home,
            &legacy_state_report,
            &session_recovery,
        ),
        "setup": doctor_setup_report_json(config, workspace),
        "api_key": {
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
        "external_credentials": doctor_external_credential_consent_json(config),
        "dsh_integration": doctor_dsh_integration_json(config, workspace),
        "base_url": crate::doctor::structural_url_authority(&api_target.base_url),
        "default_text_model": api_target.model,
        // DGF-01: this report describes the route a session launched now
        // would resolve; a running session keeps its launch-time route.
        "route_scope": "configured_at_launch",
        "model_resolution": match api_target.resolution {
            DoctorModelResolution::Resolved => "resolved",
            DoctorModelResolution::ConfiguredOnly => "configured_unresolved",
        },
        "route": doctor_route_report(config),
        "strict_tool_mode": doctor_strict_tool_mode_report_json(&strict_tool_mode),
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
        "sandbox": match crate::sandbox::get_platform_sandbox_with_bwrap_preference(
            config.prefer_bwrap.unwrap_or(false),
        ) {
            Some(kind) => json!({"available": true, "kind": kind.to_string()}),
            None => json!({"available": false, "kind": null}),
        },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "api_connectivity": {
            "checked": false,
            "status": "not_probed",
            "note": "JSON doctor is offline; use `codewhale doctor --probe-api` or `--probe-local` for an explicit live check.",
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
    let configured_model = config.default_model();
    let route_result =
        crate::route_runtime::resolve_runtime_route(config, provider, Some(&configured_model));
    let route_error = route_result
        .is_err()
        .then_some("route_resolution_failed_details_omitted");
    let route = route_result.ok();
    let resolved_model = route
        .as_ref()
        .map_or(configured_model.as_str(), |route| route.model.as_str());
    // Wire-aware so a custom provider's `wire = "responses" | "anthropic"`
    // reports the payload mode the client will actually speak instead of the
    // static Chat default.
    let cap = crate::config::provider_capability_with_wire(
        provider,
        resolved_model,
        config.provider_wire_dialect(provider),
    );
    let route_profile = route.as_ref().map(|route| {
        crate::model_profile::resolved_capability_profile_for_route(
            provider,
            resolved_model,
            route.candidate.capabilities(),
            route.candidate.limits(),
        )
    });
    let context_window = route
        .as_ref()
        .map_or(cap.context_window, |route| route.context_window.tokens);
    let context_window_source = route.as_ref().map_or(
        crate::route_runtime::ContextWindowSource::Fallback.label(),
        |route| route.context_window.source.label(),
    );
    // `null` when neither the resolved route nor the compatibility matrix
    // publishes an output ceiling — doctor must not invent one.
    let max_output = route_profile
        .as_ref()
        .and_then(|profile| profile.max_output)
        .or(cap.max_output);
    let is_exact_kimi_code_k3 = route.as_ref().is_some_and(|route| {
        crate::config::is_exact_kimi_code_k3_route(
            provider,
            &route.candidate.endpoint().base_url,
            route.candidate.wire_model_id().as_str(),
        )
    });
    let thinking_supported = is_exact_kimi_code_k3
        || route_profile
            .as_ref()
            .map_or(cap.thinking_supported, |profile| {
                profile.supports_reasoning()
            });
    let cache_telemetry_supported = route_profile
        .as_ref()
        .map_or(cap.cache_telemetry_supported, |profile| {
            profile.prompt_caching.is_supported()
        });
    let request_payload_mode = route_profile
        .as_ref()
        .map_or(cap.request_payload_mode, |profile| {
            profile.request_payload_mode
        });
    let alias_deprecation = config.active_deepseek_alias_deprecation();

    json!({
        "resolved_provider": config.provider_identity_for(provider),
        "resolved_model": resolved_model,
        "context_window": context_window,
        "context_window_source": context_window_source,
        "max_output": max_output,
        "thinking_supported": thinking_supported,
        "cache_telemetry_supported": cache_telemetry_supported,
        "request_payload_mode": serde_json::to_value(request_payload_mode).unwrap_or_default(),
        "route_error": route_error,
        "alias_deprecation": alias_deprecation,
    })
}

fn doctor_route_report(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let target = doctor_api_target(config);
    let provider = config.api_provider();
    let redacted_base_url = crate::doctor::structural_url_authority(&target.base_url);
    let route_result =
        crate::route_runtime::resolve_runtime_route(config, provider, Some(&target.model));
    let route_error = route_result
        .is_err()
        .then_some("route_resolution_failed_details_omitted");
    let context_window = route_result
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

    let route_identity =
        crate::config::moonshot_k3_route_display_name(&target.base_url, &target.model);
    let credential = resolve_credential_diagnostic(config);

    json!({
        "provider": target.provider,
        "provider_source": doctor_provider_source(config),
        "provider_config_table": doctor_provider_config_table(config, provider),
        "model": target.model,
        "route_identity": route_identity,
        "wire_protocol": doctor_wire_protocol(provider),
        "base_url": {
            "redacted": redacted_base_url,
            "class": doctor_base_url_class(provider, &target.base_url),
            "fingerprint": crate::utils::redacted_identifier_for_log(&target.base_url),
        },
        "auth": {
            "scheme": doctor_auth_scheme(config),
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
        "context_window": context_window,
        "route_error": route_error,
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
    let policy = provider
        .metadata()
        .map(|metadata| metadata.wire_policy())
        .unwrap_or(codewhale_config::provider::WirePolicy::Fixed(
            codewhale_config::provider::WireFormat::ChatCompletions,
        ));
    match policy.fixed() {
        Some(codewhale_config::provider::WireFormat::ChatCompletions) => "chat_completions",
        Some(codewhale_config::provider::WireFormat::Responses) => "responses",
        Some(codewhale_config::provider::WireFormat::AnthropicMessages) => "anthropic_messages",
        None => "model_aware",
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
        && doctor_xiaomi_mimo_base_url_uses_token_plan(&config.deepseek_base_url())
    {
        "api-key"
    } else if provider == crate::config::ApiProvider::XiaomiMimo {
        // The alternate MiMo scheme depends on a credential prefix. Ordinary
        // doctor does not read credentials merely to make this label precise.
        "unknown"
    } else if matches!(
        provider,
        crate::config::ApiProvider::Sglang
            | crate::config::ApiProvider::Vllm
            | crate::config::ApiProvider::Ollama
    ) {
        "optional_bearer"
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
        ApiKeySource::ConfigDeclared => "config_declared",
        ApiKeySource::EnvDeclared => "env_declared",
        ApiKeySource::ExternalAuthDeclared => "external_auth_declared",
        ApiKeySource::SecretStoreUnprobed => "secret_store_unprobed",
        ApiKeySource::SecretStoreUnavailable => "secret_store_unavailable",
        ApiKeySource::OAuth => "oauth_unprobed",
        ApiKeySource::ExternalConsent => "external_consent",
        ApiKeySource::NoAuth => "none",
        ApiKeySource::LocalRuntime => "local_runtime",
        ApiKeySource::Unknown => "unknown",
    }
}

fn doctor_search_provider_line(config: &Config) -> String {
    let search_provider = config.search_provider_resolution();
    let switch_hint = if matches!(
        (search_provider.provider, search_provider.source),
        (
            crate::config::SearchProvider::Firecrawl,
            crate::config::SearchProviderSource::Default
        )
    ) {
        "; set [search] provider = \"baidu\" | \"metaso\" | \"volcengine\" for China"
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
        "reachability": "not_checked",
        "reachability_reason": "offline_json",
    })
}

/// Whether the model in a [`DoctorApiTarget`] is the wire id the engine
/// resolver produced, or only the raw configured value because resolution
/// failed. Doctor never prints resolution error details — the JSON route
/// report already redacts them for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorModelResolution {
    Resolved,
    ConfiguredOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorApiTarget {
    provider: String,
    base_url: String,
    model: String,
    resolution: DoctorModelResolution,
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
    // Report the model through the same resolver the live client uses at
    // session launch (`client.rs` → `resolve_runtime_route`), so doctor's
    // answer matches what a session started now would actually serve —
    // saved provider models, alias normalization, and roster preference
    // included — instead of re-deriving a config default that can diverge
    // from the engine (DGF-01, dogfood 2026-08-02).
    let (model, resolution) =
        match crate::route_runtime::resolve_runtime_route(config, provider, None) {
            Ok(route) => (route.model.clone(), DoctorModelResolution::Resolved),
            Err(_) => (
                config.default_model(),
                DoctorModelResolution::ConfiguredOnly,
            ),
        };
    DoctorApiTarget {
        provider: config.provider_identity_for(provider),
        base_url: config.deepseek_base_url(),
        model,
        resolution,
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

fn doctor_strict_tool_mode_report_json(status: &DoctorStrictToolModeStatus) -> serde_json::Value {
    serde_json::json!({
        "enabled": status.enabled,
        "status": status.status,
        "function_strict_sent": status.function_strict_sent,
        "message": status.message,
        "recommended_base_url": status
            .recommended_base_url
            .as_deref()
            .map(crate::doctor::structural_url_authority),
    })
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
        crate::doctor::structural_url_authority(&target.base_url)
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

    if crate::doctor::is_keyless_ds4_route(config) {
        return crate::doctor::probe_ds4_models(config).await;
    }

    // Minimal request: single word prompt, 1 max token
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: Role::User,
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
        // This is a one-token transport probe, not a reasoning task.
        reasoning_effort: Some("off".to_string()),
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
    // `RustC::available()` resolves the tool once, capturing the `--version`
    // banner as a side effect of the probe; reuse it instead of launching a
    // second rustc process (each launch loads libLLVM).
    if !crate::dependencies::RustC::available() {
        return "unknown".to_string();
    }
    crate::dependencies::rustc_version_banner().unwrap_or_else(|| "unknown".to_string())
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
    let (sky_r, sky_g, sky_b) = palette::WHALE_ACTION_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_ACTION_RGB;

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

    let (sky_r, sky_g, sky_b) = palette::WHALE_ACTION_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_ACTION_RGB;
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

/// Doctor is a structural report unless the user explicitly asks it to probe
/// a provider endpoint. Keep credential-bearing environment values out of the
/// regular diagnostic configuration so an unrelated renderer or error path
/// cannot disclose them.
fn load_doctor_config_from_cli(cli: &Cli, args: &DoctorArgs) -> Result<Config> {
    if args.probe_api || args.probe_local {
        return load_config_from_cli(cli);
    }
    load_structural_config_from_cli(cli)
}

fn load_structural_config_from_cli(cli: &Cli) -> Result<Config> {
    let profile = effective_config_profile(cli);
    let mut config = Config::load_structural(cli.config.clone(), profile.as_deref())?;
    if let Ok(settings) = crate::settings::Settings::load_read_only() {
        apply_saved_reasoning_preference(&mut config, &settings);
    }
    cli.feature_toggles.apply(&mut config)?;
    Ok(config)
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
    // Config loading is shared by diagnostics and mutating runtimes. Read the
    // saved preference without migrating or creating state here; interactive
    // startup performs any permitted migration later through `Settings::load`.
    if let Ok(settings) = crate::settings::Settings::load_read_only() {
        apply_saved_reasoning_preference(&mut config, &settings);
    }
    cli.feature_toggles.apply(&mut config)?;
    // Install the foreign-instruction opt-in before anything can load project
    // context. This is the single funnel every runtime goes through — TUI,
    // exec, ACP, and the app-server passthrough all resolve config here — so
    // the loader never has to be handed the setting at each of its call sites.
    install_foreign_instruction_imports(&config);
    Ok((config, profile))
}

/// Apply the selected v2 Fleet's operator to a fresh root session.
///
/// Provider/model are one atomic route: any explicit launch override for
/// either half keeps the caller's full route and bypasses the saved operator.
/// Reasoning is independent, so an explicit reasoning flag keeps its value
/// while the Fleet may still select the provider/model pair. Resumes call
/// this helper only for fresh sessions and therefore retain their saved route.
fn apply_selected_fleet_operator_for_launch(
    config: &mut Config,
    workspace: &Path,
    explicit_route_override: bool,
    explicit_reasoning_override: bool,
) -> Result<bool> {
    if explicit_route_override {
        return Ok(false);
    }
    let Some(selected) = crate::fleet::store::resolve_selected_fleet(workspace).map_err(|_| {
        anyhow!(
            "Selected Fleet is missing or unreadable; inspect /fleet and repair or clear the selection."
        )
    })?
    else {
        return Ok(false);
    };
    let fleet_name = crate::safe_label::SafeLabel::phrase(&selected.name);
    let (fleet, _) = crate::fleet::store::load_fleet_at(&selected.path).map_err(|_| {
        anyhow!(
            "selected Fleet '{}' ({}) is invalid or unreadable; inspect /fleet and repair or clear the selection.",
            fleet_name,
            selected.scope.label()
        )
    })?;
    let Some(operator) = fleet.operator.as_ref() else {
        return Ok(false);
    };
    let provider_id = operator.provider.trim();
    let model_id = operator.model.trim();
    if provider_id.is_empty() || model_id.is_empty() {
        bail!(
            "selected Fleet '{}' has an incomplete operator route; provider and model must both be non-empty",
            fleet_name
        );
    }
    let safe_provider_id = crate::safe_label::SafeLabel::identifier(provider_id);
    let safe_model_id = crate::safe_label::SafeLabel::catalog_model(model_id);

    let identity = config
        .resolve_provider_pin_identity(provider_id)
        .map_err(|error| {
            anyhow!(
                "selected Fleet '{}' operator provider '{}' is unavailable: {}",
                fleet_name,
                safe_provider_id,
                crate::safe_label::safe_error_text(&error)
            )
        })?;
    let resolved =
        crate::route_runtime::resolve_runtime_route_for_identity(config, &identity, Some(model_id))
            .map_err(|error| {
                anyhow!(
                    "selected Fleet '{}' operator route {}/{} is invalid: {}",
                    fleet_name,
                    safe_provider_id,
                    safe_model_id,
                    crate::safe_label::safe_error_text(&error)
                )
            })?;
    let mut selected_config = *resolved.config;
    selected_config.fleet_operator_route_applied = true;
    selected_config.fleet_operator_reasoning_applied = false;
    if !explicit_reasoning_override
        && let Some(reasoning) = operator
            .reasoning
            .as_deref()
            .map(str::trim)
            .filter(|reasoning| !reasoning.is_empty())
        && let Some(reasoning) = normalize_cli_reasoning_effort(reasoning).map_err(|error| {
            anyhow!(
                "selected Fleet '{}' has invalid operator reasoning: {}",
                fleet_name,
                crate::safe_label::safe_error_text(&error.to_string())
            )
        })?
    {
        selected_config.reasoning_effort = Some(reasoning);
        selected_config.reasoning_effort_inferred_from_legacy_alias = false;
        selected_config.fleet_operator_reasoning_applied = true;
    }
    *config = selected_config;
    Ok(true)
}

/// Resolve `project_instruction_imports` into the loader's opt-in set.
///
/// Unrecognized names are reported rather than dropped: a typo in this key
/// silently means "import nothing", which is exactly the failure mode a user
/// would not notice.
fn install_foreign_instruction_imports(config: &Config) {
    let (imports, unknown) = crate::project_context::ForeignInstructionImports::from_config(
        &config.project_instruction_imports,
    );
    for name in unknown {
        tracing::warn!(
            target: "project_context",
            value = %name,
            "Ignoring unknown project_instruction_imports entry; expected one of              claude, cursor, cline, windsurf, gemini, copilot, muse, all"
        );
    }
    crate::project_context::set_foreign_instruction_imports(imports);
}

/// Apply the same reasoning-preference precedence as interactive `App`
/// construction to non-TUI runtimes.
///
/// `/model` and the config editor persist this preference in `settings.toml`.
/// Exec, review, workflow, ACP, and runtime-thread launches all begin with a
/// `Config`, so copying the saved value here keeps those entry points from
/// silently falling back to a route classifier or an older config.toml value.
fn apply_saved_reasoning_preference(config: &mut Config, settings: &crate::settings::Settings) {
    let Some(reasoning_effort) = settings.reasoning_effort.as_ref() else {
        return;
    };
    config.reasoning_effort = Some(reasoning_effort.clone());
    config.reasoning_effort_inferred_from_legacy_alias = false;
}

fn run_login(api_key: Option<String>) -> Result<()> {
    if api_key.is_some() {
        bail!(
            "`login --api-key` is not account sign-in. \
             Use `codewhale login` for the Codewhale account, \
             or `codewhale auth set --provider <id>` for a provider key."
        );
    }
    bail!(
        "This binary's `login` command does not store provider keys. \
         Use the `codewhale` CLI: `codewhale login` for the Codewhale account device flow, \
         or `codewhale auth set --provider <id>` for a provider key."
    );
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

async fn run_chatgpt_pkce_auth(config_path: Option<&Path>) -> Result<()> {
    let pending = chatgpt_oauth::pkce_login().await?;
    let activation = chatgpt_oauth::activate_pkce_login(pending, config_path, None)?;
    println!(
        "ChatGPT OAuth is ready; activated {} via {}",
        codewhale_config::quote_os_path(&activation.auth_path),
        codewhale_config::quote_os_path(&activation.config_path)
    );
    Ok(())
}

fn run_chatgpt_pkce_revoke(config_path: Option<&Path>) -> Result<()> {
    chatgpt_oauth::revoke_owned_login(config_path, None)?;
    println!("Revoked Codewhale-owned ChatGPT tokens. Codex CLI consent is unchanged.");
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

    // Resolved before anything is fetched or billed so an unknown
    // `--provider` fails fast with the provider vocabulary hint.
    let (config, force_configured_route) = review_execution_route(config, &args)?;
    let config = &config;

    if args.pr.is_some() && !is_command_available("gh") {
        bail!(
            "`gh` CLI not found on PATH. Install GitHub CLI \
             (https://cli.github.com) and authenticate (`gh auth login`) \
             so `codewhale review --pr` can fetch the pull request."
        );
    }
    // Fetched before the diff so a missing/hidden PR fails before any model
    // route is resolved or billed.
    let pr_view = match args.pr {
        Some(number) => Some((number, run_gh_pr_view(number, args.repo.as_deref())?)),
        None => None,
    };
    let diff = collect_diff(&args)?;
    if diff.trim().is_empty() {
        bail!("No diff to review.");
    }
    validate_review_receipt_args(&args)?;
    if args.check_receipt {
        return run_review_receipt_check(&diff, &args);
    }

    let model = resolve_review_model(config, args.model.as_deref());
    let route = resolve_cli_exec_route(config, &model, &diff, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let route_provider = execution_config.provider_identity_for(route.provider);
    let model = route.model.clone();
    // PR reviews run under the structured JSON review contract so findings
    // carry file/line positions that can be posted as inline review comments.
    let (user_prompt, system) = if let Some((number, view)) = &pr_view {
        (
            format_pr_prompt(*number, view, &diff),
            SystemPrompt::Text(crate::tools::review::review_system_prompt().to_string()),
        )
    } else {
        (
            format!("Review the following diff and provide feedback:\n\n{diff}\n\nEnd of diff."),
            SystemPrompt::Text(
                "You are a senior code reviewer. Focus on bugs, risks, behavioral regressions, and missing tests. \
Provide findings ordered by severity with file references, then open questions, then a brief summary."
                    .to_string(),
            ),
        )
    };
    let reasoning_effort = route.reasoning_effort.and_then(|effort| {
        cli_reasoning_effort_value_for_prompt(&execution_config, &model, effort, &user_prompt)
    });

    let client = DeepSeekClient::new(&execution_config)?;
    let request_route = client.effective_route_envelope(&model, chrono::Utc::now());
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_prompt,
                cache_control: None,
            }],
        }],
        max_tokens: client.effective_max_output_tokens(&request_route.model),
        system: Some(system),
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
    let review_stop_reason = response.stop_reason.clone();
    let review_incomplete = crate::models::is_incomplete_stop_reason(review_stop_reason.as_deref());
    let mut output = String::new();
    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            output.push_str(&text);
        }
    }
    let structured = pr_view
        .as_ref()
        .map(|_| crate::tools::review::ReviewOutput::from_str(&output));
    // A truncated review must not be posted or become a receipt. The partial
    // text is still printed for diagnostics below.
    if args.post && !review_incomplete {
        let (number, view) = pr_view
            .as_ref()
            .expect("--post requires --pr (enforced by clap)");
        let review = structured
            .as_ref()
            .expect("structured output exists for PR reviews");
        post_pr_review(*number, view, args.repo.as_deref(), review, &diff)?;
    }
    let receipt = if args.write_receipt && !review_incomplete {
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
    let review_error = review_incomplete.then(|| {
        format!(
            "Model response incomplete: provider stop reason `{}`; the partial review was not accepted.",
            crate::models::stop_reason_detail(review_stop_reason.as_deref())
        )
    });
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "review",
                "provider": route_provider,
                "model": model,
                "success": !review_incomplete,
                "content": output,
                "pr": pr_view.as_ref().map(|(number, view)| serde_json::json!({
                    "number": number,
                    "url": view.url,
                    "title": view.title,
                    "head_sha": view.head_sha,
                })),
                "review": structured,
                "stop_reason": review_stop_reason,
                "error": review_error,
                "receipt_path": receipt
                    .as_ref()
                    .map(|(path, _)| path.display().to_string()),
                "receipt": receipt.as_ref().map(|(_, receipt)| receipt),
            }))?
        );
        if let Some(error) = review_error {
            anyhow::bail!(error);
        }
    } else if let Some((number, view)) = &pr_view {
        let review = structured
            .as_ref()
            .expect("structured output exists for PR reviews");
        println!(
            "{}",
            render_pr_review_markdown(*number, view, review, args.post)
        );
        if let Some((path, _)) = receipt {
            eprintln!("Review receipt written: {}", path.display());
        }
        if let Some(error) = review_error {
            anyhow::bail!(error);
        }
    } else {
        println!("{output}");
        if let Some((path, _)) = receipt {
            eprintln!("Review receipt written: {}", path.display());
        }
        if let Some(error) = review_error {
            anyhow::bail!(error);
        }
    }
    Ok(())
}

/// Apply `codewhale review --provider <name>` and decide whether the route is
/// authoritative (no cross-provider inventory inference).
///
/// This mirrors `codewhale exec --provider` (#4093): the flag sets ONLY the
/// non-secret provider identity, and pinning the route is what lets a model
/// offered by more than one configured route resolve instead of hard-erroring
/// in `resolve_cli_auto_route`.
fn review_execution_route(config: &Config, args: &ReviewArgs) -> Result<(Config, bool)> {
    let explicit_provider = non_empty_flag(args.provider.as_deref());
    let explicit_model = non_empty_flag(args.model.as_deref());
    let mut resolved = config.clone();
    if let Some(provider_arg) = explicit_provider {
        apply_exec_provider_override(&mut resolved, provider_arg)?;
    }
    let force_configured_route =
        should_force_configured_exec_route(false, explicit_provider, explicit_model);
    Ok((resolved, force_configured_route))
}

fn non_empty_flag(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
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
    pending_telemetry_notice: Option<crate::telemetry_notice::PendingTelemetryNotice>,
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
        pending_telemetry_notice,
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
    /// Head commit SHA (`headRefOid`). Anchors posted review comments to the
    /// exact revision that was reviewed.
    head_sha: String,
}

fn run_gh_pr_view(number: u32, repo: Option<&str>) -> Result<GhPullRequest> {
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("pr").arg("view").arg(number.to_string());
    if let Some(r) = repo {
        cmd.arg("--repo").arg(r);
    }
    cmd.arg("--json")
        .arg("title,body,baseRefName,headRefName,url,headRefOid");
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
        head_sha: pick("headRefOid"),
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

/// Resolve the `owner/name` repository `gh` believes the current workspace
/// belongs to. `gh api` needs an explicit repository path, unlike `gh pr`
/// which infers it from the working directory.
fn run_gh_repo_name() -> Result<String> {
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("repo")
        .arg("view")
        .arg("--json")
        .arg("nameWithOwner")
        .arg("--jq")
        .arg(".nameWithOwner");
    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gh repo view`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh repo view failed: {stderr}");
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        bail!("`gh repo view` returned an empty repository name");
    }
    Ok(name)
}

/// Pick a fence long enough to wrap `replacement` without the replacement's
/// own backticks closing the block early.
fn suggestion_fence(replacement: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for ch in replacement.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.saturating_add(1).max(3))
}

/// GitHub turns any fenced `suggestion` block in a review comment into a
/// one-click commit. Model prose is not vetted for that, so a fence the model
/// wrote inside its own explanation is downgraded to a plain code block:
/// only the `replacement` this function validated against the diff hunks may
/// ever be committable.
fn neutralize_model_suggestion_fences(prose: &str) -> String {
    prose
        .split('\n')
        .map(neutralize_suggestion_fence_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Neutralize one line's fence if it opens a `suggestion` block.
///
/// The fence may sit behind leading whitespace or behind a blockquote cue or
/// list marker (`- ```suggestion`, `> ```suggestion`, `1. ```suggestion`).
/// Whether GitHub renders those container-nested blocks applicable is
/// unverified, so all of those shapes are treated as live and rewritten.
fn neutralize_suggestion_fence_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let (prefix, fence) = split_container_prefix(trimmed);
    let fence_char = match fence.chars().next() {
        Some(ch @ ('`' | '~')) => ch,
        _ => return line.to_string(),
    };
    let ticks = fence.chars().take_while(|ch| *ch == fence_char).count();
    if ticks < 3 {
        return line.to_string();
    }
    let info = fence[ticks..].trim();
    if info.to_ascii_lowercase().starts_with("suggestion") {
        let fence: String = std::iter::repeat_n(fence_char, ticks).collect();
        format!("{indent}{prefix}{fence}text")
    } else {
        line.to_string()
    }
}

/// Split the blockquote cues and list markers off the front of a line,
/// returning `(prefix_to_preserve, rest)`. Nesting is followed (`> - `),
/// but a line of ordinary prose is left untouched — the prefix only matters
/// when what follows it is a fence.
fn split_container_prefix(trimmed: &str) -> (&str, &str) {
    let mut rest = trimmed;
    while let Some(after) = strip_container_token(rest) {
        rest = after;
    }
    let split = trimmed.len() - rest.len();
    trimmed.split_at(split)
}

/// Strip one container token (`> ` blockquote cue, `-`/`*`/`+` bullet, or an
/// ordered-list marker) plus its trailing whitespace, or `None` when the
/// line does not start with one.
fn strip_container_token(rest: &str) -> Option<&str> {
    if let Some(after) = rest.strip_prefix('>') {
        return Some(after.trim_start_matches([' ', '\t']));
    }
    if let Some(after) = rest
        .strip_prefix(['-', '*', '+'])
        .filter(|after| after.starts_with([' ', '\t']))
    {
        return Some(after.trim_start_matches([' ', '\t']));
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let after_marker = &rest[digits..];
        if let Some(after) = after_marker
            .strip_prefix(['.', ')'])
            .filter(|after| after.starts_with([' ', '\t']))
        {
            return Some(after.trim_start_matches([' ', '\t']));
        }
    }
    None
}

/// Map structured review issues to GitHub inline-review-comment payloads.
///
/// Anchors are checked against the diff's actual hunks, not just its file
/// lists: GitHub 422s the *entire* review when one comment lands outside a
/// hunk, so a model-estimated line that misses now drops a single comment.
/// Issues without a locatable position stay in the summary body instead.
///
/// SAFETY: `title` and `description` are raw model text interpolated into a
/// comment body. A model-written ```suggestion fence there would become a
/// one-click-mergeable block that bypasses every span and size check, so
/// both fields pass through `neutralize_model_suggestion_fences` — only a
/// replacement validated against the diff hunks may ever be committable.
/// (`severity` is safe unneutralized: it is normalized to a fixed
/// error/warning/info vocabulary before it reaches here.)
fn inline_issue_comments(
    review: &crate::tools::review::ReviewOutput,
    hunks: &crate::tools::review_hunks::DiffHunks,
    plan: &mut InlineReviewPlan,
) -> Vec<serde_json::Value> {
    review
        .issues
        .iter()
        .filter_map(|issue| {
            let (Some(path), Some(line)) = (
                crate::tools::review::normalize_review_path(issue.path.as_deref()),
                issue.line,
            ) else {
                // No position at all: the summary body is the only home.
                return None;
            };
            if !hunks.contains_line(&path, line) {
                plan.note_unanchorable(hunks, &path);
                return None;
            }
            Some(serde_json::json!({
                "path": path,
                "line": line,
                "side": "RIGHT",
                "body": format!(
                    "**[{}] {}**\n\n{}",
                    issue.severity.to_uppercase(),
                    neutralize_model_suggestion_fences(&issue.title),
                    neutralize_model_suggestion_fences(&issue.description)
                ),
            }))
        })
        .collect()
}

/// Render one structured suggestion as an inline review comment.
///
/// A committable ```` ```suggestion ```` block is emitted only when **both**
/// safety conditions hold:
///
/// 1. the model supplied literal `replacement` code (not prose), and
/// 2. every line of the replaced span is a RIGHT-side line inside a diff hunk
///    (never a deleted LEFT-side line, which GitHub rejects), and the span is
///    small enough to be a mechanical fix.
///
/// Otherwise the comment degrades to prose at the same anchor — a wrong
/// committable suggestion is worse than prose because it is one click from
/// being merged. With no valid anchor at all the suggestion stays in the
/// summary body and `None` is returned.
fn inline_suggestion_comment(
    suggestion: &crate::tools::review::ReviewSuggestion,
    hunks: &crate::tools::review_hunks::DiffHunks,
    plan: &mut InlineReviewPlan,
) -> Option<serde_json::Value> {
    // The committable decision lives in `resolve_suggestion_anchor` so the
    // review receipt records exactly what this path emits for the same diff.
    let (path, start, end, committable) =
        match crate::tools::review::resolve_suggestion_anchor(suggestion, hunks) {
            crate::tools::review::SuggestionAnchor::Anchored {
                path,
                start,
                end,
                committable,
            } => (path, start, end, committable),
            crate::tools::review::SuggestionAnchor::Unanchorable { path } => {
                plan.note_unanchorable(hunks, &path);
                return None;
            }
            crate::tools::review::SuggestionAnchor::NoPosition => return None,
        };
    let prose = if suggestion.suggestion.is_empty() {
        "Suggested change.".to_string()
    } else {
        neutralize_model_suggestion_fences(&suggestion.suggestion)
    };

    let Some(replacement) = suggestion.replacement.as_deref().filter(|_| committable) else {
        // Degradation path: keep the finding, drop the one-click apply.
        plan.degraded_to_prose += 1;
        return Some(serde_json::json!({
            "path": path,
            "line": end,
            "side": "RIGHT",
            "body": prose,
        }));
    };

    let fence = suggestion_fence(replacement);
    let body = format!("{prose}\n\n{fence}suggestion\n{replacement}\n{fence}");
    let mut comment = serde_json::json!({
        "path": path,
        "line": end,
        "side": "RIGHT",
        "body": body,
    });
    if start < end {
        comment["start_line"] = serde_json::json!(start);
        comment["start_side"] = serde_json::json!("RIGHT");
    }
    Some(comment)
}

/// The inline-comment payload for one review, plus a truthful count of what
/// did not survive anchoring. Findings are never silently discarded: the
/// counts are reported on stderr next to the posted review.
#[derive(Debug, Default)]
struct InlineReviewPlan {
    comments: Vec<serde_json::Value>,
    /// Findings whose file is in the diff but whose line is not inside any
    /// hunk — a model-estimated line number that missed.
    dropped_out_of_hunk: usize,
    /// Findings pointing at a file this diff does not touch at all.
    dropped_untouched_file: usize,
    /// Suggestions posted as prose because committing them was not safe.
    degraded_to_prose: usize,
}

impl InlineReviewPlan {
    fn note_unanchorable(&mut self, hunks: &crate::tools::review_hunks::DiffHunks, path: &str) {
        if hunks.touches_path(path) {
            self.dropped_out_of_hunk += 1;
        } else {
            self.dropped_untouched_file += 1;
        }
    }

    /// One-line receipt, or `None` when every finding landed as intended.
    fn receipt(&self) -> Option<String> {
        if self.dropped_out_of_hunk == 0
            && self.dropped_untouched_file == 0
            && self.degraded_to_prose == 0
        {
            return None;
        }
        Some(format!(
            "note: {} finding(s) had no line inside a diff hunk, {} pointed at a file \
             outside the diff (both stay in the summary body), and {} suggestion(s) \
             posted as prose instead of a committable block",
            self.dropped_out_of_hunk, self.dropped_untouched_file, self.degraded_to_prose
        ))
    }
}

/// Every inline comment for a review: issues first, then suggestions (which
/// carry the committable ```` ```suggestion ```` blocks).
fn plan_inline_review_comments(
    review: &crate::tools::review::ReviewOutput,
    diff: &str,
) -> InlineReviewPlan {
    let hunks = crate::tools::review_hunks::DiffHunks::parse(diff);
    let mut plan = InlineReviewPlan::default();
    let mut comments = inline_issue_comments(review, &hunks, &mut plan);
    comments.extend(
        review
            .suggestions
            .iter()
            .filter_map(|suggestion| inline_suggestion_comment(suggestion, &hunks, &mut plan)),
    );
    plan.comments = comments;
    plan
}

/// Render a structured review as the markdown body printed for — and, with
/// `--post`, attached to — a pull-request review.
fn render_pr_review_markdown(
    number: u32,
    view: &GhPullRequest,
    review: &crate::tools::review::ReviewOutput,
    posted: bool,
) -> String {
    let mut body = String::new();
    body.push_str("## Codewhale review\n\n");
    if !review.summary.is_empty() {
        body.push_str(review.summary.trim());
        body.push_str("\n\n");
    }
    if !review.issues.is_empty() {
        body.push_str("### Findings\n\n");
        for issue in &review.issues {
            let location = match (&issue.path, issue.line) {
                (Some(path), Some(line)) => format!("`{}:{line}`", path.trim()),
                (Some(path), None) => format!("`{}`", path.trim()),
                _ => String::new(),
            };
            if location.is_empty() {
                body.push_str(&format!(
                    "- **[{}] {}**\n",
                    issue.severity.to_uppercase(),
                    issue.title
                ));
            } else {
                body.push_str(&format!(
                    "- **[{}] {}** ({location})\n",
                    issue.severity.to_uppercase(),
                    issue.title
                ));
            }
            if !issue.description.is_empty() {
                body.push_str(&format!("  {}\n", issue.description));
            }
        }
        body.push('\n');
    }
    if !review.suggestions.is_empty() {
        body.push_str("### Suggestions\n\n");
        for suggestion in &review.suggestions {
            let location = match (&suggestion.path, suggestion.line) {
                (Some(path), Some(line)) => format!("`{}:{line}`", path.trim()),
                (Some(path), None) => format!("`{}`", path.trim()),
                _ => String::new(),
            };
            if location.is_empty() {
                body.push_str(&format!("- {}\n", suggestion.suggestion));
            } else {
                body.push_str(&format!("- {location} — {}\n", suggestion.suggestion));
            }
            // The replacement is the computed fix itself; rendering only the
            // prose used to compute and validate it, then throw it away.
            // Show it as a fenced block indented into the list item. In the
            // local report the fence is live — the user asked for the fix and
            // this block is the artifact to apply. In a *posted* PR body it
            // degrades to a plain code block: GitHub must only ever be handed
            // a one-click suggestion block this pipeline validated against
            // the diff hunks (the inline suggestion comments), never one the
            // summary duplicated from a suggestion that failed anchoring or
            // the span gates.
            if let Some(replacement) = suggestion
                .replacement
                .as_deref()
                .filter(|replacement| !replacement.trim().is_empty())
            {
                let fence = suggestion_fence(replacement);
                let info = if posted { "text" } else { "suggestion" };
                body.push_str(&format!("\n  {fence}{info}\n"));
                for line in replacement.split('\n') {
                    body.push_str(&format!("  {line}\n"));
                }
                body.push_str(&format!("  {fence}\n"));
            }
        }
        body.push('\n');
    }
    if !review.overall_assessment.is_empty() {
        body.push_str("### Assessment\n\n");
        body.push_str(review.overall_assessment.trim());
        body.push_str("\n\n");
    }
    if posted {
        body.push_str(&format!(
            "---\n*Advisory review by Codewhale (`codewhale review --pr {number} --post`, \
             head `{head}`). Line-specific findings are also posted as inline review \
             comments; mechanical fixes arrive as committable suggestions you can \
             apply from the Files tab. CODEOWNERS approval still governs merge.*\n",
            head = if view.head_sha.is_empty() {
                "unknown"
            } else {
                view.head_sha.as_str()
            }
        ));
    }
    body
}

/// Post one COMMENT review: summary body plus inline comments anchored to the
/// PR head SHA. Never approves or requests changes — the review is advisory
/// and posts alongside CODEOWNERS, like `claude-review.yml`.
fn run_gh_post_pr_review(
    repo: &str,
    number: u32,
    body: &str,
    commit_id: &str,
    comments: &[serde_json::Value],
) -> Result<()> {
    let mut payload = serde_json::json!({
        "body": body,
        "event": "COMMENT",
    });
    if !commit_id.is_empty() {
        payload["commit_id"] = serde_json::json!(commit_id);
    }
    if !comments.is_empty() {
        payload["comments"] = serde_json::json!(comments);
    }
    let mut cmd = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow::anyhow!("gh not found on PATH"))?;
    cmd.arg("api")
        .arg("--method")
        .arg("POST")
        .arg(format!("repos/{repo}/pulls/{number}/reviews"))
        .arg("--input")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run `gh api`: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(serde_json::to_string(&payload)?.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write review payload: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("Failed to wait for `gh api`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh api POST repos/{repo}/pulls/{number}/reviews failed: {stderr}");
    }
    Ok(())
}

/// Publish a completed PR review: resolve the repository, render the summary,
/// and post it (with inline comments where the diff confirms the position).
fn post_pr_review(
    number: u32,
    view: &GhPullRequest,
    repo: Option<&str>,
    review: &crate::tools::review::ReviewOutput,
    diff: &str,
) -> Result<()> {
    let repo_name = match repo.map(str::trim).filter(|repo| !repo.is_empty()) {
        Some(repo) => repo.to_string(),
        None => run_gh_repo_name()?,
    };
    let plan = plan_inline_review_comments(review, diff);
    if let Some(receipt) = plan.receipt() {
        eprintln!("{receipt}");
    }
    let inline = plan.comments;
    let body = render_pr_review_markdown(number, view, review, true);
    if inline.is_empty() {
        run_gh_post_pr_review(&repo_name, number, &body, &view.head_sha, &[])?;
    } else if let Err(err) =
        run_gh_post_pr_review(&repo_name, number, &body, &view.head_sha, &inline)
    {
        // Anchors are pre-filtered against the parsed diff hunks, so this
        // path should now be unreachable in the common case. It stays as a
        // last resort for anchors GitHub rejects for reasons the diff cannot
        // show (a stale head SHA, a suppressed large file), and it announces
        // exactly how many inline comments were lost rather than failing
        // silently.
        eprintln!(
            "warning: {} inline review comment(s) rejected ({err}); retrying summary-only",
            inline.len()
        );
        run_gh_post_pr_review(&repo_name, number, &body, &view.head_sha, &[])?;
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
    let mut diff = if let Some(number) = args.pr {
        run_gh_pr_diff(number, args.repo.as_deref())?
    } else {
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
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    if diff.len() > args.max_chars {
        diff = crate::utils::truncate_with_ellipsis(&diff, args.max_chars, "\n...[truncated]\n");
    }
    Ok(diff)
}

fn review_target_label(args: &ReviewArgs) -> String {
    let mut label = if let Some(number) = args.pr {
        format!("pr:{number}")
    } else if args.staged {
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

            let mut args = Vec::with_capacity(if workspace.is_some() { 4 } else { 2 });
            if let Some(ref ws) = workspace {
                args.push("--workspace".to_string());
                args.push(ws.clone());
            }
            args.push("serve".to_string());
            args.push("--mcp".to_string());

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
                "  args:    {}serve --mcp",
                workspace.map_or(String::new(), |ws| format!("--workspace {ws} "))
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
fn doctor_mcp_command_status(server: &McpServerConfig) -> McpCommandAvailability {
    if server.url.is_some() {
        return McpCommandAvailability::NotApplicable;
    }
    match server.command.as_deref() {
        Some("") => McpCommandAvailability::Missing,
        Some(_) | None => McpCommandAvailability::NotChecked,
    }
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
        "transport": if server.url.is_some() { "http" } else { "stdio" },
        "endpoint": server.url.as_deref().map(crate::doctor::structural_url_authority),
        "command_configured": server.command.is_some(),
        "args_count": server.args.len(),
        "env_count": server.env.len(),
        "headers_count": server.headers.len(),
        "env_headers_count": server.env_headers.len(),
        "check_scope": "configuration",
        "checks": {
            "configuration": {
                "status": status.configuration_status(),
                "detail": status.detail(),
            },
            "command": {
                "status": doctor_mcp_command_status(server).as_str(),
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

    // URL-based server: omit userinfo, query, and fragment entirely.
    if let Some(ref url) = server.url {
        let authority = crate::doctor::structural_url_authority(url);
        return if authority.starts_with("unparseable") {
            McpServerDoctorStatus::Warning(
                "HTTP/SSE server URL is invalid; configured value omitted".to_string(),
            )
        } else {
            McpServerDoctorStatus::Ok(format!("HTTP/SSE server at {authority}"))
        };
    }

    // Command-based: validate command path exists.
    let cmd = server.command.as_deref().unwrap_or("");
    if cmd.is_empty() {
        return McpServerDoctorStatus::Error("empty command".to_string());
    }

    if server.cwd.is_none() {
        if is_relative_stdio_path_arg(cmd) {
            return McpServerDoctorStatus::Warning(
                "stdio server uses a relative command without cwd; command value omitted"
                    .to_string(),
            );
        }
        if server
            .args
            .iter()
            .any(|arg| is_relative_stdio_path_arg(arg) && !is_scoped_npm_package_arg(cmd, arg))
        {
            return McpServerDoctorStatus::Warning(
                "stdio server uses a relative path argument without cwd; argument values omitted"
                    .to_string(),
            );
        }
    }

    McpServerDoctorStatus::Ok(format!(
        "stdio server configured (command omitted; {} argument(s), {} environment binding(s))",
        server.args.len(),
        server.env.len()
    ))
}

/// `@scope/package@version` is an npm package spec, not a relative filesystem
/// path, even though it contains `/`. Keep this exception tied to the npx
/// launcher so similarly shaped arguments to other commands retain the
/// relative-path warning.
fn is_scoped_npm_package_arg(command: &str, argument: &str) -> bool {
    let launcher = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    if !launcher.eq_ignore_ascii_case("npx") && !launcher.eq_ignore_ascii_case("npx.cmd") {
        return false;
    }

    let Some(scoped) = argument.strip_prefix('@') else {
        return false;
    };
    let Some((scope, package_and_version)) = scoped.split_once('/') else {
        return false;
    };
    if scope.is_empty()
        || package_and_version.is_empty()
        || package_and_version.contains('/')
        || package_and_version.contains('\\')
    {
        return false;
    }

    let (package, version) = match package_and_version.split_once('@') {
        Some((package, version)) => (package, Some(version)),
        None => (package_and_version, None),
    };
    let valid_name = |value: &str| {
        !value.is_empty()
            && !value.starts_with(['.', '_'])
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    };
    valid_name(scope)
        && valid_name(package)
        && version.is_none_or(|value| {
            !value.is_empty()
                && !value.contains(['@', '/', '\\'])
                && !value.chars().any(char::is_whitespace)
        })
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

/// Screen the interactive TUI starts on.
///
/// `tui.alternate_screen` is the existing knob and keeps its existing
/// vocabulary: `auto`/`always` stay on the alternate screen (the default),
/// while `never` now selects the full-height inline viewport instead of being
/// parsed and ignored. `/inline` and `/fullscreen` move it at runtime.
fn startup_screen_mode(_cli: &Cli, config: &Config) -> ScreenMode {
    config
        .tui
        .as_ref()
        .and_then(|tui| tui.alternate_screen.as_deref())
        .and_then(ScreenMode::parse)
        .unwrap_or_default()
}

/// The user's mouse-capture preference with the screen factored out: the
/// CLI flags, then `tui.mouse_capture`, then the host default. Which screen
/// the session is on decides whether it applies — see
/// [`ScreenMode::mouse_capture`], which startup and the runtime switch share.
fn mouse_capture_preference(cli: &Cli, config: &Config) -> bool {
    let terminal_emulator = std::env::var("TERMINAL_EMULATOR").ok();
    let wt_session = std::env::var("WT_SESSION").ok().filter(|s| !s.is_empty());
    let conemu_pid = std::env::var("ConEmuPID").ok().filter(|s| !s.is_empty());
    mouse_capture_preference_with(
        cli,
        config,
        terminal_emulator.as_deref(),
        wt_session.as_deref(),
        conemu_pid.as_deref(),
    )
}

fn mouse_capture_preference_with(
    cli: &Cli,
    config: &Config,
    terminal_emulator: Option<&str>,
    wt_session: Option<&str>,
    conemu_pid: Option<&str>,
) -> bool {
    if cli.no_mouse_capture {
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

#[cfg(test)]
fn should_use_mouse_capture_with(
    cli: &Cli,
    config: &Config,
    use_alt_screen: bool,
    terminal_emulator: Option<&str>,
    wt_session: Option<&str>,
    conemu_pid: Option<&str>,
) -> bool {
    let mode = if use_alt_screen {
        ScreenMode::Fullscreen
    } else {
        ScreenMode::Inline
    };
    mode.mouse_capture(mouse_capture_preference_with(
        cli,
        config,
        terminal_emulator,
        wt_session,
        conemu_pid,
    ))
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
/// Resolve the session `--continue` attaches to.
///
/// Recovery promotes the newest same-workspace checkpoint to a session file
/// and clears the checkpoint. That is only correct when the TUI can actually
/// start: a non-TTY launch fails `require_interactive_terminal` later and must
/// not consume the crash record on the way out, or the next real `--continue`
/// has nothing left to recover. Without a terminal, only the latest saved
/// session is considered (and the launch still fails the TTY check).
fn resolve_continue_session_id(launch_workspace: &Path, interactive: bool) -> Option<String> {
    if interactive {
        recover_interrupted_checkpoint_for_resume(launch_workspace).or_else(|| {
            latest_session_id_for_workspace(launch_workspace)
                .ok()
                .flatten()
        })
    } else {
        latest_session_id_for_workspace(launch_workspace)
            .ok()
            .flatten()
    }
}

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
    // #4479: removed save_session call — checkpoint should not be auto-promoted to session
    if newest.source == session_manager::CheckpointSource::Legacy {
        // Migrate legacy single-slot checkpoint to per-session format
        // before clearing the legacy file, or the data is unrecoverable.
        let _ = manager.save_checkpoint(&newest.session);
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
    let path = match crate::config::resolve_load_config_path(config_path) {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to resolve workspace config overlay; refusing to substitute another file"
            );
            return;
        }
    };
    let raw = match read_user_config_file(&path) {
        Ok(Some(raw)) => raw,
        Ok(None) => return,
        Err(error) => {
            eprintln!(
                "warning: could not read user config at {}: {error}. \
                 Ignoring it — `[workspace]`/`[projects]` grants (e.g. `allow_shell`) \
                 revert to defaults for this session. Fix or remove the file to \
                 restore them.",
                path.display()
            );
            return;
        }
    };
    let doc = match toml::from_str::<toml::Value>(&raw) {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!(
                "warning: could not parse user config at {}: {error}. \
                 Ignoring it — `[workspace]`/`[projects]` grants (e.g. `allow_shell`) \
                 revert to defaults for this session. Fix the TOML syntax to \
                 restore them.",
                path.display()
            );
            return;
        }
    };
    merge_user_workspace_config_from_doc(config, &doc, workspace);
    if allow_shell_from_env {
        config.allow_shell = allow_shell_before;
    }
}

fn read_user_config_file(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(path) {
                Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(metadata_error) => Err(metadata_error),
                _ => Err(error),
            }
        }
        Err(error) => Err(error),
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
    pending_telemetry_notice: Option<crate::telemetry_notice::PendingTelemetryNotice>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    run_interactive_with_notice(
        cli,
        config,
        resume_session_id,
        initial_input,
        None,
        pending_telemetry_notice,
        plugin_registry,
    )
    .await
}

/// As [`run_interactive`], but carrying a one-line startup receipt to show in
/// the transcript — used by auto-resume to explain why it did or did not
/// reattach to a previous session (#2934).
async fn run_interactive_with_notice(
    cli: &Cli,
    config: &Config,
    resume_session_id: Option<String>,
    initial_input: Option<tui::InitialInput>,
    startup_notice: Option<String>,
    pending_telemetry_notice: Option<crate::telemetry_notice::PendingTelemetryNotice>,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    let initial_input = if cli.remote_control {
        Some(tui::InitialInput::RemoteControl)
    } else {
        initial_input
    };
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
    if resume_session_id.is_none() {
        let explicit_route_override = crate::config::explicit_launch_provider_override().is_some()
            || crate::config::explicit_launch_model_override().is_some();
        apply_selected_fleet_operator_for_launch(
            &mut merged_config,
            &workspace,
            explicit_route_override,
            false,
        )?;
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
    let screen_mode = startup_screen_mode(cli, config);
    let mouse_capture_preference = mouse_capture_preference(cli, config);
    let use_mouse_capture = screen_mode.mouse_capture(mouse_capture_preference);
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
    // Best-effort per-provider catalog refresh: fetches the active provider's
    // own /v1/models endpoint and merges live rows into the provider lake
    // alongside the Models.dev snapshot. Currently active for TelecomJS, whose
    // model list is not covered by the Models.dev catalog.
    crate::client::DeepSeekClient::spawn_active_provider_catalog_refresh(config);

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

        // Cloud-dispatch orphan reconciliation: a previous TUI could quit
        // (or crash) with a detached cloud-agent runner in flight, leaving
        // an active job record — and possibly a billing sandbox — behind.
        // Stale active jobs are failed and torn down, then any
        // dispatch-labeled sandbox whose job no longer needs it is deleted
        // by label. Best effort: one bounded listing call when credentials
        // exist, fail-closed quiet otherwise, never fatal.
        if let Ok(store) = crate::cloud_dispatch::CloudJobStore::from_env() {
            let receipt = crate::dispatch_runner::startup_reconcile(
                &store,
                &crate::cloud_dispatch::LiveDaytonaLauncher,
            );
            if !receipt.is_empty() {
                logging::info(receipt);
            }
        }
    });

    // A launcher can forward `--yolo` to this binary via the CODEWHALE_YOLO
    // env var (config.yolo), not as a CLI flag. Honour either.
    let yolo = cli.yolo || config.yolo.unwrap_or(false);

    tui::run_tui(
        config,
        tui::TuiOptions {
            model,
            workspace,
            config_path: cli.config.clone(),
            config_profile: effective_config_profile(cli),
            allow_shell: interactive_tui_allow_shell(yolo, config),
            screen_mode,
            use_mouse_capture,
            mouse_capture_preference,
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
            startup_notice,
            max_subagents,
        },
        plugin_registry,
        pending_telemetry_notice,
    )
    .await
}

#[derive(Debug)]
struct CliAutoRoute {
    provider: crate::config::ApiProvider,
    model: String,
    reasoning_effort: Option<crate::tui::app::ReasoningEffort>,
    /// Whether the runtime should continue resolving reasoning per prompt.
    ///
    /// This is independent from `auto_model`: an Auto model can carry a fixed
    /// saved effort, while a fixed Fleet model can still request Auto effort.
    auto_controls_reasoning: bool,
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

fn cli_reasoning_effort_value_for_prompt(
    config: &Config,
    model: &str,
    effort: crate::tui::app::ReasoningEffort,
    prompt: &str,
) -> Option<String> {
    let resolved = if effort == crate::tui::app::ReasoningEffort::Auto {
        crate::auto_reasoning::select(false, prompt)
    } else {
        effort
    };
    cli_reasoning_effort_value(config, model, resolved)
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
        let preference = config
            .reasoning_effort()
            .filter(|_| config.reasoning_effort_is_explicit())
            .map(crate::tui::app::ReasoningEffort::from_setting);
        let (reasoning_effort, auto_controls_reasoning) =
            model_routing::resolve_auto_model_reasoning(preference, selection.reasoning_effort);
        Ok(CliAutoRoute {
            provider: selection.provider,
            model: selection.model,
            reasoning_effort,
            auto_controls_reasoning,
            auto_model: true,
        })
    } else {
        if let Some(selection) = model_routing::resolve_explicit_route_with_inventory(config, model)
        {
            let auto_controls_reasoning = matches!(
                selection.reasoning_effort,
                Some(crate::tui::app::ReasoningEffort::Auto)
            );
            return Ok(CliAutoRoute {
                provider: selection.provider,
                model: selection.model,
                reasoning_effort: selection.reasoning_effort,
                auto_controls_reasoning,
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
        let reasoning_effort = config
            .reasoning_effort()
            .map(crate::tui::app::ReasoningEffort::from_setting);
        Ok(CliAutoRoute {
            provider: config.api_provider(),
            model: model.to_string(),
            auto_controls_reasoning: matches!(
                reasoning_effort,
                Some(crate::tui::app::ReasoningEffort::Auto)
            ),
            reasoning_effort,
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
        let reasoning_effort = config
            .reasoning_effort()
            .map(crate::tui::app::ReasoningEffort::from_setting);
        return Ok(CliAutoRoute {
            provider: config.api_provider(),
            model: model.to_string(),
            auto_controls_reasoning: matches!(
                reasoning_effort,
                Some(crate::tui::app::ReasoningEffort::Auto)
            ),
            reasoning_effort,
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
    use crate::models::{
        ContentBlock, Message, MessageRequest, is_incomplete_stop_reason, stop_reason_detail,
    };

    let route = resolve_cli_exec_route(config, model, prompt, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let client = DeepSeekClient::new(&execution_config)?;
    let reasoning_effort = route.reasoning_effort.and_then(|effort| {
        cli_reasoning_effort_value_for_prompt(&execution_config, &route.model, effort, prompt)
    });
    let model = route.model;
    let request_route = client.effective_route_envelope(&model, chrono::Utc::now());

    let request = MessageRequest {
        model,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: client.effective_max_output_tokens(&request_route.model),
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
    let stop_reason = response.stop_reason.clone();

    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            println!("{text}");
        }
    }

    if is_incomplete_stop_reason(stop_reason.as_deref()) {
        anyhow::bail!(
            "Model response incomplete: provider stop reason `{}`; the partial response was printed but the command did not succeed.",
            stop_reason_detail(stop_reason.as_deref())
        );
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
    use crate::models::{
        ContentBlock, Message, MessageRequest, SystemPrompt, is_incomplete_stop_reason,
        stop_reason_detail,
    };

    let route = resolve_cli_exec_route(config, model, prompt, force_configured_route).await?;
    let execution_config = config_for_cli_route(config, &route);
    let provider = execution_config.provider_identity_for(route.provider);
    let client = DeepSeekClient::new(&execution_config)?;
    let model = route.model.clone();
    let reasoning_effort = route.reasoning_effort.and_then(|effort| {
        cli_reasoning_effort_value_for_prompt(&execution_config, &model, effort, prompt)
    });
    let request_route = client.effective_route_envelope(&model, chrono::Utc::now());
    let request = MessageRequest {
        model: model.clone(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        }],
        max_tokens: client.effective_max_output_tokens(&request_route.model),
        system: Some(SystemPrompt::Text(
            "You are a coding assistant. Give concise, actionable responses.".to_string(),
        )),
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
    let stop_reason = response.stop_reason.clone();
    let usage = response.usage.clone();
    let mut output = String::new();
    for block in response.content {
        if let ContentBlock::Text { text, .. } = block {
            output.push_str(&text);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&one_shot_exec_json_receipt(
            provider,
            model,
            output,
            stop_reason.clone(),
            usage,
        ))?
    );
    if is_incomplete_stop_reason(stop_reason.as_deref()) {
        anyhow::bail!(
            "Model response incomplete: provider stop reason `{}`; the JSON receipt records success=false.",
            stop_reason_detail(stop_reason.as_deref())
        );
    }
    Ok(())
}

fn one_shot_exec_json_receipt(
    provider: String,
    model: String,
    output: String,
    stop_reason: Option<String>,
    usage: crate::models::Usage,
) -> serde_json::Value {
    let incomplete = crate::models::is_incomplete_stop_reason(stop_reason.as_deref());
    let error = incomplete.then(|| {
        format!(
            "Model response incomplete: provider stop reason `{}`.",
            crate::models::stop_reason_detail(stop_reason.as_deref())
        )
    });
    serde_json::json!({
        "mode": "one-shot",
        "provider": provider,
        "model": model,
        "success": !incomplete,
        "output": output,
        "stop_reason": stop_reason,
        "usage": usage,
        "error": error,
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
    /// Resolved output ceiling the route actually requested (post-catalogue).
    #[serde(skip_serializing_if = "Option::is_none")]
    codewhale_max_output_tokens: Option<u32>,
    /// Provenance of that ceiling: `documented`, `uncatalogued`, or
    /// `route-declared`.
    #[serde(skip_serializing_if = "Option::is_none")]
    codewhale_max_output_tokens_source: Option<&'static str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    /// A sub-agent was launched, and the model it was launched on.
    ///
    /// Without this, a delegated child is invisible to anything reading the
    /// stream: a parent turn on one route could spawn children billed on
    /// another and the only place it surfaced was the invoice. That is not
    /// hypothetical — the `Fast` loadout re-priced scout children onto a
    /// cheaper sibling until it was fixed, and nothing in the output said so.
    #[serde(rename = "agent_spawned")]
    AgentSpawned {
        id: String,
        model: String,
        spawn_depth: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<String>,
        /// Why the child got this route, when the spawn path resolved one.
        #[serde(skip_serializing_if = "Option::is_none")]
        route_source: Option<String>,
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
    #[serde(rename = "service_released")]
    #[cfg(unix)]
    ServiceReleased {
        task_id: String,
        pid: u32,
        process_group_id: u32,
        ownership: String,
    },
    /// Per-model-call usage receipt. Field names mirror the terminal
    /// `metadata` receipt (`prompt_cache_hit_tokens` is the provider's
    /// cache-read count, `prompt_cache_write_tokens` the cache-creation
    /// count). Optional fields are omitted — never emitted as null or zero —
    /// when the provider does not report them; the whole event is skipped
    /// for model calls whose provider reported no usage at all.
    #[serde(rename = "turn_usage")]
    TurnUsage {
        /// 1-based index of the model call within this exec run.
        turn: u32,
        input_tokens: u32,
        output_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_hit_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_miss_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_write_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_replay_tokens: Option<u32>,
        duration_ms: u64,
    },
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

/// Process exit code `codewhale exec` uses when a turn ends on a retryable
/// infrastructure failure (provider/transport) rather than a genuine task
/// failure. 75 is `EX_TEMPFAIL` from sysexits.h — "temporary failure; the
/// invocation is expected to succeed on retry" — so bench harnesses and
/// supervisors can distinguish retryable infra exits from genuine task
/// failures (exit 1) without parsing the stream-json metadata.
const EXEC_EXIT_RETRYABLE_INFRA: i32 = 75; // EX_TEMPFAIL

/// Map a terminal exec error category to the process exit code.
///
/// `network` / `timeout` mean the provider connection dropped or stalled
/// after every in-session retry budget was exhausted: the task itself
/// neither passed nor failed, and re-running the same command is safe.
/// `rate_limit` is deliberately NOT mapped to the retryable code — the same
/// category also covers quota exhaustion, which a blind retry would hammer.
fn exec_failure_exit_code(error_category: Option<&str>) -> i32 {
    match error_category {
        Some("network" | "timeout") => EXEC_EXIT_RETRYABLE_INFRA,
        _ => 1,
    }
}

/// Should a mid-turn engine error event force the final exec summary into
/// failure? Only non-recoverable envelopes do. Recoverable warnings (stream
/// stall notices, transient retry noise) are emitted on the stream for
/// visibility, but the terminal `TurnComplete` event carries the
/// authoritative turn outcome — a warning must never fail a run whose turn
/// later completes.
fn exec_error_event_is_fatal(envelope: &crate::error_taxonomy::ErrorEnvelope) -> bool {
    !envelope.recoverable
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
            codewhale_max_output_tokens: None,
            codewhale_max_output_tokens_source: None,
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
            error: (!completed)
                .then(|| format!("workflow run ended with terminal status {workflow_status}")),
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
    let mode = AppMode::Operate;
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
    .with_elevated_sandbox_policy(crate::core::authority::sandbox_policy_for_turn(
        mode,
        if yolo {
            crate::tui::approval::ApprovalMode::Bypass
        } else {
            crate::tui::approval::ApprovalMode::Suggest
        },
        config.sandbox_mode.as_deref(),
        workspace,
        crate::core::authority::SandboxNetworkAccess::from_config(config.sandbox_network_access),
    ));
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
    let roster = Arc::new(crate::fleet::identity::load_effective_roster(
        &config.fleet_config(),
        workspace,
        Some(plugin_registry.as_ref()),
    ));
    let mut role_models = roster.model_overrides();
    role_models.extend(config.subagent_model_overrides());

    let features = config.features();
    let mut surface = AgentToolSurfaceOptions::new(shell_policy);
    surface.apply_patch_enabled = features.enabled(Feature::ApplyPatch);
    surface.web_search_enabled = features.enabled(Feature::WebSearch);
    surface.memory_tool_enabled = config.memory_enabled();
    surface.vision_config = features
        .enabled(Feature::VisionModel)
        .then(|| config.vision_model_config())
        .flatten();
    surface.speech_output_dir = config.speech_output_dir();
    surface.goal_state = Some(new_shared_goal_state());

    let client = DeepSeekClient::new(config)?;
    // A FIXED model with `reasoning_effort = auto` (the shape a Fleet worker
    // subprocess launches with: `--model <exact> --reasoning-effort auto`) is
    // still Auto. Deriving the auto flag from `route.auto_model` alone left it
    // raw AND non-auto: the runtime carried the literal string `"auto"` while
    // nothing was allowed to resolve it. Auto is a reasoning decision, not a
    // model decision — it does not require `--model auto`.
    let reasoning_effort_auto = route.auto_controls_reasoning;
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
    .with_auto_model(route.auto_model)
    .with_reasoning_effort(reasoning_effort, reasoning_effort_auto)
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
    if let crate::core::events::Event::WorkflowUi { run_id, event, .. } = event {
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    released_services: Vec<crate::tools::shell::PersistentServiceReceipt>,
}

fn validate_exec_tool_authority_resume(
    tool_authority_json: Option<&str>,
    resuming: bool,
) -> Result<()> {
    if tool_authority_json.is_some() && resuming {
        bail!(
            "Fleet tool authority cannot be combined with exec --resume, --session-id, or --continue"
        );
    }
    Ok(())
}

fn exec_network_policy(
    config: &Config,
    outer_network_access: Option<bool>,
) -> Option<crate::network_policy::NetworkPolicyDecider> {
    // Fleet caps are an outer authority boundary: user configuration may
    // narrow them further, but it may never widen an explicit network denial.
    if outer_network_access == Some(false) {
        return Some(crate::network_policy::NetworkPolicyDecider::new(
            crate::network_policy::NetworkPolicy {
                default: crate::network_policy::DecisionToml::Deny,
                ..crate::network_policy::NetworkPolicy::default()
            },
            None,
        ));
    }
    config.network.clone().map(|toml_cfg| {
        crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
    })
}

fn apply_fleet_engine_feature_caps(
    features: &mut crate::features::Features,
    fleet_authority_active: bool,
    outer_network_access: Option<bool>,
    shell_authority: crate::tools::spec::ToolShellAuthority,
) {
    if fleet_authority_active {
        features.disable(crate::features::Feature::Subagents);
        features.disable(crate::features::Feature::Mcp);
        if shell_authority != crate::tools::spec::ToolShellAuthority::ReadOnly {
            features.disable(crate::features::Feature::ShellTool);
        }
    }
    if outer_network_access == Some(false) {
        features.disable(crate::features::Feature::WebSearch);
    }
}

/// Resolve the optional headless safety budget without imposing a hidden
/// default. Benchmarks and other long-running exec callers continue until the
/// model finishes unless they opt into a finite `--max-turns` value.
/// Watch stdin for EOF — the manager closed the pipe because it died — and
/// terminate our own process group. Fleet workers run in their own session
/// (setsid, host.rs), so nothing else kills them after a parent crash; the
/// same guard makes a task-timeout worker stop its own tree deterministically
/// (R7). Windows workers are reaped by the host's Job Object instead, so the
/// watcher is a Unix-only concern.
fn spawn_parent_death_watch() {
    #[cfg(not(unix))]
    {
        return;
    }
    #[cfg(unix)]
    std::thread::Builder::new()
        .name("parent-death-watch".to_string())
        .spawn(|| {
            use std::io::Read as _;
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 512];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => continue,
                }
            }
            tracing::info!(
                target: "fleet",
                "parent stdin closed; terminating worker process group"
            );
            // SAFETY: kill is async-signal-safe; 0 targets this process's own
            // group, which after setsid is exactly the worker tree.
            unsafe { libc::kill(0, libc::SIGTERM) };
        })
        .expect("spawn parent-death watch thread");
}

// The non-interactive exec agent assembly lives in `exec_agent`; the
// glob re-export keeps the dispatch and test references unchanged (#5586).
mod exec_agent;
pub(crate) use exec_agent::*;

#[cfg(test)]
mod serve_bind_host_tests {
    use super::*;

    #[test]
    fn http_defaults_to_loopback() {
        assert_eq!(
            resolve_serve_bind_host(false, None),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
            }
        );
    }

    #[test]
    fn mobile_defaults_to_loopback() {
        assert_eq!(
            resolve_serve_bind_host(true, None),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
            }
        );
    }

    #[test]
    fn mobile_respects_explicit_loopback_host() {
        assert_eq!(
            resolve_serve_bind_host(true, Some("127.0.0.1".to_string())),
            ServeBindHost {
                host: "127.0.0.1".to_string(),
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
            }
        );
    }
}

#[cfg(test)]
#[path = "tests/exec_exit_semantics.rs"]
mod exec_exit_semantics_tests;
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
        assert_eq!(
            report["provider_model"]["auth"]["source"],
            "secret_store_unprobed"
        );
        assert_eq!(
            report["provider_model"]["auth"]["availability"],
            "not_probed"
        );
        assert_eq!(
            report["provider_model"]["auth"]["credential_url"],
            "https://platform.deepseek.com"
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
            "https://platform.deepseek.com"
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
            "https://platform.kimi.ai"
        );
        assert_eq!(
            kimi_report["provider_model"]["auth"]["credential_docs_url"],
            "https://platform.kimi.ai"
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
            api_key: Some("TEST-STRUCTURAL-LITERAL".to_string()),
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

        let unprobed_config = Config {
            api_key: Some(crate::config::API_KEYRING_SENTINEL.to_string()),
            ..config.clone()
        };
        let unprobed_report = doctor_setup_report_json(&unprobed_config, &workspace);
        assert_eq!(unprobed_report["credential"]["ready"], false);
        assert_eq!(unprobed_report["credential"]["availability"], "not_probed");
        assert_eq!(unprobed_report["first_run_ready"], true);
        assert_eq!(unprobed_report["update_ready"], true);
    }

    #[test]
    fn doctor_reports_settings_permission_posture_when_approval_policy_unset() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, codewhale_home) = prepare_env(&tmp);
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(
            codewhale_home.join("settings.toml"),
            "permission_posture = \"full-access\"\n",
        )
        .expect("write settings.toml");

        let config = Config::default();
        assert!(config.approval_policy.is_none());

        let line = doctor_runtime_posture_line(&config, &workspace);
        assert!(
            line.contains("permission_posture=full-access (settings)"),
            "text doctor should report saved settings posture: {line}"
        );
        assert!(
            line.contains("approval_policy=on-request (default)"),
            "text doctor should keep unset config approval_policy default: {line}"
        );

        let report = doctor_setup_report_json(&config, &workspace);
        assert_eq!(
            report["runtime_posture"]["permission_posture"]["value"],
            "full-access"
        );
        assert_eq!(
            report["runtime_posture"]["permission_posture"]["source"],
            "settings"
        );
        assert_eq!(
            report["runtime_posture"]["approval_policy"]["value"],
            "on-request"
        );
        assert_eq!(
            report["runtime_posture"]["approval_policy"]["source"],
            "default"
        );
    }

    /// #5441: telemetry ships ON by default, and the runtime-posture doctor
    /// section must say so — with the source that decided it — instead of
    /// staying silent about the one default users never opted into.
    #[test]
    fn doctor_reports_resolved_telemetry_with_its_source() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let (_home_guard, _codewhale_home) = prepare_env(&tmp);
        let _telemetry_env = crate::test_support::EnvVarGuard::remove("CODEWHALE_TELEMETRY");
        let _telemetry_alias_env = crate::test_support::EnvVarGuard::remove("DEEPSEEK_TELEMETRY");
        let _telemetry_floor =
            crate::test_support::EnvVarGuard::remove("CODEWHALE_TELEMETRY_FLOOR");
        let workspace = tmp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        // Nothing configured anywhere: the shipped default applies and is
        // named, in both the text line and the JSON posture section.
        let config = Config::default();
        assert!(config.telemetry.is_none());
        let line = doctor_runtime_posture_line(&config, &workspace);
        assert!(
            line.contains("telemetry=on (default)"),
            "doctor line should name the defaulted consent: {line}"
        );
        let report = doctor_setup_report_json(&config, &workspace);
        assert_eq!(report["runtime_posture"]["telemetry"]["value"], true);
        assert_eq!(report["runtime_posture"]["telemetry"]["source"], "default");

        // A persisted opt-out is reported as the config file's decision.
        let config = Config {
            telemetry: Some(false),
            ..Config::default()
        };
        let line = doctor_runtime_posture_line(&config, &workspace);
        assert!(
            line.contains("telemetry=off (config)"),
            "doctor line should name the persisted opt-out: {line}"
        );
        let report = doctor_setup_report_json(&config, &workspace);
        assert_eq!(report["runtime_posture"]["telemetry"]["value"], false);
        assert_eq!(report["runtime_posture"]["telemetry"]["source"], "config");
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

        let config = Config {
            api_key: Some("TEST-STRUCTURAL-LITERAL".to_string()),
            ..Config::default()
        };
        let report = doctor_setup_report_json(&config, &workspace);

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
        assert_eq!(target.resolution, DoctorModelResolution::Resolved);
    }

    #[test]
    fn doctor_api_target_falls_back_to_configured_model_when_resolution_fails() {
        // `custom` with no custom provider table cannot resolve an identity;
        // doctor must fall back to the raw configured model and say so
        // instead of presenting an unresolved value as the engine's route.
        let config = Config {
            provider: Some("custom".to_string()),
            ..Default::default()
        };

        let target = doctor_api_target(&config);

        assert_eq!(target.resolution, DoctorModelResolution::ConfiguredOnly);
        assert_eq!(target.model, config.default_model());
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
        assert_eq!(target.resolution, DoctorModelResolution::Resolved);
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
        assert_eq!(
            doctor_strict_tool_mode_report_json(&status)["recommended_base_url"],
            "https://api.deepseek.com"
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
            "https://tokenhub.tencentmaas.com"
        );
        assert_eq!(report["base_url"]["class"], "custom");
        assert_eq!(report["auth"]["scheme"], "bearer");
        assert_eq!(report["auth"]["source"], "config_declared");
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
            crate::doctor::structural_url_authority(crate::config::DEFAULT_SILICONFLOW_CN_BASE_URL)
        );
        assert_eq!(report["base_url"]["class"], "default");
        assert_eq!(report["auth"]["scheme"], "bearer");
        assert_eq!(report["auth"]["source"], "config_declared");
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
    fn provider_capability_report_uses_exact_kimi_code_route_facts() {
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

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], crate::config::KIMI_CODE_K3_MODEL);
        assert_eq!(report["context_window"], 262_144);
        assert_eq!(
            report["context_window_source"],
            "static Kimi Code safe floor"
        );
        assert_eq!(report["thinking_supported"], true);
    }

    #[test]
    fn provider_capability_report_honors_kimi_code_context_override() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("kimi-plan-secret".to_string()),
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    context_window: Some(1_048_576),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = provider_capability_report(&config);

        assert_eq!(
            report["resolved_model"],
            crate::config::KIMI_CODE_K3_MODEL,
            "the configured window must preserve Kimi Code's bare wire id"
        );
        assert_eq!(report["context_window"], 1_048_576);
        assert_eq!(report["context_window_source"], "configured");
        assert_eq!(report["thinking_supported"], true);
    }

    #[test]
    fn provider_capability_report_uses_direct_moonshot_k3_route_facts() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("moonshot-secret".to_string()),
                    base_url: Some(crate::config::DEFAULT_MOONSHOT_BASE_URL.to_string()),
                    model: Some("kimi-k3".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], "kimi-k3");
        assert_eq!(report["context_window"], 1_048_576);
        assert_eq!(report["context_window_source"], "catalog");
        assert_eq!(report["max_output"], 1_048_576);
        assert_eq!(report["thinking_supported"], true);
    }

    #[test]
    fn doctor_search_provider_line_includes_firecrawl_default_source_and_switch_hint() {
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };

        let line = doctor_search_provider_line(&Config::default());

        match prev {
            Some(value) => unsafe { std::env::set_var("DEEPSEEK_SEARCH_PROVIDER", value) },
            None => unsafe { std::env::remove_var("DEEPSEEK_SEARCH_PROVIDER") },
        }
        assert!(line.contains("search_provider: firecrawl"));
        assert!(line.contains("source: default"));
        assert!(line.contains("[search] provider"));
        assert!(line.contains("provider = \"baidu\""));
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
        assert_eq!(report["reachability"], "not_checked");
        assert_eq!(report["reachability_reason"], "offline_json");
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
        assert_eq!(report["reachability"], "not_checked");
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
    fn headless_consultant_authority_overrides_network_allow_and_disables_web_search() {
        let config = Config {
            network: Some(crate::config::NetworkPolicyToml {
                default: "allow".to_string(),
                audit: false,
                ..crate::config::NetworkPolicyToml::default()
            }),
            ..Config::default()
        };
        let authority = crate::tools::spec::ToolAuthorityEnvelope {
            schema_version: 1,
            owner: "consultant-1".to_string(),
            authority: crate::tools::spec::ToolMutationAuthority::ReadOnly,
            network_access: Some(false),
            shell: crate::tools::spec::ToolShellAuthority::None,
            verification: crate::tools::spec::ToolVerificationAuthority::None,
            writable_roots: Vec::new(),
            writable_files: Vec::new(),
            coordination_contracts: Vec::new(),
        }
        .normalized()
        .expect("Consultant authority");

        let policy = exec_network_policy(&config, authority.network_access)
            .expect("explicit network=false always installs a policy");
        assert_eq!(
            policy.evaluate("example.com", "web_search"),
            crate::network_policy::Decision::Deny,
            "the permissive user config must not widen Consultant network authority"
        );
        let mut features = crate::features::Features::default();
        features.enable(crate::features::Feature::ShellTool);
        features.enable(crate::features::Feature::WebSearch);
        apply_fleet_engine_feature_caps(
            &mut features,
            true,
            authority.network_access,
            authority.shell,
        );
        assert!(!features.enabled(crate::features::Feature::WebSearch));
        assert!(!features.enabled(crate::features::Feature::ShellTool));

        let worker_policy = exec_network_policy(&config, Some(true)).expect("configured policy");
        assert_eq!(
            worker_policy.evaluate("example.com", "web_search"),
            crate::network_policy::Decision::Allow,
            "a network-capable role keeps the configured policy"
        );
    }
    #[test]
    fn hidden_remote_control_flag_starts_the_interactive_handoff() {
        let cli = parse_cli(&["codewhale-tui", "--remote-control"]);
        assert!(cli.remote_control);
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

    #[test]
    fn doctor_operate_fleet_json_lists_multi_layer_profile_paths() {
        // #5098: doctor must name the winning layer and every losing path
        // when project and personal both define the same id.
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        let personal = home.join("agents");
        let project = workspace.join(".codewhale").join("agents");
        std::fs::create_dir_all(&personal).expect("personal agents");
        std::fs::create_dir_all(&project).expect("project agents");
        std::fs::write(
            personal.join("builder.toml"),
            "id = \"builder\"\nrole_hint = \"builder\"\nmodel = \"deepseek-v4-flash\"\n",
        )
        .expect("personal builder");
        std::fs::write(
            project.join("builder.toml"),
            "id = \"builder\"\nrole_hint = \"builder\"\nmodel = \"deepseek-v4-pro\"\n",
        )
        .expect("project builder");
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);

        let operate = doctor_operate_fleet_report_json(&Config::default(), &workspace);
        let layers = operate["roster"]["multi_layer"]
            .as_array()
            .expect("multi_layer array");
        let builder = layers
            .iter()
            .find(|entry| entry["id"] == "builder")
            .expect("builder multi-layer entry");
        assert_eq!(builder["effective"], "project");
        let paths: Vec<&str> = builder["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .filter_map(|layer| layer["path"].as_str())
            .collect();
        assert!(
            paths.iter().any(|path| path.ends_with("builder.toml")),
            "layer paths include the profile files: {builder}"
        );
        assert!(
            builder["layers"]
                .as_array()
                .expect("layers")
                .iter()
                .any(|layer| layer["origin"] == "personal" && layer["wins"] == false),
            "personal layer is listed as ignored: {builder}"
        );
        assert!(
            builder["layers"]
                .as_array()
                .expect("layers")
                .iter()
                .any(|layer| layer["origin"] == "project" && layer["wins"] == true),
            "project layer wins: {builder}"
        );
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
    fn chatgpt_auth_subcommand_parses() {
        let cli = parse_cli(&["codewhale-tui", "auth", "chatgpt"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(TuiAuthArgs {
                command: TuiAuthCommand::Chatgpt
            }))
        ));
        let cli = parse_cli(&["codewhale-tui", "auth", "chatgpt-revoke"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(TuiAuthArgs {
                command: TuiAuthCommand::ChatgptRevoke
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
            auto_controls_reasoning: false,
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

    #[test]
    fn fresh_launch_uses_selected_fleet_operator_unless_route_is_explicit() {
        let workspace = tempfile::tempdir().expect("workspace");
        let fleets = workspace.path().join(".codewhale").join("fleets");
        std::fs::create_dir_all(&fleets).expect("fleet directory");
        std::fs::write(fleets.join("selected"), "Launch\n").expect("selection");
        std::fs::write(
            fleets.join("launch.toml"),
            r#"schema = "fleet"
schema_revision = 2
name = "Launch"

[operator]
provider = "deepseek"
model = "deepseek-v4-flash-vision-exp"
reasoning = "high"
"#,
        )
        .expect("fleet file");

        let base = Config {
            api_key: Some("test-key".to_string()),
            provider: Some("openrouter".to_string()),
            reasoning_effort: Some("off".to_string()),
            ..Default::default()
        };
        let mut explicit = base.clone();
        assert!(
            !apply_selected_fleet_operator_for_launch(
                &mut explicit,
                workspace.path(),
                true,
                false,
            )
            .expect("explicit route bypasses Fleet operator")
        );
        assert_eq!(
            explicit.api_provider(),
            crate::config::ApiProvider::Openrouter
        );

        let mut selected = base;
        assert!(
            apply_selected_fleet_operator_for_launch(
                &mut selected,
                workspace.path(),
                false,
                false,
            )
            .expect("selected operator applies")
        );
        assert_eq!(
            selected.api_provider(),
            crate::config::ApiProvider::Deepseek
        );
        assert_eq!(selected.default_model(), "deepseek-v4-flash-vision-exp");
        assert_eq!(selected.reasoning_effort(), Some("high"));
        assert!(selected.fleet_operator_route_applied);
        assert!(selected.fleet_operator_reasoning_applied);

        let mut reasoning_override = Config {
            api_key: Some("test-key".to_string()),
            reasoning_effort: Some("off".to_string()),
            ..Default::default()
        };
        apply_selected_fleet_operator_for_launch(
            &mut reasoning_override,
            workspace.path(),
            false,
            true,
        )
        .expect("explicit reasoning coexists with Fleet route");
        assert_eq!(
            reasoning_override.default_model(),
            "deepseek-v4-flash-vision-exp"
        );
        assert_eq!(reasoning_override.reasoning_effort(), Some("off"));
        assert!(reasoning_override.fleet_operator_route_applied);
        assert!(!reasoning_override.fleet_operator_reasoning_applied);
    }

    #[test]
    fn selected_fleet_operator_load_error_redacts_paths_excerpts_and_opaque_name() {
        let workspace = tempfile::tempdir().expect("workspace");
        let fleets = workspace.path().join(".codewhale").join("fleets");
        std::fs::create_dir_all(&fleets).expect("fleet directory");
        let secret_marker = "sk-live-abcdef0123456789abcdef";
        std::fs::write(fleets.join("selected"), format!("{secret_marker}\n")).expect("selection");
        std::fs::write(
            fleets.join(format!("{secret_marker}.toml")),
            format!("invalid TOML /Users/operator/private {secret_marker}\n"),
        )
        .expect("invalid Fleet");

        let mut config = Config::default();
        let message =
            apply_selected_fleet_operator_for_launch(&mut config, workspace.path(), false, false)
                .expect_err("invalid selected Fleet must fail")
                .to_string();

        assert!(!message.contains(&workspace.path().display().to_string()));
        assert!(!message.contains("/Users/operator"));
        assert!(!message.contains(secret_marker));
        assert!(!message.contains("invalid TOML"));
        assert!(message.chars().count() <= 700, "{message}");
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

    #[tokio::test]
    async fn cli_auto_model_honors_a_fixed_reasoning_preference() {
        let config = Config {
            provider: Some("vllm".to_string()),
            reasoning_effort: Some("low".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    base_url: Some("http://127.0.0.1:18190/v1".to_string()),
                    model: Some("local-auto-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let route = resolve_cli_auto_route(&config, "auto", "debug a failing test")
            .await
            .expect("Auto model route");

        assert!(route.auto_model);
        assert_eq!(
            route.reasoning_effort,
            Some(crate::tui::app::ReasoningEffort::Low)
        );
        assert!(
            !route.auto_controls_reasoning,
            "a fixed saved tier must not be replaced per prompt"
        );
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
            auto_controls_reasoning: true,
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
            auto_controls_reasoning: false,
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

    #[test]
    fn exec_resume_honours_dispatcher_forwarded_launch_overrides() {
        // `codewhale --provider X --model Y exec --resume ID ...` reaches this
        // binary with X/Y only in CODEWHALE_PROVIDER / CODEWHALE_MODEL; a
        // resume must treat them as explicit instead of restoring the saved
        // route.
        assert_eq!(
            exec_resume_route_overrides(None, None, None, None),
            (false, None)
        );
        assert_eq!(
            exec_resume_route_overrides(None, None, Some("modelstudio-token-plan"), None),
            (true, None)
        );
        assert_eq!(
            exec_resume_route_overrides(None, None, None, Some("qwen3.8-flash")),
            (false, Some("qwen3.8-flash".to_string()))
        );
        assert_eq!(
            exec_resume_route_overrides(
                None,
                None,
                Some("modelstudio-token-plan"),
                Some(" qwen3.8-flash ")
            ),
            (true, Some("qwen3.8-flash".to_string()))
        );
        // Exec-level flags still win over the forwarded launch env.
        assert_eq!(
            exec_resume_route_overrides(
                Some("deepseek"),
                Some("deepseek-v4-pro"),
                Some("x"),
                Some("y")
            ),
            (true, Some("deepseek-v4-pro".to_string()))
        );
        // Blank values are not overrides.
        assert_eq!(
            exec_resume_route_overrides(None, None, Some("  "), Some("")),
            (false, None)
        );

        let saved = saved_exec_session("custom-a", crate::config::ZAI_GLM_5_2_MODEL);
        let mut launch_model_only = custom_exec_config("custom-b");
        let (explicit_provider, explicit_model) =
            exec_resume_route_overrides(None, None, None, Some("override-model"));
        let model = resolve_exec_resume_route(
            &mut launch_model_only,
            &saved,
            explicit_provider,
            explicit_model.as_deref(),
        )
        .expect("launch model override keeps saved provider");
        assert_eq!(launch_model_only.provider.as_deref(), Some("custom-a"));
        assert_eq!(model, "override-model");

        let mut launch_provider = custom_exec_config("custom-b");
        let (explicit_provider, explicit_model) =
            exec_resume_route_overrides(None, None, Some("custom-b"), None);
        let model = resolve_exec_resume_route(
            &mut launch_provider,
            &saved,
            explicit_provider,
            explicit_model.as_deref(),
        )
        .expect("launch provider override keeps the launched route");
        assert_eq!(launch_provider.provider.as_deref(), Some("custom-b"));
        assert_eq!(model, "model-b");
    }

    #[test]
    fn exec_resume_uses_dispatcher_env_route_loaded_by_config() {
        // Exercise the production sequence without starting an Engine turn:
        // `Config::load` applies the dispatcher-forwarded environment, then
        // the resume seam must treat the same launch env as explicit and skip
        // the unavailable persisted route rather than restoring it.
        let _env_lock = crate::test_support::lock_test_env();
        let _legacy_provider = crate::test_support::EnvVarGuard::remove("DEEPSEEK_PROVIDER");
        let _legacy_model = crate::test_support::EnvVarGuard::remove("DEEPSEEK_MODEL");
        let _provider = crate::test_support::EnvVarGuard::set("CODEWHALE_PROVIDER", "launch-route");
        let _model = crate::test_support::EnvVarGuard::set("CODEWHALE_MODEL", "dispatcher-model");
        let tmp = tempfile::tempdir().expect("config tempdir");
        let codewhale_home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("isolated Codewhale home");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"provider = "stored-route"

[providers.launch-route]
kind = "openai-compatible"
base_url = "https://launch.example.test/v1"
model = "configured-launch-model"
api_key = "test-only-key"
"#,
        )
        .expect("write config");

        let mut config = Config::load(Some(config_path), None).expect("load launch config");
        assert_eq!(config.provider.as_deref(), Some("launch-route"));
        assert_eq!(config.default_model(), "dispatcher-model");

        // The saved route deliberately has no live config table. This control
        // proves that a non-explicit resume would fail closed instead of
        // silently falling back; the dispatcher overrides must prevent that
        // restore attempt.
        let saved = saved_exec_session("stored-route", "stored-model");
        let mut restore_attempt = config.clone();
        let restore_error = resolve_exec_resume_route(&mut restore_attempt, &saved, false, None)
            .expect_err("unavailable saved route must not be restored");
        assert!(
            restore_error.to_string().contains("stored-route"),
            "{restore_error}"
        );

        let (explicit_provider, explicit_model) = exec_resume_route_overrides(
            None,
            None,
            crate::config::explicit_launch_provider_override().as_deref(),
            crate::config::explicit_launch_model_override().as_deref(),
        );
        assert!(explicit_provider);
        assert_eq!(explicit_model.as_deref(), Some("dispatcher-model"));

        let model = resolve_exec_resume_route(
            &mut config,
            &saved,
            explicit_provider,
            explicit_model.as_deref(),
        )
        .expect("dispatcher environment must keep the launch route on resume");
        assert_eq!(config.provider.as_deref(), Some("launch-route"));
        assert_eq!(model, "dispatcher-model");
    }

    #[test]
    fn exec_model_reads_wait_for_foreign_test_env_overrides_to_restore() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();

        let (reader, expected_after_restore) = {
            let lock = crate::test_support::lock_test_env();
            let expected_after_restore = exec_model_env_override();
            let temporary =
                crate::test_support::EnvVarGuard::set("CODEWHALE_MODEL", "temporary-model");
            let reader = std::thread::spawn(move || {
                started_tx.send(()).expect("signal model read start");
                tx.send(exec_model_env_override())
                    .expect("send resolved model override");
            });

            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("reader reached model read");
            assert!(
                rx.recv_timeout(std::time::Duration::from_millis(50))
                    .is_err(),
                "a foreign reader observed another test's temporary model override"
            );
            drop(temporary);
            drop(lock);
            (reader, expected_after_restore)
        };

        let observed = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reader resumed after model override was restored");
        reader.join().expect("reader thread");
        assert_eq!(observed, expected_after_restore);
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

    fn review_args(argv: &[&str]) -> ReviewArgs {
        let cli = parse_cli(argv);
        let Some(Commands::Review(args)) = cli.command else {
            panic!("expected review command");
        };
        args
    }

    #[test]
    fn review_parses_provider_flag_alongside_model() {
        let args = review_args(&[
            "codewhale",
            "review",
            "--pr",
            "5709",
            "--provider",
            "zai",
            "--model",
            "GLM-5.3",
        ]);

        assert_eq!(args.provider.as_deref(), Some("zai"));
        assert_eq!(args.model.as_deref(), Some("GLM-5.3"));
        assert_eq!(args.pr, Some(5709));
        // The threaded id round-trips through the provider vocabulary the
        // override validates against — never a model-id sniff.
        assert_eq!(
            crate::config::ApiProvider::parse(args.provider.as_deref().unwrap()),
            Some(crate::config::ApiProvider::Zai)
        );
    }

    #[tokio::test]
    async fn review_provider_flag_pins_route_for_multi_route_model() {
        // A genuinely multi-route model: `shared-review-model` is the
        // configured model of BOTH named routes below, while the active route
        // (custom-a) does not offer it. (Two custom providers can never be
        // ambiguous — the route inventory resolves only the active custom
        // entry — so the multi-route pair has to be named providers.)
        // Without `--provider`, an explicit `--model` lets cross-provider
        // inventory inference run, and a model offered by more than one
        // configured route hard-errors in `resolve_cli_auto_route`
        // ("available from configured provider route(s): ..."). That error
        // already tells the user to "Pass `--provider <provider>`" — until
        // now `codewhale review` had no such flag to pass.
        let mut config = custom_exec_config("custom-a");
        {
            let providers = config.providers.as_mut().expect("providers");
            for entry in [&mut providers.deepseek, &mut providers.openrouter] {
                *entry = crate::config::ProviderConfig {
                    model: Some("shared-review-model".to_string()),
                    api_key: Some("local-test-key".to_string()),
                    ..Default::default()
                };
            }
        }

        let inferred = review_args(&["codewhale", "review", "--model", "shared-review-model"]);
        let (inferred_config, inferred_force) =
            review_execution_route(&config, &inferred).expect("model-only route");
        assert!(
            !inferred_force,
            "an explicit model with no provider stays open to inventory inference"
        );
        assert_eq!(inferred_config.provider.as_deref(), Some("custom-a"));

        // Without the flag the multi-route model must refuse to guess.
        let err = resolve_cli_exec_route(
            &inferred_config,
            &resolve_review_model(&inferred_config, inferred.model.as_deref()),
            "review diff",
            inferred_force,
        )
        .await
        .expect_err("a model offered by two configured routes must hard-error");
        let message = err.to_string();
        assert!(
            message.contains("available from configured provider route(s)"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("deepseek") && message.contains("openrouter"),
            "both candidate routes must be named: {message}"
        );

        // With the flag the same model pins to the named route and resolves.
        let pinned = review_args(&[
            "codewhale",
            "review",
            "--provider",
            "deepseek",
            "--model",
            "shared-review-model",
        ]);
        let (pinned_config, pinned_force) =
            review_execution_route(&config, &pinned).expect("pinned route");
        assert!(pinned_force, "--provider makes the route authoritative");
        assert_eq!(pinned_config.provider.as_deref(), Some("deepseek"));

        let route = resolve_cli_exec_route(
            &pinned_config,
            &resolve_review_model(&pinned_config, pinned.model.as_deref()),
            "review diff",
            pinned_force,
        )
        .await
        .expect("pinned review route");
        let execution = config_for_cli_route(&pinned_config, &route);

        assert_eq!(route.provider, crate::config::ApiProvider::Deepseek);
        assert_eq!(route.model, "shared-review-model");
        assert_eq!(
            execution.provider_identity_for(route.provider),
            "deepseek",
            "the review runs on the provider the flag named"
        );

        // Pinning a configured custom provider (the workflow's
        // CODEWHALE_REVIEW_PROVIDER="my-proxy" case) is equally authoritative
        // for the same multi-route model.
        let pinned_custom = review_args(&[
            "codewhale",
            "review",
            "--provider",
            "custom-b",
            "--model",
            "shared-review-model",
        ]);
        let (custom_config, custom_force) =
            review_execution_route(&config, &pinned_custom).expect("pinned custom route");
        assert!(custom_force);
        let custom_route = resolve_cli_exec_route(
            &custom_config,
            &resolve_review_model(&custom_config, pinned_custom.model.as_deref()),
            "review diff",
            custom_force,
        )
        .await
        .expect("pinned custom review route");
        let custom_execution = config_for_cli_route(&custom_config, &custom_route);

        assert_eq!(custom_route.provider, crate::config::ApiProvider::Custom);
        assert_eq!(custom_route.model, "shared-review-model");
        assert_eq!(
            custom_execution.provider_identity_for(custom_route.provider),
            "custom-b",
            "the review runs on the custom provider the flag named"
        );
    }

    #[test]
    fn review_provider_flag_rejects_unknown_provider() {
        let config = custom_exec_config("custom-a");
        let args = review_args(&["codewhale", "review", "--provider", "not-a-provider"]);

        let err = review_execution_route(&config, &args)
            .expect_err("unknown provider must fail before any diff is fetched");
        assert!(
            err.to_string().contains("Unrecognized --provider"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn review_without_provider_flag_keeps_configured_route_authoritative() {
        let config = custom_exec_config("custom-a");
        let args = review_args(&["codewhale", "review"]);

        let (resolved, force) = review_execution_route(&config, &args).expect("default route");
        assert!(
            force,
            "the configured/default review route is authoritative"
        );
        assert_eq!(resolved.provider.as_deref(), Some("custom-a"));
    }

    fn review_issue(
        severity: &str,
        path: Option<&str>,
        line: Option<u32>,
    ) -> crate::tools::review::ReviewIssue {
        crate::tools::review::ReviewIssue {
            severity: severity.to_string(),
            title: format!("{severity} finding"),
            description: "detail".to_string(),
            path: path.map(str::to_string),
            line,
        }
    }

    fn review_suggestion(
        path: Option<&str>,
        line: Option<u32>,
        replacement: Option<&str>,
    ) -> crate::tools::review::ReviewSuggestion {
        crate::tools::review::ReviewSuggestion {
            path: path.map(str::to_string),
            line,
            start_line: None,
            end_line: None,
            suggestion: "Use the checked variant".to_string(),
            replacement: replacement.map(str::to_string),
        }
    }

    fn review_with(
        issues: Vec<crate::tools::review::ReviewIssue>,
        suggestions: Vec<crate::tools::review::ReviewSuggestion>,
    ) -> crate::tools::review::ReviewOutput {
        crate::tools::review::ReviewOutput {
            summary: "summary".to_string(),
            issues,
            suggestions,
            overall_assessment: String::new(),
        }
    }

    /// Two hunks in one file: right lines 10..=12 and 40..=41.
    ///
    /// Built from a slice, not one string literal: a `\` continuation strips
    /// the leading space that marks a context line.
    fn review_test_diff() -> String {
        [
            "diff --git a/crates/tui/src/lib.rs b/crates/tui/src/lib.rs",
            "index 1111111..2222222 100644",
            "--- a/crates/tui/src/lib.rs",
            "+++ b/crates/tui/src/lib.rs",
            "@@ -10,2 +10,3 @@ fn head() {",
            " let a = 1;",
            "+let b = a.unwrap();",
            " let c = 2;",
            "@@ -40,2 +40,2 @@ fn tail() {",
            " let d = 3;",
            "-let e = d;",
            "+let e = d + 1;",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn inline_review_comments_keep_only_hunk_locatable_issues() {
        let review = review_with(
            vec![
                review_issue("error", Some("./crates/tui/src/lib.rs"), Some(11)),
                review_issue("warning", Some("docs/NOT_IN_DIFF.md"), Some(3)),
                review_issue("info", Some("crates/tui/src/lib.rs"), None),
                review_issue("error", None, Some(7)),
            ],
            Vec::new(),
        );
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        // GitHub rejects the entire review (422) when a comment's path is not
        // part of the diff, and inline comments need a line to anchor to.
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["path"], "crates/tui/src/lib.rs");
        assert_eq!(comments[0]["line"], 11);
        assert_eq!(comments[0]["side"], "RIGHT");
    }

    #[test]
    fn inline_review_comments_drop_one_out_of_hunk_anchor_not_the_whole_review() {
        // Line 25 is inside the file but between the two hunks. Before the
        // hunk filter this single bad anchor 422'd the review and every inline
        // comment was lost to the summary-only retry.
        let review = review_with(
            vec![
                review_issue("error", Some("crates/tui/src/lib.rs"), Some(11)),
                review_issue("error", Some("crates/tui/src/lib.rs"), Some(25)),
                review_issue("warning", Some("crates/tui/src/lib.rs"), Some(41)),
            ],
            Vec::new(),
        );
        let plan = plan_inline_review_comments(&review, &review_test_diff());
        let lines: Vec<_> = plan.comments.iter().map(|c| c["line"].clone()).collect();
        assert_eq!(lines, vec![serde_json::json!(11), serde_json::json!(41)]);
        // The loss is counted and reported, never silent.
        assert_eq!(plan.dropped_out_of_hunk, 1);
        assert_eq!(plan.dropped_untouched_file, 0);
        assert!(
            plan.receipt()
                .expect("receipt")
                .contains("no line inside a diff hunk")
        );
    }

    #[test]
    fn review_suggestion_with_replacement_posts_a_committable_block() {
        let review = review_with(
            Vec::new(),
            vec![review_suggestion(
                Some("crates/tui/src/lib.rs"),
                Some(11),
                Some("let b = a.unwrap_or_default();"),
            )],
        );
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        assert_eq!(comments.len(), 1);
        let body = comments[0]["body"].as_str().expect("body");
        assert!(body.starts_with("Use the checked variant"), "{body}");
        assert!(
            body.contains("```suggestion\nlet b = a.unwrap_or_default();\n```"),
            "{body}"
        );
        assert_eq!(comments[0]["side"], "RIGHT");
        // Single-line suggestions must not carry start_line.
        assert!(comments[0].get("start_line").is_none());
    }

    #[test]
    fn multi_line_review_suggestion_spans_start_line_to_line() {
        let mut suggestion = review_suggestion(
            Some("crates/tui/src/lib.rs"),
            Some(12),
            Some("let b = a.unwrap_or_default();\nlet c = 2;"),
        );
        suggestion.start_line = Some(11);
        suggestion.end_line = Some(12);
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["start_line"], 11);
        assert_eq!(comments[0]["start_side"], "RIGHT");
        assert_eq!(comments[0]["line"], 12);
        assert_eq!(comments[0]["side"], "RIGHT");
    }

    #[test]
    fn review_suggestion_without_replacement_degrades_to_prose() {
        // SAFETY: a committable suggestion is one click from being merged, so
        // a suggestion the model did not back with literal code must never
        // render as an applicable block.
        let review = review_with(
            Vec::new(),
            vec![review_suggestion(
                Some("crates/tui/src/lib.rs"),
                Some(11),
                None,
            )],
        );
        let plan = plan_inline_review_comments(&review, &review_test_diff());
        assert_eq!(plan.comments.len(), 1);
        let body = plan.comments[0]["body"].as_str().expect("body");
        assert_eq!(body, "Use the checked variant");
        assert!(!body.contains("```suggestion"), "{body}");
        assert_eq!(plan.degraded_to_prose, 1);
    }

    #[test]
    fn review_suggestion_spanning_outside_a_hunk_degrades_to_prose() {
        // Line 12 is in a hunk but line 13 is not, so the span cannot be
        // committed. Keep the finding, drop the one-click apply.
        let mut suggestion = review_suggestion(
            Some("crates/tui/src/lib.rs"),
            Some(12),
            Some("let c = 2;\nlet d = 3;"),
        );
        suggestion.start_line = Some(12);
        suggestion.end_line = Some(13);
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        // end_line 13 is outside every hunk, so there is no anchor at all.
        assert!(comments.is_empty());

        // Same replacement anchored at an in-hunk end line but with a start
        // line outside the hunk: anchor is valid, span is not -> prose.
        let mut suggestion = review_suggestion(
            Some("crates/tui/src/lib.rs"),
            Some(11),
            Some("let a = 1;\nlet b = a.unwrap_or_default();"),
        );
        suggestion.start_line = Some(9);
        suggestion.end_line = Some(11);
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        assert_eq!(comments.len(), 1);
        let body = comments[0]["body"].as_str().expect("body");
        assert!(!body.contains("```suggestion"), "{body}");
        assert!(comments[0].get("start_line").is_none());
    }

    #[test]
    fn review_suggestion_anchored_to_a_deleted_line_is_not_committable() {
        // Right line 41 is `+let e = d + 1;`; the deleted `-let e = d;` has no
        // RIGHT-side number, so a model that anchors at the pre-image line 42
        // gets nothing rather than a suggestion on the wrong line.
        let review = review_with(
            Vec::new(),
            vec![review_suggestion(
                Some("crates/tui/src/lib.rs"),
                Some(42),
                Some("let e = d + 2;"),
            )],
        );
        assert!(
            plan_inline_review_comments(&review, &review_test_diff())
                .comments
                .is_empty()
        );
    }

    #[test]
    fn review_suggestion_replacement_containing_backticks_is_fenced_safely() {
        let review = review_with(
            Vec::new(),
            vec![review_suggestion(
                Some("crates/tui/src/lib.rs"),
                Some(11),
                Some("let b = \"```\";"),
            )],
        );
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        let body = comments[0]["body"].as_str().expect("body");
        assert!(body.contains("````suggestion\n"), "{body}");
        assert!(body.ends_with("\n````"), "{body}");
    }

    #[test]
    fn review_suggestion_multi_line_replacement_without_span_degrades_to_prose() {
        // SAFETY: with no start_line/end_line, GitHub would insert these two
        // lines at line 11 and leave the original line 11 duplicated below.
        let review = review_with(
            Vec::new(),
            vec![review_suggestion(
                Some("crates/tui/src/lib.rs"),
                Some(11),
                Some("let b = a.unwrap_or_default();\nlet c = 2;"),
            )],
        );
        let plan = plan_inline_review_comments(&review, &review_test_diff());
        assert_eq!(plan.comments.len(), 1);
        assert!(
            !plan.comments[0]["body"]
                .as_str()
                .expect("body")
                .contains("```suggestion")
        );
        assert_eq!(plan.degraded_to_prose, 1);
    }

    #[test]
    fn review_suggestion_prose_fence_is_never_committable() {
        // SAFETY: only the replacement this reviewer validated against the
        // diff may be one click from merging. A fence the model wrote inside
        // its explanation is downgraded to a plain code block.
        let mut suggestion = review_suggestion(Some("crates/tui/src/lib.rs"), Some(11), None);
        suggestion.suggestion =
            "Try this:\n\n```suggestion\nlet b = a.unwrap_or_default();\n```".to_string();
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        let body = comments[0]["body"].as_str().expect("body");
        assert!(!body.contains("```suggestion"), "{body}");
        assert!(body.contains("```text"), "{body}");
        assert!(body.contains("let b = a.unwrap_or_default();"), "{body}");
    }

    #[test]
    fn review_suggestion_tilde_prose_fence_is_neutralized() {
        let mut suggestion = review_suggestion(Some("crates/tui/src/lib.rs"), Some(11), None);
        suggestion.suggestion = "  ~~~suggestion\n  x\n  ~~~".to_string();
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        let body = comments[0]["body"].as_str().expect("body");
        assert!(body.contains("  ~~~text"), "{body}");
    }

    #[test]
    fn suggestion_fence_behind_list_marker_or_blockquote_is_neutralized() {
        // Whether GitHub renders a fence nested behind a list marker,
        // blockquote cue, or ordered-list marker applicable is unverified,
        // so those shapes are treated as live and downgraded too. Ordinary
        // prose lines that merely start with a marker stay untouched.
        let neutralized = neutralize_model_suggestion_fences(
            "```suggestion\n- ```suggestion\n  rm -rf /\n> ```suggestion\n1. ```suggestion\n> - ~~~suggestion\n- fix the `foo` call",
        );
        assert_eq!(
            neutralized,
            "```text\n- ```text\n  rm -rf /\n> ```text\n1. ```text\n> - ~~~text\n- fix the `foo` call"
        );
    }

    #[test]
    fn inline_issue_comment_neutralizes_model_suggestion_fences_in_issue_text() {
        // SAFETY: regression for a proven hole — the issue title and
        // description are raw model text, and a ```suggestion fence in
        // either used to reach the inline comment body verbatim: a
        // one-click mergeable block that bypassed every span and size
        // check. Only a replacement validated against the diff hunks may
        // ever be committable.
        let mut fenced_description = review_issue("error", Some("crates/tui/src/lib.rs"), Some(11));
        fenced_description.title = "Cleanup script".to_string();
        fenced_description.description = "Run this:\n\n```suggestion\nrm -rf /\n```\n".to_string();
        let mut fenced_title = review_issue("warning", Some("crates/tui/src/lib.rs"), Some(12));
        fenced_title.title = "Fix all\n```suggestion\nrm -rf ~\n```".to_string();
        fenced_title.description = "detail".to_string();
        let review = review_with(vec![fenced_description, fenced_title], Vec::new());

        let comments = plan_inline_review_comments(&review, &review_test_diff()).comments;
        assert_eq!(comments.len(), 2);
        for comment in &comments {
            let body = comment["body"].as_str().expect("body");
            assert!(!body.contains("```suggestion"), "{body}");
        }
        let description_body = comments[0]["body"].as_str().expect("body");
        assert!(
            description_body.contains("```text\nrm -rf /"),
            "{description_body}"
        );
        let title_body = comments[1]["body"].as_str().expect("body");
        assert!(title_body.contains("```text\nrm -rf ~"), "{title_body}");
    }

    #[test]
    fn pr_review_markdown_shows_the_computed_replacement() {
        // A plain `codewhale review --pr` (no --post) computes and validates
        // the literal fix; the local report must show it, not just the prose.
        let view = GhPullRequest {
            title: "T".to_string(),
            body: String::new(),
            base: "main".to_string(),
            head: "feature".to_string(),
            url: "https://example.invalid/pr/1".to_string(),
            head_sha: "abc123".to_string(),
        };
        let mut single = review_suggestion(
            Some("crates/tui/src/lib.rs"),
            Some(11),
            Some("let b = a.unwrap_or_default();"),
        );
        single.suggestion = "Use the checked variant".to_string();
        let mut multi = review_suggestion(
            Some("crates/tui/src/lib.rs"),
            Some(11),
            Some("let b = a.unwrap_or_default();\nlet c = 2;"),
        );
        multi.suggestion = "Replace both lines".to_string();
        multi.start_line = Some(11);
        multi.end_line = Some(12);
        let review = review_with(Vec::new(), vec![single, multi]);

        let local = render_pr_review_markdown(1, &view, &review, false);
        assert!(local.contains("### Suggestions"), "{local}");
        assert!(
            local.contains("\n  ```suggestion\n  let b = a.unwrap_or_default();\n  ```\n"),
            "{local}"
        );
        assert!(
            local.contains(
                "\n  ```suggestion\n  let b = a.unwrap_or_default();\n  let c = 2;\n  ```\n"
            ),
            "{local}"
        );

        // The posted body keeps the fix visible but never as a live
        // one-click block: only diff-validated inline suggestion comments
        // may carry those to GitHub.
        let posted = render_pr_review_markdown(1, &view, &review, true);
        assert!(
            posted.contains("let b = a.unwrap_or_default();"),
            "{posted}"
        );
        assert!(!posted.contains("```suggestion"), "{posted}");
        assert!(posted.contains("```text"), "{posted}");
    }

    #[test]
    fn oversized_review_suggestion_degrades_to_prose() {
        use crate::tools::review::MAX_COMMITTABLE_SUGGESTION_LINES;

        // A whole-hunk rewrite is not a mechanical fix, so it must not be
        // committable even though every line is inside the diff.
        let span = MAX_COMMITTABLE_SUGGESTION_LINES + 5;
        let added: String = (1..=span).map(|i| format!("+line {i}\n")).collect();
        let diff = format!(
            "diff --git a/big.rs b/big.rs\nnew file mode 100644\n--- /dev/null\n\
             +++ b/big.rs\n@@ -0,0 +1,{span} @@\n{added}"
        );
        let replacement: String = (1..=span)
            .map(|i| format!("line {i} fixed"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut suggestion = review_suggestion(Some("big.rs"), Some(span), Some(&replacement));
        suggestion.start_line = Some(1);
        suggestion.end_line = Some(span);
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &diff).comments;
        assert_eq!(comments.len(), 1);
        let body = comments[0]["body"].as_str().expect("body");
        assert!(!body.contains("```suggestion"), "{body}");

        // The same shape, trimmed to the budget, stays committable.
        let mut suggestion = review_suggestion(
            Some("big.rs"),
            Some(MAX_COMMITTABLE_SUGGESTION_LINES),
            Some("line 1 fixed"),
        );
        suggestion.start_line = Some(1);
        suggestion.end_line = Some(MAX_COMMITTABLE_SUGGESTION_LINES);
        let review = review_with(Vec::new(), vec![suggestion]);
        let comments = plan_inline_review_comments(&review, &diff).comments;
        assert!(
            comments[0]["body"]
                .as_str()
                .expect("body")
                .contains("```suggestion"),
            "a span at the budget limit stays committable"
        );
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
        let one_shot = one_shot_exec_json_receipt(
            provider.clone(),
            "model-a".to_string(),
            "done".to_string(),
            Some("end_turn".to_string()),
            crate::models::Usage {
                input_tokens: 12,
                output_tokens: 3,
                ..Default::default()
            },
        );
        assert_eq!(one_shot["provider"], "custom-a");
        assert_eq!(one_shot["success"], true);

        let truncated = one_shot_exec_json_receipt(
            provider.clone(),
            "model-a".to_string(),
            "partial".to_string(),
            Some("max_output_tokens".to_string()),
            crate::models::Usage {
                input_tokens: 20,
                output_tokens: 9,
                ..Default::default()
            },
        );
        assert_eq!(truncated["success"], false);
        assert_eq!(truncated["stop_reason"], "max_output_tokens");
        assert_eq!(truncated["usage"]["input_tokens"], 20);
        assert_eq!(truncated["usage"]["output_tokens"], 9);
        assert!(truncated["error"].as_str().is_some_and(|error| {
            error.contains("Model response incomplete") && error.contains("max_output_tokens")
        }));

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
            migrated_legacy_ollama_cloud_route: false,
        };
        let literal = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: Some("custom".to_string()),
            migrated_legacy_ollama_cloud_route: false,
        };
        let root = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: None,
            migrated_legacy_ollama_cloud_route: false,
        };
        let built_in = crate::config::ProviderIdentity {
            provider: crate::config::ApiProvider::Deepseek,
            key: "deepseek".to_string(),
            exact_id: Some("deepseek".to_string()),
            migrated_legacy_ollama_cloud_route: false,
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
        // The thinking ladder split these: `xhigh` is a tier the CLI can now
        // name, `ultracode` is still an alias and resolves to `ultra`.
        assert_eq!(
            normalize_cli_reasoning_effort("xhigh").unwrap().as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            normalize_cli_reasoning_effort("ultracode")
                .unwrap()
                .as_deref(),
            Some("ultra")
        );
        assert_eq!(normalize_cli_reasoning_effort("default").unwrap(), None);
        assert!(normalize_cli_reasoning_effort("expensive").is_err());
    }

    #[test]
    fn cli_prompt_paths_resolve_auto_before_k3_route_normalization() {
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        for (prompt, expected) in [
            ("lookup the public docs", "low"),
            ("debug this error", "max"),
            ("review this ordinary change", "high"),
        ] {
            assert_eq!(
                cli_reasoning_effort_value_for_prompt(
                    &config,
                    crate::config::KIMI_CODE_K3_MODEL,
                    crate::tui::app::ReasoningEffort::Auto,
                    prompt,
                )
                .as_deref(),
                Some(expected),
                "prompt selector must resolve Auto for `{prompt}`"
            );
        }

        assert_eq!(
            cli_reasoning_effort_value_for_prompt(
                &config,
                crate::config::KIMI_CODE_K3_MODEL,
                crate::tui::app::ReasoningEffort::Off,
                "debug must not override an explicit effort",
            )
            .as_deref(),
            Some("low"),
            "membership K3 still applies its exact-route always-thinking floor"
        );
    }

    #[test]
    fn cli_route_tracks_auto_reasoning_independently_from_auto_model() {
        use crate::tui::app::ReasoningEffort;

        let fixed_model_auto_reasoning = CliAutoRoute {
            provider: crate::config::ApiProvider::Deepseek,
            model: crate::config::DEFAULT_TEXT_MODEL.to_string(),
            reasoning_effort: Some(ReasoningEffort::Auto),
            auto_controls_reasoning: true,
            auto_model: false,
        };
        let auto_model_fixed_reasoning = CliAutoRoute {
            provider: crate::config::ApiProvider::OpenaiCodex,
            model: crate::config::DEFAULT_OPENAI_CODEX_MODEL.to_string(),
            reasoning_effort: Some(ReasoningEffort::High),
            auto_controls_reasoning: false,
            auto_model: true,
        };

        assert!(fixed_model_auto_reasoning.auto_controls_reasoning);
        assert!(!fixed_model_auto_reasoning.auto_model);
        assert!(!auto_model_fixed_reasoning.auto_controls_reasoning);
        assert!(auto_model_fixed_reasoning.auto_model);
    }

    #[test]
    fn saved_reasoning_preference_overrides_config_for_non_tui_runtimes() {
        let mut config = Config {
            reasoning_effort: Some("max".to_string()),
            reasoning_effort_inferred_from_legacy_alias: true,
            ..Default::default()
        };
        let settings = crate::settings::Settings {
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };

        apply_saved_reasoning_preference(&mut config, &settings);

        assert_eq!(config.reasoning_effort(), Some("low"));
        assert!(config.reasoning_effort_is_explicit());
    }

    /// `run_exec_agent` must hand the engine a concrete tier, never the literal
    /// `"auto"` sentinel, for a fixed-model Auto launch.
    #[test]
    fn fixed_model_exec_auto_resolves_to_a_concrete_tier_not_the_auto_sentinel() {
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let resolved = cli_reasoning_effort_value_for_prompt(
            &config,
            crate::config::ZAI_GLM_5_2_MODEL,
            crate::tui::app::ReasoningEffort::Auto,
            "debug this failing integration test",
        )
        .expect("Auto must resolve to a concrete tier");

        assert_ne!(
            resolved, "auto",
            "the literal auto sentinel must never reach a provider"
        );
        assert!(
            matches!(resolved.as_str(), "off" | "low" | "medium" | "high" | "max"),
            "unexpected resolved tier: {resolved}"
        );
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
        let envelope = r#"{"schema_version":1,"owner":"fleet-worker-1","authority":"read_only"}"#;
        let cli = parse_cli(&[
            "codewhale",
            "exec",
            "--allowed-tools",
            "File,Git",
            "--disallowed-tools",
            "Bash",
            "--max-turns",
            "7",
            "--max-tool-calls",
            "9",
            "--append-system-prompt",
            "extra rules",
            "--tool-authority-json",
            envelope,
            "do the thing",
        ]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(
            args.allowed_tools.as_deref(),
            Some(&["File".to_string(), "Git".to_string()][..])
        );
        assert_eq!(
            args.disallowed_tools.as_deref(),
            Some(&["Bash".to_string()][..])
        );
        assert_eq!(args.max_turns, Some(7));
        assert_eq!(args.max_tool_calls, Some(9));
        assert_eq!(args.append_system_prompt.as_deref(), Some("extra rules"));
        assert_eq!(args.tool_authority_json.as_deref(), Some(envelope));
        assert_eq!(args.prompt, vec!["do the thing"]);
    }

    #[test]
    fn exec_rejects_zero_max_tool_calls() {
        let err =
            Cli::try_parse_from(["codewhale", "exec", "--max-tool-calls", "0", "do the thing"])
                .expect_err("max-tool-calls must be >= 1");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn fleet_tool_authority_cannot_cross_an_exec_resume_boundary() {
        assert!(validate_exec_tool_authority_resume(None, true).is_ok());
        assert!(validate_exec_tool_authority_resume(Some("{}"), false).is_ok());
        let error = validate_exec_tool_authority_resume(Some("{}"), true)
            .expect_err("authority must remain bound to its fresh Fleet launch")
            .to_string();
        assert!(error.contains("cannot be combined with exec --resume"));
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

        assert_eq!(allowed_tools, vec!["bash".to_string()]);
    }

    #[test]
    fn exec_explicit_allowed_tools_override_shell_only_env() {
        let _env_lock = crate::test_support::lock_test_env();
        let _surface =
            crate::test_support::EnvVarGuard::set(CODEWHALE_TOOL_SURFACE_ENV, "shell-only");
        let explicit = vec![" File ".to_string(), "GIT".to_string()];

        let allowed_tools =
            resolve_exec_allowed_tools(Some(&explicit), exec_tool_surface_from_env())
                .expect("explicit allowlist should be preserved");

        assert_eq!(allowed_tools, vec!["file".to_string(), "git".to_string()]);
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

    /// R1: omitting `--max-turns` used to resolve to `u32::MAX`, i.e. a
    /// headless run with no bound at all. It now resolves to the finite
    /// default, and an explicit value still wins.
    #[test]
    fn exec_defaults_to_a_finite_headless_turn_cap() {
        let cli = parse_cli(&["codewhale", "exec", "--auto", "benchmark this"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.max_turns, None);
        let defaulted = exec_max_steps(args.max_turns);
        assert_eq!(
            defaulted,
            crate::core::engine::turn_budget::DEFAULT_EXEC_MAX_TURNS
        );
        assert!(defaulted < u32::MAX, "the headless default must be finite");
        assert_eq!(exec_max_steps(Some(7)), 7);
        assert_eq!(
            exec_max_steps(Some(u32::MAX)),
            crate::core::engine::turn_budget::MAX_MAX_MODEL_STEPS,
            "even the largest override stays finite"
        );
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
    fn exec_stream_turn_usage_event_serializes_reported_fields() {
        let event = ExecStreamEvent::TurnUsage {
            turn: 2,
            input_tokens: 1200,
            output_tokens: 180,
            reasoning_tokens: Some(90),
            prompt_cache_hit_tokens: Some(900),
            prompt_cache_miss_tokens: Some(300),
            prompt_cache_write_tokens: Some(0),
            reasoning_replay_tokens: Some(40),
            duration_ms: 1834,
        };

        let value = exec_stream_value(&event).expect("serializes");
        let json = serde_json::to_string(&value).expect("serializes");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["type"], "turn_usage");
        assert_eq!(parsed["schema"], "codewhale.exec-stream");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["turn"], 2);
        assert_eq!(parsed["input_tokens"], 1200);
        assert_eq!(parsed["output_tokens"], 180);
        assert_eq!(parsed["reasoning_tokens"], 90);
        assert_eq!(parsed["prompt_cache_hit_tokens"], 900);
        assert_eq!(parsed["prompt_cache_miss_tokens"], 300);
        assert_eq!(parsed["prompt_cache_write_tokens"], 0);
        assert_eq!(parsed["reasoning_replay_tokens"], 40);
        assert_eq!(parsed["duration_ms"], 1834);
    }

    #[test]
    fn exec_stream_turn_usage_event_omits_unreported_fields() {
        // Honest absence: optional token fields the provider did not report
        // are dropped from the object entirely — never emitted as null and
        // never backfilled with fabricated zeros.
        let event = ExecStreamEvent::TurnUsage {
            turn: 1,
            input_tokens: 11,
            output_tokens: 3,
            reasoning_tokens: None,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            prompt_cache_write_tokens: None,
            reasoning_replay_tokens: None,
            duration_ms: 250,
        };

        let value = exec_stream_value(&event).expect("serializes");
        let parsed = value;
        assert_eq!(parsed["type"], "turn_usage");
        assert_eq!(parsed["input_tokens"], 11);
        assert_eq!(parsed["output_tokens"], 3);
        assert_eq!(parsed["duration_ms"], 250);
        let object = parsed.as_object().expect("event object");
        for absent in [
            "reasoning_tokens",
            "prompt_cache_hit_tokens",
            "prompt_cache_miss_tokens",
            "prompt_cache_write_tokens",
            "reasoning_replay_tokens",
        ] {
            assert!(!object.contains_key(absent), "{absent} leaked: {parsed}");
        }
    }

    #[test]
    fn exec_stream_pre_existing_event_type_tags_are_unchanged() {
        // Contract guard for existing stream consumers (bench harness, fleet
        // ledger): the pre-turn_usage event vocabulary keeps its exact tags.
        let cases: Vec<(ExecStreamEvent, &str)> = vec![
            (
                ExecStreamEvent::Content {
                    content: "hi".to_string(),
                },
                "content",
            ),
            (
                ExecStreamEvent::ToolUse {
                    name: "read_file".to_string(),
                    id: "call_1".to_string(),
                    input: serde_json::json!({}),
                    started_at: "2026-08-03T00:00:00Z".to_string(),
                },
                "tool_use",
            ),
            (
                ExecStreamEvent::ToolResult {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    output: "ok".to_string(),
                    status: "success".to_string(),
                    started_at: "2026-08-03T00:00:00Z".to_string(),
                    completed_at: "2026-08-03T00:00:01Z".to_string(),
                    duration_ms: 1,
                    side_effect_status: "unknown".to_string(),
                    error_category: None,
                    truncated: None,
                    artifact: None,
                    result_metadata: None,
                },
                "tool_result",
            ),
            (
                ExecStreamEvent::SandboxDenied {
                    tool_id: "call_1".to_string(),
                    tool_name: "exec_shell".to_string(),
                    reason: "denied".to_string(),
                    outcome: "approval_required".to_string(),
                },
                "sandbox_denied",
            ),
            (
                ExecStreamEvent::WorkflowEvent {
                    run_id: "workflow_1".to_string(),
                    event: serde_json::json!({"type": "task_completed"}),
                },
                "workflow_event",
            ),
            (
                ExecStreamEvent::SessionCapture {
                    content: "x".to_string(),
                },
                "session_capture",
            ),
            (
                ExecStreamEvent::Error {
                    error: "boom".to_string(),
                },
                "error",
            ),
            (ExecStreamEvent::Done, "done"),
        ];

        for (event, expected_type) in cases {
            let value = exec_stream_value(&event).expect("serializes");
            assert_eq!(value["type"], expected_type, "event tag drifted");
            assert_eq!(value["schema"], "codewhale.exec-stream");
            assert_eq!(value["schema_version"], 1);
        }
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
                codewhale_max_output_tokens: Some(384_000),
                codewhale_max_output_tokens_source: Some("documented"),
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
                error: None,
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
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "run tests".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::thinking("checking context"),
                    ContentBlock::Text {
                        text: "working".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "exec_shell".to_string(),
                        input: serde_json::json!({"command": "cargo test"}),
                        caller: None,
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
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

        assert_eq!(startup_screen_mode(&cli, &config), ScreenMode::Fullscreen);
    }

    #[test]
    fn screen_mode_round_trips_through_its_own_vocabulary() {
        for mode in [ScreenMode::Fullscreen, ScreenMode::Inline] {
            assert_eq!(ScreenMode::parse(mode.as_str()), Some(mode));
        }
        // The `tui.alternate_screen` words the config file already documents.
        assert_eq!(ScreenMode::parse("auto"), Some(ScreenMode::Fullscreen));
        assert_eq!(ScreenMode::parse("always"), Some(ScreenMode::Fullscreen));
        assert_eq!(ScreenMode::parse("never"), Some(ScreenMode::Inline));
        assert_eq!(ScreenMode::parse("  INLINE "), Some(ScreenMode::Inline));
        assert_eq!(ScreenMode::parse("sideways"), None);
        assert!(ScreenMode::Fullscreen.uses_alt_screen());
        assert!(!ScreenMode::Inline.uses_alt_screen());
    }

    #[test]
    fn removed_no_alt_screen_flag_is_rejected() {
        // Negative test: the retired compatibility flag must not be silently
        // accepted and must not reach the alternate-screen decision at all.
        let error = Cli::try_parse_from(["codewhale", "--no-alt-screen"])
            .expect_err("--no-alt-screen must no longer parse");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "retired flag should fail as an unknown argument, not be absorbed"
        );
    }

    #[test]
    fn config_never_selects_the_inline_screen() {
        let cli = parse_cli(&["codewhale"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: Some("never".to_string()),
                mouse_capture: None,
                terminal_probe_timeout_ms: None,
                stream_chunk_timeout_secs: None,
                max_model_steps: None,
                turn_wall_clock_secs: None,
                stream_max_content_mb: None,
                stream_max_duration_secs: None,
                status_items: None,
                osc8_links: None,
                composer_arrows_scroll: None,
                notification_condition: None,
                header_items: None,
            }),
            ..Config::default()
        };

        assert_eq!(startup_screen_mode(&cli, &config), ScreenMode::Inline);
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
                max_model_steps: None,
                turn_wall_clock_secs: None,
                stream_max_content_mb: None,
                stream_max_duration_secs: None,
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
                max_model_steps: None,
                turn_wall_clock_secs: None,
                stream_max_content_mb: None,
                stream_max_duration_secs: None,
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
                max_model_steps: None,
                turn_wall_clock_secs: None,
                stream_max_content_mb: None,
                stream_max_duration_secs: None,
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
    fn missing_user_config_is_absent_not_an_error() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join("config.toml");

        assert_eq!(
            read_user_config_file(&missing).expect("missing config is a normal first-run state"),
            None
        );
    }

    #[test]
    fn existing_unreadable_user_config_remains_an_error() {
        let tmp = tempdir().expect("tempdir");
        let unreadable = tmp.path().join("config.toml");
        fs::create_dir(&unreadable).expect("create directory at config path");

        assert!(
            read_user_config_file(&unreadable).is_err(),
            "an existing path that cannot be read as a config must still warn"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_user_config_symlink_remains_an_error() {
        use std::os::unix::fs::symlink;

        let tmp = tempdir().expect("tempdir");
        let missing_target = tmp.path().join("missing-target.toml");
        let config_path = tmp.path().join("config.toml");
        symlink(&missing_target, &config_path).expect("create dangling config symlink");

        assert!(
            read_user_config_file(&config_path).is_err(),
            "a dangling symlink is an existing but unreadable config and must still warn"
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
    fn exec_no_project_config_skips_user_workspace_overlay() {
        // #4641: `codewhale --no-project-config exec` must skip the
        // workspace-specific `[workspace]`/`[projects]` overlay so a headless
        // launch sees a reproducible config surface. This documents the overlay
        // the `Commands::Exec` gate skips; the end-to-end wiring is proven by
        // `tests/verifiers_harness_contract.rs`.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("project");
        fs::create_dir_all(&workspace).expect("mkdir workspace");
        let raw = format!(
            "[workspace.'{}']\nallow_shell = true\n",
            workspace.display()
        );
        let doc: toml::Value = toml::from_str(&raw).expect("parse config");

        // Default (flag off): the overlay applies.
        let mut applied = Config::default();
        let no_project_config = false;
        if !no_project_config {
            merge_user_workspace_config_from_doc(&mut applied, &doc, &workspace);
        }
        assert_eq!(applied.allow_shell, Some(true));

        // `--no-project-config`: Exec skips the overlay, leaving config untouched.
        let mut skipped = Config::default();
        let no_project_config = true;
        if !no_project_config {
            merge_user_workspace_config_from_doc(&mut skipped, &doc, &workspace);
        }
        assert_eq!(skipped.allow_shell, None);
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
    fn test_scoped_npm_package_spec_without_cwd_is_not_a_path_warning() {
        let absolute_npx = if cfg!(windows) {
            r"C:\Program Files\nodejs\npx.cmd"
        } else {
            "/opt/homebrew/bin/npx"
        };
        for command in ["npx", "npx.cmd", absolute_npx] {
            let server = make_server(
                Some(command),
                &["-y", "@playwright/mcp@0.0.79", "--isolated"],
                None,
            );
            match doctor_check_mcp_server(&server) {
                McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("stdio")),
                other => panic!("Expected Ok for scoped npm package via {command}, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_scoped_npm_exception_does_not_hide_relative_paths() {
        for (command, argument) in [
            ("npx", "scripts/server.js"),
            ("npx", "@scope/package/extra"),
            ("npx", "@scope/package@"),
            ("npx", "@scope/package@@1.0.0"),
            ("npx", "@.scope/package"),
            ("npx.cmd", "@scope/_package"),
            ("node", "@scope/package@1.0.0"),
        ] {
            let server = make_server(Some(command), &[argument], None);
            assert!(
                matches!(
                    doctor_check_mcp_server(&server),
                    McpServerDoctorStatus::Warning(_)
                ),
                "Expected a relative-path warning for {command} {argument}"
            );
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
        let executable = std::env::current_exe().expect("current test executable");
        let executable = executable.to_string_lossy();
        let server = make_server(Some(&executable), &["serve", "--mcp"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("stdio server")),
            McpServerDoctorStatus::Warning(detail) => {
                panic!("Absolute path should not warn: {detail}")
            }
            McpServerDoctorStatus::Error(detail) => panic!("unexpected error: {detail}"),
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
            crate::doctor::DoctorProbeRequest::default(),
        ));
        assert!(doctor_should_probe_api(
            crate::config::ApiProvider::Ollama,
            "http://127.0.0.1:11434/v1",
            crate::doctor::DoctorProbeRequest {
                probe_local: true,
                ..crate::doctor::DoctorProbeRequest::default()
            },
        ));
    }

    #[test]
    fn ollama_cloud_probe_uses_hosted_opt_in_not_local_opt_in() {
        let cloud = codewhale_config::provider::OLLAMA_CLOUD_BASE_URL;
        assert!(!doctor_should_probe_api(
            crate::config::ApiProvider::OllamaCloud,
            cloud,
            crate::doctor::DoctorProbeRequest::default(),
        ));
        assert!(doctor_should_probe_api(
            crate::config::ApiProvider::OllamaCloud,
            cloud,
            crate::doctor::DoctorProbeRequest {
                probe_api: true,
                ..crate::doctor::DoctorProbeRequest::default()
            },
        ));
        assert!(!doctor_should_probe_api(
            crate::config::ApiProvider::OllamaCloud,
            cloud,
            crate::doctor::DoctorProbeRequest {
                probe_local: true,
                ..crate::doctor::DoctorProbeRequest::default()
            },
        ));
    }

    #[test]
    fn custom_loopback_probe_also_requires_explicit_opt_in() {
        assert!(!doctor_should_probe_api(
            crate::config::ApiProvider::Custom,
            "http://localhost:8000/v1",
            crate::doctor::DoctorProbeRequest::default(),
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
                role: Role::User,
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
            // #4479: checkpoint is no longer promoted to session file.
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_some(),
                "checkpoint stays in checkpoints/ for --continue"
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
                    role: Role::User,
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
            // #4479: checkpoint is no longer promoted to session file.
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_some(),
                "checkpoint stays in checkpoints/ for --continue"
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
                role: Role::User,
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

    #[test]
    fn continue_without_interactive_terminal_leaves_checkpoint_for_a_real_launch() {
        // `codewhale --continue </dev/null` (and `run --continue`) used to
        // promote and clear the in-flight checkpoint before the TTY check
        // failed, so the crash record was consumed by a launch that never
        // started and the next real `--continue` found nothing.
        let _guard = crate::test_support::lock_test_env();
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        with_home(tmp.path(), || {
            let manager = SessionManager::default_location().expect("manager");
            let messages = vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "continue me".to_string(),
                    cache_control: None,
                }],
            }];
            let session = create_saved_session(&messages, "test-model", &workspace, 0, None);
            let session_id = session.metadata.id.clone();
            manager.save_checkpoint(&session).expect("save checkpoint");

            let non_interactive = resolve_continue_session_id(&workspace, false);
            assert_eq!(
                non_interactive, None,
                "no saved session exists yet, so a non-TTY launch resolves nothing"
            );
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_some(),
                "a non-TTY --continue must not consume the checkpoint"
            );
            assert!(
                manager.load_session(&session_id).is_err(),
                "a non-TTY --continue must not promote the checkpoint to a session"
            );

            let interactive = resolve_continue_session_id(&workspace, true);
            assert_eq!(interactive.as_deref(), Some(session_id.as_str()));
            assert!(
                manager
                    .load_session_checkpoint(&session_id)
                    .expect("load checkpoint")
                    .is_none(),
                "an interactive --continue consumes the checkpoint"
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
                role: Role::User,
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
                role: Role::User,
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
                    role: Role::User,
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
                role: Role::Assistant,
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

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::EnvDeclared);
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

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::Unknown);
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

        assert_eq!(resolve_api_key_source(&config), ApiKeySource::Unknown);
        assert!(config.deepseek_api_key().is_err());
    }

    #[test]
    fn ollama_doctor_credential_source_is_route_aware() {
        let local = Config {
            provider: Some("ollama".to_string()),
            ..Config::default()
        };
        assert_eq!(resolve_api_key_source(&local), ApiKeySource::LocalRuntime);
        assert_eq!(
            resolve_credential_diagnostic(&local).availability,
            CredentialAvailability::NotRequired
        );

        let ollama_config = |base_url: &str| Config {
            provider: Some("ollama".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                ollama: crate::config::ProviderConfig {
                    base_url: Some(base_url.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let cloud = ollama_config(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL);
        assert_eq!(
            cloud.api_provider(),
            crate::config::ApiProvider::OllamaCloud
        );
        assert_eq!(
            resolve_api_key_source(&cloud),
            ApiKeySource::SecretStoreUnprobed
        );
        assert_eq!(
            resolve_credential_diagnostic(&cloud).availability,
            CredentialAvailability::NotProbed
        );
        assert_eq!(doctor_auth_scheme(&cloud), "bearer");
        let report = doctor_route_report(&cloud);
        assert_eq!(report["provider"], "ollama-cloud");
        assert_eq!(report["provider_config_table"], "ollama_cloud");

        let custom_remote = ollama_config("https://ollama-gateway.example.test/v1");
        assert_eq!(
            resolve_api_key_source(&custom_remote),
            ApiKeySource::Unknown
        );
        assert_eq!(
            resolve_credential_diagnostic(&custom_remote).availability,
            CredentialAvailability::Unknown
        );
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
        assert_eq!(source, ApiKeySource::ConfigDeclared);
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

        assert_eq!(source, ApiKeySource::SecretStoreUnprobed);
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

        assert_eq!(source, ApiKeySource::ExternalAuthDeclared);
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

        assert_eq!(source, ApiKeySource::ExternalAuthDeclared);
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

        assert_eq!(source, ApiKeySource::SecretStoreUnprobed);
    }

    #[test]
    fn provider_status_helpers_use_provider_metadata() {
        assert_eq!(
            provider_config_table_key(crate::config::ApiProvider::Anthropic),
            "anthropic"
        );
        assert_eq!(
            provider_config_table_key(crate::config::ApiProvider::SiliconflowCn),
            "siliconflow_cn"
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
#[path = "tests/pr_prompt.rs"]
mod pr_prompt_tests;

#[cfg(test)]
#[path = "tests/telemetry_surface.rs"]
mod telemetry_surface_tests;

#[cfg(test)]
#[path = "tests/telemetry_counters.rs"]
mod telemetry_counter_tests;
