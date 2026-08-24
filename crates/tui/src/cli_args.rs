//! CLI argument surface: the clap `Cli`/`Commands` tree, per-subcommand
//! arg structs, feature toggles, and the pure helpers that interpret CLI
//! input (exec tool-surface resolution, model/provider override parsing,
//! prompt joining, resume-session resolution, telemetry session markers,
//! terminating-signal handling).
//!
//! Extracted verbatim from `lib.rs` (#5586). Items were crate-private in
//! the root and are `pub(crate)` here purely so the root's glob re-export
//! keeps every `crate::<name>` path and in-crate reference unchanged;
//! nothing is exported beyond the crate.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use crate::SessionManager;
use crate::config::Config;
use crate::remote_setup;
use crate::session_manager::{self};
use crate::tui;
use crate::{exec_stream_session_ref, latest_session_id_for_workspace};

#[derive(Parser, Debug)]
#[command(
    name = "codewhale-tui",
    bin_name = "codewhale-tui",
    author,
    version = env!("CODEWHALE_BUILD_VERSION"),
    about = "Codewhale terminal coding agent",
    long_about = "Terminal-native TUI and CLI for open-source and open-weight coding models.\n\nRun 'codewhale' to start.\n\nProvider routes include DeepSeek, Arcee, Hugging Face, OpenRouter, Xiaomi MiMo, local vLLM/SGLang/Ollama, and more."
)]
pub(crate) struct Cli {
    /// Subcommand to run
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[command(flatten)]
    pub(crate) feature_toggles: FeatureToggles,

    /// Initial prompt to submit in the interactive TUI. Use `exec` for non-interactive runs.
    #[arg(short, long, value_name = "PROMPT", num_args = 1..)]
    pub(crate) prompt: Vec<String>,

    /// Legacy compatibility alias for Act + Full Access.
    #[arg(long, hide = true)]
    pub(crate) yolo: bool,

    /// Maximum number of concurrent sub-agents (1-128; default 64)
    #[arg(long)]
    pub(crate) max_subagents: Option<usize>,

    /// Path to config file
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub(crate) verbose: bool,

    /// Config profile name
    #[arg(long)]
    pub(crate) profile: Option<String>,

    /// Workspace directory for file operations
    #[arg(short, long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Resume a previous session by ID or prefix
    #[arg(short, long)]
    pub(crate) resume: Option<String>,

    /// Continue the most recent session in this workspace
    #[arg(short = 'c', long = "continue")]
    pub(crate) continue_session: bool,

    /// Enable TUI mouse capture for internal scrolling, transcript selection,
    /// and scrollbar dragging
    /// (default off on Windows)
    #[arg(long = "mouse-capture", conflicts_with = "no_mouse_capture")]
    pub(crate) mouse_capture: bool,

    /// Disable TUI mouse capture so terminal-native text selection works
    #[arg(long = "no-mouse-capture", conflicts_with = "mouse_capture")]
    pub(crate) no_mouse_capture: bool,

    /// Skip onboarding screens
    #[arg(long)]
    pub(crate) skip_onboarding: bool,

    /// Start account-owned web remote control for this interactive session.
    #[arg(long, hide = true)]
    pub(crate) remote_control: bool,

    /// Start a fresh session, ignoring any crash-recovery checkpoint
    #[arg(long = "fresh")]
    pub(crate) fresh: bool,

    /// Skip loading project-level config from $WORKSPACE/.codewhale/config.toml
    #[arg(long = "no-project-config")]
    pub(crate) no_project_config: bool,
}

