//! CLI entry point for the `DeepSeek` client.

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use dotenvy::dotenv;
use tempfile::NamedTempFile;
use wait_timeout::ChildExt;

mod acp_server;
mod audit;
mod auto_reasoning;
mod automation_manager;
mod child_env;
mod client;
mod command_safety;
mod commands;
mod compaction;
mod composer_history;
mod composer_stash;
mod config;
mod config_ui;
mod core;
mod cost_status;
mod cycle_manager;
mod deepseek_theme;
mod error_taxonomy;
mod eval;
mod execpolicy;
mod features;
mod handoff;
mod hooks;
mod llm_client;
mod localization;
mod logging;
mod lsp;
mod mcp;
mod mcp_server;
mod memory;
mod models;
mod network_policy;
mod palette;
mod pricing;
mod project_context;
mod project_doc;
mod prompts;
pub mod repl;
mod retry_status;
pub mod rlm;
mod runtime_api;
mod runtime_threads;
mod sandbox;
mod schema_migration;
mod seam_manager;
mod session_manager;
mod settings;
mod skill_state;
mod skills;
mod snapshot;
mod task_manager;
#[cfg(test)]
mod test_support;
mod tools;
mod tui;
mod utils;
mod working_set;
mod workspace_trust;

use crate::config::{Config, DEFAULT_TEXT_MODEL, MAX_SUBAGENTS};
use crate::eval::{EvalHarness, EvalHarnessConfig, ScenarioStepKind};
use crate::features::{Feature, render_feature_table};
use crate::llm_client::LlmClient;
use crate::mcp::{McpConfig, McpPool, McpServerConfig};
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

#[derive(Parser, Debug)]
#[command(
    name = "deepseek",
    author,
    version = env!("DEEPSEEK_BUILD_VERSION"),
    about = "DeepSeek TUI/CLI for DeepSeek models",
    long_about = "Terminal-native TUI and CLI for DeepSeek models.\n\nRun 'deepseek' to start.\n\nNot affiliated with DeepSeek Inc."
)]
struct Cli {
    /// Subcommand to run
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    feature_toggles: FeatureToggles,

    /// Send a one-shot prompt (non-interactive)
    #[arg(short, long, value_name = "PROMPT", num_args = 1..)]
    prompt: Vec<String>,

    /// YOLO mode: enable agent tools + shell execution
    #[arg(long)]
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

    /// Enable TUI mouse capture for internal scrolling and transcript selection
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

    /// Skip loading project-level config from $WORKSPACE/.deepseek/config.toml
    #[arg(long = "no-project-config")]
    no_project_config: bool,
}

#[derive(Subcommand, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Run system diagnostics and check configuration
    Doctor(DoctorArgs),
    /// Bootstrap MCP config and/or skills directories
    Setup(SetupArgs),
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
    /// Save a DeepSeek API key to the shared user config
    Login {
        /// API key to store (otherwise read from stdin)
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Remove the saved API key
    Logout,
    /// List available models from the configured API endpoint
    Models(ModelsArgs),
    /// Run a non-interactive prompt
    Exec(ExecArgs),
    /// Run a code review over a git diff
    Review(ReviewArgs),
    /// Open the TUI pre-seeded with a GitHub PR's title, body, and diff (#451)
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
struct ExecArgs {
    /// Prompt to send to the model
    #[arg(
        value_name = "PROMPT",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    prompt: Vec<String>,
    /// Override model for this run
    #[arg(long)]
    model: Option<String>,
    /// Enable agentic mode with tool access and auto-approvals
    #[arg(long, default_value_t = false)]
    auto: bool,
    /// Emit machine-readable JSON output
    #[arg(long, default_value_t = false)]
    json: bool,
}

fn join_prompt_parts(parts: &[String]) -> String {
    parts.join(" ")
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
    /// Start ACP server over stdio for editor clients such as Zed
    #[arg(long)]
    acp: bool,
    /// Bind host for HTTP server (default localhost)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Bind port for HTTP server
    #[arg(long, default_value_t = 7878)]
    port: u16,
    /// Background task worker count (1-8)
    #[arg(long, default_value_t = 2)]
    workers: usize,
    /// Additional CORS origin to allow (repeatable). Stacks on top of the
    /// built-in defaults (localhost:3000, localhost:1420, tauri://localhost).
    /// Also reads `DEEPSEEK_CORS_ORIGINS` (comma-separated) and
    /// `[runtime_api] cors_origins` from `config.toml`. Whalescale#255.
    #[arg(long = "cors-origin", value_name = "URL")]
    cors_origin: Vec<String>,
    /// Require this bearer token for `/v1/*` runtime API routes. Also reads
    /// `DEEPSEEK_RUNTIME_TOKEN` when omitted.
    #[arg(long = "auth-token", value_name = "TOKEN")]
    auth_token: Option<String>,
    /// Disable runtime API auth when no token is configured. Only use on a trusted loopback.
    #[arg(long = "insecure")]
    insecure_no_auth: bool,
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
        /// Arguments for command-based servers
        #[arg(long = "arg")]
        args: Vec<String>,
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
    /// Register this DeepSeek binary as a local MCP stdio server.
    ///
    /// This adds a config entry that runs `deepseek serve --mcp` (stdio protocol).
    /// For the HTTP/SSE runtime API, use `deepseek serve --http` directly instead.
    #[command(
        name = "add-self",
        long_about = "Register this DeepSeek binary as a local MCP stdio server.\n\nAdds a config entry to ~/.deepseek/mcp.json that launches `deepseek serve --mcp`\nvia the stdio transport. Other DeepSeek sessions (or any MCP client) can then\ndiscover and call tools exposed by this server.\n\nUse `deepseek serve --http` instead if you need the HTTP/SSE runtime API."
    )]
    AddSelf {
        /// Server name in mcp.json (default: "deepseek")
        #[arg(long, default_value = "deepseek")]
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

#[tokio::main]
async fn main() -> Result<()> {
    configure_windows_console_utf8();

    // Set up process panic hook before anything else — writes crash dumps
    // to ~/.deepseek/crashes/ even if the panic happens before tokio is up,
    // and restores the terminal so a panicked TUI doesn't leave the user's
    // shell stuck in alt-screen mode.
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Restore the terminal first so the panic message itself, plus the
        // user's shell after exit, are visible. Best-effort — we may not be
        // in raw / alt-screen mode if the panic happens pre-TUI.
        use crossterm::event::{
            DisableBracketedPaste, DisableMouseCapture, PopKeyboardEnhancementFlags,
        };
        use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
        let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        // Best-effort: turn off bracketed paste + mouse capture so the user's
        // parent shell doesn't get stuck wrapping pastes in `\e[200~…\e[201~`
        // or printing `\e[<…M` on every click after a TUI panic.
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);

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

    dotenv().ok();
    let cli = Cli::parse();
    logging::set_verbose(cli.verbose || logging::env_requests_verbose_logging());

    // Handle subcommands first
    if let Some(command) = cli.command.clone() {
        return match command {
            Commands::Doctor(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                if args.json {
                    run_doctor_json(&config, &workspace, cli.config.as_deref())
                } else {
                    run_doctor(&config, &workspace, cli.config.as_deref()).await;
                    Ok(())
                }
            }
            Commands::Setup(args) => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                run_setup(&config, &workspace, args)
            }
            Commands::Completions { shell } => {
                generate_completions(shell);
                Ok(())
            }
            Commands::Sessions { limit, search } => list_sessions(limit, search),
            Commands::Init => init_project(),
            Commands::Login { api_key } => run_login(api_key),
            Commands::Logout => run_logout(),
            Commands::Models(args) => {
                let config = load_config_from_cli(&cli)?;
                run_models(&config, args).await
            }
            Commands::Exec(args) => {
                let config = load_config_from_cli(&cli)?;
                let model = args
                    .model
                    .or_else(|| config.default_text_model.clone())
                    .unwrap_or_else(|| config.default_model());
                let prompt = join_prompt_parts(&args.prompt);
                if args.auto || cli.yolo {
                    let workspace = cli.workspace.clone().unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                    let max_subagents = cli.max_subagents.map_or_else(
                        || config.max_subagents(),
                        |value| value.clamp(1, MAX_SUBAGENTS),
                    );
                    let auto_mode = args.auto || cli.yolo;
                    run_exec_agent(
                        &config,
                        &model,
                        &prompt,
                        workspace,
                        max_subagents,
                        true,
                        auto_mode,
                        args.json,
                    )
                    .await
                } else if args.json {
                    run_one_shot_json(&config, &model, &prompt).await
                } else {
                    run_one_shot(&config, &model, &prompt).await
                }
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
                run_pr(&cli, &config, number, repo.as_deref(), checkout).await
            }
            Commands::Apply(args) => run_apply(args),
            Commands::Eval(args) => run_eval(args),
            Commands::Mcp { command } => {
                let config = load_config_from_cli(&cli)?;
                run_mcp_command(&config, command).await
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
                let selected_modes = [args.mcp, args.http, args.acp]
                    .into_iter()
                    .filter(|selected| *selected)
                    .count();
                if selected_modes != 1 {
                    bail!("Choose exactly one server mode: --mcp, --http, or --acp");
                }
                if args.mcp {
                    mcp_server::run_mcp_server(workspace)
                } else if args.http {
                    let config = load_config_from_cli(&cli)?;
                    let cors_origins = resolve_cors_origins(&config, &args.cors_origin);
                    runtime_api::run_http_server(
                        config,
                        workspace,
                        runtime_api::RuntimeApiOptions {
                            host: args.host,
                            port: args.port,
                            workers: args.workers.clamp(1, 8),
                            cors_origins,
                            auth_token: args.auth_token,
                            insecure_no_auth: args.insecure_no_auth,
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
                run_interactive(&cli, &config, Some(resume_id), None).await
            }
            Commands::Fork { session_id, last } => {
                let config = load_config_from_cli(&cli)?;
                let workspace = resolve_workspace(&cli);
                let new_session_id = fork_session(session_id, last, &workspace)?;
                run_interactive(&cli, &config, Some(new_session_id), None).await
            }
        };
    }

    // One-shot prompt mode
    let config = load_config_from_cli(&cli)?;
    if !cli.prompt.is_empty() {
        let prompt = join_prompt_parts(&cli.prompt);
        let model = config.default_model();
        return run_one_shot(&config, &model, &prompt).await;
    }

    // Handle session resume. Plain `deepseek` starts fresh: interrupted
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
    // --yolo starts in YOLO mode (shell + trust + auto-approve)
    run_interactive(&cli, &config, resume_session_id, None).await
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
            url: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
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
     `deepseek-tui setup --status` and surfaced in `deepseek-tui doctor`.\n\n\
     Each script should start with a frontmatter-style header so the\n\
     description is visible without executing the file:\n\n\
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
     printf 'deepseek-tui local tool ok: %s\\n' \"${1:-world}\"\n"
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
     Plugins are richer than tools: each one lives in its own subdirectory\n\
     with a `PLUGIN.md` describing what it does and how to enable it. The\n\
     directory is created so users have a documented place to drop\n\
     experiments without touching `~/.deepseek/skills/`.\n\n\
     A plugin layout looks like:\n\n\
     ```\n\
     plugins/\n\
       my-plugin/\n\
         PLUGIN.md   # frontmatter + body, same shape as SKILL.md\n\
         scripts/    # optional helpers invoked by the plugin\n\
     ```\n\n\
     Plugins are not loaded automatically. Wire them up through skills,\n\
     hooks, or MCP servers when you want them active in a session.\n"
}

fn plugin_example_template() -> &'static str {
    "---\n\
     name: example\n\
     description: Placeholder plugin so /skills and doctor have something to show\n\
     status: example\n\
     ---\n\n\
     This is a starter plugin layout. Edit or replace it once you have a\n\
     real plugin. The agent does not load this file directly; reference it\n\
     from a skill or MCP wrapper if you want it active in a session.\n"
}

fn init_plugins_dir(
    plugins_dir: &Path,
    force: bool,
) -> Result<(PathBuf, PathBuf, WriteStatus, WriteStatus)> {
    std::fs::create_dir_all(plugins_dir)
        .with_context(|| format!("Failed to create plugins dir {}", plugins_dir.display()))?;

    let readme_path = plugins_dir.join("README.md");
    let readme_status = write_template_file(&readme_path, plugins_readme_template(), force)?;

    let example_path = plugins_dir.join("example").join("PLUGIN.md");
    ensure_parent_dir(&example_path)?;
    let example_status = write_template_file(&example_path, plugin_example_template(), force)?;

    Ok((readme_path, example_path, readme_status, example_status))
}

/// Resolve the user-supplied CORS origins for `deepseek serve --http`.
///
/// Sources, in priority order (later sources extend earlier ones):
/// 1. `--cors-origin URL` flags (repeatable)
/// 2. `DEEPSEEK_CORS_ORIGINS` env var (comma-separated)
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
    if let Ok(env_value) = std::env::var("DEEPSEEK_CORS_ORIGINS") {
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
    dirs::home_dir().map_or_else(|| PathBuf::from(".deepseek"), |h| h.join(".deepseek"))
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
    let candidates = ["latest.json", "offline_queue.json"];
    let targets = candidates
        .iter()
        .map(|name| checkpoints_dir.join(name))
        .filter(|p| p.exists())
        .collect();
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

fn run_setup(config: &Config, workspace: &Path, args: SetupArgs) -> Result<()> {
    if args.status {
        return run_setup_status(config, workspace);
    }
    if args.clean {
        return run_setup_clean(&default_checkpoints_dir(), args.force);
    }

    use crate::palette;
    use colored::Colorize;

    let (aqua_r, aqua_g, aqua_b) = palette::DEEPSEEK_SKY_RGB;
    let (sky_r, sky_g, sky_b) = palette::DEEPSEEK_SKY_RGB;

    let any_explicit = args.mcp || args.skills || args.tools || args.plugins;
    let run_mcp = args.mcp || args.all || !any_explicit;
    let run_skills = args.skills || args.all || !any_explicit;
    let run_tools = args.tools || args.all;
    let run_plugins = args.plugins || args.all;

    println!(
        "{}",
        "DeepSeek Setup".truecolor(aqua_r, aqua_g, aqua_b).bold()
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
        println!("    Next: edit the file, then run `deepseek mcp list` or `deepseek mcp tools`.");
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
        let (readme_path, example_path, readme_status, example_status) =
            init_plugins_dir(&plugins_dir, args.force)?;
        report_write_status("Plugins README", &readme_path, readme_status);
        report_write_status("Example plugin", &example_path, example_status);
        println!(
            "    Plugins dir: {}",
            crate::utils::display_path(&plugins_dir)
        );
        println!("    Next: copy the example dir, edit PLUGIN.md, wire via skill/MCP.");
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

/// Source of the resolved DeepSeek API key, used in status reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiKeySource {
    Env,
    Config,
    Keyring,
    Missing,
}

fn resolve_api_key_source(config: &Config) -> ApiKeySource {
    if std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .is_some()
    {
        match std::env::var("DEEPSEEK_API_KEY_SOURCE").ok().as_deref() {
            Some("config") => return ApiKeySource::Config,
            Some("keyring") => return ApiKeySource::Keyring,
            _ => {}
        }
    }

    if config
        .api_key
        .as_ref()
        .is_some_and(|k| !k.trim().is_empty())
        || config
            .provider_config()
            .and_then(|entry| entry.api_key.as_ref())
            .is_some_and(|k| !k.trim().is_empty())
    {
        ApiKeySource::Config
    } else if std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .is_some()
    {
        ApiKeySource::Env
    } else {
        ApiKeySource::Missing
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

fn run_setup_status(config: &Config, workspace: &Path) -> Result<()> {
    use crate::palette;
    use colored::Colorize;

    let (aqua_r, aqua_g, aqua_b) = palette::DEEPSEEK_SKY_RGB;
    let (sky_r, sky_g, sky_b) = palette::DEEPSEEK_SKY_RGB;
    let (red_r, red_g, red_b) = palette::DEEPSEEK_RED_RGB;

    println!(
        "{}",
        "DeepSeek Status".truecolor(aqua_r, aqua_g, aqua_b).bold()
    );
    println!("{}", "===============".truecolor(sky_r, sky_g, sky_b));
    println!("workspace: {}", workspace.display());

    match resolve_api_key_source(config) {
        ApiKeySource::Env => println!(
            "  {} api_key: set via DEEPSEEK_API_KEY",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Keyring => println!(
            "  {} api_key: set via OS keyring",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Config => println!(
            "  {} api_key: set via config",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        ),
        ApiKeySource::Missing => {
            let (env_var, login_hint) = match config.api_provider() {
                crate::config::ApiProvider::NvidiaNim => (
                    "NVIDIA_API_KEY",
                    "deepseek auth set --provider nvidia-nim --api-key \"...\"",
                ),
                crate::config::ApiProvider::Openai => (
                    "OPENAI_API_KEY",
                    "deepseek auth set --provider openai --api-key \"...\"",
                ),
                crate::config::ApiProvider::Openrouter => (
                    "OPENROUTER_API_KEY",
                    "deepseek auth set --provider openrouter --api-key \"...\"",
                ),
                crate::config::ApiProvider::Novita => (
                    "NOVITA_API_KEY",
                    "deepseek auth set --provider novita --api-key \"...\"",
                ),
                crate::config::ApiProvider::Fireworks => (
                    "FIREWORKS_API_KEY",
                    "deepseek auth set --provider fireworks --api-key \"...\"",
                ),
                crate::config::ApiProvider::Sglang => (
                    "SGLANG_API_KEY",
                    "deepseek auth set --provider sglang --api-key \"...\"",
                ),
                crate::config::ApiProvider::Vllm => (
                    "VLLM_API_KEY",
                    "deepseek auth set --provider vllm --api-key \"...\"",
                ),
                crate::config::ApiProvider::Ollama => {
                    ("OLLAMA_API_KEY", "deepseek auth set --provider ollama")
                }
                crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN => {
                    ("DEEPSEEK_API_KEY", "deepseek auth set --provider deepseek")
                }
            };
            println!(
                "  {} api_key: missing  (set {env_var} or `[providers.{}].api_key` in ~/.deepseek/config.toml; or run `{login_hint}`)",
                "✗".truecolor(red_r, red_g, red_b),
                match config.api_provider() {
                    crate::config::ApiProvider::NvidiaNim => "nvidia_nim",
                    crate::config::ApiProvider::Openai => "openai",
                    crate::config::ApiProvider::Openrouter => "openrouter",
                    crate::config::ApiProvider::Novita => "novita",
                    crate::config::ApiProvider::Fireworks => "fireworks",
                    crate::config::ApiProvider::Sglang => "sglang",
                    crate::config::ApiProvider::Vllm => "vllm",
                    crate::config::ApiProvider::Ollama => "ollama",
                    crate::config::ApiProvider::Deepseek
                    | crate::config::ApiProvider::DeepseekCN => "deepseek",
                }
            );
        }
    }
    println!("  · base_url: {}", config.deepseek_base_url());
    let model = config
        .default_text_model
        .clone()
        .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string());
    println!("  · default_text_model: {model}");

    let mcp_path = config.mcp_config_path();
    let mcp_count = match load_mcp_config(&mcp_path) {
        Ok(cfg) => cfg.servers.len(),
        Err(_) => 0,
    };
    let mcp_present = if mcp_path.exists() { "" } else { "  (missing)" };
    println!(
        "  · mcp servers: {mcp_count} at {}{mcp_present}",
        mcp_path.display()
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
    println!("Run `deepseek doctor --json` for a machine-readable check.");
    Ok(())
}

fn dotenv_status_line(workspace: &Path) -> String {
    let dotenv = workspace.join(".env");
    if dotenv.exists() {
        return format!(".env present at {}", dotenv.display());
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

/// Run system diagnostics
async fn run_doctor(config: &Config, workspace: &Path, config_path_override: Option<&Path>) {
    use crate::palette;
    use colored::Colorize;

    let (blue_r, blue_g, blue_b) = palette::DEEPSEEK_BLUE_RGB;
    let (sky_r, sky_g, sky_b) = palette::DEEPSEEK_SKY_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::DEEPSEEK_SKY_RGB;
    let (red_r, red_g, red_b) = palette::DEEPSEEK_RED_RGB;

    println!(
        "{}",
        "DeepSeek TUI Doctor"
            .truecolor(blue_r, blue_g, blue_b)
            .bold()
    );
    println!("{}", "==================".truecolor(sky_r, sky_g, sky_b));
    println!();

    // Version info
    println!("{}", "Version Information:".bold());
    println!("  deepseek-tui: {}", env!("DEEPSEEK_BUILD_VERSION"));
    println!("  rust: {}", rustc_version());
    println!();

    // Configuration summary
    println!("{}", "Configuration:".bold());
    let default_config_dir =
        dirs::home_dir().map_or_else(|| PathBuf::from(".deepseek"), |h| h.join(".deepseek"));
    let config_path = config_path_override
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("DEEPSEEK_CONFIG_PATH")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_config_dir.join("config.toml"));

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

    // Check API keys
    println!();
    println!("{}", "API Keys:".bold());

    // Per-provider state: env + config file only (no values printed).
    // Keep doctor/status prompt-free even for unsigned rebuilt binaries.
    let dispatcher_api_key_source = std::env::var("DEEPSEEK_API_KEY_SOURCE").ok();
    for (provider, slot, env_names) in [
        (
            crate::config::ApiProvider::Deepseek,
            "deepseek",
            &["DEEPSEEK_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::NvidiaNim,
            "nvidia-nim",
            &["NVIDIA_API_KEY", "NVIDIA_NIM_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Openrouter,
            "openrouter",
            &["OPENROUTER_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Novita,
            "novita",
            &["NOVITA_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Fireworks,
            "fireworks",
            &["FIREWORKS_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Sglang,
            "sglang",
            &["SGLANG_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Vllm,
            "vllm",
            &["VLLM_API_KEY"][..],
        ),
        (
            crate::config::ApiProvider::Ollama,
            "ollama",
            &["OLLAMA_API_KEY"][..],
        ),
    ] {
        let in_env = env_names.iter().any(|n| {
            std::env::var(n)
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
    println!("  · credential precedence: ~/.deepseek/config.toml, OS keyring, then env");

    let api_key_source = resolve_api_key_source(config);
    let has_api_key = if config.deepseek_api_key().is_ok() {
        let source_label = match api_key_source {
            ApiKeySource::Config => "config.toml",
            ApiKeySource::Keyring => "OS keyring",
            ApiKeySource::Env => "environment",
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
            "    Run 'deepseek auth set --provider <name>' to save a key to ~/.deepseek/config.toml."
        );
        false
    };

    // API connectivity test
    println!();
    println!("{}", "API Connectivity:".bold());
    let api_target = doctor_api_target(config);
    println!("  · provider: {}", api_target.provider);
    println!("  · base_url: {}", api_target.base_url);
    println!("  · model: {}", api_target.model);
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
    if has_api_key {
        print!("  {} Testing connection...", "·".dimmed());
        use std::io::Write;
        std::io::stdout().flush().ok();

        match test_api_connectivity(config).await {
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
                        "    Invalid API key. Check `deepseek auth status`, DEEPSEEK_API_KEY, or config.toml"
                    );
                    if matches!(api_key_source, ApiKeySource::Keyring) {
                        println!(
                            "    The rejected key came from the OS keyring via the dispatcher."
                        );
                        println!(
                            "    Run `deepseek auth status` to inspect config/keyring/env sources."
                        );
                    } else if matches!(api_key_source, ApiKeySource::Env) {
                        println!(
                            "    The rejected key came from DEEPSEEK_API_KEY; no saved config key is present."
                        );
                        println!(
                            "    Run `deepseek auth set --provider deepseek` to save a config key that overrides stale env."
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
                    println!("    Error: {}", error_msg);
                }
            }
        }
    } else {
        println!("  {} Skipped (no API key configured)", "·".dimmed());
    }

    // MCP configuration
    println!();
    println!("{}", "MCP Servers:".bold());
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
    if mcp_config_path.exists() {
        println!(
            "  {} MCP config found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&mcp_config_path)
        );
        match load_mcp_config(&mcp_config_path) {
            Ok(cfg) if cfg.servers.is_empty() => {
                println!("  {} 0 server(s) configured", "·".dimmed());
            }
            Ok(cfg) => {
                println!(
                    "  {} {} server(s) configured",
                    "·".dimmed(),
                    cfg.servers.len()
                );
                for (name, server) in &cfg.servers {
                    let status = doctor_check_mcp_server(server);
                    let icon = match status {
                        McpServerDoctorStatus::Ok(ref detail) => {
                            format!(
                                "  {} {name}: {}",
                                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                                detail
                            )
                        }
                        McpServerDoctorStatus::Warning(ref detail) => {
                            format!(
                                "  {} {name}: {}",
                                "!".truecolor(sky_r, sky_g, sky_b),
                                detail
                            )
                        }
                        McpServerDoctorStatus::Error(ref detail) => {
                            format!(
                                "  {} {name}: {}",
                                "✗".truecolor(red_r, red_g, red_b),
                                detail
                            )
                        }
                    };
                    println!("{icon}");
                    if !server.enabled {
                        println!("      (disabled)");
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
    } else {
        println!(
            "  {} MCP config not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&mcp_config_path)
        );
        println!("    Run `deepseek mcp init` or `deepseek setup --mcp`.");
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
        println!("    Run `deepseek setup --skills` (or add --local for ./skills).");
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
        println!("    Run `deepseek setup --tools` to scaffold a starter dir.");
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
        println!("    Run `deepseek setup --plugins` to scaffold a starter dir.");
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
    let stash_path = dirs::home_dir().map(|h| h.join(".deepseek").join("composer_stash.jsonl"));
    if let Some(stash_path) = stash_path {
        let stash_count = crate::composer_stash::load_stash().len();
        if stash_path.exists() {
            println!(
                "  {} composer stash at {} ({} parked draft{})",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(&stash_path),
                stash_count,
                if stash_count == 1 { "" } else { "s" }
            );
        } else {
            println!(
                "  {} composer stash empty (Ctrl+S in the composer to park a draft)",
                "·".dimmed()
            );
        }
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

/// Machine-readable counterpart to `run_doctor`. Skips the live API call so it
/// is safe to run in CI and from non-interactive scripts.
fn run_doctor_json(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
) -> Result<()> {
    use serde_json::json;

    let default_config_dir =
        dirs::home_dir().map_or_else(|| PathBuf::from(".deepseek"), |h| h.join(".deepseek"));
    let config_path = config_path_override
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("DEEPSEEK_CONFIG_PATH")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_config_dir.join("config.toml"));

    let api_key_state = match resolve_api_key_source(config) {
        ApiKeySource::Env => "env",
        ApiKeySource::Config => "config",
        ApiKeySource::Keyring => "keyring",
        ApiKeySource::Missing => "missing",
    };

    let mcp_config_path = config.mcp_config_path();
    let mcp_present = mcp_config_path.exists();
    let mcp_summary = match load_mcp_config(&mcp_config_path) {
        Ok(cfg) => {
            let servers: Vec<serde_json::Value> = cfg
                .servers
                .iter()
                .map(|(name, server)| {
                    let status = doctor_check_mcp_server(server);
                    let (kind, detail) = match &status {
                        McpServerDoctorStatus::Ok(d) => ("ok", d.clone()),
                        McpServerDoctorStatus::Warning(d) => ("warning", d.clone()),
                        McpServerDoctorStatus::Error(d) => ("error", d.clone()),
                    };
                    json!({
                        "name": name,
                        "enabled": server.enabled && !server.disabled,
                        "status": kind,
                        "detail": detail,
                    })
                })
                .collect();
            json!({
                "config_path": mcp_config_path.display().to_string(),
                "present": mcp_present,
                "servers": servers,
            })
        }
        Err(err) => json!({
            "config_path": mcp_config_path.display().to_string(),
            "present": mcp_present,
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
    let memory_enabled_env = std::env::var("DEEPSEEK_MEMORY")
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

    let report = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "config_path": config_path.display().to_string(),
        "config_present": config_path.exists(),
        "workspace": workspace.display().to_string(),
        "api_key": {
            "source": api_key_state,
        },
        "base_url": api_target.base_url,
        "default_text_model": api_target.model,
        "strict_tool_mode": {
            "enabled": strict_tool_mode.enabled,
            "status": strict_tool_mode.status,
            "function_strict_sent": strict_tool_mode.function_strict_sent,
            "message": strict_tool_mode.message,
            "recommended_base_url": strict_tool_mode.recommended_base_url,
        },
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
                "path": dirs::home_dir()
                    .map(|h| h.join(".deepseek").join("composer_stash.jsonl").display().to_string())
                    .unwrap_or_default(),
                "present": dirs::home_dir()
                    .map(|h| h.join(".deepseek").join("composer_stash.jsonl"))
                    .is_some_and(|p| p.exists()),
                "count": crate::composer_stash::load_stash().len(),
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
            "note": "Skipped in --json mode; run `deepseek doctor` for a live check.",
        },
        "capability": provider_capability_report(config),
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
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

    json!({
        "resolved_provider": provider.as_str(),
        "resolved_model": cap.resolved_model,
        "context_window": cap.context_window,
        "max_output": cap.max_output,
        "thinking_supported": cap.thinking_supported,
        "cache_telemetry_supported": cap.cache_telemetry_supported,
        "request_payload_mode": serde_json::to_value(cap.request_payload_mode).unwrap_or_default(),
        "alias_deprecation": cap.alias_deprecation,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorApiTarget {
    provider: &'static str,
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
        provider: provider.as_str(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeepSeekBaseUrlKind {
    Beta,
    NonBeta,
}

fn known_deepseek_base_url_kind(base_url: &str) -> Option<DeepSeekBaseUrlKind> {
    match base_url.trim_end_matches('/').to_ascii_lowercase().as_str() {
        "https://api.deepseek.com/beta" | "https://api.deepseeki.com/beta" => {
            Some(DeepSeekBaseUrlKind::Beta)
        }
        "https://api.deepseek.com"
        | "https://api.deepseek.com/v1"
        | "https://api.deepseeki.com"
        | "https://api.deepseeki.com/v1" => Some(DeepSeekBaseUrlKind::NonBeta),
        _ => None,
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
                "If this is a custom DeepSeek-compatible endpoint, set its HTTPS base URL in ~/.deepseek/config.toml and rerun `deepseek doctor`."
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
        "Run `deepseek doctor --json` and include `base_url`, `default_text_model`, and `api_connectivity` when filing an issue."
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
    // Try to get rustc version, fall back to "unknown"
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string())
}

/// List saved sessions
fn list_sessions(limit: usize, search: Option<String>) -> Result<()> {
    use crate::palette;
    use colored::Colorize;
    use session_manager::{SessionManager, format_session_line};

    let (blue_r, blue_g, blue_b) = palette::DEEPSEEK_BLUE_RGB;
    let (sky_r, sky_g, sky_b) = palette::DEEPSEEK_SKY_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::DEEPSEEK_SKY_RGB;

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
            "deepseek".truecolor(blue_r, blue_g, blue_b)
        );
        return Ok(());
    }

    println!(
        "{}",
        "Saved Sessions".truecolor(blue_r, blue_g, blue_b).bold()
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
        "deepseek --resume".truecolor(blue_r, blue_g, blue_b),
        "<session-id>".dimmed()
    );
    println!(
        "Continue latest in this workspace: {}",
        "deepseek --continue".truecolor(blue_r, blue_g, blue_b)
    );

    Ok(())
}

/// Initialize a new project with AGENTS.md
fn init_project() -> Result<()> {
    use crate::palette;
    use colored::Colorize;
    use project_context::create_default_agents_md;

    let (sky_r, sky_g, sky_b) = palette::DEEPSEEK_SKY_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::DEEPSEEK_SKY_RGB;
    let (red_r, red_g, red_b) = palette::DEEPSEEK_RED_RGB;

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
            println!("The instructions will be loaded automatically when you run deepseek.");
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
    let profile = cli
        .profile
        .clone()
        .or_else(|| std::env::var("DEEPSEEK_PROFILE").ok());
    let mut config = Config::load(cli.config.clone(), profile.as_deref())?;
    cli.feature_toggles.apply(&mut config)?;
    Ok(config)
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

fn resolve_session_id(session_id: Option<String>, last: bool, workspace: &Path) -> Result<String> {
    if last {
        return latest_session_id_for_workspace(workspace)?.ok_or_else(|| {
            anyhow!(
                "No saved sessions found for workspace {}. Use `deepseek sessions` to list all sessions, or `deepseek resume <SESSION_ID>` to resume one explicitly.",
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

fn fork_session(session_id: Option<String>, last: bool, workspace: &Path) -> Result<String> {
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

    let system_prompt = saved
        .system_prompt
        .as_ref()
        .map(|text| SystemPrompt::Text(text.clone()));
    let forked = create_saved_session(
        &saved.messages,
        &saved.metadata.model,
        &saved.metadata.workspace,
        saved.metadata.total_tokens,
        system_prompt.as_ref(),
    );
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

    let model = args
        .model
        .or_else(|| config.default_text_model.clone())
        .unwrap_or_else(|| config.default_model());
    let route = resolve_cli_auto_route(config, &model, &diff).await;
    let model = route.model;
    let reasoning_effort = route
        .reasoning_effort
        .map(|effort| effort.as_setting().to_string());

    let system = SystemPrompt::Text(
        "You are a senior code reviewer. Focus on bugs, risks, behavioral regressions, and missing tests. \
Provide findings ordered by severity with file references, then open questions, then a brief summary."
            .to_string(),
    );
    let user_prompt =
        format!("Review the following diff and provide feedback:\n\n{diff}\n\nEnd of diff.");

    let client = DeepSeekClient::new(config)?;
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
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "review",
                "model": model,
                "success": true,
                "content": output
            }))?
        );
    } else {
        println!("{output}");
    }
    Ok(())
}

/// `deepseek pr <N>` (#451) — fetch a GitHub PR via `gh`, format
/// title + body + diff as the composer's first message, and launch
/// the interactive TUI. Falls back gracefully if `gh` is missing.
async fn run_pr(
    cli: &Cli,
    config: &Config,
    number: u32,
    repo: Option<&str>,
    checkout: bool,
) -> Result<()> {
    if !is_command_available("gh") {
        bail!(
            "`gh` CLI not found on PATH. Install GitHub CLI \
             (https://cli.github.com) and authenticate (`gh auth login`) \
             so `deepseek pr <N>` can fetch PR metadata and the diff."
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
    run_interactive(cli, config, resume_session_id, Some(prompt)).await
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
    let mut cmd = Command::new("gh");
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
    let mut cmd = Command::new("gh");
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
    let mut cmd = Command::new("gh");
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
    let mut cmd = Command::new("git");
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
        .map_err(|e| anyhow::anyhow!("Failed to run git diff. Is git installed? ({})", e))?;
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

    let output = Command::new("git")
        .arg("apply")
        .arg("--whitespace=nowarn")
        .arg(&tmp_path)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run git apply: {}", e))?;

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

async fn run_mcp_command(config: &Config, command: McpCommand) -> Result<()> {
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
            println!("Edit the file, then run `deepseek mcp list` or `deepseek mcp tools`.");
            Ok(())
        }
        McpCommand::List => {
            let cfg = load_mcp_config(&config_path)?;
            if cfg.servers.is_empty() {
                println!("No MCP servers configured in {}", config_path.display());
                return Ok(());
            }
            println!("MCP servers ({}):", cfg.servers.len());
            for (name, server) in cfg.servers {
                let status = if server.enabled && !server.disabled {
                    "enabled"
                } else {
                    "disabled"
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
                println!("  - {name} [{status}{required}] {cmd_str}");
            }
            Ok(())
        }
        McpCommand::Connect { server } => {
            let mut pool = McpPool::from_config_path(&config_path)?;
            if let Some(name) = server {
                pool.get_or_connect(&name).await?;
                println!("Connected to MCP server: {name}");
            } else {
                let errors = pool.connect_all().await;
                if errors.is_empty() {
                    println!("Connected to all configured MCP servers.");
                } else {
                    for (name, err) in errors {
                        eprintln!("Failed to connect {name}: {err:#}");
                    }
                }
            }
            Ok(())
        }
        McpCommand::Tools { server } => {
            let mut pool = McpPool::from_config_path(&config_path)?;
            if let Some(name) = server {
                let conn = pool.get_or_connect(&name).await?;
                if conn.tools().is_empty() {
                    println!("No tools found for MCP server: {name}");
                } else {
                    println!("Tools for {name}:");
                    for tool in conn.tools() {
                        println!(
                            "  - {}{}",
                            tool.name,
                            tool.description
                                .as_ref()
                                .map_or(String::new(), |d| format!(": {d}"))
                        );
                    }
                }
            } else {
                let _ = pool.connect_all().await;
                let tools = pool.all_tools();
                if tools.is_empty() {
                    println!("No MCP tools discovered.");
                } else {
                    println!("MCP tools:");
                    for (name, tool) in tools {
                        println!(
                            "  - {}{}",
                            name,
                            tool.description
                                .as_ref()
                                .map_or(String::new(), |d| format!(": {d}"))
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
            args,
        } => {
            if command.is_none() && url.is_none() {
                bail!("Provide either --command or --url for `mcp add`.");
            }
            let mut cfg = load_mcp_config(&config_path)?;
            cfg.servers.insert(
                name.clone(),
                McpServerConfig {
                    command,
                    args,
                    env: std::collections::HashMap::new(),
                    url,
                    connect_timeout: None,
                    execute_timeout: None,
                    read_timeout: None,
                    disabled: false,
                    enabled: true,
                    required: false,
                    enabled_tools: Vec::new(),
                    disabled_tools: Vec::new(),
                },
            );
            save_mcp_config(&config_path, &cfg)?;
            println!("Added MCP server '{name}' in {}", config_path.display());
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
            let mut pool = McpPool::from_config_path(&config_path)?;
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
                    "MCP server '{name}' already exists in {}. Use `deepseek mcp remove {name}` first, or choose a different --name.",
                    config_path.display()
                );
            }
            cfg.servers.insert(
                name.clone(),
                McpServerConfig {
                    command: Some(exe_str.clone()),
                    args,
                    env: std::collections::HashMap::new(),
                    url: None,
                    connect_timeout: None,
                    execute_timeout: None,
                    read_timeout: None,
                    disabled: false,
                    enabled: true,
                    required: false,
                    enabled_tools: Vec::new(),
                    disabled_tools: Vec::new(),
                },
            );
            save_mcp_config(&config_path, &cfg)?;
            println!(
                "Registered DeepSeek as MCP server '{name}' in {}",
                config_path.display()
            );
            println!("  command: {exe_str}");
            println!(
                "  args:    serve --mcp{}",
                workspace.map_or(String::new(), |ws| format!(" --workspace {ws}"))
            );
            println!();
            println!("Tip: Use `deepseek mcp validate` to test the connection.");
            println!("     Use `deepseek serve --http` for the HTTP/SSE runtime API instead.");
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
    let cfg: McpConfig = serde_json::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse MCP config: {e}"))?;
    Ok(cfg)
}

/// Diagnostic status for an MCP server entry.
#[derive(Debug)]
enum McpServerDoctorStatus {
    Ok(String),
    Warning(String),
    Error(String),
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

    let cmd_path = Path::new(cmd);
    // Also accept Unix-style `/` prefix on Windows, where Path::is_absolute()
    // requires a drive letter.
    let is_absolute = cmd_path.is_absolute() || cmd.starts_with('/');

    if is_absolute && !cmd_path.exists() {
        return McpServerDoctorStatus::Error(format!("command not found: {cmd}"));
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
            eprint!("{}", stderr_str);
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
    should_use_mouse_capture_with(cli, config, use_alt_screen, terminal_emulator.as_deref())
}

fn should_use_mouse_capture_with(
    cli: &Cli,
    config: &Config,
    use_alt_screen: bool,
    terminal_emulator: Option<&str>,
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
        .unwrap_or_else(|| default_mouse_capture_enabled(terminal_emulator))
}

/// Whether to enable terminal mouse capture by default for this platform/host.
///
/// Returns `false` on Windows (legacy console mouse-mode reporting is flaky;
/// `--mouse-capture` opts in) and on JetBrains' JediTerm, which advertises
/// mouse support but delivers SGR mouse-event escape sequences as raw text
/// in the input stream — visible to users as garbled characters in the
/// composer when they move the mouse over the TUI (#878, #898). The user
/// can still opt back in with `[tui] mouse_capture = true` in
/// `~/.deepseek/config.toml` or `--mouse-capture`.
fn default_mouse_capture_enabled(terminal_emulator: Option<&str>) -> bool {
    if cfg!(windows) {
        return false;
    }
    if matches!(terminal_emulator, Some(t) if t.eq_ignore_ascii_case("JetBrains-JediTerm")) {
        return false;
    }
    true
}

/// Load a recent crash-recovery checkpoint, pruning stale checkpoints first.
fn load_recent_checkpoint(
    manager: &session_manager::SessionManager,
) -> Option<(session_manager::SavedSession, std::time::Duration)> {
    let session = manager.load_checkpoint().ok().flatten()?;

    let home = dirs::home_dir()?;
    let checkpoint_path = home
        .join(".deepseek")
        .join("sessions")
        .join("checkpoints")
        .join("latest.json");
    let metadata = std::fs::metadata(&checkpoint_path).ok()?;
    let mtime = metadata.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(mtime).ok()?;
    if age > std::time::Duration::from_secs(24 * 3600) {
        let _ = manager.clear_checkpoint();
        return None;
    }

    Some((session, age))
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
/// The checkpoint must exist and its file mtime must be within 24 hours.
/// **The checkpoint's workspace must also match the resolved launch workspace
/// after canonicalisation.** If the workspace doesn't match, the checkpoint is
/// persisted as a regular session (so the user can find it via
/// `deepseek sessions` / `deepseek resume <id>`) and cleared, but not loaded.
fn recover_interrupted_checkpoint_for_resume(launch_workspace: &Path) -> Option<String> {
    let manager = session_manager::SessionManager::default_location().ok()?;
    let (session, age) = load_recent_checkpoint(&manager)?;

    // Refuse to silently restore a session from another workspace. Compare
    // against the resolved launch workspace, not the shell cwd, so callers
    // using `--workspace` cannot accidentally recover a checkpoint from the
    // directory their shell happened to be in.
    let session_workspace = session.metadata.workspace.clone();
    let workspace_matches =
        session_manager::workspace_scope_matches(&session_workspace, launch_workspace);

    if !workspace_matches {
        // Persist the checkpoint so the user can find it via `deepseek
        // sessions`, then clear it so the next launch in this folder doesn't
        // re-trip the nag. Print a one-line notice pointing at the explicit
        // resume command — but DO NOT auto-load the session here.
        let _ = manager.save_session(&session);
        let _ = manager.clear_checkpoint();
        eprintln!(
            "Note: an interrupted session from another workspace ({}) is \
             available. Run `deepseek sessions` to list saved sessions. Starting \
             fresh in {}.",
            session_workspace.display(),
            launch_workspace.display(),
        );
        return None;
    }

    let session_id = session.metadata.id.clone();

    // Persist the checkpoint as a regular session so the TUI can load it by id.
    if manager.save_session(&session).is_err() {
        return None;
    }

    // Clear the checkpoint now that it has been recovered.
    let _ = manager.clear_checkpoint();

    let age_str = checkpoint_age_label(age);
    eprintln!("Recovered interrupted session ({age_str}). Use --fresh to start fresh.",);

    Some(session_id)
}

/// Preserve an interrupted checkpoint on a normal fresh launch without
/// attaching it to the new TUI instance. This keeps "open another deepseek in
/// the same folder" from re-entering the previous in-flight session while still
/// leaving an explicit resume path.
fn preserve_interrupted_checkpoint_for_explicit_resume(launch_workspace: &Path) {
    let Some(manager) = session_manager::SessionManager::default_location().ok() else {
        return;
    };
    let Some((session, age)) = load_recent_checkpoint(&manager) else {
        return;
    };

    let session_workspace = session.metadata.workspace.clone();
    let _ = manager.save_session(&session);
    let _ = manager.clear_checkpoint();

    let age_str = checkpoint_age_label(age);
    if session_manager::workspace_scope_matches(&session_workspace, launch_workspace) {
        eprintln!(
            "Found an in-flight session snapshot ({age_str}). Starting a new \
             session. Run `deepseek --continue` to resume it."
        );
    } else {
        eprintln!(
            "Note: an interrupted session from another workspace ({}) is \
             available. Run `deepseek sessions` to list saved sessions. Starting \
             fresh in {}.",
            session_workspace.display(),
            launch_workspace.display(),
        );
    }
}

/// Load project-level config from `$WORKSPACE/.deepseek/config.toml` and
/// apply its fields as overrides on top of the global config (#485).
/// Only explicitly set fields in the project file are applied; everything
/// else falls back to the global value.
fn merge_project_config(config: &mut Config, workspace: &Path) {
    let path = workspace.join(".deepseek").join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return,
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
    //
    // The overlay path is non-interactive; users can't visually
    // confirm a rogue project config is hijacking these. We surface
    // a stderr warning on first encounter so a user who *did* expect
    // the override has a chance to notice the deny instead of silent
    // discard.
    const DENY_AT_PROJECT_SCOPE: &[&str] = &["api_key", "base_url", "provider", "mcp_config_path"];
    for key in DENY_AT_PROJECT_SCOPE {
        if table.contains_key(*key) {
            eprintln!(
                "warning: project-scope config key `{key}` is ignored — \
                 set it in `~/.deepseek/config.toml` instead. \
                 (See #417 for the deny-list rationale.)"
            );
        }
    }

    // String fields a project may legitimately override (model,
    // approval/sandbox tightening, notes path, reasoning effort).
    // Loosening *values* like `approval_policy = "auto"` and
    // `sandbox_mode = "danger-full-access"` are denied unconditionally
    // — those are pure escalation regardless of the user's prior
    // value. Sub-tightening comparisons (e.g. user `"never"` →
    // project `"on-request"`) stay v0.8.9 follow-up because they
    // need a richer ordering check.
    for (key, field) in [
        ("model", &mut config.default_text_model),
        ("reasoning_effort", &mut config.reasoning_effort),
        ("approval_policy", &mut config.approval_policy),
        ("sandbox_mode", &mut config.sandbox_mode),
        ("notes_path", &mut config.notes_path),
    ] {
        if let Some(v) = table.get(key).and_then(toml::Value::as_str)
            && !v.is_empty()
        {
            // #417 escalation deny: project cannot push the session
            // to the loosest values. Other strings flow through the
            // existing config validator on load.
            let is_escalation = matches!(
                (key, v),
                ("approval_policy", "auto") | ("sandbox_mode", "danger-full-access")
            );
            if is_escalation {
                eprintln!(
                    "warning: project-scope `{key} = \"{v}\"` is ignored — \
                     project config cannot escalate to the loosest value. \
                     (See #417.)"
                );
                continue;
            }
            *field = Some(v.to_string());
        }
    }

    // Numeric / bool fields that benefit from per-project overrides.
    if let Some(v) = table.get("max_subagents").and_then(toml::Value::as_integer)
        && v > 0
    {
        config.max_subagents = Some((v as usize).clamp(1, crate::config::MAX_SUBAGENTS));
    }
    if let Some(v) = table.get("allow_shell").and_then(toml::Value::as_bool) {
        config.allow_shell = Some(v);
    }

    // #454: instructions array — project replaces user. Empty arrays
    // count: explicit `instructions = []` clears the user's list for
    // this repo, useful when the user has a verbose global file that
    // doesn't apply to the current project. Non-string entries are
    // skipped silently rather than failing the load.
    if let Some(arr) = table.get("instructions").and_then(toml::Value::as_array) {
        let entries: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.trim().is_empty())
            .collect();
        config.instructions = Some(entries);
    }
}

async fn run_interactive(
    cli: &Cli,
    config: &Config,
    resume_session_id: Option<String>,
    initial_input: Option<String>,
) -> Result<()> {
    let workspace = cli
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Merge project-level config from $WORKSPACE/.deepseek/config.toml
    // unless --no-project-config was passed (#485).
    let mut merged_config = config.clone();
    if !cli.no_project_config {
        merge_project_config(&mut merged_config, &workspace);
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

    let model = config.default_model();
    let max_subagents = cli.max_subagents.map_or_else(
        || config.max_subagents(),
        |value| value.clamp(1, MAX_SUBAGENTS),
    );
    let use_alt_screen = should_use_alt_screen(cli, config);
    let use_mouse_capture = should_use_mouse_capture(cli, config, use_alt_screen);
    let use_bracketed_paste = crate::settings::Settings::load()
        .map(|s| s.bracketed_paste)
        .unwrap_or(true);

    // Auto-install bundled system skills (e.g. skill-creator) on first launch.
    // Errors are non-fatal: log a warning and continue.
    let skills_dir = config.skills_dir();
    if let Err(e) = crate::skills::install_system_skills(&skills_dir) {
        logging::warn(format!("Failed to install system skills: {e}"));
    }

    // Prune stale workspace snapshots from prior sessions (7-day default).
    // Non-fatal: a flaky disk, missing `git`, or read-only home should
    // never block the TUI from starting.
    let snapshots = config.snapshots_config();
    if snapshots.enabled {
        session_manager::prune_workspace_snapshots(&workspace, snapshots.max_age());
    }

    // Prune stale tool-output spillover files (#422). Non-fatal: home
    // missing or directory unreadable just means nothing got pruned;
    // we never block startup. Runs unconditionally because the
    // spillover store is created lazily on first write — there's no
    // user-facing setting to gate.
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

    tui::run_tui(
        config,
        tui::TuiOptions {
            model,
            workspace,
            config_path: cli.config.clone(),
            config_profile: cli.profile.clone(),
            allow_shell: cli.yolo || config.allow_shell(),
            use_alt_screen,
            use_mouse_capture,
            use_bracketed_paste,
            skills_dir,
            memory_path: config.memory_path(),
            notes_path: config.notes_path(),
            mcp_config_path: config.mcp_config_path(),
            use_memory: config.memory_enabled(),
            start_in_agent_mode: cli.yolo,
            skip_onboarding: cli.skip_onboarding,
            yolo: cli.yolo, // YOLO mode auto-approves all tool executions
            resume_session_id,
            initial_input,
            max_subagents,
        },
    )
    .await
}

struct CliAutoRoute {
    model: String,
    reasoning_effort: Option<crate::tui::app::ReasoningEffort>,
    auto_model: bool,
}

async fn resolve_cli_auto_route(config: &Config, model: &str, prompt: &str) -> CliAutoRoute {
    if model.trim().eq_ignore_ascii_case("auto") {
        let selection =
            commands::resolve_auto_route_with_flash(config, prompt, "", "auto", "auto").await;
        CliAutoRoute {
            model: selection.model,
            reasoning_effort: selection.reasoning_effort,
            auto_model: true,
        }
    } else {
        CliAutoRoute {
            model: model.to_string(),
            reasoning_effort: None,
            auto_model: false,
        }
    }
}

async fn run_one_shot(config: &Config, model: &str, prompt: &str) -> Result<()> {
    use crate::client::DeepSeekClient;
    use crate::models::{ContentBlock, Message, MessageRequest};

    let client = DeepSeekClient::new(config)?;
    let route = resolve_cli_auto_route(config, model, prompt).await;
    let reasoning_effort = route
        .reasoning_effort
        .map(|effort| effort.as_setting().to_string());

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

async fn run_one_shot_json(config: &Config, model: &str, prompt: &str) -> Result<()> {
    use crate::client::DeepSeekClient;
    use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

    let client = DeepSeekClient::new(config)?;
    let route = resolve_cli_auto_route(config, model, prompt).await;
    let model = route.model;
    let reasoning_effort = route
        .reasoning_effort
        .map(|effort| effort.as_setting().to_string());
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
        serde_json::to_string_pretty(&serde_json::json!({
            "mode": "one-shot",
            "model": model,
            "success": true,
            "output": output
        }))?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_exec_agent(
    config: &Config,
    model: &str,
    prompt: &str,
    workspace: PathBuf,
    max_subagents: usize,
    auto_approve: bool,
    trust_mode: bool,
    json_output: bool,
) -> Result<()> {
    use crate::compaction::CompactionConfig;
    use crate::core::engine::{EngineConfig, spawn_engine};
    use crate::core::events::Event;
    use crate::core::ops::Op;
    use crate::models::compaction_threshold_for_model;
    use crate::tools::plan::new_shared_plan_state;
    use crate::tools::todo::new_shared_todo_list;
    use crate::tui::app::AppMode;

    let route = resolve_cli_auto_route(config, model, prompt).await;
    let auto_model = route.auto_model;
    let effective_model = route.model;
    let effective_reasoning_effort = route
        .reasoning_effort
        .map(|effort| effort.as_setting().to_string());

    // Compaction defaults to disabled in v0.6.6: the checkpoint-restart cycle
    // architecture (issue #124) handles long-context resets via fresh contexts
    // rather than progressive summarization. The compaction config is still
    // wired through so users who explicitly opt back in through TUI settings
    // or direct engine config keep their old behavior.
    let compaction = CompactionConfig {
        enabled: false,
        model: effective_model.clone(),
        token_threshold: compaction_threshold_for_model(&effective_model),
        ..Default::default()
    };

    let network_policy = config.network.clone().map(|toml_cfg| {
        crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
    });

    let lsp_config = config
        .lsp
        .clone()
        .map(crate::config::LspConfigToml::into_runtime);

    let engine_config = EngineConfig {
        model: effective_model.clone(),
        workspace: workspace.clone(),
        allow_shell: auto_approve || config.allow_shell(),
        trust_mode,
        notes_path: config.notes_path(),
        mcp_config_path: config.mcp_config_path(),
        skills_dir: config.skills_dir(),
        instructions: config.instructions_paths(),
        project_context_pack_enabled: config.project_context_pack_enabled(),
        max_steps: 100,
        max_subagents,
        features: config.features(),
        compaction,
        cycle: crate::cycle_manager::CycleConfig::default(),
        capacity: crate::core::capacity::CapacityControllerConfig::from_app_config(config),
        todos: new_shared_todo_list(),
        plan_state: new_shared_plan_state(),
        max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
        network_policy,
        snapshots_enabled: config.snapshots_config().enabled,
        lsp_config,
        runtime_services: crate::tools::spec::RuntimeToolServices::default(),
        subagent_model_overrides: config.subagent_model_overrides(),
        memory_enabled: config.memory_enabled(),
        memory_path: config.memory_path(),
        strict_tool_mode: config.strict_tool_mode.unwrap_or(false),
        goal_objective: None,
        locale_tag: crate::localization::resolve_locale(
            &crate::settings::Settings::load().unwrap_or_default().locale,
        )
        .tag()
        .to_string(),
        workshop: config.workshop.clone(),
    };

    let engine_handle = spawn_engine(engine_config, config);
    let mode = if auto_approve {
        AppMode::Yolo
    } else {
        AppMode::Agent
    };

    engine_handle
        .send(Op::SendMessage {
            content: prompt.to_string(),
            mode,
            model: effective_model.clone(),
            goal_objective: None,
            reasoning_effort: effective_reasoning_effort,
            reasoning_effort_auto: auto_model,
            auto_model,
            allow_shell: auto_approve || config.allow_shell(),
            trust_mode,
            auto_approve,
            approval_mode: if auto_approve {
                crate::tui::approval::ApprovalMode::Auto
            } else {
                config
                    .approval_policy
                    .as_deref()
                    .and_then(crate::tui::approval::ApprovalMode::from_config_value)
                    .unwrap_or_default()
            },
        })
        .await?;

    #[derive(serde::Serialize)]
    struct ExecToolEntry {
        name: String,
        success: bool,
        output: String,
    }
    #[derive(serde::Serialize, Default)]
    struct ExecSummary {
        mode: String,
        model: String,
        prompt: String,
        output: String,
        tools: Vec<ExecToolEntry>,
        status: Option<String>,
        error: Option<String>,
    }
    let mut summary = ExecSummary {
        mode: "agent".to_string(),
        model: effective_model,
        prompt: prompt.to_string(),
        ..ExecSummary::default()
    };

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
                if !json_output {
                    print!("{content}");
                    stdout.flush()?;
                }
                ends_with_newline = content.ends_with('\n');
            }
            Event::MessageComplete { .. } if !json_output && !ends_with_newline => {
                println!();
            }
            Event::ToolCallStarted { name, input, .. } if !json_output => {
                let summary = summarize_tool_args(&input);
                if let Some(summary) = summary {
                    eprintln!("tool: {name} ({summary})");
                } else {
                    eprintln!("tool: {name}");
                }
            }
            Event::ToolCallProgress { id, output } if !json_output => {
                eprintln!("tool {id}: {}", summarize_tool_output(&output));
            }
            Event::ToolCallComplete { name, result, .. } => match result {
                Ok(output) => {
                    summary.tools.push(ExecToolEntry {
                        name: name.clone(),
                        success: output.success,
                        output: output.content.clone(),
                    });
                    if name == "exec_shell" && !output.content.trim().is_empty() {
                        if !json_output {
                            eprintln!("tool {name} completed");
                            eprintln!(
                                "--- stdout/stderr ---\n{}\n---------------------",
                                output.content
                            );
                        }
                    } else if !json_output {
                        eprintln!(
                            "tool {name} completed: {}",
                            summarize_tool_output(&output.content)
                        );
                    }
                }
                Err(err) => {
                    summary.tools.push(ExecToolEntry {
                        name: name.clone(),
                        success: false,
                        output: err.to_string(),
                    });
                    if !json_output {
                        eprintln!("tool {name} failed: {err}");
                    }
                }
            },
            Event::AgentSpawned { id, prompt } => {
                eprintln!("sub-agent {id} spawned: {}", summarize_tool_output(&prompt));
            }
            Event::AgentProgress { id, status } => {
                eprintln!("sub-agent {id}: {status}");
            }
            Event::AgentComplete { id, result } => {
                eprintln!(
                    "sub-agent {id} completed: {}",
                    summarize_tool_output(&result)
                );
            }
            Event::ApprovalRequired { id, .. } => {
                if auto_approve {
                    let _ = engine_handle.approve_tool_call(id).await;
                } else {
                    let _ = engine_handle.deny_tool_call(id).await;
                }
            }
            Event::ElevationRequired {
                tool_id,
                tool_name,
                denial_reason,
                ..
            } => {
                if auto_approve {
                    eprintln!("sandbox denied {tool_name}: {denial_reason} (auto-elevating)");
                    let policy = crate::sandbox::SandboxPolicy::DangerFullAccess;
                    let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                } else {
                    eprintln!("sandbox denied {tool_name}: {denial_reason}");
                    let _ = engine_handle.deny_tool_call(tool_id).await;
                }
            }
            Event::Error {
                envelope,
                recoverable: _,
            } => {
                summary.error = Some(envelope.message.clone());
                if !json_output {
                    eprintln!("error: {}", envelope.message);
                }
            }
            Event::TurnComplete { status, error, .. } => {
                summary.status = Some(format!("{status:?}").to_lowercase());
                summary.error = error;
                let _ = engine_handle.send(Op::Shutdown).await;
                break;
            }
            _ => {}
        }
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }

    Ok(())
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
    fn provider_capability_report_exposes_alias_deprecation_for_deepseek_chat() {
        let config = Config {
            default_text_model: Some("deepseek-chat".to_string()),
            ..Default::default()
        };

        let report = provider_capability_report(&config);

        assert_eq!(report["resolved_model"], "deepseek-chat");
        assert_eq!(report["context_window"], 1_000_000);
        assert_eq!(report["thinking_supported"], true);
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
    fn timeout_recovery_keeps_default_deepseek_users_on_default_endpoint() {
        let config = Config::default();

        let text = doctor_timeout_recovery_lines(&config).join("\n");

        assert!(text.contains("api.deepseek.com"));
        assert!(text.contains("custom DeepSeek-compatible endpoint"));
        assert!(!text.contains("provider = \"deepseek-cn\""));
        assert!(text.contains("deepseek doctor --json"));
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
    fn prompt_flag_accepts_split_prompt_words_for_windows_cmd_shims() {
        let cli = parse_cli(&["deepseek", "-p", "hello", "world"]);

        assert_eq!(cli.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn exec_accepts_split_prompt_words_for_windows_cmd_shims() {
        let cli = parse_cli(&["deepseek", "exec", "hello", "world"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn exec_keeps_flags_before_split_prompt_words() {
        let cli = parse_cli(&["deepseek", "exec", "--json", "hello", "world"]);
        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert!(args.json);
        assert_eq!(args.prompt, vec!["hello", "world"]);
    }

    #[test]
    fn alternate_screen_defaults_on_in_auto_mode() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config::default();

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    fn no_alt_screen_flag_is_accepted_but_keeps_alternate_screen() {
        let cli = parse_cli(&["deepseek", "--no-alt-screen"]);
        let config = Config::default();

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    fn config_never_is_accepted_but_keeps_alternate_screen() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: Some("never".to_string()),
                mouse_capture: None,
                terminal_probe_timeout_ms: None,
                status_items: None,
                osc8_links: None,
                notification_condition: None,
            }),
            ..Config::default()
        };

        assert!(should_use_alt_screen(&cli, &config));
    }

    #[test]
    #[cfg(not(windows))]
    fn mouse_capture_defaults_on_when_alternate_screen_is_active() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    #[cfg(windows)]
    fn mouse_capture_defaults_off_on_windows_when_alternate_screen_is_active() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    fn no_mouse_capture_flag_disables_mouse_capture() {
        let cli = parse_cli(&["deepseek", "--no-mouse-capture"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    fn config_can_disable_default_mouse_capture() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(false),
                terminal_probe_timeout_ms: None,
                status_items: None,
                osc8_links: None,
                notification_condition: None,
            }),
            ..Config::default()
        };

        assert!(!should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    fn mouse_capture_flag_enables_mouse_capture() {
        let cli = parse_cli(&["deepseek", "--mouse-capture"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    fn config_can_enable_mouse_capture() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(true),
                terminal_probe_timeout_ms: None,
                status_items: None,
                osc8_links: None,
                notification_condition: None,
            }),
            ..Config::default()
        };

        assert!(should_use_mouse_capture_with(&cli, &config, true, None));
    }

    #[test]
    fn mouse_capture_is_off_without_alternate_screen() {
        let cli = parse_cli(&["deepseek", "--mouse-capture"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(&cli, &config, false, None));
    }

    // Issue #878 / #898: JetBrains JediTerm advertises mouse support but
    // forwards SGR mouse-event escapes as raw input characters, producing
    // the "input box auto-fills with garbled characters when I move the
    // mouse" failure mode in PyCharm/IDEA terminals. Default the capture
    // off when we see TERMINAL_EMULATOR=JetBrains-JediTerm; explicit
    // config / --mouse-capture still wins.

    #[test]
    fn mouse_capture_defaults_off_in_jetbrains_jediterm() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config::default();

        assert!(!should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
        ));
    }

    #[test]
    fn jetbrains_default_off_is_case_insensitive() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config::default();

        // JetBrains has occasionally varied the casing across releases;
        // a case-insensitive match keeps the protection in place.
        assert!(!should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("jetbrains-jediterm"),
        ));
    }

    #[test]
    fn mouse_capture_flag_overrides_jetbrains_default() {
        let cli = parse_cli(&["deepseek", "--mouse-capture"]);
        let config = Config::default();

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
        ));
    }

    #[test]
    fn config_mouse_capture_true_overrides_jetbrains_default() {
        let cli = parse_cli(&["deepseek"]);
        let config = Config {
            tui: Some(crate::config::TuiConfig {
                alternate_screen: None,
                mouse_capture: Some(true),
                terminal_probe_timeout_ms: None,
                status_items: None,
                osc8_links: None,
                notification_condition: None,
            }),
            ..Config::default()
        };

        assert!(should_use_mouse_capture_with(
            &cli,
            &config,
            true,
            Some("JetBrains-JediTerm"),
        ));
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
        // are all on the deny-list. A malicious project must not be
        // able to redirect prompts or hijack MCP servers via these.
        let tmp = workspace_with_project_config(
            r#"
api_key = "ATTACKER_KEY"
base_url = "https://evil.example.com"
provider = "nvidia-nim"
mcp_config_path = "/tmp/attacker-mcp.json"
"#,
        );
        let mut config = Config {
            api_key: Some("USER_KEY".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
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
    fn project_overlay_overrides_max_subagents_and_allow_shell() {
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
            provider: Some("deepseek".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Untouched.
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
    }

    #[test]
    fn project_overlay_skips_malformed_toml() {
        let tmp = workspace_with_project_config("this is not valid TOML !!");
        let mut config = Config {
            provider: Some("deepseek".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Untouched on parse error — better to fall back to global than crash.
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
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
            provider: Some("deepseek".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Empty strings are ignored — they're rarely a deliberate override.
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            config.default_text_model.as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn project_overlay_replaces_user_instructions_array_wholesale() {
        let tmp = workspace_with_project_config(
            r#"
instructions = ["./AGENTS.md", "./extra.md"]
"#,
        );
        // User had a global file in their config; the project array
        // should REPLACE it, not merge.
        let mut config = Config {
            instructions: Some(vec!["~/global.md".to_string()]),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.instructions.as_deref(),
            Some(&["./AGENTS.md".to_string(), "./extra.md".to_string()][..]),
            "project instructions array replaces user array wholesale"
        );
    }

    #[test]
    fn project_overlay_empty_instructions_array_clears_user_list() {
        let tmp = workspace_with_project_config(
            r#"
instructions = []
"#,
        );
        let mut config = Config {
            instructions: Some(vec![
                "~/global.md".to_string(),
                "~/team-prefs.md".to_string(),
            ]),
            ..Config::default()
        };
        merge_project_config(&mut config, tmp.path());
        // Explicit empty array clears the user list — project says
        // "this repo doesn't want any of those globals".
        assert_eq!(
            config.instructions.as_deref(),
            Some(&[][..]),
            "explicit empty array clears the user instructions list"
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
    fn project_overlay_drops_empty_string_entries_in_instructions_array() {
        let tmp = workspace_with_project_config(
            r#"
instructions = ["./AGENTS.md", "", "  ", "./extra.md"]
"#,
        );
        let mut config = Config::default();
        merge_project_config(&mut config, tmp.path());
        assert_eq!(
            config.instructions.as_deref(),
            Some(&["./AGENTS.md".to_string(), "./extra.md".to_string()][..]),
            "empty / whitespace-only entries are filtered"
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
            url: url.map(String::from),
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
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
        let server = make_server(Some("node"), &["server.js"], None);
        match doctor_check_mcp_server(&server) {
            McpServerDoctorStatus::Ok(detail) => assert!(detail.contains("stdio")),
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn test_self_hosted_absolute_is_ok() {
        let server = make_server(Some("/usr/local/bin/deepseek"), &["serve", "--mcp"], None);
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

    #[test]
    fn test_self_hosted_relative_is_warning() {
        let server = make_server(Some("deepseek"), &["serve", "--mcp"], None);
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
}

#[cfg(test)]
mod setup_helper_tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    // Serialize tests that mutate process-global env vars. Without this,
    // `cargo test` runs them in parallel and they race on `DEEPSEEK_API_KEY`,
    // causing intermittent CI failures (one test reads while another's set
    // is still active). `unwrap_or_else` recovers from poisoning so a panic
    // in one test doesn't cascade through the whole module.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let (readme_path, example_path, readme_status, example_status) =
            init_plugins_dir(&dir, false).unwrap();

        assert_eq!(readme_path, dir.join("README.md"));
        assert_eq!(example_path, dir.join("example").join("PLUGIN.md"));
        assert!(matches!(readme_status, WriteStatus::Created));
        assert!(matches!(example_status, WriteStatus::Created));
        assert!(readme_path.exists());
        assert!(example_path.exists());

        let plugin_md = std::fs::read_to_string(&example_path).unwrap();
        assert!(plugin_md.contains("---"));
        assert!(plugin_md.contains("name: example"));
    }

    #[test]
    fn collect_clean_targets_finds_only_known_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("latest.json"), "{}").unwrap();
        std::fs::write(dir.join("offline_queue.json"), "[]").unwrap();
        std::fs::write(dir.join("unrelated.json"), "{}").unwrap();

        let plan = collect_clean_targets(dir);
        assert_eq!(plan.targets.len(), 2);
        assert!(plan.targets.iter().any(|p| p.ends_with("latest.json")));
        assert!(
            plan.targets
                .iter()
                .any(|p| p.ends_with("offline_queue.json"))
        );
        assert!(!plan.targets.iter().any(|p| p.ends_with("unrelated.json")));
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
                    .load_checkpoint()
                    .expect("load checkpoint")
                    .is_none(),
                "normal launch should clear latest checkpoint after preserving it"
            );
            assert!(
                manager.load_session(&session_id).is_ok(),
                "normal launch should keep an explicit resume target"
            );
        });
    }

    #[test]
    fn continue_recovers_same_workspace_checkpoint() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
                    .load_checkpoint()
                    .expect("load checkpoint")
                    .is_none(),
                "--continue should consume the checkpoint"
            );
            assert!(manager.load_session(&session_id).is_ok());
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
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_MODEL",
            "NVIDIA_API_KEY",
            "NIM_BASE_URL",
            "RUST_LOG",
            "DEEPSEEK_APPROVAL_POLICY",
            "DEEPSEEK_SANDBOX_MODE",
        ] {
            assert!(
                keys.contains(required),
                ".env.example is missing {required}"
            );
        }

        let sources = [
            include_str!("config.rs"),
            include_str!("logging.rs"),
            include_str!("../../config/src/lib.rs"),
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    fn resolve_api_key_source_prefers_config_over_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
            !is_command_available("this-command-cannot-exist-deepseek-tui-test-ENOENT-marker"),
            "missing command should return false, not panic"
        );
    }
}