#[derive(Subcommand, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Commands {
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
pub(crate) struct ExecArgs {
    /// Override model for this run
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Override the provider for this run (e.g. `deepseek`, `openrouter`).
    /// Non-secret identifier only — credentials still resolve from the
    /// environment/config. Fleet uses this to launch a worker on its
    /// profile-pinned provider even when the parent session is on another
    /// one (#4093).
    #[arg(long)]
    pub(crate) provider: Option<String>,
    /// Override reasoning/thinking effort for this run.
    /// Accepted values: auto, off, low, medium, high, max.
    #[arg(long = "reasoning-effort", value_name = "EFFORT")]
    pub(crate) reasoning_effort: Option<String>,
    /// Enable agent-with-tools mode with automatic tool approvals. This does
    /// not authorize sandbox elevation.
    #[arg(long, default_value_t = false)]
    pub(crate) auto: bool,
    /// Sandbox policy for this exec run; independent from --auto.
    #[arg(long, value_name = "POLICY")]
    pub(crate) sandbox: Option<String>,
    /// Explicitly allow a denied tool to retry with danger-full-access.
    #[arg(long, default_value_t = false)]
    pub(crate) allow_sandbox_elevation: bool,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false, conflicts_with = "output_format")]
    pub(crate) json: bool,
    /// Resume a previous session by ID or prefix
    #[arg(long, value_name = "SESSION_ID", conflicts_with_all = ["session_id", "continue_session"])]
    pub(crate) resume: Option<String>,
    /// Resume a previous session by ID or prefix
    #[arg(long = "session-id", value_name = "SESSION_ID", conflicts_with_all = ["resume", "continue_session"])]
    pub(crate) session_id: Option<String>,
    /// Continue the most recent session for this workspace
    #[arg(long = "continue", default_value_t = false, conflicts_with_all = ["resume", "session_id"])]
    pub(crate) continue_session: bool,
    /// Output format for exec mode
    #[arg(long, value_enum, default_value_t = ExecOutputFormat::Text)]
    pub(crate) output_format: ExecOutputFormat,
    /// Comma-separated list of canonical tools to allow (all others denied).
    /// Names are case-insensitive: Bash, File, Git, Run, etc.
    #[arg(long, value_delimiter = ',')]
    pub(crate) allowed_tools: Option<Vec<String>>,
    /// Comma-separated list of tools to deny (deny wins over allow).
    #[arg(long, value_delimiter = ',')]
    pub(crate) disallowed_tools: Option<Vec<String>>,
    /// Maximum number of model steps before the run ends. Omitted means unlimited.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) max_turns: Option<u32>,
    /// Maximum number of tool calls admitted in one model turn. Omitted means unlimited.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) max_tool_calls: Option<u32>,
    /// Shut down when the parent closes this process's stdin. Fleet workers
    /// pass this automatically: a dead manager must not leave detached workers
    /// spending forever (R7).
    #[arg(long, default_value_t = false)]
    pub(crate) parent_death_watch: bool,
    /// Extra text appended to the system prompt for this run.
    #[arg(long)]
    pub(crate) append_system_prompt: Option<String>,
    /// Internal Fleet worker authority envelope. Non-secret, versioned JSON.
    #[arg(long, value_name = "JSON", hide = true)]
    pub(crate) tool_authority_json: Option<String>,
    /// Prompt to send to the model
    #[arg(
        value_name = "PROMPT",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) prompt: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct WorkflowToolArgs {
    /// Authority provenance stamped by the public `workflow run` command.
    #[arg(long, value_name = "SOURCE")]
    pub(crate) approval_source: String,
    /// Exact Workflow tool input serialized as one JSON object.
    #[arg(long, value_name = "JSON")]
    pub(crate) input_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExecOutputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct TuiAuthArgs {
    #[command(subcommand)]
    pub(crate) command: TuiAuthCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum TuiAuthCommand {
    /// Sign in to xAI/Grok with an SSH-friendly device code.
    #[command(name = "xai-device")]
    XaiDevice,
}

pub(crate) const CODEWHALE_TOOL_SURFACE_ENV: &str = "CODEWHALE_TOOL_SURFACE";
pub(crate) const SHELL_ONLY_EXEC_TOOLS: &[&str] = &["bash"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecToolSurface {
    ShellOnly,
}

pub(crate) fn exec_tool_surface_from_env() -> Option<ExecToolSurface> {
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

pub(crate) fn parse_exec_tool_surface(value: &str) -> Option<ExecToolSurface> {
    match value.trim().to_ascii_lowercase().as_str() {
        "shell-only" | "shell_only" | "shell" => Some(ExecToolSurface::ShellOnly),
        "full" | "native-tools" | "native_tools" | "" => None,
        _ => None,
    }
}

pub(crate) fn should_warn_unknown_exec_tool_surface(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !matches!(
        normalized.as_str(),
        "" | "shell-only" | "shell_only" | "shell" | "full" | "native-tools" | "native_tools"
    )
}

pub(crate) fn normalize_exec_tool_names(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .map(|name| name.to_ascii_lowercase().trim().to_string())
        .collect()
}

pub(crate) fn shell_only_exec_allowed_tools() -> Vec<String> {
    SHELL_ONLY_EXEC_TOOLS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

pub(crate) fn resolve_exec_allowed_tools(
    cli_allowed_tools: Option<&[String]>,
    env_tool_surface: Option<ExecToolSurface>,
) -> Option<Vec<String>> {
    if let Some(tools) = cli_allowed_tools {
        return Some(normalize_exec_tool_names(tools));
    }

    env_tool_surface.map(|ExecToolSurface::ShellOnly| shell_only_exec_allowed_tools())
}

#[derive(Args, Debug, Clone)]
pub(crate) struct FleetArgs {
    #[command(subcommand)]
    pub(crate) command: FleetCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum FleetCommand {
    /// Initialize the local fleet ledger for this workspace
    Init,
    /// Create a run from a task spec and start the foreground manager loop
    Run(FleetRunArgs),
    /// List durable Fleet runs from this workspace's ledger
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
pub(crate) struct FleetRunArgs {
    /// JSON or TOML task spec to enqueue
    #[arg(value_name = "TASK_SPEC")]
    pub(crate) task_spec: PathBuf,
    /// Maximum local workers to lease concurrently
    #[arg(long, default_value_t = 4)]
    pub(crate) max_workers: usize,
    /// Seconds without heartbeat before a running task is counted stale
    #[arg(long, default_value_t = 300)]
    pub(crate) stale_after_seconds: u64,
    /// Schedule once and return instead of staying in the manager loop
    #[arg(long, hide = true, default_value_t = false)]
    pub(crate) once: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct FleetAlertDryRunArgs {
    /// Alert event class to render
    #[arg(long, value_enum)]
    pub(crate) event: FleetAlertEventArg,
    /// Fleet run id
    #[arg(long)]
    pub(crate) run_id: String,
    /// Worker id, when the event belongs to one worker
    #[arg(long)]
    pub(crate) worker_id: Option<String>,
    /// Task id, when the event belongs to one task
    #[arg(long)]
    pub(crate) task_id: Option<String>,
    /// Short human-readable reason for the alert
    #[arg(long, default_value = "manual fleet alert dry-run")]
    pub(crate) reason: String,
    /// Status label to include in the payload
    #[arg(long)]
    pub(crate) status: Option<String>,
    /// Adapter payload shape to render
    #[arg(long, value_enum, default_value_t = FleetAlertAdapterArg::Slack)]
    pub(crate) adapter: FleetAlertAdapterArg,
    /// Environment variable containing the Slack webhook URL
    #[arg(long, default_value = "CODEWHALE_FLEET_SLACK_WEBHOOK")]
    pub(crate) slack_webhook_env: String,
    /// Environment variable containing the generic webhook URL
    #[arg(long, default_value = "CODEWHALE_FLEET_WEBHOOK_URL")]
    pub(crate) webhook_url_env: String,
    /// Optional environment variable containing the generic webhook secret
    #[arg(long)]
    pub(crate) webhook_secret_env: Option<String>,
    /// Environment variable containing the PagerDuty routing key
    #[arg(long, default_value = "CODEWHALE_FLEET_PAGERDUTY_ROUTING_KEY")]
    pub(crate) pagerduty_routing_key_env: String,
    /// PagerDuty severity to render
    #[arg(long, default_value = "error")]
    pub(crate) pagerduty_severity: String,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub(crate) enum FleetAlertEventArg {
    Stale,
    RestartExhausted,
    NeedsHuman,
    BudgetExceeded,
    VerifierFailed,
    RunCompleted,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub(crate) enum FleetAlertAdapterArg {
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
pub(crate) fn spawn_signal_cleanup_task() {
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
pub(crate) static TELEMETRY_SESSION_START: std::sync::OnceLock<std::time::Instant> =
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
pub(crate) fn telemetry_session_end() -> codewhale_telemetry::Event {
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
pub(crate) fn record_signal_session_end() {
    codewhale_telemetry::set_exit_class(codewhale_telemetry::ExitClass::Signal);
    codewhale_telemetry::record_blocking(telemetry_session_end());
}

/// Terminating-signal streams, registered up front and awaited later.
///
/// Splitting registration from the await is the point: the OS disposition
/// changes when `register` returns, not when the waiting task is first polled.
#[cfg(unix)]
pub(crate) struct TerminatingSignals {
    pub(crate) sigint: Option<tokio::signal::unix::Signal>,
    pub(crate) sigterm: Option<tokio::signal::unix::Signal>,
    pub(crate) sighup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl TerminatingSignals {
    /// Install the handlers. Failing to install any individual stream is
    /// non-fatal: we still want the others to work.
    pub(crate) fn register() -> Self {
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
pub(crate) struct TerminatingSignals {
    pub(crate) ctrl_c: Option<tokio::signal::windows::CtrlC>,
}

#[cfg(not(unix))]
impl TerminatingSignals {
    pub(crate) fn register() -> Self {
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

pub(crate) fn join_prompt_parts(parts: &[String]) -> String {
    parts.join(" ")
}

pub(crate) fn resolve_exec_model(config: &Config, explicit_model: Option<&str>) -> String {
    explicit_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .or_else(exec_model_env_override)
        .unwrap_or_else(|| config.default_model())
}

pub(crate) fn apply_exec_provider_override(config: &mut Config, provider_arg: &str) -> Result<()> {
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

pub(crate) fn exec_model_env_override() -> Option<String> {
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

pub(crate) fn top_level_prompt_initial_input(parts: &[String]) -> Option<tui::InitialInput> {
    (!parts.is_empty()).then(|| tui::InitialInput::Submit(join_prompt_parts(parts)))
}

pub(crate) fn resolve_exec_resume_session_id(
    args: &ExecArgs,
    workspace: &Path,
) -> Result<Option<String>> {
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

pub(crate) fn load_exec_resume_session(session_id: &str) -> Result<session_manager::SavedSession> {
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
pub(crate) fn resolve_exec_resume_route(
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
pub(crate) struct SetupArgs {
    /// Initialize MCP configuration at the configured path
    #[arg(long, default_value_t = false)]
    pub(crate) mcp: bool,
    /// Initialize skills directory and an example skill
    #[arg(long, default_value_t = false)]
    pub(crate) skills: bool,
    /// Initialize tools directory with a self-describing example script
    #[arg(long, default_value_t = false)]
    pub(crate) tools: bool,
    /// Initialize plugins directory with a self-describing example
    #[arg(long, default_value_t = false)]
    pub(crate) plugins: bool,
    /// Initialize MCP config, skills, tools, and plugins
    #[arg(long, default_value_t = false)]
    pub(crate) all: bool,
    /// Create a local workspace skills directory (./skills)
    #[arg(long, default_value_t = false)]
    pub(crate) local: bool,
    /// Overwrite existing template files
    #[arg(long, default_value_t = false)]
    pub(crate) force: bool,
    /// Print a compact, read-only status report (no network calls)
    #[arg(long, default_value_t = false, conflicts_with_all = ["mcp", "skills", "tools", "plugins", "all", "local", "clean"])]
    pub(crate) status: bool,
    /// Remove regenerable session checkpoints (latest + offline_queue)
    #[arg(long, default_value_t = false, conflicts_with_all = ["mcp", "skills", "tools", "plugins", "all", "local", "status"])]
    pub(crate) clean: bool,
}

#[derive(Args, Debug, Clone, Default)]
pub(crate) struct DoctorArgs {
    /// Emit machine-readable structural JSON output (always offline)
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
    /// Emit only the diagnostic context source map as JSON
    #[arg(long, default_value_t = false, conflicts_with = "json")]
    pub(crate) context_json: bool,
    /// Opt in to probing a local provider endpoint (may start a local service)
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) probe_local: bool,
    /// Opt in to probing the configured hosted provider API
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) probe_api: bool,
    /// Opt in to contacting the release service for an update check
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) check_updates: bool,
    /// Opt in to starting enabled MCP servers and checking process/protocol reachability
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) probe_mcp: bool,
    /// Opt in to a credential-free transport probe of the selected search provider
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) probe_search: bool,
    /// Plan and apply automatic repairs with consent (#5552)
    #[arg(
        long,
        default_value_t = false,
        conflicts_with_all = ["json", "context_json"]
    )]
    pub(crate) fix: bool,
    /// Apply the planned repairs without prompting (requires --fix)
    #[arg(long, default_value_t = false, requires = "fix")]
    pub(crate) yes: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct SessionDiagnosticsArgs {
    /// JSONL session log to inspect
    #[arg(value_name = "JSONL")]
    pub(crate) path: PathBuf,
    /// Emit machine-readable JSON with redacted source handles
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ScorecardArgs {
    /// JSON file with the recorded turns to score: an array of
    /// `{ "turn_id", "provider", "model", "billing_surface", "usage": {…} }`.
    /// `turn_end` hooks emit this route provenance plus `created_at`; persisted
    /// runtime exports may instead use `id`, `effective_provider`,
    /// `effective_model`, and `effective_billing_surface`.
    /// Shell-only hook rows marked `model_backed: false` are excluded. Legacy
    /// rows without provider remain readable but their cost is unavailable.
    #[arg(long, value_name = "FILE")]
    pub(crate) input: PathBuf,
    /// Optional baseline scorecard-metrics JSON to compare against. When set,
    /// the command exits non-zero if any metric regresses past the threshold.
    #[arg(long, value_name = "FILE")]
    pub(crate) baseline: Option<PathBuf>,
    /// Regression threshold, in percent increase over the baseline.
    #[arg(long, default_value_t = 5.0)]
    pub(crate) threshold: f64,
    /// Emit machine-readable JSON instead of the human summary.
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct EvalArgs {
    /// Intentionally fail a specific step (list, read, search, edit, patch, shell)
    #[arg(long, value_name = "STEP")]
    pub(crate) fail_step: Option<String>,
    /// Shell command to run during the exec step
    #[arg(long, default_value = "printf eval-harness")]
    pub(crate) shell_command: String,
    /// Token that must appear in shell output for validation
    #[arg(long, default_value = "eval-harness")]
    pub(crate) shell_expect_token: String,
    /// Maximum characters stored per step output summary
    #[arg(long, default_value_t = 240)]
    pub(crate) max_output_chars: usize,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
    /// Append one JSONL fixture line per step to `<DIR>/<scenario>.jsonl`.
    /// Mock LLM tests can later replay these fixtures.
    #[arg(long, value_name = "DIR")]
    pub(crate) record: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default)]
pub(crate) struct ModelsArgs {
    /// Print models as pretty JSON
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct SpeechArgs {
    /// Text to synthesize. This is sent as the assistant message content.
    #[arg(value_name = "TEXT")]
    pub(crate) text: String,

    /// Output audio path. Defaults to `speech.<format>` in `--output-dir`,
    /// `[speech].output_dir`, or the current directory.
    #[arg(short, long, value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,

    /// Directory for the default `speech.<format>` output file when `-o`/`--output` is omitted.
    #[arg(long = "output-dir", value_name = "DIR")]
    pub(crate) output_dir: Option<PathBuf>,

    /// TTS model. Defaults to built-in voices, or is inferred from --voice-prompt/--clone-voice.
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Built-in voice ID, or a data:audio/...;base64,... URI for voice clone.
    #[arg(long)]
    pub(crate) voice: Option<String>,

    /// Natural language style instruction; not spoken verbatim.
    #[arg(long)]
    pub(crate) instruction: Option<String>,

    /// Voice design prompt. Implies mimo-v2.5-tts-voicedesign when --model is omitted.
    #[arg(long = "voice-prompt")]
    pub(crate) voice_prompt: Option<String>,

    /// MP3/WAV sample used for voice cloning. Implies mimo-v2.5-tts-voiceclone when --model is omitted.
    #[arg(long = "clone-voice", value_name = "FILE")]
    pub(crate) clone_voice: Option<PathBuf>,

    /// Output audio format requested from the API
    #[arg(long, default_value = "wav")]
    pub(crate) format: String,

    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(Args, Debug, Default, Clone)]
pub(crate) struct FeatureToggles {
    /// Enable a feature (repeatable). Equivalent to `features.<name>=true`.
    #[arg(long = "enable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    pub(crate) enable: Vec<String>,

    /// Disable a feature (repeatable). Equivalent to `features.<name>=false`.
    #[arg(long = "disable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    pub(crate) disable: Vec<String>,
}

impl FeatureToggles {
    pub(crate) fn apply(&self, config: &mut Config) -> Result<()> {
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
pub(crate) struct ReviewArgs {
    /// Review staged changes instead of the working tree
    #[arg(long, conflicts_with = "base")]
    pub(crate) staged: bool,
    /// Base ref to diff against (e.g. origin/main)
    #[arg(long)]
    pub(crate) base: Option<String>,
    /// Limit diff to a specific path
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Override model for this review
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Maximum diff characters to include
    #[arg(long, default_value_t = 200_000)]
    pub(crate) max_chars: usize,
    /// Write a durable pre-push review receipt after a successful review
    #[arg(long, default_value_t = false)]
    pub(crate) write_receipt: bool,
    /// Validate the current diff against a durable review receipt without calling a model
    #[arg(long, default_value_t = false)]
    pub(crate) check_receipt: bool,
    /// Override where the review receipt is written or read
    #[arg(long)]
    pub(crate) receipt_path: Option<PathBuf>,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ApplyArgs {
    /// Patch file to apply (defaults to stdin)
    #[arg(value_name = "PATCH_FILE")]
    pub(crate) patch_file: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ServeArgs {
    /// Start MCP server over stdio
    #[arg(long)]
    pub(crate) mcp: bool,
    /// Start runtime HTTP/SSE API server
    #[arg(long)]
    pub(crate) http: bool,
    /// Start runtime HTTP/SSE API server with the built-in mobile control page
    #[arg(long)]
    pub(crate) mobile: bool,
    /// Start the embedded loopback-only browser client and open it
    #[arg(long)]
    pub(crate) web: bool,
    /// Show a QR code for the mobile URL in the terminal (requires --mobile)
    #[arg(long, requires = "mobile")]
    pub(crate) qr: bool,
    /// Start ACP server over stdio for editor clients such as Zed
    #[arg(long)]
    pub(crate) acp: bool,
    /// Bind host for HTTP server (default localhost; --mobile defaults to 0.0.0.0)
    #[arg(long)]
    pub(crate) host: Option<String>,
    /// Bind port for HTTP server
    #[arg(long, default_value_t = 7878)]
    pub(crate) port: u16,
    /// Background task worker count (1-8)
    #[arg(long, default_value_t = 2)]
    pub(crate) workers: usize,
    /// Additional CORS origin to allow (repeatable). Stacks on top of the
    /// built-in defaults (localhost:3000, localhost:1420, tauri://localhost).
    /// Also reads `CODEWHALE_CORS_ORIGINS` (comma-separated), then
    /// `DEEPSEEK_CORS_ORIGINS` as an alias, and `[runtime_api] cors_origins`
    /// from `config.toml`. Whalescale#255.
    #[arg(long = "cors-origin", value_name = "URL")]
    pub(crate) cors_origin: Vec<String>,
    /// Require this bearer token for `/v1/*` runtime API routes. Also reads
    /// `CODEWHALE_RUNTIME_TOKEN` when omitted, then `DEEPSEEK_RUNTIME_TOKEN`
    /// as an alias.
    #[arg(long = "auth-token", value_name = "TOKEN")]
    pub(crate) auth_token: Option<String>,
    /// Disable runtime API auth when no token is configured. Only use on a trusted loopback.
    #[arg(long = "insecure")]
    pub(crate) insecure_no_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServeBindHost {
    pub(crate) host: String,
    pub(crate) mobile_rebound_to_lan: bool,
}

pub(crate) fn resolve_serve_bind_host(mobile: bool, host: Option<String>) -> ServeBindHost {
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

pub(crate) fn validate_serve_mode_selection(
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
pub(crate) enum McpCommand {
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
pub(crate) struct FeaturesCli {
    #[command(subcommand)]
    pub(crate) command: FeaturesSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum FeaturesSubcommand {
    /// List known feature flags and their state
    List,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct SandboxArgs {
    #[command(subcommand)]
    pub(crate) command: SandboxCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum SandboxCommand {
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
