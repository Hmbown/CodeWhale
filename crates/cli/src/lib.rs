#![allow(clippy::uninlined_format_args)]

mod cloud;
mod config_bundles;
mod credential_handoff;
mod dispatch;
mod metrics;
#[cfg(not(target_env = "ohos"))]
mod update;

use std::io::{self, IsTerminal, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use codewhale_agent::ModelRegistry;
use codewhale_app_server::daemon_socket::{DaemonSocketOptions, run_daemon_socket};
use codewhale_app_server::{
    AppServerOptions, run as run_app_server, run_stdio as run_app_server_stdio,
};
use codewhale_config::route::{ProvidersExport, parse_route_kind};
use codewhale_config::{
    CliRuntimeOverrides, ConfigApiKeyValueKind, ConfigStore, ConfigToml, ProviderKind,
    ProviderSource, ResolvedRuntimeOptions, RuntimeApiKeySource, SetupState,
    classify_config_api_key_value, provider_base_url_is_official,
};
use codewhale_execpolicy::{AskForApproval, ExecPolicyContext, ExecPolicyEngine};
use codewhale_mcp::{McpServerDefinition, run_stdio_server};
use codewhale_secrets::Secrets;
use codewhale_state::{StateStore, ThreadListFilters};
use codewhale_telemetry::{
    self as telemetry, Counters, DurationBucket, Errors, Event, ExitClass, SessionSource, Surface,
    TelemetryDecision, TurnWall,
};

/// Catalog-backed `--provider` parser. Replaces the closed 47-arm `ProviderArg` enum.
fn parse_catalog_route(value: &str) -> std::result::Result<ProviderKind, String> {
    parse_route_kind(value).ok_or_else(|| {
        format!(
            "unknown route '{value}'; expected a catalog route id (see `codewhale providers export --json`)"
        )
    })
}

fn builtin_provider_arg(value: &str) -> Option<ProviderKind> {
    parse_route_kind(value)
}

fn parse_provider_identifier(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || value == "__custom__"
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(
            "provider must be a simple identifier using letters, numbers, '-', '_', or '.'"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

#[derive(Debug, Parser)]
#[command(
    name = "codewhale",
    version = env!("CODEWHALE_BUILD_VERSION"),
    bin_name = "codewhale",
    override_usage = "codewhale [OPTIONS] [PROMPT]\n       codewhale [OPTIONS] <COMMAND> [ARGS]"
)]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    profile: Option<String>,
    #[arg(
        long,
        value_name = "PROVIDER",
        value_parser = parse_provider_identifier,
        help = "Provider selector; exec/fleet also accept configured custom provider identifiers"
    )]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long = "output-mode")]
    output_mode: Option<String>,
    #[arg(
        long = "verbosity",
        value_name = "LEVEL",
        help = "Controls transcript and output verbosity (normal, concise)"
    )]
    verbosity: Option<String>,
    #[arg(long = "log-level")]
    log_level: Option<String>,
    #[arg(
        long,
        value_name = "BOOL",
        help = "Control anonymous usage counting for this run (default on; \
                CODEWHALE_TELEMETRY=0 always wins)"
    )]
    telemetry: Option<bool>,
    #[arg(long)]
    approval_policy: Option<String>,
    #[arg(long)]
    sandbox_mode: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    /// Workspace directory for Codewhale file tools.
    #[arg(short = 'C', long = "workspace", alias = "cd", value_name = "DIR")]
    workspace: Option<PathBuf>,
    #[arg(long = "mouse-capture", conflicts_with = "no_mouse_capture")]
    mouse_capture: bool,
    #[arg(long = "no-mouse-capture", conflicts_with = "mouse_capture")]
    no_mouse_capture: bool,
    #[arg(long = "skip-onboarding")]
    skip_onboarding: bool,
    /// Skip loading project-level config, including the workspace-specific
    /// `[workspace]`/`[projects]` overlay from user config. Must appear before
    /// the subcommand; it is applied before subcommand dispatch.
    #[arg(long = "no-project-config")]
    no_project_config: bool,
    /// Legacy compatibility alias for Act + Full Access.
    #[arg(long, hide = true)]
    yolo: bool,
    /// Continue the most recent interactive session for this workspace.
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,
    /// Resume a saved interactive session by id or unique id prefix.
    #[arg(
        short = 'r',
        long = "resume",
        value_name = "SESSION_ID",
        conflicts_with_all = ["continue_session", "session_id"]
    )]
    resume: Option<String>,
    /// Alias of `--resume` matching `codewhale exec --session-id`.
    #[arg(
        long = "session-id",
        value_name = "SESSION_ID",
        conflicts_with_all = ["continue_session", "resume"]
    )]
    session_id: Option<String>,
    #[arg(short = 'p', long = "prompt", value_name = "PROMPT")]
    prompt_flag: Option<String>,
    #[arg(
        value_name = "PROMPT",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    prompt: Vec<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run an interactive or non-interactive task.
    Run(RunArgs),
    /// Run Codewhale diagnostics.
    Doctor(TuiPassthroughArgs),
    /// List live models from the selected provider.
    Models(TuiPassthroughArgs),
    /// Generate speech audio with Xiaomi MiMo TTS models.
    #[command(visible_alias = "tts")]
    Speech(TuiPassthroughArgs),
    /// List saved sessions.
    Sessions(TuiPassthroughArgs),
    /// Resume a saved session.
    Resume(TuiPassthroughArgs),
    /// Launch an interactive session and hand it to the Codewhale web app.
    Rc(TuiPassthroughArgs),
    /// Fork a saved session.
    Fork(TuiPassthroughArgs),
    /// Create a default AGENTS.md in the current directory.
    Init(TuiPassthroughArgs),
    /// Bootstrap MCP config and/or skills directories.
    Setup(TuiPassthroughArgs),
    /// Generate a remote Codewhale agent deploy bundle (cloud + chat bridge).
    RemoteSetup(RemoteSetupArgs),
    /// Run a non-interactive prompt.
    #[command(after_help = "\
Examples:
  codewhale exec \"explain this function\"
  codewhale exec --auto \"list crates/ with ls\"
  codewhale exec --auto --output-format stream-json \"fix the failing test\"

Common forwarded flags:
  --auto                           Enable tool-backed agent mode with auto-approvals
  --json                           Emit summary JSON
  --resume <SESSION_ID>            Resume a previous session by ID or prefix
  --session-id <SESSION_ID>        Resume a previous session by ID or prefix
  --continue                       Continue the most recent session for this workspace
  --output-format <FORMAT>         Output format: text or stream-json

Plain `codewhale exec` is a one-shot model response. Use `--auto` for
non-interactive filesystem/shell tool use, matching the supported automation
path used by stream-json wrappers.
")]
    Exec(TuiPassthroughArgs),
    /// Manage durable Agent fleet runs.
    #[command(
        name = "fleet",
        after_help = "\
Examples:
  codewhale fleet init
  codewhale fleet run tasks.json --max-workers 4
  codewhale fleet status

The durable ledger `.codewhale/fleet.jsonl`, saved rosters `fleets/<name>.toml`,
the `[fleet]` and `[fleets.*]` config tables, and `workflow run --fleet` keep
the Fleet name across versions."
    )]
    Fleet(TuiPassthroughArgs),
    /// Internal model-free Workflow tool dispatcher used by Lane Runtime.
    #[command(name = "workflow-tool", hide = true)]
    WorkflowTool(TuiPassthroughArgs),
    /// Internal detached-runtime output/receipt supervisor.
    #[command(name = "lane-log-proxy", hide = true)]
    LaneLogProxy(LaneLogProxyArgs),
    /// Run checked-in Workflows through a Lane Runtime backend.
    #[command(after_help = "\
Examples:
  codewhale workflow run stopship --fleet stopship --runtime tmux --goal verify-release-candidate
  codewhale workflow run stopship --fleet stopship --runtime inline --verify

`workflow run` validates the checked-in Workflow source and named Fleet roster,
creates a Lane record, then dispatches the Workflow tool directly through the
selected Runtime backend without an operator model turn.
")]
    Workflow(WorkflowArgs),
    /// Manage running workflow instances (Lanes) and Runtime backends (#4176).
    #[command(after_help = "\
Examples:
  codewhale lane list
  codewhale lane status <lane-id>
  codewhale lane attach <lane-id>
  codewhale lane logs <lane-id>
  codewhale lane interrupt <lane-id>
  codewhale lane interrupt <lane-id>@<lifecycle-seq>
  codewhale lane start --workflow stopship --fleet stopship --runtime tmux --goal verify-release-candidate -- echo hello

Lane records persist under $CODEWHALE_HOME/lanes/. tmux durability belongs to
Runtime, not Fleet.

list/status/interrupt/restart/resume share one control-plane contract with the
`/lane` slash command and its hotbar action: same verb ids, same availability,
same read-vs-write authority, same exact-identity target selection, and the
same receipt (`--json`). `lane stop` is a compatibility spelling of
`lane interrupt`. Appending `@<lifecycle-seq>` fences a write to the exact
lifecycle generation you observed.
")]
    Lane(LaneArgs),
    /// Run a Codewhale-powered code review over a git diff.
    Review(TuiPassthroughArgs),
    /// Apply a patch file or stdin to the working tree.
    Apply(TuiPassthroughArgs),
    /// Run the offline evaluation harness.
    Eval(TuiPassthroughArgs),
    /// Manage MCP servers.
    Mcp(TuiPassthroughArgs),
    /// Inspect feature flags.
    Features(TuiPassthroughArgs),
    /// Connect third-party harnesses through Codewhale (e.g. `integrations dsh status`).
    Integrations(TuiPassthroughArgs),
    /// Run a local Codewhale server.
    #[command(after_help = "\
Forwarded serve options:
      --mcp                 Start MCP server over stdio
      --http                Start runtime HTTP/SSE API server
      --mobile              Start runtime HTTP/SSE API server with the mobile control page
      --web                 Start the embedded loopback-only browser client
      --qr                  Show a QR code for the mobile URL (requires --mobile)
      --acp                 Start ACP server over stdio for editor clients
      --host <HOST>         Bind host (default 127.0.0.1; --mobile defaults to 0.0.0.0)
      --port <PORT>         Bind port [default: 7878]
      --workers <WORKERS>   Background task worker count (1-8)
      --cors-origin <URL>   Additional CORS origin to allow (repeatable)
      --auth-token <TOKEN>  Require this bearer token for /v1/* runtime API routes
      --insecure            Disable runtime API auth when no token is configured

`codewhale serve --http` and `codewhale serve --mobile` remain compatibility
aliases for `codewhale app-server --http` and `codewhale app-server --mobile`.
New integrations should prefer `codewhale app-server`.")]
    Serve(TuiPassthroughArgs),
    /// Open the first-class local browser client over the canonical Runtime API.
    #[command(
        after_help = "The browser receives a one-time loopback bootstrap capability, never the Runtime token.\nThe capability is exchanged for a bounded, process-local HttpOnly, SameSite=Strict web session and then invalidated."
    )]
    Web(WebArgs),
    /// Sign in to your Codewhale account (browser device flow).
    Login(LoginArgs),
    /// Remove saved authentication state.
    Logout,
    /// Manage authentication credentials and provider mode.
    Auth(AuthArgs),
    /// Sign in to your Codewhale account and manage account-scoped provider keys.
    #[command(visible_alias = "cloud")]
    Account(cloud::CloudArgs),
    /// Offload a coding agent to the Codewhale cloud. Never spends or pushes without --confirm.
    #[command(visible_alias = "cloud-agent")]
    Dispatch(dispatch::DispatchArgs),
    /// Run MCP server mode over stdio.
    McpServer,
    /// Read/write/list config values.
    Config(ConfigArgs),
    /// Resolve or list available models across providers.
    Model(ModelArgs),
    /// Manage thread/session metadata and resume/fork flows.
    Thread(ThreadArgs),
    /// Evaluate sandbox/approval policy decisions.
    Sandbox(SandboxArgs),
    /// Run the canonical runtime API / control plane (HTTP/SSE, mobile, stdio).
    #[command(after_help = "\
Transports:
  codewhale app-server --http              Full HTTP/SSE runtime API (/v1/*) on 127.0.0.1:7878
  codewhale app-server --mobile            Runtime API + phone control page (binds 0.0.0.0)
  codewhale app-server --stdio             JSON-RPC control transport over stdio (no listener)
  codewhale app-server                     Legacy in-process app-server HTTP on 127.0.0.1:8787

`--http` and `--mobile` serve the same mature runtime API as `codewhale serve
--http`/`--mobile`, which remain as compatibility aliases. The runtime API token
is read from --auth-token, CODEWHALE_RUNTIME_TOKEN, or DEEPSEEK_RUNTIME_TOKEN.

See docs/RUNTIME_API.md.")]
    AppServer(AppServerArgs),
    /// Generate shell completions.
    #[command(
        visible_alias = "completions",
        after_help = r#"Every script completes both `codewhale` and the `codew` shorthand.

Examples:
  Bash (current shell only):
    source <(codewhale completion bash)

  Bash (persistent, Linux/bash-completion):
    mkdir -p ~/.local/share/bash-completion/completions
    codewhale completion bash > ~/.local/share/bash-completion/completions/codewhale
    # Requires bash-completion to be installed and loaded by your shell.

  Zsh:
    mkdir -p ~/.zfunc
    codewhale completion zsh > ~/.zfunc/_codewhale
    # Add to ~/.zshrc if needed:
    #   fpath=(~/.zfunc $fpath)
    #   autoload -Uz compinit && compinit

  Fish:
    mkdir -p ~/.config/fish/completions
    codewhale completion fish > ~/.config/fish/completions/codewhale.fish

  PowerShell (current shell only):
    codewhale completion powershell | Out-String | Invoke-Expression

  PowerShell (persistent):
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PROFILE)
    codewhale completion powershell >> $PROFILE

  Elvish:
    codewhale completion elvish >> ~/.config/elvish/rc.elv

The command prints the completion script to stdout; redirect it to a path your shell loads automatically."#
    )]
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Print a usage rollup from the audit log and session store.
    Metrics(MetricsArgs),
    /// Check for and apply updates to the `codewhale` binary.
    Update(UpdateArgs),
    /// Export the route catalog (`providers export --json`).
    Providers(ProvidersArgs),
}

#[derive(Debug, Args)]
struct ProvidersArgs {
    #[command(subcommand)]
    command: ProvidersCommand,
}

#[derive(Debug, Subcommand)]
enum ProvidersCommand {
    /// Write the owned route catalog as JSON (cwc contract).
    Export {
        /// Required. The export is the generated cwc catalog source of truth.
        #[arg(long)]
        json: bool,
    },
}

/// The name of this crate's `[[bin]]` target, and the command users actually
/// type. Completion scripts must register *this*, not the in-tree
/// `codewhale-tui` binary that used to render them (#5526).
///
/// GitHub releases do not ship a separately compiled TUI: `release-artifacts.yml`
/// builds `-p codewhale-cli` and publishes `codewhale` plus a byte-identical
/// `codew` copy. The `codewhale-tui-*` filenames still attached to the release
/// are that same binary (a v0.9.4 updater bridge), not a third runtime.
const COMPLETION_BIN_NAME: &str = "codewhale";

/// Releases publish `codew` as a byte-identical copy of `codewhale`
/// (`release-artifacts.yml` copies the binary and `cmp`s it), so a completion
/// script that fires only for `codewhale` is half-installed for anyone who
/// types the short name.
const COMPLETION_ALIAS_NAME: &str = "codew";

/// Render the completion script for `shell` from this binary's own clap tree,
/// registered for both published command names.
fn render_completion_script(shell: Shell) -> String {
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    generate(shell, &mut cmd, COMPLETION_BIN_NAME, &mut buf);
    let script = String::from_utf8_lossy(&buf).into_owned();
    register_completion_alias(shell, script)
}

/// Extend a clap_complete script so the `codew` shorthand completes too.
///
/// Each shell gets its own idiomatic hook rather than a second copy of the
/// script: bash re-binds the generated function, zsh widens the `#compdef`
/// tag line, fish wraps the primary command, PowerShell registers an array
/// of command names, and Elvish aliases the completer map entry. `Shell` is
/// non-exhaustive, so any future variant falls through unchanged.
fn register_completion_alias(shell: Shell, script: String) -> String {
    let bin = COMPLETION_BIN_NAME;
    let alias = COMPLETION_ALIAS_NAME;
    match shell {
        Shell::Bash => format!(
            "{script}\n\
             if [[ \"${{BASH_VERSINFO[0]}}\" -eq 4 && \"${{BASH_VERSINFO[1]}}\" -ge 4 || \"${{BASH_VERSINFO[0]}}\" -gt 4 ]]; then\n    \
             complete -F _{bin} -o nosort -o bashdefault -o default {alias}\n\
             else\n    \
             complete -F _{bin} -o bashdefault -o default {alias}\n\
             fi\n"
        ),
        // Two install paths, two hooks. Autoloaded from `fpath` the tag line
        // on the first line is what binds the names; sourced directly, the
        // `compdef` call clap emits at the bottom is. Cover both, and reuse
        // clap's own `funcstack` guard so the appended call is skipped when
        // the body runs as the completion function itself.
        Shell::Zsh => {
            let tagged = match script.strip_prefix(&format!("#compdef {bin}\n")) {
                Some(rest) => format!("#compdef {bin} {alias}\n{rest}"),
                None => script,
            };
            format!(
                "{tagged}\nif [ \"$funcstack[1]\" != \"_{bin}\" ]; then\n    \
                 compdef _{bin} {alias}\n\
                 fi\n"
            )
        }
        Shell::Fish => format!("{script}\ncomplete -c {alias} -w {bin}\n"),
        Shell::PowerShell => script.replacen(
            &format!("-CommandName '{bin}'"),
            &format!("-CommandName '{bin}','{alias}'"),
            1,
        ),
        Shell::Elvish => format!(
            "{script}\n\
             set edit:completion:arg-completer[{alias}] = $edit:completion:arg-completer[{bin}]\n"
        ),
        _ => script,
    }
}

fn command_accepts_raw_provider(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::Exec(_) | Commands::Fleet(_)))
}

fn top_level_provider_override(
    provider: Option<&str>,
    command: Option<&Commands>,
) -> Result<Option<ProviderKind>> {
    let Some(provider) = provider else {
        return Ok(None);
    };
    if let Some(provider) = builtin_provider_arg(provider) {
        return Ok(Some(provider));
    }
    if command_accepts_raw_provider(command) {
        return Ok(None);
    }

    let expected = ProviderKind::names_hint();
    bail!(
        "invalid value '{provider}' for '--provider <PROVIDER>': expected one of {expected}; configured custom providers are accepted only by exec and fleet"
    )
}

fn prepare_raw_provider_tui_dispatch(
    cli: &Cli,
    command: Option<&Commands>,
    runtime_overrides: &CliRuntimeOverrides,
) -> Result<Option<(ResolvedRuntimeOptions, Vec<String>)>> {
    let Some(provider) = cli.provider.as_deref() else {
        return Ok(None);
    };
    if builtin_provider_arg(provider).is_some() || !command_accepts_raw_provider(command) {
        return Ok(None);
    }

    let passthrough = match command {
        Some(Commands::Exec(args)) => {
            reject_exec_global_flags(&args.args)?;
            tui_args("exec", args.clone())
        }
        Some(Commands::Fleet(args)) => tui_args("fleet", args.clone()),
        _ => unreachable!("raw provider validation only permits Exec and Fleet"),
    };

    // Dynamic provider config belongs to the TUI schema. Do not parse it
    // through the dispatcher's enum-backed ConfigStore or recover credentials
    // for an unrelated fallback provider before the TUI sees the raw id.
    let resolved_runtime = ConfigToml::default().resolve_runtime_options(runtime_overrides);
    Ok(Some((resolved_runtime, passthrough)))
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Update to the latest beta release instead of the latest stable release.
    #[arg(long)]
    beta: bool,
    /// Only check the latest release; do not download or replace binaries.
    #[arg(long)]
    check: bool,
    /// Proxy URL to use for update HTTP requests.
    #[arg(long, value_name = "URL")]
    proxy: Option<String>,
}

#[derive(Debug, Args)]
struct MetricsArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
    /// Restrict to events newer than this duration (e.g. 7d, 24h, 30m, now-2h).
    #[arg(long, value_name = "DURATION")]
    since: Option<String>,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct TuiPassthroughArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Args)]
struct WebArgs {
    /// Loopback port for the local Runtime API and embedded client.
    #[arg(long, default_value_t = 7878)]
    port: u16,
}

#[derive(Debug, Args)]
struct LaneLogProxyArgs {
    #[arg(long, value_name = "PATH")]
    log_path: PathBuf,
    #[arg(long, value_name = "PATH")]
    receipt_path: PathBuf,
    #[arg(long, value_name = "PATH")]
    receipt_tmp_path: PathBuf,
    #[arg(long, value_name = "PATH")]
    environment_path: Option<PathBuf>,
    #[arg(long)]
    lane_id: String,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    command: Vec<String>,
}

/// `codewhale lane …` — running workflow instances (#4176).
#[derive(Debug, Args)]
struct LaneArgs {
    #[command(subcommand)]
    command: LaneCommand,
}

#[derive(Debug, Subcommand)]
// Clap constructs this command enum once at process startup. Keeping the
// fields inline makes the generated CLI shape explicit; boxing them only to
// reduce this transient value would add indirection without runtime benefit.
#[allow(clippy::large_enum_variant)]
enum LaneCommand {
    /// List known lanes (newest first).
    List {
        /// Emit JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show one lane's status and attach metadata.
    Status {
        /// Lane id (e.g. `lane-a1b2c3d4`).
        lane_id: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Attach to a tmux-backed lane (prints attach command; execs when possible).
    Attach {
        lane_id: String,
        /// Only print the attach command; do not exec.
        #[arg(long, default_value_t = false)]
        print: bool,
    },
    /// Tail the lane stream-json / NDJSON journal.
    Logs {
        lane_id: String,
        /// Follow the log file (like `tail -f`).
        #[arg(long, short = 'f', default_value_t = false)]
        follow: bool,
        /// Number of trailing lines when not following (default 50).
        #[arg(long, default_value_t = 50)]
        tail: usize,
    },
    /// Stop a running lane and run worktree TTL cleanup.
    ///
    /// Compatibility spelling for `lane interrupt`; both resolve to the
    /// `lane.interrupt` control-plane verb (#1888).
    Stop { lane_id: String },
    /// Interrupt a running lane (durable `lane.interrupt`).
    ///
    /// Accepts an exact lane id, optionally fenced as `<lane-id>@<seq>` so the
    /// stop only applies to the lifecycle generation you observed.
    Interrupt {
        lane_id: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Restart a lane in place (declared, no backend — reports why).
    Restart {
        lane_id: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Resume a stopped lane (declared, no backend — reports why).
    Resume {
        lane_id: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Start a lane under a Runtime backend (tmux|inline|vm|ci).
    Start {
        /// Workflow name (e.g. `stopship`).
        #[arg(long)]
        workflow: Option<String>,
        /// Fleet roster name (e.g. `stopship`); the flag keeps its compatibility spelling.
        #[arg(long)]
        fleet: Option<String>,
        /// Issue id binding.
        #[arg(long)]
        issue: Option<String>,
        /// Free-form goal text.
        #[arg(long)]
        goal: Option<String>,
        /// Runtime backend: tmux, inline, vm, or ci.
        #[arg(long, default_value = "tmux")]
        runtime: String,
        /// Create an isolated worktree under this repo root.
        #[arg(long, value_name = "DIR")]
        worktree_repo: Option<PathBuf>,
        /// Branch name for the worktree (requires `--worktree-repo`).
        #[arg(long)]
        branch: Option<String>,
        /// Worktree path (defaults to `<repo>/.codewhale/lanes/<lane-id>`).
        #[arg(long, value_name = "DIR")]
        worktree_path: Option<PathBuf>,
        /// Worktree cleanup TTL seconds after stop (0 = immediate on stop).
        #[arg(long)]
        worktree_ttl_secs: Option<u64>,
        /// Command to run in the runtime (after `--`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

/// `codewhale workflow …` — Workflow entrypoints backed by Lanes (#4177/#4178).
#[derive(Debug, Args)]
struct WorkflowArgs {
    #[command(subcommand)]
    command: WorkflowCommand,
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    /// Run a checked-in Workflow through a Runtime-backed Lane.
    Run {
        /// Workflow name or path. `stopship` maps to workflows/stopship.workflow.js.
        workflow: String,
        /// Named Fleet roster (e.g. stopship). The flag keeps its compatibility
        /// spelling. Without one, roles resolve against the built-in roster
        /// and the session route.
        #[arg(long)]
        fleet: Option<String>,
        /// Issue id binding recorded on the Lane and passed into workflow args.
        #[arg(long)]
        issue: Option<String>,
        /// Free-form goal text recorded on the Lane and passed into workflow args.
        #[arg(long)]
        goal: Option<String>,
        /// Runtime backend: tmux, inline, vm, or ci.
        #[arg(long, default_value = "tmux")]
        runtime: String,
        /// Explicit Workflow source path, overriding name-based resolution.
        #[arg(long, value_name = "PATH")]
        source_path: Option<PathBuf>,
        /// Optional shared Workflow token budget.
        #[arg(long)]
        token_budget: Option<u64>,
        /// Run verifier gates after a successful Workflow completion.
        #[arg(long, default_value_t = false)]
        verify: bool,
        /// Create an isolated worktree under this repo root.
        #[arg(long, value_name = "DIR")]
        worktree_repo: Option<PathBuf>,
        /// Branch name for the worktree (requires `--worktree-repo`).
        #[arg(long)]
        branch: Option<String>,
        /// Worktree path (defaults to `<repo>/.codewhale/lanes/<lane-id>`).
        #[arg(long, value_name = "DIR")]
        worktree_path: Option<PathBuf>,
        /// Worktree cleanup TTL seconds after stop (0 = immediate on stop).
        #[arg(long)]
        worktree_ttl_secs: Option<u64>,
    },
}

struct LaneStartRequest {
    workflow: Option<String>,
    fleet: Option<String>,
    issue: Option<String>,
    goal: Option<String>,
    runtime: String,
    worktree_repo: Option<PathBuf>,
    branch: Option<String>,
    worktree_path: Option<PathBuf>,
    worktree_ttl_secs: Option<u64>,
    command: Vec<String>,
    environment: Vec<(String, String)>,
    cwd: Option<PathBuf>,
}

fn start_lane(request: LaneStartRequest) -> Result<()> {
    use codewhale_lane::{
        LaneRegistry, LaneStartSpec, RuntimeBackendKind, WorktreeProvision, resolve_backend,
    };

    let LaneStartRequest {
        workflow,
        fleet,
        issue,
        goal,
        runtime,
        worktree_repo,
        branch,
        worktree_path,
        worktree_ttl_secs,
        command,
        environment,
        cwd,
    } = request;
    let kind = RuntimeBackendKind::parse(&runtime)?;
    let reg = LaneRegistry::open_default()?;
    let mut record = reg.create_pending(workflow, fleet, issue, goal, kind, worktree_ttl_secs)?;
    let worktree = match (worktree_repo, branch) {
        (Some(repo_root), Some(branch_name)) => {
            let path = worktree_path
                .unwrap_or_else(|| repo_root.join(".codewhale").join("lanes").join(&record.id));
            Some(WorktreeProvision {
                repo_root,
                branch: branch_name,
                path,
                base_ref: None,
            })
        }
        (None, None) => None,
        _ => bail!("--worktree-repo and --branch must be provided together"),
    };
    let cmd = if command.is_empty() {
        vec![
            "sh".into(),
            "-c".into(),
            format!("echo lane {} started", record.id),
        ]
    } else {
        command
    };
    let spec = LaneStartSpec {
        command: cmd,
        cwd,
        environment,
        log_proxy: (kind == RuntimeBackendKind::Tmux)
            .then(std::env::current_exe)
            .transpose()
            .context("resolve current Codewhale executable for tmux log proxy")?,
        worktree,
    };
    let backend = resolve_backend(kind);
    backend.start(&reg, &mut record, &spec)?;
    println!("started {}", record.id);
    println!("status:  {}", record.status.as_str());
    println!("runtime: {}", record.runtime.as_str());
    println!("log:     {}", record.log_path.display());
    if let Some(attach) = backend.attach_command(&record) {
        println!("attach:  {attach}");
    }
    Ok(())
}

/// Print one shared control receipt on the CLI surface.
///
/// The CLI does not format Lane control results itself: it renders the same
/// [`codewhale_lane::ControlReceipt`] the slash command and hotbar render, so
/// the three surfaces cannot drift in what they report (#1888).
fn emit_control_receipt(receipt: &codewhale_lane::ControlReceipt, json: bool) -> Result<()> {
    if json {
        // v0.9.2 compatibility: `lane list --json` has always emitted an array
        // of `LaneRecord`, and `lane status --json` a single one. Scripts
        // select `.[].id`, `.worktree_path`, `.log_path` off that shape, so the
        // receipt does not replace it. The receipt is what every other verb
        // emits, and what the human renderer shows for these two.
        match receipt.operation {
            codewhale_lane::ControlOperation::LaneList => {
                println!("{}", serde_json::to_string_pretty(&receipt.lane_records)?);
            }
            codewhale_lane::ControlOperation::LaneStatus => match receipt.lane_records.first() {
                Some(record) => println!("{}", serde_json::to_string_pretty(record)?),
                // Legacy behaviour for an unknown id: `reg.load()` failed, so
                // the command errored on stderr and printed *nothing* on
                // stdout. Emitting a receipt (or a bare `null`) here would make
                // `lane status --json <bad-id> | jq` succeed where it used to
                // fail. Stay silent and let the bail! below set the exit code.
                None if receipt.is_error() => {}
                None => println!("{}", serde_json::to_string_pretty(receipt)?),
            },
            _ => println!("{}", serde_json::to_string_pretty(receipt)?),
        }
    } else if receipt.is_error() {
        eprintln!("{}", receipt.render());
    } else {
        println!("{}", receipt.render());
    }
    if receipt.is_error() {
        let detail = receipt
            .failure
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| receipt.outcome.as_str().to_string());
        bail!("{}: {detail}", receipt.operation_id);
    }
    Ok(())
}

fn run_lane_control(
    operation: codewhale_lane::ControlOperation,
    lane_id: Option<&str>,
    json: bool,
) -> Result<()> {
    let receipt = codewhale_lane::control::execute_lane_control(
        codewhale_lane::ControlSurface::Cli,
        operation,
        lane_id,
    );
    emit_control_receipt(&receipt, json)
}

fn run_lane_command(args: LaneArgs) -> Result<()> {
    use codewhale_lane::{ControlOperation, LaneRegistry, backend_for};
    use std::io::{BufRead, Seek, Write};
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    match args.command {
        LaneCommand::List { json } => run_lane_control(ControlOperation::LaneList, None, json),
        LaneCommand::Status { lane_id, json } => {
            run_lane_control(ControlOperation::LaneStatus, Some(&lane_id), json)
        }
        LaneCommand::Interrupt { lane_id, json } => {
            run_lane_control(ControlOperation::LaneInterrupt, Some(&lane_id), json)
        }
        LaneCommand::Restart { lane_id, json } => {
            run_lane_control(ControlOperation::LaneRestart, Some(&lane_id), json)
        }
        LaneCommand::Resume { lane_id, json } => {
            run_lane_control(ControlOperation::LaneResume, Some(&lane_id), json)
        }
        LaneCommand::Attach { lane_id, print } => {
            let reg = LaneRegistry::open_default()?;
            let mut lane = reg.load(&lane_id)?;
            let backend = backend_for(&lane);
            backend.reconcile(&reg, &mut lane)?;
            let Some(attach) = backend.attach_command(&lane) else {
                if !lane.status.is_active() {
                    bail!(
                        "lane `{lane_id}` is {} and has no active attach target",
                        lane.status.as_str()
                    );
                }
                bail!(
                    "lane `{lane_id}` runtime `{}` has no attach target",
                    lane.runtime.as_str()
                );
            };
            if print {
                println!("{attach}");
                return Ok(());
            }
            if let Some(session) = lane.tmux_session.as_deref() {
                let socket = lane
                    .tmux_socket
                    .as_deref()
                    .context("tmux lane is missing its pinned server socket")?;
                let status = Command::new("tmux")
                    .arg("-S")
                    .arg(socket)
                    .args(["attach", "-t", session])
                    .status();
                match status {
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => bail!("tmux attach failed ({s}); command was: {attach}"),
                    Err(err) => {
                        eprintln!("could not exec tmux: {err}");
                        println!("{attach}");
                        bail!("tmux attach unavailable");
                    }
                }
            } else {
                println!("{attach}");
                Ok(())
            }
        }
        LaneCommand::Logs {
            lane_id,
            follow,
            tail,
        } => {
            let reg = LaneRegistry::open_default()?;
            let lane = reg.load(&lane_id)?;
            let path = lane.log_path;
            if !path.exists() {
                bail!("log file missing: {}", path.display());
            }
            let content = std::fs::read(&path)?;
            let lines: Vec<&[u8]> = content
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .collect();
            let start = lines.len().saturating_sub(tail);
            let mut stdout = std::io::stdout().lock();
            for line in &lines[start..] {
                stdout.write_all(String::from_utf8_lossy(line).as_bytes())?;
                stdout.write_all(b"\n")?;
            }
            stdout.flush()?;
            if !follow {
                return Ok(());
            }
            let mut file = std::fs::File::open(&path)?;
            file.seek(std::io::SeekFrom::End(0))?;
            let mut reader = std::io::BufReader::new(file);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => {
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    Ok(_) => {
                        let mut stdout = std::io::stdout().lock();
                        stdout.write_all(String::from_utf8_lossy(&line).as_bytes())?;
                        stdout.flush()?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        // `stop` is the historical spelling of `interrupt`. Both go through
        // the same verb so the durable transition, the lifecycle fence, and
        // the receipt are identical.
        LaneCommand::Stop { lane_id } => {
            run_lane_control(ControlOperation::LaneInterrupt, Some(&lane_id), false)
        }
        LaneCommand::Start {
            workflow,
            fleet,
            issue,
            goal,
            runtime,
            worktree_repo,
            branch,
            worktree_path,
            worktree_ttl_secs,
            command,
        } => start_lane(LaneStartRequest {
            workflow,
            fleet,
            issue,
            goal,
            runtime,
            worktree_repo,
            branch,
            worktree_path,
            worktree_ttl_secs,
            command,
            environment: Vec::new(),
            cwd: None,
        }),
    }
}

fn run_lane_log_proxy_command(args: LaneLogProxyArgs) -> Result<()> {
    let exit_code = codewhale_lane::run_lane_log_proxy(codewhale_lane::LaneLogProxySpec {
        command: args.command,
        log_path: args.log_path,
        receipt_path: args.receipt_path,
        receipt_tmp_path: args.receipt_tmp_path,
        environment_path: args.environment_path,
        lane_id: args.lane_id,
    })?;
    std::process::exit(exit_code);
}

fn run_workflow_command(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    config_path: &Path,
    args: WorkflowArgs,
) -> Result<()> {
    match args.command {
        WorkflowCommand::Run {
            workflow,
            fleet,
            issue,
            goal,
            runtime,
            source_path,
            token_budget,
            verify,
            worktree_repo,
            branch,
            worktree_path,
            worktree_ttl_secs,
        } => {
            let workspace = workflow_workspace_root(cli.workspace.as_deref())?;
            let source_path =
                resolve_workflow_source_path(&workflow, source_path.as_ref(), &workspace)?;
            validate_workflow_source_file(&source_path)?;

            let source_root = if let Some(repo) = worktree_repo.as_deref() {
                repo.canonicalize()
                    .with_context(|| format!("resolve --worktree-repo {}", repo.display()))?
            } else {
                workspace.clone()
            };

            // A fleet is an optional pin layer, not a requirement: role-only
            // tasks resolve against the built-in roster and the session route
            // (matching the TUI tool path). When a fleet IS given, it is
            // loaded and validated before the run starts.
            if let Some(name) = fleet.as_deref() {
                let roots = named_fleet_search_roots(&workspace);
                let loaded =
                    codewhale_workflow::load_named_fleet(name, &roots).with_context(|| {
                        format!("load Fleet `{name}` from {}", display_roots(&roots))
                    })?;
                if workflow == "stopship" || name == "stopship" {
                    loaded
                        .validate_stopship_roles()
                        .with_context(|| format!("validate stopship roles in Fleet `{name}`"))?;
                }
            }

            let process = workflow_exec_command(WorkflowExecSpec {
                cli,
                resolved_runtime,
                config_path,
                source_root: &source_root,
                source_path: &source_path,
                workflow: &workflow,
                fleet: fleet.as_deref(),
                issue: issue.as_deref(),
                goal: goal.as_deref(),
                token_budget,
                verify,
            })?;
            start_lane(LaneStartRequest {
                workflow: Some(workflow),
                fleet,
                issue,
                goal,
                runtime,
                worktree_repo,
                branch,
                worktree_path,
                worktree_ttl_secs,
                command: process.command,
                environment: process.environment,
                cwd: Some(workspace),
            })
        }
    }
}

fn workflow_workspace_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return path
            .canonicalize()
            .with_context(|| format!("resolve workflow workspace {}", path.display()));
    }
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .output();
    if let Ok(output) = output
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let root = text.trim();
        if !root.is_empty() {
            let root = PathBuf::from(root);
            return Ok(root.canonicalize().unwrap_or(root));
        }
    }
    Ok(cwd)
}

fn resolve_workflow_source_path(
    workflow: &str,
    source_path: Option<&PathBuf>,
    workspace: &Path,
) -> Result<PathBuf> {
    let candidates = workflow_source_candidates(workflow, source_path, workspace);
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    bail!(
        "workflow source for `{workflow}` not found; tried {}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn workflow_source_candidates(
    workflow: &str,
    source_path: Option<&PathBuf>,
    workspace: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = source_path {
        candidates.push(resolve_against_workspace(path, workspace));
        return candidates;
    }

    let raw = workflow.trim();
    let workflow_path = PathBuf::from(raw);
    if raw.contains('/') || raw.contains('\\') || raw.ends_with(".js") || raw.ends_with(".ts") {
        candidates.push(resolve_against_workspace(&workflow_path, workspace));
        return candidates;
    }

    let normalized = raw.replace('-', "_");
    for rel in [
        format!("workflows/{raw}.workflow.js"),
        format!("workflows/{normalized}.workflow.js"),
    ] {
        let path = workspace.join(rel);
        if !candidates.iter().any(|existing| existing == &path) {
            candidates.push(path);
        }
    }
    candidates
}

fn resolve_against_workspace(path: &Path, workspace: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn validate_workflow_source_file(path: &Path) -> Result<()> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if source.trim_start().starts_with("export default workflow(")
        || source.trim_start().starts_with("workflow(")
        || source.contains("\nworkflow(")
    {
        let identifier = path.display().to_string();
        if path.extension().and_then(|ext| ext.to_str()) == Some("ts") {
            codewhale_workflow::compile_typescript_workflow(&identifier, &source)
                .with_context(|| format!("parse declarative Workflow {}", path.display()))?;
        } else {
            codewhale_workflow::compile_javascript_workflow(&identifier, &source)
                .with_context(|| format!("parse declarative Workflow {}", path.display()))?;
        }
    }
    Ok(())
}

fn named_fleet_search_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = codewhale_config::codewhale_home() {
        roots.push(home);
    }
    roots.push(workspace.to_path_buf());
    roots
}

fn display_roots(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

struct WorkflowExecSpec<'a> {
    cli: &'a Cli,
    resolved_runtime: &'a ResolvedRuntimeOptions,
    config_path: &'a Path,
    source_root: &'a Path,
    source_path: &'a Path,
    workflow: &'a str,
    fleet: Option<&'a str>,
    issue: Option<&'a str>,
    goal: Option<&'a str>,
    token_budget: Option<u64>,
    verify: bool,
}

struct WorkflowProcessSpec {
    command: Vec<String>,
    environment: Vec<(String, String)>,
}

fn workflow_exec_command(spec: WorkflowExecSpec<'_>) -> Result<WorkflowProcessSpec> {
    let WorkflowExecSpec {
        cli,
        resolved_runtime,
        config_path,
        source_root,
        source_path,
        workflow,
        fleet,
        issue,
        goal,
        token_budget,
        verify,
    } = spec;
    let source_arg = source_path
        .strip_prefix(source_root)
        .with_context(|| {
            format!(
                "workflow source {} must be inside execution root {}",
                source_path.display(),
                source_root.display()
            )
        })?
        .display()
        .to_string();
    let mut payload = serde_json::json!({
        "action": "run",
        "source_path": source_arg,
        "fleet": fleet,
        "args": {
            "workflow": workflow,
            "fleet": fleet,
            "issue": issue,
            "goal": goal,
        },
        "verify": verify,
    });
    if let Some(token_budget) = token_budget {
        payload["token_budget"] = serde_json::json!(token_budget);
    }
    let input_json = serde_json::to_string(&payload)?;
    let passthrough = vec![
        "workflow-tool".to_string(),
        "--approval-source".to_string(),
        "explicit-workflow-command".to_string(),
        "--input-json".to_string(),
        input_json,
    ];
    let argv = {
        // Build argv with explicit config path like the previous dispatcher did.
        let mut args = Vec::new();
        let executable = std::env::current_exe()
            .context("resolve current Codewhale executable for workflow lane")?;
        let executable = executable.into_os_string().into_string().map_err(|path| {
            anyhow!(
                "current Codewhale executable path is not valid UTF-8: {}",
                PathBuf::from(path).display()
            )
        })?;
        args.push(executable);
        // config_path is the explicit workflow config path; prefer it over cli.config
        let cfg = Some(config_path);
        if let Some(cp) = cfg {
            args.push("--config".to_string());
            args.push(cp.display().to_string());
        } else if let Some(cp) = cli.config.as_deref() {
            args.push("--config".to_string());
            args.push(cp.display().to_string());
        }
        if let Some(profile) = cli.profile.as_ref() {
            args.push("--profile".to_string());
            args.push(profile.clone());
        }

        if cli.mouse_capture {
            args.push("--mouse-capture".to_string());
        }
        if cli.no_mouse_capture {
            args.push("--no-mouse-capture".to_string());
        }
        if cli.skip_onboarding {
            args.push("--skip-onboarding".to_string());
        }
        if cli.no_project_config {
            args.push("--no-project-config".to_string());
        }
        args.extend(passthrough.clone());
        args
    };
    apply_tui_env(cli, resolved_runtime, &passthrough);
    lane_process_spec_from_argv(&argv)
}

fn valid_lane_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn shell_owned_lane_environment(key: &str) -> bool {
    matches!(
        key,
        "PWD" | "OLDPWD" | "SHLVL" | "_" | "TERM" | "TMUX" | "TMUX_PANE"
    )
}

fn lane_process_spec_from_argv(argv: &[String]) -> Result<WorkflowProcessSpec> {
    let mut environment = std::collections::BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        if valid_lane_environment_key(key) && !shell_owned_lane_environment(key) {
            environment.insert(key.to_string(), value.to_string());
        }
    }
    Ok(WorkflowProcessSpec {
        command: argv.to_vec(),
        environment: environment.into_iter().collect(),
    })
}

/// Flags for `codewhale remote-setup`. Forwarded to the TUI binary, which owns
/// the interactive wizard and bundle generation.
#[derive(Debug, Args, Clone, Default)]
struct RemoteSetupArgs {
    /// Cloud target slug (lighthouse, azure, digitalocean). Skips the prompt.
    #[arg(long)]
    cloud: Option<String>,
    /// Chat bridge slug (feishu, telegram). Skips the prompt.
    #[arg(long)]
    bridge: Option<String>,
    /// Provider slug; validated against the provider registry. Skips the prompt.
    #[arg(long)]
    provider: Option<String>,
    /// Bundle output directory (default `./codewhale-deploy/<cloud>-<bridge>`).
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// Emit the bundle, do not provision (default).
    #[arg(long, default_value_t = false)]
    generate_only: bool,
    /// Run the cloud CLI to auto-provision (not yet implemented).
    #[arg(long, default_value_t = false, conflicts_with = "generate_only")]
    apply: bool,
    /// Skip the final confirmation gate (CI / non-interactive).
    #[arg(long, default_value_t = false)]
    yes: bool,
    /// Fail instead of prompting if any required value is missing.
    #[arg(long, default_value_t = false)]
    non_interactive: bool,
}

/// Build the forwarded argv for the TUI `remote-setup` subcommand from the
/// structured CLI flags. Mirrors the named flags exactly so the TUI clap parser
/// re-derives the same `RemoteSetupArgs`.
fn remote_setup_tui_args(args: RemoteSetupArgs) -> Vec<String> {
    let mut forwarded = vec!["remote-setup".to_string()];
    if let Some(cloud) = args.cloud {
        forwarded.push("--cloud".to_string());
        forwarded.push(cloud);
    }
    if let Some(bridge) = args.bridge {
        forwarded.push("--bridge".to_string());
        forwarded.push(bridge);
    }
    if let Some(provider) = args.provider {
        forwarded.push("--provider".to_string());
        forwarded.push(provider);
    }
    if let Some(out) = args.out {
        forwarded.push("--out".to_string());
        forwarded.push(out.to_string_lossy().into_owned());
    }
    if args.generate_only {
        forwarded.push("--generate-only".to_string());
    }
    if args.apply {
        forwarded.push("--apply".to_string());
    }
    if args.yes {
        forwarded.push("--yes".to_string());
    }
    if args.non_interactive {
        forwarded.push("--non-interactive".to_string());
    }
    forwarded
}

#[derive(Debug, Args)]
struct LoginArgs {
    /// Print the verification URL without trying to open a browser.
    #[arg(long, default_value_t = false)]
    no_open: bool,
    /// Maximum time to wait for browser authorization.
    #[arg(
        long = "timeout-seconds",
        default_value_t = cloud::DEFAULT_LOGIN_TIMEOUT_SECONDS,
        value_parser = clap::value_parser!(u64).range(1..=cloud::MAX_LOGIN_TIMEOUT_SECONDS)
    )]
    timeout_seconds: u64,
    /// Legacy provider-key flag: rejected with a redirect to `auth set`.
    #[arg(long, hide = true)]
    api_key: Option<String>,
    /// Legacy provider flag: rejected with a redirect to `auth set`.
    #[arg(long, value_parser = parse_catalog_route, hide = true)]
    provider: Option<ProviderKind>,
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Sign in to xAI/Grok with an SSH-friendly device code.
    #[command(name = "xai-device")]
    XaiDevice,
    /// Sign in with ChatGPT for Codex subscription access (PKCE loopback).
    #[command(name = "chatgpt")]
    Chatgpt,
    /// Revoke Codewhale-owned ChatGPT tokens. Codex CLI consent is unchanged.
    #[command(name = "chatgpt-revoke")]
    ChatgptRevoke,
    /// Explicitly allow read-only access to one credential file owned by
    /// another CLI. Managed mutation is currently unsupported and fails closed.
    #[command(name = "external-consent")]
    ExternalConsent {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
        #[arg(long, value_enum)]
        mode: ExternalCredentialModeArg,
        /// Exact credential file path. Defaults to the selected CLI's resolved
        /// path without probing whether the file exists.
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
        /// Confirm the disclosed exact read-only grant without an interactive
        /// prompt. Required when stdin is not a terminal.
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// Revoke access to another CLI's credential file for one provider.
    #[command(name = "external-revoke")]
    ExternalRevoke {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
    },
    /// Show current provider and runtime-effective credential route state.
    /// Without `--provider`, shows all known providers.
    /// With `--provider`, shows detailed status for that provider.
    Status {
        /// Show status for a specific provider only.
        #[arg(long, value_parser = parse_catalog_route)]
        provider: Option<ProviderKind>,
        /// Report resolved home/config/settings/backend paths and structural
        /// credential-source presence without printing credential values or
        /// probing provider credential stores.
        #[arg(long, default_value_t = false)]
        diagnostic: bool,
    },
    /// Save an API key to the shared user config file. Reads from
    /// `--api-key`, `--api-key-stdin`, or prompts on stdin when
    /// neither is given. Does not echo the key.
    Set {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
        /// Inline value (discouraged — appears in shell history).
        #[arg(long)]
        api_key: Option<String>,
        /// Read the key from stdin instead of prompting.
        #[arg(long = "api-key-stdin", default_value_t = false)]
        api_key_stdin: bool,
    },
    /// Report the effective credential route for a provider. Never prints a
    /// credential; reports the source layer or structural OAuth/repair state.
    Get {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
    },
    /// Pipe the runtime-effective API key to a local client; refuses terminals.
    PrintApiKey {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
    },
    /// Delete a provider's key from config and secret-store storage.
    Clear {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: ProviderKind,
    },
    /// List all known providers with their runtime-effective auth state,
    /// without revealing credentials.
    List,
    /// Advanced: migrate config-file keys into a platform credential store.
    #[command(hide = true)]
    Migrate {
        /// Don't actually write anything; print what would change.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExternalCredentialModeArg {
    ReadOnly,
    Managed,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    List,
    Path,
    /// Import a portable config bundle from a file, HTTPS URL, or stdin (-).
    Import(config_bundles::ImportArgs),
    /// Export a portable, secret-free config bundle.
    Export(config_bundles::ExportArgs),
}

#[derive(Debug, Args)]
struct ModelArgs {
    #[command(subcommand)]
    command: ModelCommand,
}

#[derive(Debug, Subcommand)]
enum ModelCommand {
    List {
        #[arg(long, value_parser = parse_catalog_route)]
        provider: Option<ProviderKind>,
    },
    Resolve {
        model: Option<String>,
        #[arg(long, value_parser = parse_catalog_route)]
        provider: Option<ProviderKind>,
    },
    /// Set the default model (e.g. "pro", "flash", "deepseek-v4-pro").
    Set { model: String },
}

#[derive(Debug, Args)]
struct ThreadArgs {
    #[command(subcommand)]
    command: ThreadCommand,
}

#[derive(Debug, Subcommand)]
enum ThreadCommand {
    List {
        #[arg(long, default_value_t = false)]
        all: bool,
        #[arg(long)]
        limit: Option<usize>,
    },
    Read {
        thread_id: String,
    },
    Resume {
        thread_id: String,
    },
    Fork {
        thread_id: String,
    },
    Archive {
        thread_id: String,
    },
    Unarchive {
        thread_id: String,
    },
    SetName {
        thread_id: String,
        name: String,
    },
    /// Remove the custom name from a thread, restoring the default
    /// `(unnamed)` rendering in `thread list`.
    ClearName {
        thread_id: String,
    },
}

#[derive(Debug, Args)]
struct SandboxArgs {
    #[command(subcommand)]
    command: SandboxCommand,
}

#[derive(Debug, Subcommand)]
enum SandboxCommand {
    Check {
        command: String,
        #[arg(long, value_enum, default_value_t = ApprovalModeArg::OnRequest)]
        ask: ApprovalModeArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ApprovalModeArg {
    UnlessTrusted,
    OnFailure,
    OnRequest,
    Never,
}

impl From<ApprovalModeArg> for AskForApproval {
    fn from(value: ApprovalModeArg) -> Self {
        match value {
            ApprovalModeArg::UnlessTrusted => AskForApproval::UnlessTrusted,
            ApprovalModeArg::OnFailure => AskForApproval::OnFailure,
            ApprovalModeArg::OnRequest => AskForApproval::OnRequest,
            ApprovalModeArg::Never => AskForApproval::Never,
        }
    }
}

#[derive(Debug, Args)]
struct AppServerArgs {
    /// Serve the full HTTP/SSE runtime API (`/v1/*`: sessions, threads, turns,
    /// approvals, events, usage, fleet, tasks). This is the canonical runtime
    /// API surface; it delegates to the same server as `codewhale serve --http`.
    #[arg(long, conflicts_with_all = ["stdio", "mobile"])]
    http: bool,
    /// Serve the runtime API plus the phone-friendly mobile control page.
    /// Equivalent to the legacy `codewhale serve --mobile`.
    #[arg(long, conflicts_with = "stdio")]
    mobile: bool,
    /// Run the app-server JSON-RPC control transport over stdio (no listener).
    /// Used by local SDKs and JSON-RPC integrations.
    #[arg(long, default_value_t = false)]
    stdio: bool,
    /// Run as the desktop daemon: the same JSON-RPC control transport as
    /// `--stdio`, served on a user-private unix domain socket under the
    /// Codewhale runtime directory. Clients must `daemon/attach` first.
    /// Not yet supported on Windows (fails with a typed error).
    #[arg(long, default_value_t = false, conflicts_with_all = ["stdio", "http", "mobile"])]
    socket: bool,
    /// Socket path override for --socket. Defaults to
    /// `$CODEWHALE_HOME/run/daemon.sock`, else `$XDG_RUNTIME_DIR/codewhale/daemon.sock`,
    /// else `~/Library/Application Support/codewhale/daemon.sock` (macOS) or
    /// `~/.codewhale/run/daemon.sock`.
    #[arg(long = "socket-path", requires = "socket")]
    socket_path: Option<PathBuf>,
    /// Show a QR code for the mobile URL in the terminal (requires --mobile).
    #[arg(long, requires = "mobile")]
    qr: bool,
    /// Bind host. Defaults to 127.0.0.1; with --mobile and no host, binds
    /// 0.0.0.0 so LAN devices can reach the mobile page.
    #[arg(long)]
    host: Option<String>,
    /// Bind port. Defaults to 7878 for --http/--mobile (the runtime API) and
    /// 8787 for the legacy in-process app-server HTTP transport.
    #[arg(long)]
    port: Option<u16>,
    /// Background task worker count (1-8). Only used with --http/--mobile.
    #[arg(long)]
    workers: Option<usize>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long = "auth-token")]
    auth_token: Option<String>,
    #[arg(long, default_value_t = false)]
    insecure_no_auth: bool,
    #[arg(long = "cors-origin")]
    cors_origin: Vec<String>,
}

const MCP_SERVER_DEFINITIONS_KEY: &str = "mcp.server_definitions";

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn run_cli() -> std::process::ExitCode {
    install_rustls_crypto_provider();

    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // Use the full anyhow chain so callers see the underlying
            // cause (e.g. the actual TOML parse error with line/column)
            // instead of just the top-level context message. The bare
            // `{err}` Display impl drops the chain — see #767, where
            // users hit "failed to parse config at <path>" with no
            // hint that the real error was a stray BOM or unbalanced
            // quote a few lines down.
            eprintln!("error: {err}");
            for cause in err.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
            // A Codewhale account failure carries a class: CI logs must be
            // able to tell a bad credential from an unconfigured agent model
            // without parsing English, and the machine-readable code beside
            // it names the control-plane branch that was taken.
            if let Some(machine) = err.downcast_ref::<cloud::machine::MachineError>() {
                eprintln!(
                    "  codewhale: code={} status={}",
                    machine.code, machine.status
                );
                if let Ok(code) = u8::try_from(machine.exit_code) {
                    return std::process::ExitCode::from(code);
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn split_lane_log_proxy_command(
    command: Option<Commands>,
) -> (Option<LaneLogProxyArgs>, Option<Commands>) {
    match command {
        Some(Commands::LaneLogProxy(args)) => (Some(args), None),
        command => (None, command),
    }
}

fn config_command_targets_project(matches: &clap::ArgMatches) -> bool {
    let Some(config_matches) = matches.subcommand_matches("config") else {
        return false;
    };
    let Some((command, command_matches)) = config_matches.subcommand() else {
        return false;
    };
    if !matches!(command, "import" | "export") {
        return false;
    }
    command_matches
        .try_get_one::<bool>("project")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

fn config_store_path_for_dispatch(
    explicit_path: Option<PathBuf>,
    project_bundle_scope: bool,
    cwd: &Path,
) -> Option<PathBuf> {
    if explicit_path.is_none() && project_bundle_scope {
        // Mirror the project-config loader: the current app dir wins, but a
        // workspace that still keeps its document under the legacy app dir
        // must be read and updated in place rather than shadowed by a new
        // empty document.
        let current = cwd
            .join(codewhale_config::CODEWHALE_APP_DIR)
            .join(codewhale_config::CONFIG_FILE_NAME);
        let legacy = cwd
            .join(codewhale_config::LEGACY_APP_DIR)
            .join(codewhale_config::CONFIG_FILE_NAME);
        if !current.is_file() && legacy.is_file() {
            return Some(legacy);
        }
        return Some(current);
    }
    explicit_path
}

fn run() -> Result<()> {
    let matches = Cli::command().get_matches();
    let project_bundle_scope = config_command_targets_project(&matches);
    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    // The detached log proxy must not depend on user config parsing: its job
    // is to frame child output and publish a terminal receipt even when the
    // delegated command's own config is malformed.
    let (proxy, command) = split_lane_log_proxy_command(cli.command.take());
    if let Some(args) = proxy {
        return run_lane_log_proxy_command(args);
    }

    let pipe_api_key_handoff = matches!(
        &command,
        Some(Commands::Auth(AuthArgs {
            command: AuthCommand::PrintApiKey { .. }
        }))
    );
    if pipe_api_key_handoff {
        credential_handoff::prepare_stdout(io::stdout().is_terminal())?;
    }
    let runtime_provider = top_level_provider_override(cli.provider.as_deref(), command.as_ref())?;
    let uses_raw_tui_provider = cli.provider.is_some() && runtime_provider.is_none();
    let runtime_overrides = CliRuntimeOverrides {
        provider: runtime_provider,
        model: cli.model.clone(),
        api_key: cli.api_key.clone(),
        base_url: cli.base_url.clone(),
        auth_mode: None,
        output_mode: cli.output_mode.clone(),
        log_level: cli.log_level.clone(),
        telemetry: cli.telemetry,
        approval_policy: cli.approval_policy.clone(),
        sandbox_mode: cli.sandbox_mode.clone(),
        yolo: Some(cli.yolo),
        verbosity: cli.verbosity.clone(),
    };
    if uses_raw_tui_provider
        && let Some((resolved_runtime, passthrough)) =
            prepare_raw_provider_tui_dispatch(&cli, command.as_ref(), &runtime_overrides)?
    {
        return run_tui_in_process(&cli, &resolved_runtime, passthrough);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config_path =
        config_store_path_for_dispatch(cli.config.clone(), project_bundle_scope, &cwd);
    let mut store = ConfigStore::load(config_path).map_err(|error| {
        if pipe_api_key_handoff {
            anyhow!("unavailable credential")
        } else {
            error
        }
    })?;
    // Root session flags only reach the TUI through the `None` branch below;
    // no subcommand handler reads them. Accepting them silently resumes
    // nothing -- `codewhale --resume abc exec "..."` would start a fresh
    // session while looking like it continued one.
    if command.is_some()
        && (cli.continue_session || cli.resume.is_some() || cli.session_id.is_some())
    {
        anyhow::bail!(
            "--continue/--resume/--session-id apply to the interactive session and \
             cannot be combined with a subcommand. Run them without a subcommand, or \
             use the subcommand's own flag (for example `codewhale exec --session-id <id>`)."
        );
    }

    match command {
        Some(Commands::Run(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, args.args)
        }
        Some(Commands::Doctor(args)) => {
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("doctor", args))
        }
        Some(Commands::Models(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("models", args))
        }
        Some(Commands::Speech(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("speech", args))
        }
        Some(Commands::Sessions(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("sessions", args))
        }
        Some(Commands::Resume(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_resume_command(&cli, &resolved_runtime, args)
        }
        Some(Commands::Rc(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            let mut passthrough = vec!["--remote-control".to_string()];
            passthrough.extend(args.args);
            run_tui_in_process(&cli, &resolved_runtime, passthrough)
        }
        Some(Commands::Fork(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("fork", args))
        }
        Some(Commands::Init(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("init", args))
        }
        Some(Commands::Setup(args)) => {
            let resolved_runtime = if setup_is_status_report(&args) {
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides)
            } else {
                resolve_runtime_for_dispatch(&mut store, &runtime_overrides)
            };
            run_tui_in_process(&cli, &resolved_runtime, tui_args("setup", args))
        }
        Some(Commands::RemoteSetup(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, remote_setup_tui_args(args))
        }
        Some(Commands::Exec(args)) => {
            reject_exec_global_flags(&args.args)?;
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("exec", args))
        }
        Some(Commands::Fleet(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("fleet", args))
        }
        Some(Commands::WorkflowTool(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("workflow-tool", args))
        }
        Some(Commands::LaneLogProxy(_)) => unreachable!("lane log proxy dispatched above"),
        Some(Commands::Workflow(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            let config_path = store.path().to_path_buf();
            run_workflow_command(&cli, &resolved_runtime, &config_path, args)
        }
        Some(Commands::Lane(args)) => run_lane_command(args),
        Some(Commands::Review(args)) => {
            // CI path: a machine token authenticates as the account with no
            // local session and no browser. The account's own configured
            // provider then disambiguates a model that maps to several
            // configured routes, which review otherwise hard-errors on.
            let mut overrides = runtime_overrides.clone();
            if overrides.provider.is_none()
                && let Some(provider) = cloud::machine_review_provider()?
            {
                overrides.provider = Some(provider);
            }
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("review", args))
        }
        Some(Commands::Apply(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("apply", args))
        }
        Some(Commands::Eval(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("eval", args))
        }
        Some(Commands::Mcp(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("mcp", args))
        }
        Some(Commands::Integrations(args)) => {
            // Integrations only need route *identity*. Do not recover or
            // export a stored credential just to plan/launch a third-party
            // harness: it resolves its own keys from its own environment.
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("integrations", args))
        }
        Some(Commands::Features(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_in_process(&cli, &resolved_runtime, tui_args("features", args))
        }
        Some(Commands::Serve(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            // `serve` starts a long-running runtime API listener; supervise the
            // delegated child so it is torn down with the dispatcher (#3259).
            run_tui_server_in_process(&cli, &resolved_runtime, tui_args("serve", args))
        }
        Some(Commands::Web(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_tui_server_in_process(&cli, &resolved_runtime, web_serve_passthrough(&args))
        }
        Some(Commands::Login(args)) => {
            reject_legacy_login_provider_args(&args)?;
            cloud::reject_inline_api_key(cli.api_key.as_deref())?;
            cloud::run_account_login(
                args.no_open,
                args.timeout_seconds,
                cli.profile.as_deref(),
                &store,
            )
        }
        Some(Commands::Logout) => run_logout_command(&mut store, cli.profile.as_deref()),
        Some(Commands::Auth(args)) => match args.command {
            AuthCommand::XaiDevice => {
                let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
                run_tui_in_process(
                    &cli,
                    &resolved_runtime,
                    vec!["auth".to_string(), "xai-device".to_string()],
                )
            }
            AuthCommand::Chatgpt => {
                let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
                run_tui_in_process(
                    &cli,
                    &resolved_runtime,
                    vec!["auth".to_string(), "chatgpt".to_string()],
                )
            }
            AuthCommand::ChatgptRevoke => {
                let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
                run_tui_in_process(
                    &cli,
                    &resolved_runtime,
                    vec!["auth".to_string(), "chatgpt-revoke".to_string()],
                )
            }
            command @ AuthCommand::Status {
                diagnostic: true, ..
            } => {
                // Like `doctor`, this is a read-only diagnostic. Starting a
                // telemetry session here would create
                // `$CODEWHALE_HOME/telemetry` before the report could truthfully
                // say the isolated home is missing.
                run_auth_command_with_runtime(&mut store, command, &runtime_overrides)
            }
            command => {
                let resolved_runtime =
                    resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
                let session = start_cli_telemetry(
                    &resolved_runtime,
                    Some(store.path().to_path_buf()),
                    Surface::Cli,
                );
                let outcome =
                    run_auth_command_with_runtime(&mut store, command, &runtime_overrides);
                finish_cli_telemetry(session, &outcome);
                outcome
            }
        },
        Some(Commands::Account(args)) => {
            cloud::reject_inline_api_key(cli.api_key.as_deref())?;
            cloud::run(args, cli.profile.as_deref(), &store)
        }
        Some(Commands::Dispatch(args)) => dispatch::run(args),
        Some(Commands::McpServer) => {
            // `codewhale serve --mcp` delegates to the TUI and arms there, so
            // without this the same user action reported differently depending
            // on which spelling they typed — and `mcp-server`, a surface the
            // schema documents as emitting, could only ever read zero. A
            // structural zero a maintainer mistakes for an adoption zero is
            // the thing the "which surfaces emit" section exists to prevent.
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            let session = start_cli_telemetry(
                &resolved_runtime,
                Some(store.path().to_path_buf()),
                Surface::McpServer,
            );
            let outcome = run_mcp_server_command(&mut store);
            finish_cli_telemetry(session, &outcome);
            outcome
        }
        Some(Commands::Config(args)) => {
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            let session = start_cli_telemetry(
                &resolved_runtime,
                Some(store.path().to_path_buf()),
                Surface::Cli,
            );
            let outcome = run_config_command(&mut store, args.command, project_bundle_scope);
            finish_cli_telemetry(session, &outcome);
            outcome
        }
        Some(Commands::Model(args)) => {
            // `model resolve` is a diagnostic: it must report the same route
            // the runtime would take, so it resolves through the same
            // read-only path `doctor` uses rather than looking only at flags.
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            run_model_command(
                &mut store,
                args.command,
                runtime_overrides.provider,
                &resolved_runtime,
            )
        }
        Some(Commands::Thread(args)) => {
            run_thread_command(&cli, &mut store, &runtime_overrides, args.command)
        }
        Some(Commands::Sandbox(args)) => run_sandbox_command(args.command),
        Some(Commands::AppServer(args)) => {
            // The HTTP/mobile runtime API is delegated to the mature `serve` path
            // in the TUI binary, which reads the *global* --config. app-server has
            // historically taken a subcommand-level --config, so bridge it before
            // resolving runtime options (provider/keyring) for the delegated run.
            if (args.http || args.mobile) && cli.config.is_none() && args.config.is_some() {
                cli.config = args.config.clone();
                store = ConfigStore::load(cli.config.clone())?;
            }
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_app_server_command(&cli, &resolved_runtime, args)
        }
        Some(Commands::Completion { shell }) => {
            let mut stdout = io::stdout();
            stdout.write_all(render_completion_script(shell).as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Some(Commands::Metrics(args)) => run_metrics_command(args),
        Some(Commands::Update(args)) => {
            let resolved_runtime =
                resolve_runtime_for_diagnostic_dispatch(&store, &runtime_overrides);
            let session = start_cli_telemetry(
                &resolved_runtime,
                Some(store.path().to_path_buf()),
                Surface::Cli,
            );
            #[cfg(not(target_env = "ohos"))]
            let outcome = update::run_update(args.beta, args.check, args.proxy);
            #[cfg(target_env = "ohos")]
            let outcome = {
                let _ = args;
                Err(anyhow!(
                    "self-update is not supported on HarmonyOS/OpenHarmony yet"
                ))
            };
            finish_cli_telemetry(session, &outcome);
            outcome
        }
        Some(Commands::Providers(args)) => run_providers_command(args),
        None => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            let forwarded = root_tui_passthrough(&cli)?;
            run_tui_in_process(&cli, &resolved_runtime, forwarded)
        }
    }
}

fn root_tui_passthrough(cli: &Cli) -> Result<Vec<String>> {
    let mut forwarded = Vec::new();
    if cli.continue_session {
        forwarded.push("--continue".to_string());
    }
    let resume_session_id = cli
        .resume
        .as_deref()
        .or(cli.session_id.as_deref())
        .map(str::trim);
    if resume_session_id.is_some_and(str::is_empty) {
        // A shell expanding an unset variable -- `codewhale --resume
        // "$SESSION_ID"` -- must not quietly become a fresh session. The user
        // asked to resume; starting new loses the session they meant, and the
        // mistake is invisible until the history is gone.
        bail!(
            "--resume/--session-id needs a session id, but got an empty value \
             (an unset shell variable?). Use `codewhale --continue` to resume \
             the most recent session."
        );
    }
    if let Some(session_id) = resume_session_id {
        forwarded.push("--resume".to_string());
        forwarded.push(session_id.to_string());
    }

    let prompt =
        cli.prompt_flag
            .iter()
            .chain(cli.prompt.iter())
            .fold(String::new(), |mut acc, part| {
                if !acc.is_empty() {
                    acc.push(' ');
                }
                acc.push_str(part);
                acc
            });
    if !prompt.is_empty() {
        if cli.continue_session {
            bail!(
                "`codewhale --continue` resumes the interactive TUI. Use `codewhale exec --continue <PROMPT>` to continue a session non-interactively."
            );
        }
        if let Some(session_id) = resume_session_id {
            bail!(
                "`codewhale --resume {session_id}` resumes the interactive TUI. Use `codewhale exec --resume {session_id} <PROMPT>` to continue a session non-interactively."
            );
        }
        forwarded.push("--prompt".to_string());
        forwarded.push(prompt);
    }

    Ok(forwarded)
}

fn resolve_runtime_for_dispatch(
    store: &mut ConfigStore,
    runtime_overrides: &CliRuntimeOverrides,
) -> ResolvedRuntimeOptions {
    let runtime_secrets = Secrets::auto_detect();
    resolve_runtime_for_dispatch_with_secrets(store, runtime_overrides, &runtime_secrets)
}

/// Resolve enough routing state to delegate a static diagnostic without
/// reading or migrating the durable secret store.
///
/// The TUI's doctor/setup-status path performs its own read-only source check,
/// so this dispatcher must not recover and export a credential merely to start
/// that report. Regular runtime and authentication commands keep using
/// [`resolve_runtime_for_dispatch`].
fn resolve_runtime_for_diagnostic_dispatch(
    store: &ConfigStore,
    runtime_overrides: &CliRuntimeOverrides,
) -> ResolvedRuntimeOptions {
    store.config.resolve_runtime_options(runtime_overrides)
}

/// An armed telemetry session belonging to a subcommand that runs *in this
/// process*.
///
/// Existing at all is the permission: it is only ever constructed behind
/// [`TelemetryDecision::Enabled`] after persistent and run-scoped opt-outs are
/// applied.
struct CliTelemetrySession {
    started: std::time::Instant,
}

/// Arm telemetry for a subcommand the dispatcher executes itself.
///
/// Only the terminal branches take this path. Everything that delegates to the
/// TUI binary is armed over there, under its own surface, from the environment
/// this dispatcher forwards — naming a surface here for a delegated command
/// would report one run twice under two identities.
///
/// Persistent config and setup-state opt-outs are applied inside
/// [`telemetry::decide`].
fn start_cli_telemetry(
    resolved: &ResolvedRuntimeOptions,
    config_path: Option<PathBuf>,
    surface: Surface,
) -> Option<CliTelemetrySession> {
    let consent = resolve_cli_telemetry_consent(
        resolved,
        config_path,
        surface,
        telemetry::load_setup_state_for_decision(),
    )?;
    telemetry::init(consent);
    telemetry::record(Event::SessionStart {
        source: SessionSource::Unknown,
    });
    Some(CliTelemetrySession {
        started: std::time::Instant::now(),
    })
}

fn resolve_cli_telemetry_consent(
    resolved: &ResolvedRuntimeOptions,
    config_path: Option<PathBuf>,
    surface: Surface,
    setup: Option<SetupState>,
) -> Option<telemetry::TelemetryConsent> {
    let setup = setup?;
    let TelemetryDecision::Enabled(consent) = telemetry::decide(resolved, &setup, surface) else {
        return None;
    };
    Some(consent.with_config_path(config_path))
}

/// Close the session opened by [`start_cli_telemetry`] and flush, bounded.
///
/// The exit class comes from what actually happened, never from an exit code:
/// a cancelled run and a SIGINT both exit 130, so a code-derived class would
/// mislabel every cancel as a signal.
///
/// The flush re-resolves telemetry from disk before it sends anything, which is
/// what makes `codewhale config set telemetry false` take effect on the very run
/// that wrote it rather than on the next one.
fn finish_cli_telemetry(session: Option<CliTelemetrySession>, outcome: &Result<()>) {
    let Some(session) = session else {
        return;
    };
    telemetry::set_exit_class(if outcome.is_ok() {
        ExitClass::Clean
    } else {
        ExitClass::Error
    });
    telemetry::record(Event::SessionEnd {
        duration_bucket: DurationBucket::from_secs(session.started.elapsed().as_secs()),
        exit_class: telemetry::exit_class(),
        // Cold start is measured by the TUI's startup trace. This surface has
        // no equivalent, and inventing one from process start would be a
        // different measurement wearing the same name.
        cold_start_bucket: None,
        providers: Vec::new(),
        counters: Counters::default(),
        errors: Errors::default(),
        turn_wall: TurnWall::default(),
    });
    let _ = telemetry::shutdown_blocking(telemetry::SHUTDOWN_FLUSH_TIMEOUT);
}

fn resolve_runtime_for_dispatch_with_secrets(
    store: &mut ConfigStore,
    runtime_overrides: &CliRuntimeOverrides,
    secrets: &Secrets,
) -> ResolvedRuntimeOptions {
    store
        .config
        .resolve_runtime_options_with_secrets(runtime_overrides, secrets)
}

fn tui_args(command: &str, args: TuiPassthroughArgs) -> Vec<String> {
    let mut forwarded = Vec::with_capacity(args.args.len() + 1);
    forwarded.push(command.to_string());
    forwarded.extend(args.args);
    forwarded
}

fn setup_is_status_report(args: &TuiPassthroughArgs) -> bool {
    args.args.iter().any(|arg| arg == "--status")
}

fn reject_exec_global_flags(args: &[String]) -> Result<()> {
    const GLOBAL_ONLY_FLAGS: &[&str] = &["--provider", "--model", "--api-key", "--base-url"];

    for arg in args {
        if arg == "--" {
            break;
        }
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if GLOBAL_ONLY_FLAGS.contains(&flag) {
            bail!(
                "{flag} must be placed before `exec`.\n\nUse:\n  codewhale {flag} <value> exec \"<prompt>\""
            );
        }
    }

    Ok(())
}

/// `codewhale login` used to configure provider API keys; that surface moved
/// to `auth set --provider`. The hidden legacy flags stay parseable so the
/// redirect below can name the replacement instead of an unknown-flag error.
fn reject_legacy_login_provider_args(args: &LoginArgs) -> Result<()> {
    if args.api_key.is_none() && args.provider.is_none() {
        return Ok(());
    }
    bail!(
        "`codewhale login` now signs in to your Codewhale account via the browser device flow. \
         To configure a provider key, run `codewhale auth set --provider <provider>` (hidden prompt) \
         or `codewhale auth set --provider <provider> --api-key-stdin`."
    )
}

fn run_logout_command(store: &mut ConfigStore, profile: Option<&str>) -> Result<()> {
    run_logout_command_with_secrets(store, &Secrets::auto_detect(), profile)
}

fn run_logout_command_with_secrets(
    store: &mut ConfigStore,
    secrets: &Secrets,
    profile: Option<&str>,
) -> Result<()> {
    codewhale_config::with_xai_oauth_revocation_transaction(|| {
        run_logout_command_with_secrets_unlocked(store, secrets, profile)
    })
}

fn run_logout_command_with_secrets_unlocked(
    store: &mut ConfigStore,
    secrets: &Secrets,
    profile: Option<&str>,
) -> Result<()> {
    let original_config = store.config.clone();
    store.config.api_key = None;
    for provider in ProviderKind::ALL {
        clear_provider_api_key_from_config(store, provider);
        store
            .config
            .providers
            .for_provider_mut(provider)
            .external_credentials = None;
    }
    let xai = store.config.providers.for_provider_mut(ProviderKind::Xai);
    xai.oauth_credential_generation = None;
    xai.auth_mode = None;
    let openai_codex = store
        .config
        .providers
        .for_provider_mut(ProviderKind::OpenaiCodex);
    if openai_codex
        .oauth_credential_generation
        .as_deref()
        .is_some_and(codewhale_config::is_valid_chatgpt_oauth_generation)
    {
        openai_codex.oauth_credential_generation = None;
        if openai_codex.auth_mode.as_deref() == Some("oauth") {
            openai_codex.auth_mode = None;
        }
    }
    store.config.auth_mode = None;
    if let Err(error) = store.save() {
        store.config = original_config;
        return Err(error);
    }
    let mut keyring_failures = clear_all_provider_api_keys_from_keyring(secrets);
    // Already inside with_xai_oauth_revocation_transaction: the locked
    // variant must not re-enter the non-reentrant lifecycle mutex.
    if let Err(error) = codewhale_config::clear_all_chatgpt_oauth_credentials_locked() {
        keyring_failures.push(format!("chatgpt oauth: {error}"));
    }
    if let Err(error) = clear_daytona_slot(secrets) {
        keyring_failures.push(format!(
            "{}: {error}",
            codewhale_secrets::DAYTONA_TOKEN_SLOT
        ));
    }
    if let Err(error) = clear_account_session(profile) {
        keyring_failures.push(format!("account session: {error}"));
    }
    if keyring_failures.is_empty() {
        println!("logged out");
    } else {
        eprintln!(
            "failed to delete stored credentials for: {}",
            keyring_failures.join(", ")
        );
        println!("logged out (some stored credentials could not be deleted)");
    }
    Ok(())
}

fn clear_daytona_slot(secrets: &Secrets) -> Result<(), codewhale_secrets::SecretsError> {
    if secrets
        .get(codewhale_secrets::DAYTONA_TOKEN_SLOT)?
        .is_some_and(|value| !value.trim().is_empty())
    {
        secrets.delete(codewhale_secrets::DAYTONA_TOKEN_SLOT)?;
    }
    Ok(())
}

fn clear_account_session(profile: Option<&str>) -> Result<(), String> {
    use codewhale_secrets::account::{
        ACCOUNT_API_BASE_ENV, AccountSessionStore, DEFAULT_ACCOUNT_API_BASE,
        secure_account_session_secrets,
    };
    let secrets = secure_account_session_secrets().map_err(|error| error.to_string())?;
    let api_base = std::env::var(ACCOUNT_API_BASE_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string());
    AccountSessionStore::new(secrets, profile, &api_base)
        .clear()
        .map_err(|error| error.to_string())
}

/// Map [`ProviderKind`] to the canonical provider credential slot.
fn provider_slot(provider: ProviderKind) -> &'static str {
    // Shared-account families (SiliconFlow China, the four Model Studio
    // variants) collapse onto one slot; see ProviderKind::secret_store_slot.
    provider.secret_store_slot()
}

/// Resolve the store for credential-adjacent writes: provider selection,
/// `auth_mode` markers, and the plaintext-free metadata that accompanies a
/// saved key.
///
/// Credentials and their metadata are user-global — a key saved while
/// working in one repo must be visible from every other repo, and the secret
/// store already is (#5045). When the ambient config path is a
/// workspace-scoped document (`<repo>/.codewhale/config.toml`), login and
/// `auth set` must not bind the provider or write auth markers there: the
/// binding would be invisible from every other repo and would invite
/// plaintext keys into a committable repo file (#5198). Returns a store
/// loaded on the user-global document in that case, or `None` when the
/// ambient store is already correctly scoped, so key + provider binding +
/// auth markers share one user-global scope by default.
fn credential_metadata_store(store: &ConfigStore) -> Result<Option<ConfigStore>> {
    if !codewhale_config::config_path_is_workspace_scoped(store.path()) {
        return Ok(None);
    }
    let global = codewhale_config::default_config_path()?;
    eprintln!(
        "ambient config {} is workspace-scoped; writing credential metadata to the user-global {} instead",
        codewhale_config::quote_os_path(store.path()),
        codewhale_config::quote_os_path(&global),
    );
    ConfigStore::load(Some(global)).map(Some)
}

#[cfg(test)]
fn no_keyring_secrets() -> Secrets {
    Secrets::new(std::sync::Arc::new(
        codewhale_secrets::InMemoryKeyringStore::new(),
    ))
}

fn prepare_provider_api_key_metadata(store: &mut ConfigStore, provider: ProviderKind) {
    store.config.auth_mode = Some("api_key".to_string());
    let provider_config = store.config.providers.for_provider_mut(provider);
    provider_config.auth_mode = Some("api_key".to_string());
    provider_config.external_credentials = None;
    if provider == ProviderKind::Xai {
        provider_config.oauth_credential_generation = None;
    }
    if provider == ProviderKind::Deepseek && store.config.default_text_model.is_none() {
        store.config.default_text_model = Some(
            store
                .config
                .providers
                .deepseek
                .model
                .clone()
                .unwrap_or_else(|| "deepseek-v4-pro".to_string()),
        );
    }
}

/// Persist a provider credential to the durable secret store without silently
/// downgrading a backend failure to plaintext config storage.
fn persist_provider_api_key(
    store: &mut ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
    api_key: &str,
) -> Result<bool> {
    if provider == ProviderKind::Xai {
        return codewhale_config::with_xai_oauth_revocation_transaction(|| {
            persist_provider_api_key_unlocked(store, secrets, provider, api_key)
        });
    }
    persist_provider_api_key_unlocked(store, secrets, provider, api_key)
}

fn persist_provider_api_key_unlocked(
    store: &mut ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
    api_key: &str,
) -> Result<bool> {
    let original_config = store.config.clone();
    prepare_provider_api_key_metadata(store, provider);
    let slot = provider_slot(provider);
    // A readable prior value is required before a secret-store write so a
    // later config failure can restore the exact prior state. If the backend
    // cannot provide that snapshot, fail before changing the config file.
    let prior_secret = secrets.get(slot);
    let secret_store_saved = match prior_secret.as_ref().map_err(|error| error.to_string()) {
        Ok(_) => match secrets.set(slot, api_key) {
            Ok(()) => {
                clear_provider_api_key_from_config(store, provider);
                true
            }
            Err(err) => {
                store.config = original_config;
                return Err(anyhow::anyhow!(
                    "Secret storage write failed for {slot}: {err}. Refusing to write the API key in plaintext to {}. Fix the configured secret backend and retry; Codewhale did not change that file.",
                    codewhale_config::quote_os_path(store.path())
                ));
            }
        },
        Err(error) => {
            store.config = original_config;
            return Err(anyhow::anyhow!(
                "Secret storage snapshot failed for {slot}: {error}. Refusing to write the API key in plaintext to {}. Fix the configured secret backend and retry; Codewhale did not change that file.",
                codewhale_config::quote_os_path(store.path())
            ));
        }
    };
    if let Err(error) = store.save() {
        store.config = original_config;
        if secret_store_saved {
            let current = secrets
                .get(slot)
                .map_err(|rollback| anyhow::anyhow!(
                    "{error}; additionally could not verify secret-store rollback for {slot}: {rollback}"
                ))?;
            if current.as_deref() == Some(api_key) {
                match prior_secret.expect("snapshot succeeded before secret write") {
                    Some(previous) => secrets.set(slot, &previous),
                    None => secrets.delete(slot),
                }
                .map_err(|rollback| anyhow::anyhow!(
                    "{error}; additionally failed to restore prior secret-store state for {slot}: {rollback}"
                ))?;
            }
        }
        return Err(error);
    }
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(store.path())?;
    Ok(secret_store_saved)
}

fn clear_auth_provider(
    store: &mut ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
) -> Result<()> {
    let slot = provider_slot(provider);
    let original_config = store.config.clone();
    clear_provider_api_key_from_config(store, provider);
    if provider == ProviderKind::Xai {
        let xai = store.config.providers.for_provider_mut(provider);
        xai.oauth_credential_generation = None;
        xai.auth_mode = None;
        xai.external_credentials = None;
    }
    if let Err(error) = store.save() {
        store.config = original_config;
        return Err(error);
    }
    clear_provider_api_key_from_keyring(secrets, provider);
    if provider == ProviderKind::Xai {
        println!("cleared xAI credentials from config, secret store, and owned OAuth storage");
    } else {
        println!("cleared API key for {slot} from config and secret store");
    }
    Ok(())
}

fn clear_provider_api_key_from_config(store: &mut ConfigStore, provider: ProviderKind) {
    store.config.providers.for_provider_mut(provider).api_key = None;
    if provider == ProviderKind::Deepseek {
        store.config.api_key = None;
    }
}

fn provider_env_set(provider: ProviderKind) -> bool {
    provider_env_value(provider).is_some()
}

fn provider_env_vars(provider: ProviderKind) -> &'static [&'static str] {
    provider.provider().env_vars()
}

fn provider_env_value(provider: ProviderKind) -> Option<(&'static str, String)> {
    provider_env_vars(provider).iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| (*var, value))
    })
}

fn openai_codex_auth_file_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENAI_CODEX_AUTH_FILE") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return codewhale_config::resolve_external_credential_path(&path).unwrap_or(path);
        }
    }

    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    let path = codex_home.join("auth.json");
    codewhale_config::resolve_external_credential_path(&path).unwrap_or(path)
}

fn grok_auth_file_path() -> PathBuf {
    for key in ["GROK_AUTH_PATH", "XAI_AUTH_PATH"] {
        if let Ok(path) = std::env::var(key) {
            let path = PathBuf::from(path.trim());
            if !path.as_os_str().is_empty() {
                return codewhale_config::resolve_external_credential_path(&path).unwrap_or(path);
            }
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let home = PathBuf::from(home.trim());
        if !home.as_os_str().is_empty() {
            let path = home.join("auth.json");
            return codewhale_config::resolve_external_credential_path(&path).unwrap_or(path);
        }
    }
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("auth.json");
    codewhale_config::resolve_external_credential_path(&path).unwrap_or(path)
}

fn external_credential_target(
    provider: ProviderKind,
    path_override: Option<PathBuf>,
) -> Result<(codewhale_config::ExternalCredentialSource, PathBuf)> {
    let (source, default_path) = match provider {
        ProviderKind::OpenaiCodex => (
            codewhale_config::ExternalCredentialSource::CodexCli,
            openai_codex_auth_file_path(),
        ),
        ProviderKind::Xai => (
            codewhale_config::ExternalCredentialSource::GrokCli,
            grok_auth_file_path(),
        ),
        ProviderKind::Deepseek | ProviderKind::DeepseekAnthropic => (
            codewhale_config::ExternalCredentialSource::DshCli,
            codewhale_config::default_dsh_credentials_path(),
        ),
        ProviderKind::Antigravity => (
            codewhale_config::ExternalCredentialSource::AgyCli,
            codewhale_config::default_agy_credentials_path(),
        ),
        ProviderKind::Moonshot => bail!(
            "Kimi is API-key-only in Codewhale. Create a key at https://platform.kimi.ai/console/api-keys; Kimi CLI OAuth import is unsupported."
        ),
        _ => bail!(
            "{} has no supported external CLI credential source",
            provider.as_str()
        ),
    };
    let path =
        codewhale_config::resolve_external_credential_path(path_override.unwrap_or(default_path))?;
    Ok((source, path))
}

fn provider_config_api_key(store: &ConfigStore, provider: ProviderKind) -> Option<&str> {
    let slot = store
        .config
        .providers
        .for_provider(provider)
        .api_key
        .as_deref();
    let root = (provider == ProviderKind::Deepseek)
        .then_some(store.config.api_key.as_deref())
        .flatten();
    slot.or(root)
        .filter(|value| classify_config_api_key_value(value) == ConfigApiKeyValueKind::Literal)
}

fn provider_config_set(store: &ConfigStore, provider: ProviderKind) -> bool {
    provider_config_api_key(store, provider).is_some()
}

fn provider_keyring_api_key(secrets: &Secrets, provider: ProviderKind) -> Option<String> {
    secrets
        .get(provider_slot(provider))
        .ok()
        .flatten()
        .filter(|v| !v.trim().is_empty())
}

fn provider_keyring_set(secrets: &Secrets, provider: ProviderKind) -> bool {
    provider_keyring_api_key(secrets, provider).is_some()
}

fn clear_provider_api_key_from_keyring(secrets: &Secrets, provider: ProviderKind) {
    let _ = secrets.delete(provider_slot(provider));
}

/// Delete the keyring credential of every provider that has one stored.
///
/// Returns a human-readable entry per slot whose deletion failed, so the
/// caller can report the failure instead of claiming a clean logout while
/// credentials linger in the keyring. Slots shared by several providers
/// (e.g. the historical `siliconflow` slot) are deleted once.
fn clear_all_provider_api_keys_from_keyring(secrets: &Secrets) -> Vec<String> {
    let mut failures = Vec::new();
    let mut cleared_slots = std::collections::HashSet::new();
    for provider in ProviderKind::ALL {
        let slot = provider_slot(provider);
        if !cleared_slots.insert(slot) {
            continue;
        }
        if !provider_keyring_set(secrets, provider) {
            continue;
        }
        if let Err(error) = secrets.delete(slot) {
            failures.push(format!("{slot}: {error}"));
        }
    }
    failures
}

fn external_consent(
    store: &ConfigStore,
    provider: ProviderKind,
) -> Option<&codewhale_config::ExternalCredentialConsentToml> {
    store
        .config
        .providers
        .for_provider(provider)
        .external_credentials
        .as_ref()
}

fn external_read_consent(
    store: &ConfigStore,
    provider: ProviderKind,
) -> Option<&codewhale_config::ExternalCredentialConsentToml> {
    let (source, expected_path) = external_credential_target(provider, None).ok()?;
    external_consent(store, provider)
        .filter(|consent| consent.read_grant(provider, source, &expected_path).is_ok())
}

fn external_oauth_selected(store: &ConfigStore, provider: ProviderKind) -> bool {
    if external_read_consent(store, provider).is_none() {
        return false;
    }
    if provider == ProviderKind::OpenaiCodex {
        return true;
    }
    provider == ProviderKind::Xai
        && xai_oauth_mode_selected(store.config.providers.xai.auth_mode.as_deref())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XaiOAuthGenerationPointer {
    Absent,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XaiAuthDiagnosticRoute {
    /// Normal API-key diagnostics apply. This includes custom endpoints, where
    /// xAI OAuth is intentionally inactive.
    ApiKey,
    /// A syntactically valid Codewhale-owned generation pointer selects the
    /// owned OAuth route. Diagnostics deliberately do not inspect the file.
    OwnedOAuth,
    /// A configured but unsafe/malformed generation pointer blocks external
    /// Grok CLI access. The runtime can still fall back to API-key sources.
    NeedsRepair,
    /// With no configured generation, an exact read-only Grok CLI consent can
    /// be selected structurally. The external file is never probed here.
    ExternalConsent,
}

#[derive(Debug, Clone)]
struct XaiAuthDiagnostics {
    base_url: String,
    official_endpoint: bool,
    auth_mode: Option<String>,
    oauth_selected: bool,
    generation: XaiOAuthGenerationPointer,
    route: XaiAuthDiagnosticRoute,
}

impl XaiAuthDiagnostics {
    /// API-key routes are reported from the same endpoint-bound resolver that
    /// dispatch uses. Owned OAuth and consent-only routes remain structural so
    /// diagnostics cannot turn into a credential-store probe.
    fn evaluates_runtime_api_key(&self) -> bool {
        matches!(
            self.route,
            XaiAuthDiagnosticRoute::ApiKey | XaiAuthDiagnosticRoute::NeedsRepair
        )
    }

    fn is_custom_endpoint(&self) -> bool {
        !self.official_endpoint
    }
}

/// Source and redacted tail from the shared runtime resolver. Keeping only a
/// redacted tail prevents the presentation layer from accidentally retaining a
/// plaintext credential after it has derived the effective route.
#[derive(Debug, Clone, Default)]
struct XaiRuntimeApiKey {
    source: Option<RuntimeApiKeySource>,
    last4: Option<String>,
}

impl XaiRuntimeApiKey {
    fn source_name(&self) -> Option<&'static str> {
        match self.source {
            Some(RuntimeApiKeySource::Cli) => Some("cli"),
            Some(RuntimeApiKeySource::ConfigFile) => Some("config"),
            Some(RuntimeApiKeySource::Keyring) => Some("secret store"),
            Some(RuntimeApiKeySource::Env) => Some("env"),
            None => None,
        }
    }

    fn source_with_last4(&self) -> Option<String> {
        self.source_name()
            .map(|source| match self.last4.as_deref() {
                Some(last4) => format!("{source} (last4: {last4})"),
                None => source.to_string(),
            })
    }

    fn uses(&self, source: RuntimeApiKeySource) -> bool {
        self.source == Some(source)
    }
}

fn runtime_overrides_for_provider(
    runtime_overrides: &CliRuntimeOverrides,
    provider: ProviderKind,
) -> CliRuntimeOverrides {
    let mut overrides = runtime_overrides.clone();
    overrides.provider = Some(provider);
    overrides
}

fn xai_oauth_mode_selected(auth_mode: Option<&str>) -> bool {
    auth_mode.is_some_and(|mode| {
        matches!(
            mode.trim()
                .to_ascii_lowercase()
                .replace(['-', ' '], "_")
                .as_str(),
            "oauth"
                | "xai_oauth"
                | "xai"
                | "grok"
                | "grok_oauth"
                | "grok_cli"
                | "device"
                | "device_code"
                | "device_auth"
        )
    })
}

fn xai_oauth_generation_pointer(store: &ConfigStore) -> XaiOAuthGenerationPointer {
    match store
        .config
        .providers
        .xai
        .oauth_credential_generation
        .as_deref()
    {
        None => XaiOAuthGenerationPointer::Absent,
        Some(generation) if codewhale_config::is_valid_xai_oauth_generation(generation) => {
            XaiOAuthGenerationPointer::Valid
        }
        Some(_) => XaiOAuthGenerationPointer::Invalid,
    }
}

/// Resolve the same xAI route facts the runtime uses, without asking the
/// durable credential store for a secret. `ConfigToml::resolve_runtime_options`
/// deliberately uses an in-memory store, so this is safe for diagnostic output
/// that must remain structural/non-probing.
fn xai_auth_diagnostics(
    store: &ConfigStore,
    runtime_overrides: &CliRuntimeOverrides,
) -> XaiAuthDiagnostics {
    // We only need the effective endpoint here. Suppressing API-key
    // resolution keeps valid-owned and consent-only diagnostics structural:
    // they must not read ambient credential state merely to describe a route.
    let mut route_overrides = runtime_overrides_for_provider(runtime_overrides, ProviderKind::Xai);
    route_overrides.api_key = None;
    route_overrides.auth_mode = Some("none".to_string());
    let resolved = store.config.resolve_runtime_options(&route_overrides);
    let official_endpoint =
        provider_base_url_is_official(ProviderKind::Xai, resolved.base_url.as_str());
    // The TUI activates xAI OAuth only from `[providers.xai] auth_mode`; a
    // root-level auth mode may influence generic API-key policy but must never
    // turn an inert xAI generation pointer into an OAuth route.
    let auth_mode = store.config.providers.xai.auth_mode.clone();
    let generation = xai_oauth_generation_pointer(store);
    let oauth_selected = xai_oauth_mode_selected(auth_mode.as_deref());
    let route = if !official_endpoint || !oauth_selected {
        XaiAuthDiagnosticRoute::ApiKey
    } else {
        match generation {
            XaiOAuthGenerationPointer::Valid => XaiAuthDiagnosticRoute::OwnedOAuth,
            XaiOAuthGenerationPointer::Invalid => XaiAuthDiagnosticRoute::NeedsRepair,
            XaiOAuthGenerationPointer::Absent
                if external_read_consent(store, ProviderKind::Xai).is_some() =>
            {
                XaiAuthDiagnosticRoute::ExternalConsent
            }
            XaiOAuthGenerationPointer::Absent => XaiAuthDiagnosticRoute::ApiKey,
        }
    };

    XaiAuthDiagnostics {
        base_url: resolved.base_url,
        official_endpoint,
        auth_mode,
        oauth_selected,
        generation,
        route,
    }
}

/// Return the API-key route exactly as the dispatcher would resolve it. This
/// is the critical distinction for a global `--base-url` or `XAI_BASE_URL`:
/// official-provider config, keyring, and ambient keys must not cross onto an
/// unrelated custom endpoint.
fn xai_runtime_api_key(
    store: &ConfigStore,
    secrets: &Secrets,
    runtime_overrides: &CliRuntimeOverrides,
) -> XaiRuntimeApiKey {
    let resolved = store.config.resolve_runtime_options_with_secrets(
        &runtime_overrides_for_provider(runtime_overrides, ProviderKind::Xai),
        secrets,
    );
    debug_assert_eq!(resolved.provider, ProviderKind::Xai);
    XaiRuntimeApiKey {
        source: resolved.api_key_source,
        last4: resolved.api_key.as_deref().map(last4_label),
    }
}

fn api_key_source_name(
    config_key: Option<&str>,
    keyring_key: Option<&str>,
    env_key: Option<&(&'static str, String)>,
) -> Option<&'static str> {
    if config_key.is_some() {
        Some("config")
    } else if keyring_key.is_some() {
        Some("secret store")
    } else if env_key.is_some() {
        Some("env")
    } else {
        None
    }
}

fn xai_status_summary_source(
    diagnostics: &XaiAuthDiagnostics,
    api_key: Option<&XaiRuntimeApiKey>,
) -> String {
    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => {
            "Codewhale-owned OAuth configured/unprobed (valid generation pointer)".to_string()
        }
        XaiAuthDiagnosticRoute::NeedsRepair => {
            let api_key = api_key
                .and_then(XaiRuntimeApiKey::source_name)
                .unwrap_or("no runtime-effective API key");
            format!("needs repair (invalid OAuth generation pointer; API-key fallback: {api_key})")
        }
        XaiAuthDiagnosticRoute::ExternalConsent => {
            "external consent configured/unprobed".to_string()
        }
        XaiAuthDiagnosticRoute::ApiKey => api_key
            .and_then(XaiRuntimeApiKey::source_name)
            .unwrap_or("unset")
            .to_string(),
    }
}

fn xai_credential_route_label(
    diagnostics: &XaiAuthDiagnostics,
    api_key: Option<&XaiRuntimeApiKey>,
) -> String {
    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => {
            "Codewhale-owned OAuth configured/unprobed (valid generation pointer; storage unprobed)"
                .to_string()
        }
        XaiAuthDiagnosticRoute::NeedsRepair => {
            let api_key = api_key
                .and_then(XaiRuntimeApiKey::source_with_last4)
                .unwrap_or_else(|| "no runtime-effective API key".to_string());
            format!(
                "xAI OAuth needs repair (invalid Codewhale-owned generation pointer; Grok CLI consent blocked; API-key fallback: {api_key})"
            )
        }
        XaiAuthDiagnosticRoute::ExternalConsent => {
            "external read-only consent configured/unprobed".to_string()
        }
        XaiAuthDiagnosticRoute::ApiKey => api_key
            .and_then(XaiRuntimeApiKey::source_with_last4)
            .unwrap_or_else(|| "missing".to_string()),
    }
}

fn xai_table_storage_status(
    api_key: Option<&XaiRuntimeApiKey>,
    source: RuntimeApiKeySource,
) -> &'static str {
    match api_key {
        Some(api_key) if api_key.uses(source) => "set",
        Some(_) => "-",
        // The selected structural OAuth/consent route intentionally does not
        // establish whether any API-key storage is populated.
        None => "unprobed",
    }
}

fn xai_list_storage_status(
    api_key: Option<&XaiRuntimeApiKey>,
    source: RuntimeApiKeySource,
) -> &'static str {
    match api_key {
        Some(api_key) if api_key.uses(source) => "yes",
        Some(_) => "no",
        None => "?",
    }
}

fn xai_list_route(
    diagnostics: &XaiAuthDiagnostics,
    api_key: Option<&XaiRuntimeApiKey>,
) -> &'static str {
    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => "owned-oauth-configured",
        XaiAuthDiagnosticRoute::NeedsRepair => "needs-repair",
        XaiAuthDiagnosticRoute::ExternalConsent => "external-consent-configured",
        XaiAuthDiagnosticRoute::ApiKey => match api_key.and_then(|api_key| api_key.source) {
            Some(RuntimeApiKeySource::Cli) => "cli",
            Some(RuntimeApiKeySource::ConfigFile) => "config",
            Some(RuntimeApiKeySource::Keyring) => "store",
            Some(RuntimeApiKeySource::Env) => "env",
            None => "missing",
        },
    }
}

fn xai_storage_detail(
    diagnostics: &XaiAuthDiagnostics,
    api_key: Option<&XaiRuntimeApiKey>,
    source: RuntimeApiKeySource,
) -> String {
    match api_key {
        Some(api_key) if api_key.uses(source) => api_key
            .last4
            .as_deref()
            .map(|last4| format!("runtime-effective, last4: {last4}"))
            .unwrap_or_else(|| "runtime-effective".to_string()),
        Some(_) if diagnostics.is_custom_endpoint() => {
            "not eligible for this custom xAI endpoint".to_string()
        }
        Some(_) => "not selected by the runtime resolver".to_string(),
        None if diagnostics.evaluates_runtime_api_key() && diagnostics.is_custom_endpoint() => {
            "not eligible for this custom xAI endpoint".to_string()
        }
        None if diagnostics.evaluates_runtime_api_key() => {
            "not set for this runtime route".to_string()
        }
        None => "unprobed (structural OAuth/consent route)".to_string(),
    }
}

fn xai_lookup_order(diagnostics: &XaiAuthDiagnostics) -> String {
    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => {
            "lookup order: configured Codewhale-owned OAuth generation (storage unprobed); Grok CLI consent blocked".to_string()
        }
        XaiAuthDiagnosticRoute::NeedsRepair => {
            "lookup order: invalid Codewhale-owned OAuth generation blocks Grok CLI consent; runtime-effective API-key fallback: CLI -> config -> secret store -> env".to_string()
        }
        XaiAuthDiagnosticRoute::ExternalConsent => {
            "lookup order: configured consent-gated exact Grok CLI file (availability unprobed)".to_string()
        }
        XaiAuthDiagnosticRoute::ApiKey if diagnostics.is_custom_endpoint() => {
            "lookup order: endpoint-bound API key only for this custom xAI endpoint (explicit CLI key or route-bound config key)".to_string()
        }
        XaiAuthDiagnosticRoute::ApiKey => {
            "lookup order: CLI -> config -> secret store -> env".to_string()
        }
    }
}

fn xai_get_line(diagnostics: &XaiAuthDiagnostics, api_key: Option<&XaiRuntimeApiKey>) -> String {
    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => {
            "xai: configured (source: Codewhale-owned OAuth generation; valid pointer; storage unprobed)".to_string()
        }
        XaiAuthDiagnosticRoute::NeedsRepair => {
            let api_key = match api_key.and_then(XaiRuntimeApiKey::source_name) {
                Some("config") => "config-file".to_string(),
                Some("secret store") => "secret-store".to_string(),
                Some("env") => "env".to_string(),
                Some("cli") => "cli".to_string(),
                Some(other) => other.to_string(),
                None => "no runtime-effective API key".to_string(),
            };
            format!(
                "xai: needs repair (invalid Codewhale-owned OAuth generation pointer; Grok CLI consent blocked; API-key fallback: {api_key})"
            )
        }
        XaiAuthDiagnosticRoute::ExternalConsent => {
            "xai: configured (source: external read-only consent; availability unprobed)".to_string()
        }
        XaiAuthDiagnosticRoute::ApiKey => match api_key.and_then(XaiRuntimeApiKey::source_name) {
                Some("config") => "xai: set (source: config-file)".to_string(),
                Some("secret store") => "xai: set (source: secret-store)".to_string(),
                Some("env") => "xai: set (source: env)".to_string(),
                Some("cli") => "xai: set (source: cli)".to_string(),
                Some(other) => format!("xai: set (source: {other})"),
                None => "xai: not set".to_string(),
            },
    }
}

fn auth_get_line_with_runtime(
    store: &ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
    runtime_overrides: &CliRuntimeOverrides,
) -> String {
    let slot = provider_slot(provider);
    if provider == ProviderKind::Xai {
        let diagnostics = xai_auth_diagnostics(store, runtime_overrides);
        let api_key = diagnostics
            .evaluates_runtime_api_key()
            .then(|| xai_runtime_api_key(store, secrets, runtime_overrides));
        return xai_get_line(&diagnostics, api_key.as_ref());
    }

    let config_key = provider_config_api_key(store, provider);
    let keyring_key = config_key
        .is_none()
        .then(|| provider_keyring_api_key(secrets, provider))
        .flatten();
    let env_key = provider_env_value(provider);

    match api_key_source_name(config_key, keyring_key.as_deref(), env_key.as_ref()) {
        Some("config") => format!("{slot}: set (source: config-file)"),
        Some("secret store") => format!("{slot}: set (source: secret-store)"),
        Some("env") => format!("{slot}: set (source: env)"),
        Some(other) => format!("{slot}: set (source: {other})"),
        None => format!("{slot}: not set"),
    }
}

#[cfg(test)]
fn auth_status_all_providers(store: &ConfigStore, secrets: &Secrets) -> Vec<String> {
    auth_status_all_providers_with_runtime(store, secrets, &CliRuntimeOverrides::default())
}

fn auth_status_all_providers_with_runtime(
    store: &ConfigStore,
    secrets: &Secrets,
    runtime_overrides: &CliRuntimeOverrides,
) -> Vec<String> {
    let active_provider = store.config.provider;
    let mut lines = Vec::new();
    lines.push(account_status_line());
    lines.push(String::new());
    lines.push(format!(
        "active provider: {} (set via config or CODEWHALE_PROVIDER)",
        active_provider.as_str()
    ));
    lines.push(String::new());
    lines.push(format!(
        "{:<14} {:<8} {:<10} {:<8} {}",
        "provider", "config", "keyring", "env", "status"
    ));
    lines.push("-".repeat(70));

    for provider in ProviderKind::ALL {
        if provider == ProviderKind::Xai {
            let diagnostics = xai_auth_diagnostics(store, runtime_overrides);
            let api_key = diagnostics
                .evaluates_runtime_api_key()
                .then(|| xai_runtime_api_key(store, secrets, runtime_overrides));
            let active_marker = if provider == active_provider {
                " *"
            } else {
                ""
            };
            lines.push(format!(
                "{:<14} {:<8} {:<10} {:<8} {}{}",
                provider.as_str(),
                xai_table_storage_status(api_key.as_ref(), RuntimeApiKeySource::ConfigFile),
                xai_table_storage_status(api_key.as_ref(), RuntimeApiKeySource::Keyring),
                xai_table_storage_status(api_key.as_ref(), RuntimeApiKeySource::Env),
                xai_status_summary_source(&diagnostics, api_key.as_ref()),
                active_marker
            ));
            continue;
        }

        let config_key = provider_config_api_key(store, provider);
        let keyring_key = provider_keyring_api_key(secrets, provider);
        let env_key = provider_env_value(provider);
        let external_selected = external_oauth_selected(store, provider);

        let config_status = config_key.map(|_| "set").unwrap_or("-");
        let keyring_status = keyring_key.as_ref().map(|_| "set").unwrap_or("-");
        let env_status = env_key.as_ref().map(|_| "set").unwrap_or("-");

        let source = if provider == ProviderKind::OpenaiCodex {
            // Keep the summary consistent with `auth status`: Codex auth is
            // OAuth-file (or env token) based — config/keyring keys are not
            // consulted for it.
            if env_key.is_some() {
                "env".to_string()
            } else if external_selected {
                "external consent (not probed)".to_string()
            } else {
                "unset".to_string()
            }
        } else if external_selected {
            "external consent (not probed)".to_string()
        } else if config_key.is_some() {
            "config".to_string()
        } else if keyring_key.is_some() {
            "keyring".to_string()
        } else if env_key.is_some() {
            "env".to_string()
        } else {
            "unset".to_string()
        };

        let active_marker = if provider == active_provider {
            " *"
        } else {
            ""
        };

        lines.push(format!(
            "{:<14} {:<8} {:<10} {:<8} {}{}",
            provider.as_str(),
            config_status,
            keyring_status,
            env_status,
            source,
            active_marker
        ));
    }

    lines.push(String::new());
    lines.push("* = active provider (from config or CODEWHALE_PROVIDER)".to_string());
    lines.push("Run `codewhale auth status --provider <id>` for detailed info.".to_string());
    lines.push("Account sign-in is `codewhale login`.".to_string());
    lines
}

fn account_status_line() -> String {
    use codewhale_secrets::account::{
        ACCOUNT_API_BASE_ENV, AccountSessionState, AccountSessionStore, DEFAULT_ACCOUNT_API_BASE,
        secure_account_session_secrets,
    };
    let api_base = std::env::var(ACCOUNT_API_BASE_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNT_API_BASE.to_string());
    match secure_account_session_secrets() {
        Ok(secrets) => {
            match AccountSessionStore::new(secrets, None, &api_base)
                .runtime_info_at(chrono::Utc::now())
            {
                Ok(info) => {
                    let state = match info.state {
                        AccountSessionState::SignedOut => "not signed in",
                        AccountSessionState::Authenticated => "signed in",
                        AccountSessionState::OfflineCached => "offline (cached)",
                        AccountSessionState::Expired => "expired",
                        AccountSessionState::Revoked => "revoked",
                    };
                    format!("account: {state} (api {api_base})")
                }
                Err(error) => format!("account: unavailable ({error})"),
            }
        }
        Err(error) => format!("account: unavailable ({error})"),
    }
}

fn diagnostic_path_state(path: &Path, directory: bool) -> &'static str {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => "present (symlink; not followed)",
        Ok(metadata) if directory && metadata.is_dir() => "present",
        Ok(metadata) if !directory && metadata.is_file() => "present",
        Ok(_) => "present (unexpected type)",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unknown",
    }
}

const fn secret_backend_kind_label(
    kind: codewhale_secrets::SecretBackendDiagnosticKind,
) -> &'static str {
    match kind {
        codewhale_secrets::SecretBackendDiagnosticKind::File => "file",
        codewhale_secrets::SecretBackendDiagnosticKind::System => "system",
        codewhale_secrets::SecretBackendDiagnosticKind::Unknown => "unknown",
    }
}

const fn secret_backend_inspection_label(
    inspection: codewhale_secrets::SecretBackendInspection,
) -> &'static str {
    match inspection {
        codewhale_secrets::SecretBackendInspection::MetadataOnly => "metadata_only",
        codewhale_secrets::SecretBackendInspection::NotProbed => "not_probed",
    }
}

const fn secret_backend_presence_label(
    presence: codewhale_secrets::SecretBackendPresence,
) -> &'static str {
    match presence {
        codewhale_secrets::SecretBackendPresence::Present => "present",
        codewhale_secrets::SecretBackendPresence::Absent => "missing",
        codewhale_secrets::SecretBackendPresence::Unknown => "unknown",
    }
}

/// Value-free home and credential-source report for `auth status --diagnostic`.
///
/// Unlike ordinary `auth status`, this path never constructs [`Secrets`] and
/// never asks a provider keyring for a value. File presence comes from metadata
/// only; provider environment variables are checked with the runtime's
/// non-empty-string semantics and their contents are never formatted.
fn auth_diagnostic_lines(store: &ConfigStore, provider: Option<ProviderKind>) -> Vec<String> {
    let explicit_home = codewhale_paths::codewhale_home_is_explicit();
    let resolved_home = codewhale_paths::codewhale_home();
    let mut lines = vec![
        "auth diagnostic (structural only; credential values are never printed and provider credential stores were not opened)".to_string(),
        String::new(),
    ];

    let home = match resolved_home {
        Ok(Some(path)) => {
            lines.push(format!(
                "codewhale home: {} (source: {}; state: {})",
                codewhale_config::quote_os_path(&path),
                if explicit_home {
                    "CODEWHALE_HOME (isolated)"
                } else {
                    "platform home"
                },
                diagnostic_path_state(&path, true),
            ));
            Some(path)
        }
        Ok(None) => {
            lines.push("codewhale home: unavailable (no user home resolved)".to_string());
            None
        }
        Err(error) => {
            lines.push(format!("codewhale home: unavailable ({error})"));
            None
        }
    };

    lines.push(format!(
        "config: {} ({})",
        codewhale_config::quote_os_path(store.path()),
        diagnostic_path_state(store.path(), false),
    ));
    if let Some(home) = home.as_ref() {
        let settings = home.join("settings.toml");
        lines.push(format!(
            "settings: {} ({})",
            codewhale_config::quote_os_path(&settings),
            diagnostic_path_state(&settings, false),
        ));
    } else {
        lines.push("settings: unavailable (Codewhale home unresolved)".to_string());
    }

    let backend = codewhale_secrets::diagnose_secret_backend();
    lines.push(format!(
        "secret backend: {} (inspection: {})",
        secret_backend_kind_label(backend.backend),
        secret_backend_inspection_label(backend.inspection),
    ));
    if let Some(path) = backend.path.as_ref() {
        lines.push(format!(
            "secret store: {} ({})",
            codewhale_config::quote_os_path(path),
            secret_backend_presence_label(backend.presence),
        ));
    } else {
        lines.push(format!(
            "secret store: unavailable ({})",
            secret_backend_presence_label(backend.presence),
        ));
    }
    if let Some(path) = backend.legacy_path.as_ref() {
        lines.push(format!(
            "legacy secret store: {} ({})",
            codewhale_config::quote_os_path(path),
            secret_backend_presence_label(backend.legacy_presence),
        ));
    } else if explicit_home {
        lines.push(
            "legacy secret store: suppressed by explicit CODEWHALE_HOME isolation".to_string(),
        );
    } else {
        lines.push("legacy secret store: unavailable (not probed)".to_string());
    }

    lines.push(String::new());
    // Diagnostic mode answers "which sources will this shell use?" for one
    // route. Ordinary `auth status` remains the all-provider inventory; a
    // different provider can be inspected explicitly with `--provider`.
    let providers = [provider.unwrap_or(store.config.provider)];
    for provider in providers {
        let config_present = provider_config_api_key(store, provider).is_some();
        let environment_present = provider_env_vars(provider)
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()));
        let environment_names = match provider_env_vars(provider) {
            [] => "none configured".to_string(),
            names => names.join("/"),
        };
        let external_configured = external_consent(store, provider).is_some();
        lines.push(format!(
            "provider {} sources: config_literal={}, secret_backend={} (provider entry unprobed), environment={} ({}), external_consent={}",
            provider.as_str(),
            if config_present { "present" } else { "missing" },
            secret_backend_presence_label(backend.presence),
            if environment_present { "present" } else { "missing" },
            environment_names,
            if external_configured {
                "configured"
            } else {
                "missing"
            },
        ));
    }
    lines
}

fn run_auth_diagnostic(store: &ConfigStore, provider: Option<ProviderKind>) -> Result<()> {
    for line in auth_diagnostic_lines(store, provider) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
fn auth_list_lines(store: &ConfigStore, secrets: &Secrets) -> Vec<String> {
    auth_list_lines_with_runtime(store, secrets, &CliRuntimeOverrides::default())
}

fn auth_list_lines_with_runtime(
    store: &ConfigStore,
    secrets: &Secrets,
    runtime_overrides: &CliRuntimeOverrides,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("provider     config store env  route".to_string());
    for provider in ProviderKind::ALL {
        // Label the row by the provider, not by its credential slot. This
        // table has one row per ProviderKind, but several kinds share a slot
        // (ProviderKind::secret_store_slot): SiliconflowCN shares
        // `siliconflow`, and the four Model Studio variants share
        // `modelstudio-token-plan`. Labelling by slot printed `siliconflow`
        // twice and `modelstudio-token-plan` four times, so the reader could
        // not tell which row was which provider. The status columns still
        // read the shared slot, which is what makes one saved key light up
        // the whole family.
        let label = provider.as_str();
        if provider == ProviderKind::Xai {
            let diagnostics = xai_auth_diagnostics(store, runtime_overrides);
            let api_key = diagnostics
                .evaluates_runtime_api_key()
                .then(|| xai_runtime_api_key(store, secrets, runtime_overrides));
            lines.push(format!(
                "{label:<12}  {}     {}      {}   {}",
                xai_list_storage_status(api_key.as_ref(), RuntimeApiKeySource::ConfigFile),
                xai_list_storage_status(api_key.as_ref(), RuntimeApiKeySource::Keyring),
                xai_list_storage_status(api_key.as_ref(), RuntimeApiKeySource::Env),
                xai_list_route(&diagnostics, api_key.as_ref())
            ));
            continue;
        }

        let file = provider_config_set(store, provider);
        let keyring = (!file).then(|| provider_keyring_set(secrets, provider));
        let env = provider_env_set(provider);
        let external_selected = external_oauth_selected(store, provider);
        let active = if provider == ProviderKind::OpenaiCodex {
            if env {
                "env"
            } else if external_selected {
                "external-consent"
            } else {
                "missing"
            }
        } else if external_selected {
            "external-consent"
        } else if file {
            "config"
        } else if keyring == Some(true) {
            "store"
        } else if env {
            "env"
        } else {
            "missing"
        };
        lines.push(format!(
            "{label:<12}  {}     {}      {}   {active}",
            yes_no(file),
            keyring_status_short(keyring),
            yes_no(env)
        ));
    }
    lines
}

#[cfg(test)]
fn auth_status_lines_for_provider(
    store: &ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
) -> Vec<String> {
    auth_status_lines_for_provider_with_runtime(
        store,
        secrets,
        provider,
        &CliRuntimeOverrides::default(),
    )
}

fn auth_status_lines_for_provider_with_runtime(
    store: &ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
    runtime_overrides: &CliRuntimeOverrides,
) -> Vec<String> {
    if provider == ProviderKind::Xai {
        return xai_auth_status_lines_for_provider(store, secrets, runtime_overrides);
    }

    let config_key = provider_config_api_key(store, provider);
    let keyring_key = provider_keyring_api_key(secrets, provider);
    let env_key = provider_env_value(provider);
    let external = external_consent(store, provider);
    let external_selected = external_oauth_selected(store, provider);

    let active_label = {
        let active_source = if provider == ProviderKind::OpenaiCodex {
            if env_key.is_some() {
                "env"
            } else if store
                .config
                .providers
                .openai_codex
                .oauth_credential_generation
                .as_deref()
                .is_some_and(codewhale_config::is_valid_chatgpt_oauth_generation)
            {
                "codewhale-owned ChatGPT sign-in (availability not probed)"
            } else if external_selected {
                "external read-only consent (availability not probed)"
            } else {
                "missing"
            }
        } else if external_selected {
            "external read-only consent (availability not probed)"
        } else if config_key.is_some() {
            "config"
        } else if keyring_key.is_some() {
            "secret store"
        } else if env_key.is_some() {
            "env"
        } else {
            "missing"
        };
        let active_last4 = if provider == ProviderKind::OpenaiCodex {
            env_key.as_ref().map(|(_, value)| last4_label(value))
        } else {
            config_key
                .map(last4_label)
                .or_else(|| keyring_key.as_deref().map(last4_label))
                .or_else(|| env_key.as_ref().map(|(_, value)| last4_label(value)))
        };
        active_last4
            .map(|last4| format!("{active_source} (last4: {last4})"))
            .unwrap_or_else(|| active_source.to_string())
    };

    let env_var_label = env_key
        .as_ref()
        .map(|(name, _)| (*name).to_string())
        .unwrap_or_else(|| provider_env_vars(provider).join("/"));
    let env_status = env_key
        .as_ref()
        .map(|(_, value)| format!("set, last4: {}", last4_label(value)))
        .unwrap_or_else(|| "unset".to_string());

    let is_active = provider == store.config.provider;
    let active_marker = if is_active { " (active provider)" } else { "" };

    let provider_cfg = store.config.providers.for_provider(provider);
    let base_url = provider_cfg.base_url.as_deref().unwrap_or("(default)");
    let model = provider_cfg.model.as_deref().unwrap_or("(default)");

    let lookup_order = if provider == ProviderKind::OpenaiCodex {
        "lookup order: env -> Codewhale-owned ChatGPT sign-in -> consent-gated exact Codex CLI file"
            .to_string()
    } else {
        "lookup order: config -> secret store -> env".to_string()
    };
    let auth_mode = if provider == ProviderKind::OpenaiCodex {
        "codex_oauth".to_string()
    } else {
        provider_cfg
            .auth_mode
            .as_deref()
            .or(store.config.auth_mode.as_deref())
            .unwrap_or("api_key")
            .to_string()
    };

    let mut lines = vec![
        format!("provider: {}{}", provider.as_str(), active_marker),
        format!("route: {}", base_url),
        format!("model: {}", model),
        format!("auth mode: {auth_mode}"),
        format!("active source: {active_label}"),
        lookup_order,
        format!(
            "config file: {} ({})",
            codewhale_config::quote_os_path(store.path()),
            source_status(config_key, "missing")
        ),
        format!(
            "secret store: {} ({})",
            secrets.backend_name(),
            source_status(keyring_key.as_deref(), "missing")
        ),
        format!("env var: {env_var_label} ({env_status})"),
    ];

    if let Ok((source, expected_path)) = external_credential_target(provider, None) {
        let status = codewhale_config::external_credential_consent_status(
            external,
            provider,
            source,
            &expected_path,
            store.config.provider,
        );
        lines.push(format!(
            "external credentials: {} (provider={}, source={}, owner={}, path={}, consent_version={}, state={}, scope_valid={}, ambient_path_changed={}; file not probed)",
            status.access.as_str(),
            status.provider,
            status.source.as_str(),
            status.owner,
            codewhale_config::quote_os_path(&status.path),
            status.consent_version,
            status.route_state,
            status.scope_valid,
            status.ambient_path_changed,
        ));
        lines.push(format!("semantics: {}", status.semantics));
        lines.push(format!("revoke: {}", status.revoke_command));
        if let Some(warning) = status.ambient_path_warning() {
            lines.push(warning);
        }
    } else {
        lines.push("external credentials: disabled (no file was probed)".to_string());
    }
    lines
}

fn xai_auth_status_lines_for_provider(
    store: &ConfigStore,
    secrets: &Secrets,
    runtime_overrides: &CliRuntimeOverrides,
) -> Vec<String> {
    let diagnostics = xai_auth_diagnostics(store, runtime_overrides);
    let api_key = diagnostics
        .evaluates_runtime_api_key()
        .then(|| xai_runtime_api_key(store, secrets, runtime_overrides));
    let external = external_consent(store, ProviderKind::Xai);
    let selected_marker = if store.config.provider == ProviderKind::Xai {
        " (selected provider)"
    } else {
        ""
    };
    let provider_cfg = &store.config.providers.xai;
    let model = provider_cfg.model.as_deref().unwrap_or("(default)");
    let auth_mode = diagnostics.auth_mode.as_deref().unwrap_or("api_key");

    let mut lines = vec![
        format!("provider: xai{selected_marker}"),
        format!("route: {}", diagnostics.base_url),
        format!("model: {model}"),
        format!("auth mode: {auth_mode}"),
        format!(
            "credential route: {}",
            xai_credential_route_label(&diagnostics, api_key.as_ref())
        ),
        xai_lookup_order(&diagnostics),
        format!(
            "config file: {} ({})",
            codewhale_config::quote_os_path(store.path()),
            xai_storage_detail(
                &diagnostics,
                api_key.as_ref(),
                RuntimeApiKeySource::ConfigFile
            )
        ),
        format!(
            "secret store: {} ({})",
            secrets.backend_name(),
            xai_storage_detail(&diagnostics, api_key.as_ref(), RuntimeApiKeySource::Keyring)
        ),
        format!(
            "env var: {} ({})",
            provider_env_vars(ProviderKind::Xai).join("/"),
            xai_storage_detail(&diagnostics, api_key.as_ref(), RuntimeApiKeySource::Env)
        ),
        format!(
            "endpoint policy: {}",
            if diagnostics.official_endpoint {
                "official xAI endpoint"
            } else {
                "custom xAI endpoint; API-key-only (owned and external OAuth are inactive)"
            }
        ),
    ];

    lines.push(match diagnostics.generation {
        XaiOAuthGenerationPointer::Absent => "xAI OAuth generation: absent".to_string(),
        XaiOAuthGenerationPointer::Valid
            if diagnostics.route == XaiAuthDiagnosticRoute::OwnedOAuth =>
        {
            "xAI OAuth generation: configured Codewhale-owned pointer (storage unprobed)"
                .to_string()
        }
        XaiOAuthGenerationPointer::Valid => {
            "xAI OAuth generation: valid but inactive for this route".to_string()
        }
        XaiOAuthGenerationPointer::Invalid => {
            "xAI OAuth generation: invalid Codewhale-owned pointer".to_string()
        }
    });

    match diagnostics.route {
        XaiAuthDiagnosticRoute::OwnedOAuth => {
            lines.push(
                "external credentials: blocked by the configured Codewhale-owned xAI OAuth generation (file not probed)"
                    .to_string(),
            );
            return lines;
        }
        XaiAuthDiagnosticRoute::NeedsRepair => {
            lines.push(
                "external credentials: blocked by the invalid Codewhale-owned xAI OAuth generation pointer (file not probed)"
                    .to_string(),
            );
            lines.push(
                "repair: run `codewhale auth xai-device` to replace the owned generation, or switch [providers.xai] auth_mode to \"api_key\" and remove oauth_credential_generation. Grok CLI consent remains blocked until the pointer is absent."
                    .to_string(),
            );
            return lines;
        }
        XaiAuthDiagnosticRoute::ApiKey if diagnostics.is_custom_endpoint() => {
            lines.push(
                "external credentials: unavailable on a custom xAI endpoint (API-key-only; file not probed)"
                    .to_string(),
            );
            return lines;
        }
        XaiAuthDiagnosticRoute::ApiKey if !diagnostics.oauth_selected && external.is_some() => {
            lines.push(
                "external credentials: configured but inactive because xAI OAuth mode is not selected (file not probed)"
                    .to_string(),
            );
            return lines;
        }
        XaiAuthDiagnosticRoute::ApiKey | XaiAuthDiagnosticRoute::ExternalConsent => {}
    }

    if let Ok((source, expected_path)) = external_credential_target(ProviderKind::Xai, None) {
        let status = codewhale_config::external_credential_consent_status(
            external,
            ProviderKind::Xai,
            source,
            &expected_path,
            store.config.provider,
        );
        lines.push(format!(
            "external credentials: {} (provider={}, source={}, owner={}, path={}, consent_version={}, state={}, scope_valid={}, ambient_path_changed={}; file not probed)",
            status.access.as_str(),
            status.provider,
            status.source.as_str(),
            status.owner,
            codewhale_config::quote_os_path(&status.path),
            status.consent_version,
            status.route_state,
            status.scope_valid,
            status.ambient_path_changed,
        ));
        lines.push(format!("semantics: {}", status.semantics));
        lines.push(format!("revoke: {}", status.revoke_command));
        if let Some(warning) = status.ambient_path_warning() {
            lines.push(warning);
        }
    } else {
        lines.push("external credentials: disabled (no file was probed)".to_string());
    }
    lines
}

fn source_status(value: Option<&str>, missing_label: &str) -> String {
    value
        .map(|v| format!("set, last4: {}", last4_label(v)))
        .unwrap_or_else(|| missing_label.to_string())
}

fn last4_label(value: &str) -> String {
    let trimmed = value.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 4 {
        return "<redacted>".to_string();
    }
    let last4: String = chars[chars.len() - 4..].iter().collect();
    format!("...{last4}")
}

fn run_auth_command_with_runtime(
    store: &mut ConfigStore,
    command: AuthCommand,
    runtime_overrides: &CliRuntimeOverrides,
) -> Result<()> {
    let command = match command {
        AuthCommand::Status {
            provider,
            diagnostic: true,
        } => {
            // Keep the structural diagnostic structurally read-only: ordinary
            // status constructs the configured credential facade so it can report
            // runtime-effective sources, but diagnostic mode must not even create
            // a system-keyring handle or inspect a file-backed store.
            return run_auth_diagnostic(store, provider);
        }
        command => command,
    };
    run_auth_command_with_secrets_and_runtime(
        store,
        command,
        &Secrets::auto_detect(),
        runtime_overrides,
    )
}

#[cfg(test)]
fn run_auth_command_with_secrets(
    store: &mut ConfigStore,
    command: AuthCommand,
    secrets: &Secrets,
) -> Result<()> {
    run_auth_command_with_secrets_and_runtime(
        store,
        command,
        secrets,
        &CliRuntimeOverrides::default(),
    )
}

fn run_auth_command_with_secrets_and_runtime(
    store: &mut ConfigStore,
    command: AuthCommand,
    secrets: &Secrets,
    runtime_overrides: &CliRuntimeOverrides,
) -> Result<()> {
    match command {
        AuthCommand::XaiDevice => {
            let argv = vec![
                "codewhale".to_string(),
                "auth".to_string(),
                "xai-device".to_string(),
            ];
            let code = codewhale_tui::run(argv);
            std::process::exit(if code == std::process::ExitCode::SUCCESS {
                0
            } else {
                1
            })
        }
        AuthCommand::Chatgpt => {
            let argv = vec![
                "codewhale".to_string(),
                "auth".to_string(),
                "chatgpt".to_string(),
            ];
            let code = codewhale_tui::run(argv);
            std::process::exit(if code == std::process::ExitCode::SUCCESS {
                0
            } else {
                1
            })
        }
        AuthCommand::ChatgptRevoke => {
            let argv = vec![
                "codewhale".to_string(),
                "auth".to_string(),
                "chatgpt-revoke".to_string(),
            ];
            let code = codewhale_tui::run(argv);
            std::process::exit(if code == std::process::ExitCode::SUCCESS {
                0
            } else {
                1
            })
        }
        AuthCommand::ExternalConsent {
            provider,
            mode,
            path,
            yes,
        } => {
            let (source, path) = external_credential_target(provider, path)?;
            let preview = external_consent_preview_lines(provider, source, &path);
            for line in &preview {
                println!("{line}");
            }
            if mode == ExternalCredentialModeArg::Managed {
                bail!(
                    "managed external credential access is unsupported in v0.9.1: no provider has a reviewed schema-safe preservation adapter. Use --mode read-only, or use Codewhale-owned login/API-key storage."
                );
            }
            confirm_external_consent(yes)?;
            let path_value = path.to_str().context(
                "external credential path cannot be persisted losslessly because it is not valid UTF-8",
            )?;
            let provider_key = provider.provider().provider_config_key();
            codewhale_config::mutate_config_document(store.path(), |document| {
                if matches!(provider, ProviderKind::OpenaiCodex | ProviderKind::Xai) {
                    codewhale_config::set_config_document_value(
                        document,
                        &["providers", provider_key, "auth_mode"],
                        "oauth",
                    )?;
                }
                let prefix = &["providers", provider_key, "external_credentials"];
                codewhale_config::set_config_document_value(
                    document,
                    &[prefix[0], prefix[1], prefix[2], "access"],
                    "read_only",
                )?;
                codewhale_config::set_config_document_value(
                    document,
                    &[prefix[0], prefix[1], prefix[2], "provider"],
                    provider.as_str(),
                )?;
                codewhale_config::set_config_document_value(
                    document,
                    &[prefix[0], prefix[1], prefix[2], "source"],
                    source.as_str(),
                )?;
                codewhale_config::set_config_document_value(
                    document,
                    &[prefix[0], prefix[1], prefix[2], "path"],
                    path_value,
                )?;
                codewhale_config::set_config_document_value(
                    document,
                    &[prefix[0], prefix[1], prefix[2], "consent_version"],
                    i64::from(codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION),
                )
            })?;
            store
                .reload()
                .context("external consent was saved, but config reload failed")?;
            println!(
                "saved read-only external credential consent: provider={}, owner={}, path={}, consent_version={} ({})",
                provider.as_str(),
                source.as_str(),
                codewhale_config::quote_os_path(&path),
                codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION,
                codewhale_config::EXTERNAL_CREDENTIAL_READ_ONLY_SEMANTICS,
            );
            println!(
                "revoke with: codewhale auth external-revoke --provider {}",
                provider.as_str()
            );
            Ok(())
        }
        AuthCommand::ExternalRevoke { provider } => {
            let provider_key = provider.provider().provider_config_key();
            codewhale_config::mutate_config_document(store.path(), |document| {
                codewhale_config::unset_config_document_value(
                    document,
                    &["providers", provider_key, "external_credentials"],
                )?;
                Ok(())
            })?;
            store
                .reload()
                .context("external consent was revoked, but config reload failed")?;
            println!(
                "external credential access disabled for {}",
                provider.as_str()
            );
            Ok(())
        }
        AuthCommand::Status {
            provider,
            diagnostic,
        } => {
            if diagnostic {
                return run_auth_diagnostic(store, provider);
            }
            match provider {
                Some(provider) => {
                    for line in auth_status_lines_for_provider_with_runtime(
                        store,
                        secrets,
                        provider,
                        runtime_overrides,
                    ) {
                        println!("{line}");
                    }
                }
                None => {
                    for line in
                        auth_status_all_providers_with_runtime(store, secrets, runtime_overrides)
                    {
                        println!("{line}");
                    }
                }
            }
            Ok(())
        }
        AuthCommand::Set {
            provider,
            api_key,
            api_key_stdin,
        } => {
            let slot = provider_slot(provider);
            if provider == ProviderKind::Ollama && api_key.is_none() && !api_key_stdin {
                let provider_cfg = store.config.providers.for_provider_mut(provider);
                if provider_cfg.base_url.is_none() {
                    provider_cfg.base_url = Some("http://localhost:11434/v1".to_string());
                }
                store.save()?;
                println!(
                    "configured {slot} provider in {} (API key optional)",
                    store.path().display()
                );
                return Ok(());
            }
            let api_key = match (api_key, api_key_stdin) {
                (Some(v), _) => v,
                (None, true) => read_api_key_from_stdin()?,
                (None, false) => prompt_api_key(slot)?,
            };
            let mut credential_store = credential_metadata_store(store)?;
            let store = credential_store.as_mut().unwrap_or(store);
            let secret_store_saved = persist_provider_api_key(store, secrets, provider, &api_key)?;
            // Don't print the key. Don't echo length.
            if secret_store_saved {
                println!(
                    "saved API key for {slot} to {} (config contains metadata only)",
                    secrets.backend_name(),
                );
            } else {
                println!("saved API key for {slot} to {}", store.path().display());
            }
            Ok(())
        }
        AuthCommand::Get { provider } => {
            println!(
                "{}",
                auth_get_line_with_runtime(store, secrets, provider, runtime_overrides)
            );
            Ok(())
        }
        AuthCommand::PrintApiKey { provider } => {
            let mut stdout = io::stdout().lock();
            credential_handoff::handoff_secret_line(&mut stdout, io::stdout().is_terminal(), || {
                credential_handoff::resolve_api_key(store, secrets, provider, runtime_overrides)
            })
        }
        AuthCommand::Clear { provider } => {
            if provider == ProviderKind::Xai {
                codewhale_config::with_xai_oauth_revocation_transaction(|| {
                    clear_auth_provider(store, secrets, provider)
                })
            } else {
                clear_auth_provider(store, secrets, provider)
            }
        }
        AuthCommand::List => {
            for line in auth_list_lines_with_runtime(store, secrets, runtime_overrides) {
                println!("{line}");
            }
            Ok(())
        }
        AuthCommand::Migrate { dry_run } => run_auth_migrate(store, secrets, dry_run),
    }
}

fn external_consent_preview_lines(
    provider: ProviderKind,
    source: codewhale_config::ExternalCredentialSource,
    path: &Path,
) -> Vec<String> {
    vec![
        "External credential consent preview (nothing has been saved):".to_string(),
        format!("  provider: {}", provider.as_str()),
        format!(
            "  owning CLI: {} ({})",
            source.owner_label(),
            source.as_str()
        ),
        format!(
            "  exact resolved path: {}",
            codewhale_config::quote_os_path(path)
        ),
        format!(
            "  access: read_only ({})",
            codewhale_config::EXTERNAL_CREDENTIAL_READ_ONLY_SEMANTICS
        ),
        "  managed: unavailable (no reviewed schema-safe preservation adapter)".to_string(),
        format!(
            "  revoke: codewhale auth external-revoke --provider {}",
            provider.as_str()
        ),
    ]
}

fn confirm_external_consent(yes: bool) -> Result<()> {
    use std::io::IsTerminal;

    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "external credential consent was not saved: non-interactive use requires explicit --yes after reviewing the preview"
        );
    }
    confirm_external_consent_answer(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
}

fn confirm_external_consent_answer(
    reader: &mut impl std::io::BufRead,
    writer: &mut impl std::io::Write,
) -> Result<()> {
    write!(writer, "Type 'yes' to grant this exact read-only access: ")?;
    writer.flush()?;
    let mut answer = String::new();
    reader
        .read_line(&mut answer)
        .context("reading external credential consent confirmation")?;
    if answer.trim() != "yes" {
        bail!("external credential consent cancelled; no configuration was changed");
    }
    Ok(())
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no " }
}

fn keyring_status_short(state: Option<bool>) -> &'static str {
    match state {
        Some(true) => "yes",
        Some(false) => "no ",
        None => "n/a",
    }
}

fn prompt_api_key(slot: &str) -> Result<String> {
    use std::io::{IsTerminal, Write};
    eprint!("Enter API key for {slot}: ");
    io::stderr().flush().ok();
    if !io::stdin().is_terminal() {
        // Non-interactive: read directly without prompting twice.
        return read_api_key_from_stdin();
    }
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("failed to read API key from stdin")?;
    let key = buf.trim().to_string();
    if key.is_empty() {
        bail!("empty API key provided");
    }
    Ok(key)
}

/// Move plaintext keys from config.toml into the configured secret store.
/// Hidden in v0.8.8 because the normal setup path is config/env only.
fn run_auth_migrate(store: &mut ConfigStore, secrets: &Secrets, dry_run: bool) -> Result<()> {
    let mut migrated: Vec<(ProviderKind, &'static str)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let literal =
        |value: &String| classify_config_api_key_value(value) == ConfigApiKeyValueKind::Literal;

    for provider in ProviderKind::ALL {
        let slot = provider_slot(provider);
        let from_provider_block = store
            .config
            .providers
            .for_provider(provider)
            .api_key
            .clone()
            .filter(literal);
        let from_root = (provider == ProviderKind::Deepseek)
            .then(|| store.config.api_key.clone())
            .flatten()
            .filter(literal);
        let value = from_provider_block.or(from_root);
        let Some(value) = value else { continue };

        if let Ok(Some(existing)) = secrets.get(slot)
            && existing == value
        {
            // Already migrated; safe to strip the file slot.
        } else if dry_run {
            migrated.push((provider, slot));
            continue;
        } else if let Err(err) = secrets.set(slot, &value) {
            warnings.push(format!(
                "skipped {slot}: failed to write to secret store: {err}"
            ));
            continue;
        }
        if !dry_run {
            store.config.providers.for_provider_mut(provider).api_key = None;
            if provider == ProviderKind::Deepseek {
                store.config.api_key = None;
            }
        }
        migrated.push((provider, slot));
    }

    if !dry_run && !migrated.is_empty() {
        store
            .save()
            .context("failed to write updated config.toml")?;
    }
    if !dry_run {
        codewhale_config::scrub_plaintext_api_keys_from_config_backup(store.path())
            .context("failed to remove plaintext API keys from config backup")?;
    }

    println!("secret store backend: {}", secrets.backend_name());
    if migrated.is_empty() {
        println!("nothing to migrate (config.toml has no plaintext api_key entries)");
    } else {
        println!(
            "{} {} provider key(s):",
            if dry_run { "would migrate" } else { "migrated" },
            migrated.len()
        );
        for (_, slot) in &migrated {
            println!("  - {slot}");
        }
        if !dry_run {
            println!(
                "config.toml at {} no longer contains api_key entries for migrated providers.",
                store.path().display()
            );
        }
    }
    for w in warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

fn run_config_command(
    store: &mut ConfigStore,
    command: ConfigCommand,
    project_bundle_scope: bool,
) -> Result<()> {
    if project_bundle_scope && !codewhale_config::config_path_is_workspace_scoped(store.path()) {
        bail!(
            "--project requires a workspace config ({} is the user-global document)",
            store.path().display()
        );
    }
    match command {
        ConfigCommand::Get { key } => {
            if let Some(value) = store.config.get_display_value(&key) {
                println!("{value}");
                return Ok(());
            }
            bail!("key not found: {key}");
        }
        ConfigCommand::Set { key, value } => {
            clear_recorded_telemetry_opt_out_if_reenabled(&key, &value)?;
            store.config.set_value(&key, &value)?;
            store.save()?;
            println!("set {key}");
            Ok(())
        }
        ConfigCommand::Unset { key } => {
            store.config.unset_value(&key)?;
            store.save()?;
            println!("unset {key}");
            Ok(())
        }
        ConfigCommand::List => {
            // Configured truth, not live-session truth (DGF-01): a running
            // session keeps the route it resolved at launch, so these values
            // must not be read as "what the current session is serving".
            // `#` keeps the header safe for `key = value` line parsers.
            println!("# configured values ({})", store.path().display());
            println!(
                "# a running session keeps the route it resolved at launch; `codewhale model resolve` reports the route a new session would take"
            );
            for (key, value) in store.config.list_values() {
                println!("{key} = {value}");
            }
            Ok(())
        }
        ConfigCommand::Path => {
            println!("{}", store.path().display());
            Ok(())
        }
        ConfigCommand::Import(args) => {
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            config_bundles::run_import(&args, store, &workspace)
        }
        ConfigCommand::Export(args) => config_bundles::run_export(&args, store),
    }
}

/// An explicit `telemetry = true` re-enables a machine that previously declined
/// the notice. Fresh machines keep the notice owed, so the disclosure still
/// appears on their first interactive launch.
fn clear_recorded_telemetry_opt_out_if_reenabled(key: &str, value: &str) -> Result<()> {
    let turning_on = matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "enabled"
    );
    if key != "telemetry" || !turning_on {
        return Ok(());
    }
    if let Some(mut state) = SetupState::load()?
        && state.telemetry_opted_out()
    {
        state.record_telemetry_notice(codewhale_config::TELEMETRY_NOTICE_VERSION, true);
        state.save()?;
    }
    Ok(())
}

fn model_command_provider_hint(
    command_provider: Option<ProviderKind>,
    top_level_provider: Option<ProviderKind>,
) -> Option<ProviderKind> {
    command_provider.or(top_level_provider)
}

fn provider_source_label(source: ProviderSource) -> String {
    match source {
        ProviderSource::Cli => "--provider".to_string(),
        ProviderSource::Env(name) => format!("environment ({name})"),
        ProviderSource::Config => "config".to_string(),
    }
}

fn canonical_model_for_set(model: &str) -> &str {
    match model.to_ascii_lowercase().as_str() {
        "pro" | "deepseek-v4pro" => "deepseek-v4-pro",
        "flash" | "deepseek-v4flash" => "deepseek-v4-flash",
        "flash-vision" | "deepseek-v4flashvisionexp" => "deepseek-v4-flash-vision-exp",
        "auto" => "auto",
        _ => model,
    }
}

fn run_model_command(
    store: &mut ConfigStore,
    command: ModelCommand,
    top_level_provider: Option<ProviderKind>,
    resolved_runtime: &ResolvedRuntimeOptions,
) -> Result<()> {
    let registry = ModelRegistry::default();
    match command {
        ModelCommand::List { provider } => {
            let filter = model_command_provider_hint(provider, top_level_provider);
            for model in registry.list().into_iter().filter(|m| match filter {
                Some(p) => m.provider == p,
                None => true,
            }) {
                println!("{} ({})", model.id, model.provider.as_str());
            }
            Ok(())
        }
        ModelCommand::Resolve { model, provider } => {
            // Only `model resolve --provider X` is a hypothetical. The
            // top-level `--provider` is the route this process is actually on,
            // and it is already folded into `resolved_runtime` — treating it as
            // a hypothetical made `codewhale --provider moonshot --model
            // kimi-k3 model resolve` re-derive a registry default and report
            // `kimi-k2.7-code` while the runtime used `kimi-k3` (v0.9.1 kimi-k3 dogfood report). The
            // top-level `--model` was not consulted at all on that path.
            let subcommand_provider = provider;
            let queried = model.as_deref().map(str::trim).filter(|m| !m.is_empty());

            // With no explicit query, this reports the route the runtime would
            // actually take — the same answer `doctor` gives — rather than
            // re-deriving one from an empty flag set. Re-deriving is what made
            // a Z.ai config report `provider: deepseek` (#4832).
            if queried.is_none() && subcommand_provider.is_none() {
                let source = resolved_runtime.model_source;
                println!(
                    "requested: {}",
                    if source.is_explicit() {
                        resolved_runtime.model.as_str()
                    } else {
                        ""
                    }
                );
                println!("resolved: {}", resolved_runtime.model);
                println!("provider: {}", resolved_runtime.provider.as_str());
                println!("used_fallback: {}", !source.is_explicit());
                println!(
                    "provider_source: {}",
                    provider_source_label(resolved_runtime.provider_source)
                );
                println!("model_source: {}", source.as_str());
                return Ok(());
            }

            // An explicit model or provider makes this a hypothetical query
            // inside a named route. The subcommand provider wins; otherwise
            // the configured runtime provider remains authoritative. Model
            // text never authorizes switching providers or credential slots.
            let provider_hint = subcommand_provider.or(Some(resolved_runtime.provider));
            let resolved = registry.resolve(queried, provider_hint)?;
            println!("requested: {}", resolved.requested.unwrap_or_default());
            println!("resolved: {}", resolved.resolved.id);
            println!("provider: {}", resolved.resolved.provider.as_str());
            println!("used_fallback: {}", resolved.used_fallback);
            println!(
                "provider_source: {}",
                if subcommand_provider.is_some() {
                    "--provider".to_string()
                } else {
                    provider_source_label(resolved_runtime.provider_source)
                }
            );
            println!(
                "model_source: {}",
                if queried.is_some() {
                    "argument"
                } else {
                    // This branch is reachable only for an explicit
                    // subcommand provider with no requested model. The model
                    // therefore came from that provider's default, not from
                    // the configured runtime route we deliberately overrode.
                    "provider default"
                }
            );
            Ok(())
        }
        ModelCommand::Set { model } => {
            let trimmed = model.trim();
            if trimmed.is_empty() {
                bail!("Model name cannot be empty");
            }
            let canonical = canonical_model_for_set(trimmed);
            store.config.default_text_model = Some(canonical.to_string());
            store.save()?;
            println!("Default model set to '{canonical}'");
            Ok(())
        }
    }
}

/// The TUI passthrough a thread subcommand delegates as, if it delegates.
///
/// Exhaustive on purpose: a future `ThreadCommand` variant that starts a
/// session has to state its passthrough here, where the caller below routes it
/// through the one command builder that applies the telemetry floor.
fn thread_delegation(command: &ThreadCommand) -> Option<Vec<String>> {
    match command {
        ThreadCommand::Resume { thread_id } => Some(vec!["resume".to_string(), thread_id.clone()]),
        ThreadCommand::Fork { thread_id } => Some(vec!["fork".to_string(), thread_id.clone()]),
        ThreadCommand::List { .. }
        | ThreadCommand::Read { .. }
        | ThreadCommand::Archive { .. }
        | ThreadCommand::Unarchive { .. }
        | ThreadCommand::SetName { .. }
        | ThreadCommand::ClearName { .. } => None,
    }
}

fn run_thread_command(
    cli: &Cli,
    store: &mut ConfigStore,
    runtime_overrides: &CliRuntimeOverrides,
    command: ThreadCommand,
) -> Result<()> {
    // `thread resume`/`thread fork` start a full interactive session in the TUI
    // binary, so they delegate exactly like the top-level `resume` does —
    // through dispatcher, which forwards `--config` and states the
    // resolved telemetry value in the child's environment. They used to take a
    // bare command invocation that forwarded neither, so a session
    // launched this way re-resolved from `$CODEWHALE_HOME/config.toml` with no
    // overrides and armed telemetry even when the user had passed
    // `--telemetry false` or pointed `--config` at a file that said
    // `telemetry = false`.
    if let Some(passthrough) = thread_delegation(&command) {
        let resolved_runtime = resolve_runtime_for_dispatch(store, runtime_overrides);
        return run_tui_in_process(cli, &resolved_runtime, passthrough);
    }
    let state = StateStore::open(None)?;
    match command {
        ThreadCommand::List { all, limit } => {
            let threads = state.list_threads(ThreadListFilters {
                include_archived: all,
                limit,
            })?;
            for thread in threads {
                println!(
                    "{} | {} | {} | {}",
                    thread.id,
                    thread
                        .name
                        .clone()
                        .unwrap_or_else(|| "(unnamed)".to_string()),
                    thread.model_provider,
                    thread.cwd.display()
                );
            }
            Ok(())
        }
        ThreadCommand::Read { thread_id } => {
            let thread = state.get_thread(&thread_id)?;
            println!("{}", serde_json::to_string_pretty(&thread)?);
            Ok(())
        }
        ThreadCommand::Resume { .. } | ThreadCommand::Fork { .. } => {
            unreachable!("thread_delegation routes resume and fork before this match")
        }
        ThreadCommand::Archive { thread_id } => {
            state.mark_archived(&thread_id)?;
            println!("archived {thread_id}");
            Ok(())
        }
        ThreadCommand::Unarchive { thread_id } => {
            state.mark_unarchived(&thread_id)?;
            println!("unarchived {thread_id}");
            Ok(())
        }
        ThreadCommand::SetName { thread_id, name } => {
            let mut thread = state
                .get_thread(&thread_id)?
                .with_context(|| format!("thread not found: {thread_id}"))?;
            thread.name = Some(name);
            thread.updated_at = chrono::Utc::now().timestamp();
            state.upsert_thread(&thread)?;
            println!("renamed {thread_id}");
            Ok(())
        }
        ThreadCommand::ClearName { thread_id } => {
            let mut thread = state
                .get_thread(&thread_id)?
                .with_context(|| format!("thread not found: {thread_id}"))?;
            thread.name = None;
            thread.updated_at = chrono::Utc::now().timestamp();
            state.upsert_thread(&thread)?;
            println!("cleared name for {thread_id}");
            Ok(())
        }
    }
}

fn run_sandbox_command(command: SandboxCommand) -> Result<()> {
    match command {
        SandboxCommand::Check { command, ask } => {
            let engine = ExecPolicyEngine::new(Vec::new(), vec!["rm -rf".to_string()]);
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let decision = engine.check(ExecPolicyContext {
                command: &command,
                cwd: &cwd.display().to_string(),
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: ask.into(),
                sandbox_mode: Some("workspace-write"),
            })?;
            println!("{}", serde_json::to_string_pretty(&decision)?);
            Ok(())
        }
    }
}

fn run_app_server_command(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    args: AppServerArgs,
) -> Result<()> {
    // The full runtime API lives in the TUI crate behind `serve --http`/`--mobile`.
    // Rather than duplicate ~6.5k lines or add a CLI→TUI crate dependency, the
    // canonical `app-server --http`/`--mobile` entrypoint reuses that mature server
    // by delegating to the sibling TUI binary (the same mechanism `serve` uses).
    if args.http || args.mobile {
        // Delegated runtime API listener — supervise it so the child does not
        // outlive the dispatcher (#3259).
        return run_tui_server_in_process(
            cli,
            resolved_runtime,
            app_server_serve_passthrough(&args),
        );
    }

    // Everything below runs the app-server *in this process*, which is why the
    // surface cannot be derived from the executable: `current_exe()` would
    // report every one of these sessions as `cli`.
    let session = start_cli_telemetry(
        resolved_runtime,
        args.config.clone().or_else(|| cli.config.clone()),
        Surface::AppServer,
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let outcome = Err(error);
            finish_cli_telemetry(session, &outcome);
            return outcome;
        }
    };
    if args.stdio {
        let outcome = runtime.block_on(run_app_server_stdio(args.config));
        finish_cli_telemetry(session, &outcome);
        return outcome;
    }
    if args.socket {
        let outcome = runtime.block_on(run_daemon_socket(DaemonSocketOptions {
            socket_path: args.socket_path,
            config_path: args.config,
        }));
        finish_cli_telemetry(session, &outcome);
        return outcome;
    }
    // Legacy in-process app-server HTTP transport (`/healthz`, `/thread`, `/app`,
    // `/prompt`, `/tool`, `/jobs`). Kept for backward compatibility; defaults to
    // 127.0.0.1:8787 to avoid colliding with the runtime API default of :7878.
    // `/prompt` and `/thread` messages are not served locally: they run a real
    // turn by bridging to a runtime API child, and fail with an explicit
    // `runtime_unavailable` when one cannot be started.
    let host = args.host.as_deref().unwrap_or("127.0.0.1");
    let port = args.port.unwrap_or(8787);
    let outcome = format!("{host}:{port}")
        .parse::<SocketAddr>()
        .with_context(|| format!("invalid app-server listen address {host}:{port}"))
        .and_then(|listen| {
            runtime.block_on(run_app_server(AppServerOptions {
                listen,
                config_path: args.config,
                auth_token: args.auth_token.or_else(app_server_token_from_env),
                insecure_no_auth: args.insecure_no_auth,
                cors_origins: args.cors_origin,
            }))
        });
    finish_cli_telemetry(session, &outcome);
    outcome
}

/// Build the `serve` argv forwarded to the TUI binary for
/// `codewhale app-server --http`/`--mobile`. Maps app-server flags onto the
/// matching `serve` flags (note `--insecure-no-auth` → `--insecure`). The
/// subcommand-level `--config` is bridged through the global `--config` in the
/// dispatcher, so it is intentionally not part of this passthrough. An auth
/// token from the environment is deliberately *not* forwarded into child argv;
/// the runtime API reads CODEWHALE_RUNTIME_TOKEN/DEEPSEEK_RUNTIME_TOKEN itself.
fn app_server_serve_passthrough(args: &AppServerArgs) -> Vec<String> {
    let mut forwarded = vec!["serve".to_string()];
    forwarded.push(if args.mobile { "--mobile" } else { "--http" }.to_string());
    if let Some(host) = args.host.as_ref() {
        forwarded.push("--host".to_string());
        forwarded.push(host.clone());
    }
    if let Some(port) = args.port {
        forwarded.push("--port".to_string());
        forwarded.push(port.to_string());
    }
    if let Some(workers) = args.workers {
        forwarded.push("--workers".to_string());
        forwarded.push(workers.to_string());
    }
    for origin in &args.cors_origin {
        forwarded.push("--cors-origin".to_string());
        forwarded.push(origin.clone());
    }
    if let Some(token) = args.auth_token.as_ref() {
        forwarded.push("--auth-token".to_string());
        forwarded.push(token.clone());
    }
    if args.insecure_no_auth {
        forwarded.push("--insecure".to_string());
    }
    if args.qr {
        forwarded.push("--qr".to_string());
    }
    forwarded
}

fn web_serve_passthrough(args: &WebArgs) -> Vec<String> {
    vec![
        "serve".to_string(),
        "--web".to_string(),
        "--port".to_string(),
        args.port.to_string(),
    ]
}

fn app_server_token_from_env() -> Option<String> {
    std::env::var("CODEWHALE_APP_SERVER_TOKEN")
        .ok()
        .or_else(|| std::env::var("DEEPSEEK_APP_SERVER_TOKEN").ok())
}

fn run_mcp_server_command(store: &mut ConfigStore) -> Result<()> {
    let persisted = load_mcp_server_definitions(store);
    let updated = run_stdio_server(persisted)?;
    persist_mcp_server_definitions(store, &updated)
}

fn load_mcp_server_definitions(store: &ConfigStore) -> Vec<McpServerDefinition> {
    // `get_raw_string` first: `get_value` re-renders the extras entry as TOML,
    // which quotes a JSON payload into `'[{"config":…}]'` and makes it
    // unparseable — so every persisted definition was silently dropped and
    // `mcp-server` started with an empty server list (#4727). `get_value`
    // remains as the fallback for keys that are not plain extras strings.
    let raw = store
        .config
        .get_raw_string(MCP_SERVER_DEFINITIONS_KEY)
        .map(ToOwned::to_owned)
        .or_else(|| store.config.get_value(MCP_SERVER_DEFINITIONS_KEY));
    let Some(raw) = raw else {
        return Vec::new();
    };

    match parse_mcp_server_definitions(&raw) {
        Ok(definitions) => definitions,
        Err(err) => {
            eprintln!(
                "warning: failed to parse persisted MCP server definitions ({MCP_SERVER_DEFINITIONS_KEY}): {err}"
            );
            Vec::new()
        }
    }
}

fn parse_mcp_server_definitions(raw: &str) -> Result<Vec<McpServerDefinition>> {
    if let Ok(parsed) = serde_json::from_str::<Vec<McpServerDefinition>>(raw) {
        return Ok(parsed);
    }

    let unwrapped: String = serde_json::from_str(raw).map_err(|_| {
        anyhow!("invalid JSON payload at key {MCP_SERVER_DEFINITIONS_KEY}; contents were omitted")
    })?;
    serde_json::from_str::<Vec<McpServerDefinition>>(&unwrapped).map_err(|_| {
        anyhow!(
            "invalid MCP server definition list in key {MCP_SERVER_DEFINITIONS_KEY}; contents were omitted"
        )
    })
}

fn persist_mcp_server_definitions(
    store: &mut ConfigStore,
    definitions: &[McpServerDefinition],
) -> Result<()> {
    let encoded =
        serde_json::to_string(definitions).context("failed to encode MCP server definitions")?;
    store
        .config
        .set_value(MCP_SERVER_DEFINITIONS_KEY, &encoded)?;
    store.save()
}

/// Delegate a long-running server command (`serve --http`/`--mobile`,
/// `app-server --http`/`--mobile`) to the sibling TUI binary, supervising the
/// child so its listener does not outlive the dispatcher (#3259).
///
/// Plain [`run_tui_in_process`] blocks on `Command::status()`, which reaps the
/// child only on the child's own exit. If the dispatcher is terminated while
/// the delegated server is still running, the child can be reparented and keep
/// its listener bound. Here the child runs under a Tokio supervisor that
/// forwards termination (Ctrl+C / SIGTERM / SIGHUP) by killing and reaping the
/// child before the dispatcher exits, and `kill_on_drop` tears the child down
/// if the dispatcher unwinds.
///
/// For an *uncatchable* dispatcher death (SIGKILL, a hard crash) the Tokio
/// supervisor above can't run, so two OS-level safety nets are installed as
/// well (#3259): on Linux the child sets `PR_SET_PDEATHSIG` so the kernel
/// signals it when the dispatcher dies; on Windows the child is placed in a
/// kill-on-job-close Job Object so closing the dispatcher's handle (which the
/// OS does on process death) terminates it. macOS has no equivalent primitive,
/// so an uncatchable dispatcher death there can still orphan the child.

/// On Linux, ask the kernel to terminate the delegated server if the dispatcher
/// dies before it can run the graceful shutdown supervisor. This covers the
/// hard parent-death edge of #3259 for `SIGKILL`, OOM, or abrupt process exit.
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]

/// Outcome of supervising a delegated server child.
#[derive(Debug)]

/// Wait for the server `child` to exit, or for `shutdown` to fire first. On
/// shutdown, kill the child and reap it so no listener is left reparented.

/// Resolve when the dispatcher should tear down a delegated server child, and
/// the conventional `128 + signal` exit code to propagate: Ctrl+C on every
/// platform (130), plus SIGTERM (143) and SIGHUP (129) on Unix.
#[cfg(unix)]
#[cfg(not(unix))]

/// Assign the delegated server `child` to a kill-on-job-close Job Object so the
/// OS terminates it when the dispatcher's handle to the job closes — which it
/// does on any dispatcher exit, including an uncatchable kill (#3259). The
/// returned guard must be held for the dispatcher's lifetime. Best-effort:
/// returns `None` if the job cannot be created or assigned. Mirrors the Job
/// Object idiom in `crates/tui/src/tools/shell.rs`.
#[cfg(windows)]
#[cfg(windows)]
// SAFETY: the wrapped value is a process-wide kernel handle; moving it across
// threads does not invalidate it, and it is only ever closed once, on drop.
#[cfg(windows)]
unsafe impl Send for ServerChildJob {}

fn run_resume_command(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    args: TuiPassthroughArgs,
) -> Result<()> {
    let passthrough = tui_args("resume", args);
    if should_pick_resume_in_dispatcher(&passthrough, cfg!(windows)) {
        return run_dispatcher_resume_picker(cli, resolved_runtime);
    }
    run_tui_in_process(cli, resolved_runtime, passthrough)
}

fn run_dispatcher_resume_picker(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
) -> Result<()> {
    let argv = tui_argv(cli, vec!["sessions".to_string()]);
    apply_tui_env(cli, resolved_runtime, &argv);
    let code = codewhale_tui::run(argv);
    if code != std::process::ExitCode::SUCCESS {
        std::process::exit(if code == std::process::ExitCode::SUCCESS {
            0
        } else {
            1
        })
    }

    println!();
    println!("Windows note: enter a session id or prefix from the list above.");
    println!("You can also run `codewhale resume --last` to skip this prompt.");
    print!("Session id/prefix (Enter to cancel): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read session selection")?;
    let session_id = input.trim();
    if session_id.is_empty() {
        bail!("No session selected.");
    }

    run_tui_in_process(
        cli,
        resolved_runtime,
        vec!["resume".to_string(), session_id.to_string()],
    )
}

fn should_pick_resume_in_dispatcher(passthrough: &[String], is_windows: bool) -> bool {
    is_windows && passthrough == ["resume"]
}

fn run_tui_in_process(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
) -> Result<()> {
    let argv = tui_argv(cli, passthrough.clone());
    apply_tui_env(cli, resolved_runtime, &passthrough);
    let code = codewhale_tui::run(argv);
    std::process::exit(if code == std::process::ExitCode::SUCCESS {
        0
    } else {
        1
    })
}

fn run_tui_server_in_process(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
) -> Result<()> {
    let argv = tui_argv(cli, passthrough.clone());
    apply_tui_env(cli, resolved_runtime, &passthrough);
    let code = codewhale_tui::run(argv);
    std::process::exit(if code == std::process::ExitCode::SUCCESS {
        0
    } else {
        1
    })
}

fn tui_argv(cli: &Cli, passthrough: Vec<String>) -> Vec<String> {
    let mut args = Vec::new();
    args.push("codewhale".to_string());
    if let Some(config) = cli.config.as_deref() {
        args.push("--config".to_string());
        args.push(config.display().to_string());
    }
    if let Some(profile) = cli.profile.as_ref() {
        args.push("--profile".to_string());
        args.push(profile.clone());
    }
    if let Some(workspace) = cli.workspace.as_deref() {
        args.push("--workspace".to_string());
        args.push(workspace.display().to_string());
    }
    if cli.mouse_capture {
        args.push("--mouse-capture".to_string());
    }
    if cli.no_mouse_capture {
        args.push("--no-mouse-capture".to_string());
    }
    if cli.skip_onboarding {
        args.push("--skip-onboarding".to_string());
    }
    if cli.no_project_config {
        args.push("--no-project-config".to_string());
    }
    args.extend(passthrough);
    args
}

fn apply_tui_env(cli: &Cli, resolved_runtime: &ResolvedRuntimeOptions, passthrough: &[String]) {
    let mut verbosity = if cli.profile.is_some() {
        cli.verbosity.clone()
    } else {
        resolved_runtime.verbosity.clone()
    };
    if verbosity.is_none()
        && passthrough
            .iter()
            .any(|arg| matches!(arg.as_str(), "exec" | "eval"))
    {
        verbosity = Some("concise".to_string());
    }
    let uses_raw_tui_provider = cli
        .provider
        .as_deref()
        .is_some_and(|provider| builtin_provider_arg(provider).is_none());
    let keyring_bridge_provider = resolved_runtime.provider;
    let keyring_bridge_api_key = resolved_runtime.api_key.as_ref();
    let keyring_bridge_source = resolved_runtime.api_key_source;
    if let Some(provider) = cli.provider.as_deref() {
        let provider = builtin_provider_arg(provider).map_or_else(
            || provider.to_string(),
            |provider| provider.as_str().to_string(),
        );
        unsafe {
            std::env::set_var("CODEWHALE_PROVIDER", &provider);
            std::env::set_var("DEEPSEEK_PROVIDER", provider);
        }
    }
    if !(uses_raw_tui_provider
        || (cli.profile.is_some()
            && matches!(resolved_runtime.provider_source, ProviderSource::Config)))
        && matches!(keyring_bridge_source, Some(RuntimeApiKeySource::Keyring))
        && let Some(api_key) = keyring_bridge_api_key
    {
        unsafe {
            for var in provider_env_vars(keyring_bridge_provider) {
                std::env::set_var(var, api_key);
            }
            std::env::set_var(
                codewhale_config::CLI_API_KEY_SOURCE_ENV,
                RuntimeApiKeySource::Keyring.as_env_value(),
            );
        }
    }
    if let Some(model) = cli.model.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_MODEL", model);
            std::env::set_var("DEEPSEEK_MODEL", model);
        }
    }
    if let Some(output_mode) = cli.output_mode.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_OUTPUT_MODE", output_mode);
            std::env::set_var("DEEPSEEK_OUTPUT_MODE", output_mode);
        }
    }
    if let Some(v) = verbosity.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_VERBOSITY", v);
            std::env::set_var("DEEPSEEK_VERBOSITY", v);
        }
    }
    if let Some(log_level) = cli.log_level.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_LOG_LEVEL", log_level);
            std::env::set_var("DEEPSEEK_LOG_LEVEL", log_level);
        }
    }
    let telemetry = resolved_runtime.telemetry.to_string();
    unsafe {
        std::env::set_var("CODEWHALE_TELEMETRY", &telemetry);
        std::env::set_var("DEEPSEEK_TELEMETRY", &telemetry);
    }
    let floor = cli.telemetry == Some(false) || codewhale_config::telemetry_floor_in_force();
    unsafe {
        std::env::set_var(
            codewhale_config::TELEMETRY_FLOOR_ENV,
            if floor { "1" } else { "0" },
        );
    }
    if let Some(endpoint) = resolved_runtime.telemetry_endpoint.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_TELEMETRY_ENDPOINT", endpoint);
            std::env::set_var("DEEPSEEK_TELEMETRY_ENDPOINT", endpoint);
        }
    }
    if let Some(policy) = cli.approval_policy.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_APPROVAL_POLICY", policy);
            std::env::set_var("DEEPSEEK_APPROVAL_POLICY", policy);
        }
    }
    if let Some(mode) = cli.sandbox_mode.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_SANDBOX_MODE", mode);
            std::env::set_var("DEEPSEEK_SANDBOX_MODE", mode);
        }
    }
    if cli.yolo {
        unsafe {
            std::env::set_var("CODEWHALE_YOLO", "true");
        }
    }
    if let Some(api_key) = cli.api_key.as_ref() {
        unsafe {
            std::env::set_var(codewhale_config::CLI_API_KEY_ENV, api_key);
        }
        if !uses_raw_tui_provider && (cli.profile.is_none() || cli.provider.is_some()) {
            unsafe {
                for var in provider_env_vars(resolved_runtime.provider) {
                    std::env::set_var(var, api_key);
                }
            }
        }
        unsafe {
            std::env::set_var(codewhale_config::CLI_API_KEY_SOURCE_ENV, "cli");
        }
    }
    if let Some(base_url) = cli.base_url.as_ref() {
        unsafe {
            std::env::set_var("CODEWHALE_BASE_URL", base_url);
            std::env::set_var("DEEPSEEK_BASE_URL", base_url);
        }
    }
}

// There is deliberately no "just run the TUI with these args" helper here. One
// existed, `thread resume`/`thread fork` used it, and it forwarded neither
// `--config` nor the resolved telemetry value — so the kill switch the
// dispatcher had already applied never reached the process that emits. Every
// delegation is now in-process, and
// `only_one_function_may_locate_and_spawn_the_tui` pins that.

fn run_providers_command(args: ProvidersArgs) -> Result<()> {
    match args.command {
        ProvidersCommand::Export { json } => {
            if !json {
                bail!("`codewhale providers export` requires `--json`");
            }
            let export = ProvidersExport::from_registry(env!("CODEWHALE_BUILD_VERSION"));
            serde_json::to_writer_pretty(io::stdout(), &export)
                .context("failed to write providers export")?;
            println!();
            Ok(())
        }
    }
}

fn run_metrics_command(args: MetricsArgs) -> Result<()> {
    let since = match args.since.as_deref() {
        Some(s) => {
            Some(metrics::parse_since(s).with_context(|| format!("invalid --since value: {s:?}"))?)
        }
        None => None,
    };
    metrics::run(metrics::MetricsArgs {
        json: args.json,
        since,
    })
}

fn read_api_key_from_stdin() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read api key from stdin")?;
    let key = input.trim().to_string();
    if key.is_empty() {
        bail!("empty API key provided");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use codewhale_config::{ModelSource, ProviderSource};
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn parse_ok(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).unwrap_or_else(|err| panic!("parse failed for {argv:?}: {err}"))
    }

    fn help_for(argv: &[&str]) -> String {
        let err = Cli::try_parse_from(argv).expect_err("expected --help to short-circuit parsing");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        err.to_string()
    }

    pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) struct ScopedEnvVar {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvVar {
        pub(crate) fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            // Safety: tests using this helper serialize with env_lock() and
            // restore the original value in Drop.
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }

        pub(crate) fn remove(name: &'static str) -> Self {
            let previous = std::env::var_os(name);
            // Safety: tests using this helper serialize with env_lock() and
            // restore the original value in Drop.
            unsafe { std::env::remove_var(name) };
            Self { name, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // Safety: tests using this helper serialize with env_lock().
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.name, previous.clone());
                } else {
                    std::env::remove_var(self.name);
                }
            }
        }
    }

    #[derive(Default)]
    struct RecordingKeyringStore {
        gets: Mutex<Vec<String>>,
        values: Mutex<std::collections::BTreeMap<String, String>>,
    }

    impl RecordingKeyringStore {
        fn set_value(&self, key: &str, value: &str) {
            self.values
                .lock()
                .expect("recording values lock")
                .insert(key.to_string(), value.to_string());
        }

        fn queried(&self) -> Vec<String> {
            self.gets.lock().expect("recording gets lock").clone()
        }
    }

    impl codewhale_secrets::KeyringStore for RecordingKeyringStore {
        fn get(
            &self,
            key: &str,
        ) -> std::result::Result<Option<String>, codewhale_secrets::SecretsError> {
            self.gets
                .lock()
                .expect("recording gets lock")
                .push(key.to_string());
            Ok(self
                .values
                .lock()
                .expect("recording values lock")
                .get(key)
                .cloned())
        }

        fn set(
            &self,
            key: &str,
            value: &str,
        ) -> std::result::Result<(), codewhale_secrets::SecretsError> {
            self.set_value(key, value);
            Ok(())
        }

        fn delete(&self, key: &str) -> std::result::Result<(), codewhale_secrets::SecretsError> {
            self.values
                .lock()
                .expect("recording values lock")
                .remove(key);
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "recording"
        }
    }

    fn install_fake_tui_binary() -> (tempfile::TempDir, ScopedEnvVar) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy();
        let bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);
        (dir, bin)
    }

    fn resolved_runtime_for_test(
        provider: ProviderKind,
        provider_source: ProviderSource,
    ) -> ResolvedRuntimeOptions {
        ResolvedRuntimeOptions {
            provider,
            provider_source,
            model: "test-model".to_string(),
            model_source: ModelSource::ProviderDefault,
            api_key: None,
            api_key_source: None,
            base_url: "http://localhost:8000/v1".to_string(),
            auth_mode: None,
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            telemetry_source: codewhale_config::TelemetrySource::Default,
            telemetry_explicit_off: false,
            telemetry_endpoint: None,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
            route: None,
        }
    }

    #[test]
    fn tui_credential_handoff_stays_with_the_selected_provider() {
        let _lock = env_lock();
        let mut names = ProviderKind::ALL
            .into_iter()
            .flat_map(provider_env_vars)
            .copied()
            .collect::<Vec<_>>();
        names.extend([
            codewhale_config::CLI_API_KEY_ENV,
            codewhale_config::CLI_API_KEY_SOURCE_ENV,
            codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV,
            "CODEWHALE_PROVIDER",
            "DEEPSEEK_PROVIDER",
            "CODEWHALE_TELEMETRY",
            "DEEPSEEK_TELEMETRY",
            codewhale_config::TELEMETRY_FLOOR_ENV,
        ]);
        names.sort_unstable();
        names.dedup();
        let _clean_env = names
            .into_iter()
            .map(ScopedEnvVar::remove)
            .collect::<Vec<_>>();
        let _deepseek_key = ScopedEnvVar::set("DEEPSEEK_API_KEY", "existing-deepseek-key");

        let clear_bridge = || {
            // Safety: this test holds env_lock() and the guards above restore
            // every touched variable.
            unsafe {
                for var in ProviderKind::ALL
                    .into_iter()
                    .flat_map(provider_env_vars)
                    .filter(|var| **var != "DEEPSEEK_API_KEY")
                {
                    std::env::remove_var(var);
                }
                std::env::remove_var(codewhale_config::CLI_API_KEY_ENV);
                std::env::remove_var(codewhale_config::CLI_API_KEY_SOURCE_ENV);
                std::env::remove_var(codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV);
            }
        };

        for (provider_arg, provider) in [
            ("nvidia-nim", ProviderKind::NvidiaNim),
            ("openrouter", ProviderKind::Openrouter),
            ("anthropic", ProviderKind::Anthropic),
        ] {
            clear_bridge();
            let keyring_key = format!("{provider_arg}-keyring-key");
            let mut keyring_runtime = resolved_runtime_for_test(provider, ProviderSource::Cli);
            keyring_runtime.api_key = Some(keyring_key.clone());
            keyring_runtime.api_key_source = Some(RuntimeApiKeySource::Keyring);
            let keyring_cli = parse_ok(&["codewhale", "--provider", provider_arg]);

            apply_tui_env(&keyring_cli, &keyring_runtime, &[]);

            assert_eq!(
                std::env::var("DEEPSEEK_API_KEY").as_deref(),
                Ok("existing-deepseek-key"),
                "{provider_arg} keyring handoff replaced DeepSeek's credential"
            );
            for var in provider_env_vars(provider) {
                assert_eq!(
                    std::env::var(var).as_deref(),
                    Ok(keyring_key.as_str()),
                    "{provider_arg} keyring handoff missed {var}"
                );
            }
            assert_eq!(
                std::env::var(codewhale_config::CLI_API_KEY_SOURCE_ENV).as_deref(),
                Ok("keyring")
            );
            assert!(std::env::var(codewhale_config::CLI_API_KEY_ENV).is_err());
            assert!(
                std::env::var(codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV).is_err(),
                "new dispatchers must not write the retired vendor-named marker"
            );

            clear_bridge();
            let explicit_key = format!("{provider_arg}-explicit-key");
            let explicit_cli = parse_ok(&[
                "codewhale",
                "--provider",
                provider_arg,
                "--api-key",
                explicit_key.as_str(),
            ]);
            let explicit_runtime = resolved_runtime_for_test(provider, ProviderSource::Cli);

            apply_tui_env(&explicit_cli, &explicit_runtime, &[]);

            assert_eq!(
                std::env::var("DEEPSEEK_API_KEY").as_deref(),
                Ok("existing-deepseek-key"),
                "{provider_arg} explicit CLI credential handoff replaced DeepSeek's credential"
            );
            for var in provider_env_vars(provider) {
                assert_eq!(
                    std::env::var(var).as_deref(),
                    Ok(explicit_key.as_str()),
                    "{provider_arg} explicit CLI credential handoff missed {var}"
                );
            }
            assert_eq!(
                std::env::var(codewhale_config::CLI_API_KEY_ENV).as_deref(),
                Ok(explicit_key.as_str())
            );
            assert_eq!(
                std::env::var(codewhale_config::CLI_API_KEY_SOURCE_ENV).as_deref(),
                Ok("cli")
            );
            assert!(std::env::var(codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV).is_err());
        }
    }

    #[test]
    fn yolo_flag_writes_only_the_codewhale_env_var() {
        let _lock = env_lock();
        let _guards = [
            ScopedEnvVar::remove("CODEWHALE_TELEMETRY"),
            ScopedEnvVar::remove("DEEPSEEK_TELEMETRY"),
            ScopedEnvVar::remove(codewhale_config::TELEMETRY_FLOOR_ENV),
            ScopedEnvVar::remove("CODEWHALE_YOLO"),
            ScopedEnvVar::remove("DEEPSEEK_YOLO"),
        ];

        let cli = parse_ok(&["codewhale", "--yolo"]);
        let runtime = resolved_runtime_for_test(ProviderKind::NvidiaNim, ProviderSource::Cli);
        apply_tui_env(&cli, &runtime, &[]);

        assert_eq!(
            std::env::var("CODEWHALE_YOLO").as_deref(),
            Ok("true"),
            "--yolo must still enable the posture via CODEWHALE_YOLO"
        );
        assert!(
            std::env::var("DEEPSEEK_YOLO").is_err(),
            "--yolo must not write the retired DEEPSEEK_YOLO alias (#5443)"
        );
    }

    #[test]
    fn clap_command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    // Regression for #767: `run_cli` prints the full anyhow chain so users
    // see the underlying TOML parser error (line/column, expected token)
    // instead of just the top-level "failed to parse config at <path>"
    // wrapper. anyhow's bare `Display` impl drops the chain — pin both
    // pieces here so a future refactor of the printing path doesn't
    // silently regress.
    #[test]
    fn anyhow_chain_surfaces_toml_parse_cause() {
        use anyhow::Context;
        let inner = anyhow::anyhow!("TOML parse error at line 1, column 20");
        let err = Err::<(), _>(inner)
            .context("failed to parse config at C:\\Users\\test\\.deepseek\\config.toml")
            .unwrap_err();

        // What `eprintln!("error: {err}")` prints (top context only).
        assert_eq!(
            err.to_string(),
            "failed to parse config at C:\\Users\\test\\.deepseek\\config.toml",
        );

        // What the `for cause in err.chain().skip(1)` loop iterates over.
        let causes: Vec<String> = err.chain().skip(1).map(ToString::to_string).collect();
        assert_eq!(causes, vec!["TOML parse error at line 1, column 20"]);
    }

    #[test]
    fn malformed_persisted_mcp_json_omits_secret_contents_and_keys() {
        let secret = "sentinel";
        let raw =
            format!(r#"[{{"name":"private","env":{{"PRIVATE_TOKEN":"{secret}"}} trailing-junk}}]"#);
        let error = parse_mcp_server_definitions(&raw).expect_err("malformed JSON must fail");
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains(secret), "{diagnostic}");
        assert!(!diagnostic.contains("PRIVATE_TOKEN"), "{diagnostic}");
        assert!(diagnostic.contains("contents were omitted"), "{diagnostic}");
    }

    #[test]
    fn parses_config_command_matrix() {
        let cli = parse_ok(&["deepseek", "config", "get", "provider"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommand::Get { ref key }
            })) if key == "provider"
        ));

        let cli = parse_ok(&["deepseek", "config", "set", "model", "deepseek-v4-flash"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommand::Set { ref key, ref value }
            })) if key == "model" && value == "deepseek-v4-flash"
        ));

        let cli = parse_ok(&["deepseek", "config", "unset", "model"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommand::Unset { ref key }
            })) if key == "model"
        ));

        assert!(matches!(
            parse_ok(&["deepseek", "config", "list"]).command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommand::List
            }))
        ));
        assert!(matches!(
            parse_ok(&["deepseek", "config", "path"]).command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommand::Path
            }))
        ));
    }

    fn config_dispatch_from(
        argv: &[OsString],
        cwd: &Path,
    ) -> (Option<PathBuf>, ConfigCommand, bool) {
        let matches = Cli::command()
            .try_get_matches_from(argv.iter().cloned())
            .unwrap_or_else(|error| panic!("config command should parse: {error}"));
        let project_bundle_scope = config_command_targets_project(&matches);
        let cli = Cli::from_arg_matches(&matches)
            .unwrap_or_else(|error| panic!("config command should decode: {error}"));
        let selected_path = config_store_path_for_dispatch(cli.config, project_bundle_scope, cwd);
        let Some(Commands::Config(ConfigArgs { command })) = cli.command else {
            panic!("expected config command");
        };
        (selected_path, command, project_bundle_scope)
    }

    fn write_config_fixture(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().expect("config should have a parent"))
            .expect("create config parent");
        std::fs::write(path, body).expect("write config fixture");
    }

    #[test]
    fn project_config_dispatch_prefers_current_app_dir_and_falls_back_to_legacy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let current = workspace.join(".codewhale/config.toml");
        let legacy = workspace.join(".deepseek/config.toml");

        // Fresh workspace: create under the current app dir.
        std::fs::create_dir_all(&workspace).expect("workspace");
        assert_eq!(
            config_store_path_for_dispatch(None, true, &workspace),
            Some(current.clone())
        );

        // Legacy-only workspace: operate on the legacy document in place.
        write_config_fixture(&legacy, "verbosity = \"legacy\"\n");
        assert_eq!(
            config_store_path_for_dispatch(None, true, &workspace),
            Some(legacy.clone())
        );

        // Both present: the current app dir wins, matching the loader.
        write_config_fixture(&current, "verbosity = \"current\"\n");
        assert_eq!(
            config_store_path_for_dispatch(None, true, &workspace),
            Some(current.clone())
        );

        // An explicit --config path always wins; without --project nothing is selected.
        let explicit = temp.path().join("explicit.toml");
        assert_eq!(
            config_store_path_for_dispatch(Some(explicit.clone()), true, &workspace),
            Some(explicit)
        );
        assert_eq!(
            config_store_path_for_dispatch(None, false, &workspace),
            None
        );
    }

    #[test]
    fn project_config_import_dispatches_to_the_cwd_document() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".git")).expect("create checkout marker");
        let project_path = workspace.join(".codewhale/config.toml");
        let global_path = temp.path().join("global-config.toml");
        write_config_fixture(&project_path, "verbosity = \"project-before\"\n");
        write_config_fixture(&global_path, "verbosity = \"global-only\"\n");

        let bundle_path = temp.path().join("project-bundle.toml");
        std::fs::write(
            &bundle_path,
            r#"schema_version = 1
kind = "codewhale.portable-config"

[project]
verbosity = "project-imported"
"#,
        )
        .expect("write project bundle");
        let argv = [
            OsString::from("codewhale"),
            OsString::from("config"),
            OsString::from("import"),
            bundle_path.as_os_str().to_owned(),
            OsString::from("--yes"),
            OsString::from("--project"),
        ];
        let (selected_path, command, project_bundle_scope) =
            config_dispatch_from(&argv, &workspace);
        assert_eq!(selected_path.as_deref(), Some(project_path.as_path()));

        let mut store = ConfigStore::load(selected_path).expect("load selected project config");
        run_config_command(&mut store, command, project_bundle_scope)
            .expect("import project bundle");
        let project = ConfigStore::load(Some(project_path.clone())).expect("reload project");
        let global = ConfigStore::load(Some(global_path.clone())).expect("reload global");
        assert_eq!(
            project.config.verbosity.as_deref(),
            Some("project-imported")
        );
        assert_eq!(global.config.verbosity.as_deref(), Some("global-only"));

        let explicit_argv = [
            OsString::from("codewhale"),
            OsString::from("--config"),
            global_path.as_os_str().to_owned(),
            OsString::from("config"),
            OsString::from("import"),
            bundle_path.as_os_str().to_owned(),
            OsString::from("--yes"),
            OsString::from("--project"),
        ];
        let global_before = std::fs::read(&global_path).expect("read global before refusal");
        let (selected_path, command, project_bundle_scope) =
            config_dispatch_from(&explicit_argv, &workspace);
        assert_eq!(selected_path.as_deref(), Some(global_path.as_path()));
        let mut explicit_store =
            ConfigStore::load(selected_path).expect("load explicit global config");
        let error = run_config_command(&mut explicit_store, command, project_bundle_scope)
            .expect_err("project import must reject an explicit non-workspace config");
        assert!(
            error
                .to_string()
                .contains("--project requires a workspace config"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(&global_path).expect("read global after refusal"),
            global_before
        );
    }

    #[test]
    fn project_config_export_reads_the_cwd_document() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".git")).expect("create checkout marker");
        let project_path = workspace.join(".codewhale/config.toml");
        let global_path = temp.path().join("global-config.toml");
        let output_path = temp.path().join("portable.toml");
        write_config_fixture(&project_path, "verbosity = \"project-only\"\n");
        write_config_fixture(&global_path, "verbosity = \"global-only\"\n");

        let argv = [
            OsString::from("codewhale"),
            OsString::from("config"),
            OsString::from("export"),
            OsString::from("--portable"),
            OsString::from("--project"),
            OsString::from("--out"),
            output_path.as_os_str().to_owned(),
        ];
        let (selected_path, command, project_bundle_scope) =
            config_dispatch_from(&argv, &workspace);
        assert_eq!(selected_path.as_deref(), Some(project_path.as_path()));

        let mut store = ConfigStore::load(selected_path).expect("load selected project config");
        run_config_command(&mut store, command, project_bundle_scope)
            .expect("export project bundle");
        let body = std::fs::read_to_string(&output_path).expect("read portable export");
        let bundle = config_bundles::parse_bundle_str(&body, "portable.toml")
            .expect("parse portable export");
        assert_eq!(
            bundle
                .project
                .entries
                .get("verbosity")
                .and_then(toml::Value::as_str),
            Some("project-only")
        );
        assert!(bundle.global.entries.is_empty());

        let explicit_output_path = temp.path().join("explicit-portable.toml");
        let explicit_argv = [
            OsString::from("codewhale"),
            OsString::from("--config"),
            global_path.as_os_str().to_owned(),
            OsString::from("config"),
            OsString::from("export"),
            OsString::from("--portable"),
            OsString::from("--project"),
            OsString::from("--out"),
            explicit_output_path.as_os_str().to_owned(),
        ];
        let global_before = std::fs::read(&global_path).expect("read global before refusal");
        let (selected_path, command, project_bundle_scope) =
            config_dispatch_from(&explicit_argv, &workspace);
        assert_eq!(selected_path.as_deref(), Some(global_path.as_path()));
        let mut explicit_store =
            ConfigStore::load(selected_path).expect("load explicit global config");
        let error = run_config_command(&mut explicit_store, command, project_bundle_scope)
            .expect_err("project export must reject an explicit non-workspace config");
        assert!(
            error
                .to_string()
                .contains("--project requires a workspace config"),
            "{error:#}"
        );
        assert!(!explicit_output_path.exists());
        assert_eq!(
            std::fs::read(&global_path).expect("read global after refusal"),
            global_before
        );
    }

    #[test]
    fn parses_update_beta_flag() {
        let cli = parse_ok(&["codewhale", "update"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Update(UpdateArgs {
                beta: false,
                check: false,
                proxy: None
            }))
        ));

        let cli = parse_ok(&["codewhale", "update", "--beta"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Update(UpdateArgs {
                beta: true,
                check: false,
                proxy: None
            }))
        ));

        let cli = parse_ok(&["codewhale", "update", "--check"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Update(UpdateArgs {
                beta: false,
                check: true,
                proxy: None
            }))
        ));

        let cli = parse_ok(&["codewhale", "update", "--proxy", "socks5://127.0.0.1:1080"]);
        let Some(Commands::Update(args)) = cli.command else {
            panic!("expected update command");
        };
        assert!(!args.beta);
        assert!(!args.check);
        assert_eq!(args.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
    }

    #[test]
    fn parses_model_command_matrix() {
        let cli = parse_ok(&["deepseek", "model", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::List { provider: None }
            }))
        ));

        let cli = parse_ok(&["deepseek", "model", "list", "--provider", "openai"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::List {
                    provider: Some(ProviderKind::Openai)
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "model", "resolve", "deepseek-v4-flash"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::Resolve {
                    model: Some(ref model),
                    provider: None
                }
            })) if model == "deepseek-v4-flash"
        ));

        let cli = parse_ok(&[
            "deepseek",
            "model",
            "resolve",
            "--provider",
            "deepseek",
            "deepseek-v4-pro",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::Resolve {
                    model: Some(ref model),
                    provider: Some(ProviderKind::Deepseek)
                }
            })) if model == "deepseek-v4-pro"
        ));

        let cli = parse_ok(&["deepseek", "model", "set", "pro"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::Set { ref model }
            })) if model == "pro"
        ));
    }

    #[test]
    fn model_command_provider_hint_uses_subcommand_then_top_level_provider() {
        assert_eq!(
            model_command_provider_hint(None, Some(ProviderKind::Zai)),
            Some(ProviderKind::Zai)
        );
        assert_eq!(
            model_command_provider_hint(Some(ProviderKind::Minimax), Some(ProviderKind::Zai)),
            Some(ProviderKind::Minimax)
        );
        assert_eq!(model_command_provider_hint(None, None), None);

        let cli = parse_ok(&["codewhale", "--provider", "zai", "model", "list"]);
        assert_eq!(cli.provider.as_deref(), Some("zai"));
        assert!(matches!(
            cli.command,
            Some(Commands::Model(ModelArgs {
                command: ModelCommand::List { provider: None }
            }))
        ));
    }

    #[test]
    fn model_set_canonicalizes_deepseek_vision_aliases() {
        for alias in ["flash-vision", "deepseek-v4flashvisionexp"] {
            assert_eq!(
                canonical_model_for_set(alias),
                "deepseek-v4-flash-vision-exp"
            );
        }
        assert_eq!(
            canonical_model_for_set("deepseek-v4-flash-vision-exp"),
            "deepseek-v4-flash-vision-exp"
        );
    }

    #[test]
    fn parses_thread_command_matrix() {
        let cli = parse_ok(&["deepseek", "thread", "list", "--all", "--limit", "50"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::List {
                    all: true,
                    limit: Some(50)
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "thread", "read", "thread-1"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::Read { ref thread_id }
            })) if thread_id == "thread-1"
        ));

        let cli = parse_ok(&["deepseek", "thread", "resume", "thread-2"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::Resume { ref thread_id }
            })) if thread_id == "thread-2"
        ));

        let cli = parse_ok(&["deepseek", "thread", "fork", "thread-3"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::Fork { ref thread_id }
            })) if thread_id == "thread-3"
        ));

        let cli = parse_ok(&["deepseek", "thread", "archive", "thread-4"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::Archive { ref thread_id }
            })) if thread_id == "thread-4"
        ));

        let cli = parse_ok(&["deepseek", "thread", "unarchive", "thread-5"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::Unarchive { ref thread_id }
            })) if thread_id == "thread-5"
        ));

        let cli = parse_ok(&["deepseek", "thread", "set-name", "thread-6", "My Thread"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::SetName {
                    ref thread_id,
                    ref name
                }
            })) if thread_id == "thread-6" && name == "My Thread"
        ));

        let cli = parse_ok(&["deepseek", "thread", "clear-name", "thread-7"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Thread(ThreadArgs {
                command: ThreadCommand::ClearName { ref thread_id }
            })) if thread_id == "thread-7"
        ));
    }

    #[test]
    fn parses_sandbox_app_server_and_completion_matrix() {
        let cli = parse_ok(&[
            "deepseek",
            "sandbox",
            "check",
            "echo hello",
            "--ask",
            "on-failure",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Sandbox(SandboxArgs {
                command: SandboxCommand::Check {
                    ref command,
                    ask: ApprovalModeArg::OnFailure
                }
            })) if command == "echo hello"
        ));

        let cli = parse_ok(&[
            "deepseek",
            "app-server",
            "--host",
            "0.0.0.0",
            "--port",
            "9999",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::AppServer(AppServerArgs {
                host: Some(ref host),
                port: Some(9999),
                stdio: false,
                http: false,
                mobile: false,
                ..
            })) if host == "0.0.0.0"
        ));

        let cli = parse_ok(&["deepseek", "app-server", "--stdio"]);
        assert!(matches!(
            cli.command,
            Some(Commands::AppServer(AppServerArgs { stdio: true, .. }))
        ));

        let cli = parse_ok(&["deepseek", "completion", "bash"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Completion { shell: Shell::Bash })
        ));
    }

    /// The `[[bin]] name` declared in this crate's manifest is the only thing a
    /// user ever types. Read it from disk rather than restating it, so renaming
    /// the binary without re-pointing the completion generator fails here
    /// instead of silently shipping a script nobody's shell loads (#5526).
    fn declared_bin_name() -> String {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read crates/cli/Cargo.toml");
        let bin_section = manifest
            .split("[[bin]]")
            .nth(1)
            .expect("crates/cli/Cargo.toml declares a [[bin]] target");
        for line in bin_section.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("name") {
                let value = rest.trim_start().trim_start_matches('=').trim();
                return value.trim_matches('"').to_string();
            }
        }
        panic!("[[bin]] section has no name key");
    }

    #[test]
    fn completion_bin_name_matches_the_declared_bin_target() {
        assert_eq!(
            COMPLETION_BIN_NAME,
            declared_bin_name(),
            "completion scripts must register the binary this crate actually builds"
        );
    }

    /// Issue #5526: `codewhale completions <shell>` used to forward to the
    /// in-tree `codewhale-tui` binary, so every generated script registered
    /// `codewhale-tui` — not a GitHub-release command — and exposed the TUI's
    /// smaller subcommand tree. Pin the registered names per shell.
    #[test]
    fn generated_completion_scripts_register_the_published_command_names() {
        let bin = declared_bin_name();
        let alias = COMPLETION_ALIAS_NAME;

        // Match whole lines throughout: `codew` is a prefix of `codewhale`,
        // so a substring check for the alias is satisfied by the primary
        // binding and would pass on an unfixed build.
        let has_line =
            |script: &str, wanted: &str| script.lines().any(|line| line.trim() == wanted);

        let bash = render_completion_script(Shell::Bash);
        assert!(
            has_line(
                &bash,
                &format!("complete -F _{bin} -o bashdefault -o default {bin}")
            ),
            "bash script must bind the real binary name:\n{bash}"
        );
        assert!(
            has_line(
                &bash,
                &format!("complete -F _{bin} -o bashdefault -o default {alias}")
            ),
            "bash script must bind the {alias} shorthand too"
        );

        let zsh = render_completion_script(Shell::Zsh);
        assert_eq!(
            zsh.lines().next(),
            Some(format!("#compdef {bin} {alias}").as_str()),
            "zsh compdef tag line must list both published command names"
        );
        assert!(
            has_line(&zsh, &format!("compdef _{bin} {bin}")),
            "zsh script must bind {bin} on the sourced path"
        );
        assert!(
            has_line(&zsh, &format!("compdef _{bin} {alias}")),
            "zsh script must bind {alias} on the sourced path too"
        );

        let fish = render_completion_script(Shell::Fish);
        assert!(
            fish.contains(&format!("complete -c {bin} ")),
            "fish script must complete the real binary name"
        );
        assert!(
            has_line(&fish, &format!("complete -c {alias} -w {bin}")),
            "fish script must wrap the {alias} shorthand onto {bin}"
        );

        let powershell = render_completion_script(Shell::PowerShell);
        assert!(
            powershell.contains(&format!(
                "Register-ArgumentCompleter -Native -CommandName '{bin}','{alias}'"
            )),
            "PowerShell script must register both published command names"
        );

        let elvish = render_completion_script(Shell::Elvish);
        assert!(
            has_line(
                &elvish,
                &format!("set edit:completion:arg-completer[{bin}] = {{|@words|")
            ),
            "elvish script must bind the real binary name:\n{elvish}"
        );
        assert!(
            has_line(
                &elvish,
                &format!(
                    "set edit:completion:arg-completer[{alias}] = $edit:completion:arg-completer[{bin}]"
                )
            ),
            "elvish script must alias the {alias} shorthand onto {bin}"
        );

        for (shell, script) in [
            ("bash", &bash),
            ("zsh", &zsh),
            ("fish", &fish),
            ("powershell", &powershell),
            ("elvish", &elvish),
        ] {
            assert!(
                !script.contains("codewhale-tui"),
                "{shell} completions leaked the in-tree codewhale-tui name (#5526)"
            );
        }
    }

    /// The other half of #5526: the script has to describe *this* CLI's
    /// commands. Rendering from a different clap tree would drop or invent
    /// subcommands, which is exactly how the forwarded script went stale.
    #[test]
    fn generated_completion_scripts_cover_the_real_subcommand_surface() {
        let bash = render_completion_script(Shell::Bash);
        for sub in Cli::command().get_subcommands() {
            if sub.is_hide_set() {
                continue;
            }
            let name = sub.get_name();
            assert!(
                bash.contains(name),
                "bash completions omit the `{name}` subcommand"
            );
        }
    }

    /// `completions` is what the issue reporter typed and what the TUI called
    /// it; keep it working, now as an alias that renders in-process.
    #[test]
    fn completions_is_an_alias_for_completion() {
        assert!(matches!(
            parse_ok(&["codewhale", "completions", "powershell"]).command,
            Some(Commands::Completion {
                shell: Shell::PowerShell
            })
        ));
    }

    #[test]
    fn app_server_transports_are_mutually_exclusive() {
        assert!(matches!(
            parse_ok(&["deepseek", "app-server", "--http"]).command,
            Some(Commands::AppServer(AppServerArgs {
                http: true,
                mobile: false,
                stdio: false,
                ..
            }))
        ));
        assert!(matches!(
            parse_ok(&["deepseek", "app-server", "--mobile"]).command,
            Some(Commands::AppServer(AppServerArgs {
                mobile: true,
                http: false,
                stdio: false,
                ..
            }))
        ));

        assert!(matches!(
            parse_ok(&["deepseek", "app-server", "--socket"]).command,
            Some(Commands::AppServer(AppServerArgs {
                socket: true,
                socket_path: None,
                http: false,
                mobile: false,
                stdio: false,
                ..
            }))
        ));

        for argv in [
            ["deepseek", "app-server", "--http", "--mobile"].as_slice(),
            ["deepseek", "app-server", "--http", "--stdio"].as_slice(),
            ["deepseek", "app-server", "--mobile", "--stdio"].as_slice(),
            ["deepseek", "app-server", "--socket", "--stdio"].as_slice(),
            ["deepseek", "app-server", "--socket", "--http"].as_slice(),
            ["deepseek", "app-server", "--socket", "--mobile"].as_slice(),
        ] {
            let err = Cli::try_parse_from(argv).expect_err("conflicting transports must fail");
            assert_eq!(err.kind(), ErrorKind::ArgumentConflict, "argv={argv:?}");
        }
    }

    #[test]
    fn app_server_socket_path_requires_socket() {
        let err = Cli::try_parse_from(["deepseek", "app-server", "--socket-path", "/tmp/d.sock"])
            .expect_err("--socket-path without --socket must fail");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        match parse_ok(&[
            "deepseek",
            "app-server",
            "--socket",
            "--socket-path",
            "/tmp/d.sock",
        ])
        .command
        {
            Some(Commands::AppServer(AppServerArgs {
                socket: true,
                socket_path: Some(path),
                ..
            })) => assert_eq!(path, PathBuf::from("/tmp/d.sock")),
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn app_server_qr_requires_mobile() {
        let err = Cli::try_parse_from(["deepseek", "app-server", "--qr"])
            .expect_err("--qr without --mobile must fail");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert!(matches!(
            parse_ok(&["deepseek", "app-server", "--mobile", "--qr"]).command,
            Some(Commands::AppServer(AppServerArgs {
                mobile: true,
                qr: true,
                ..
            }))
        ));
    }

    #[test]
    fn app_server_serve_passthrough_maps_flags_to_serve() {
        let args = AppServerArgs {
            http: true,
            mobile: false,
            stdio: false,
            socket: false,
            socket_path: None,
            qr: false,
            host: Some("127.0.0.1".to_string()),
            port: Some(9000),
            workers: Some(4),
            config: None,
            auth_token: Some("tok".to_string()),
            insecure_no_auth: true,
            cors_origin: vec!["http://localhost:5173".to_string()],
        };
        let argv = app_server_serve_passthrough(&args);
        let as_str: Vec<&str> = argv.iter().map(String::as_str).collect();
        // app-server's --insecure-no-auth maps onto serve's --insecure.
        assert_eq!(
            as_str,
            vec![
                "serve",
                "--http",
                "--host",
                "127.0.0.1",
                "--port",
                "9000",
                "--workers",
                "4",
                "--cors-origin",
                "http://localhost:5173",
                "--auth-token",
                "tok",
                "--insecure",
            ]
        );
    }

    #[test]
    fn app_server_serve_passthrough_mobile_defaults_are_minimal() {
        let args = AppServerArgs {
            http: false,
            mobile: true,
            stdio: false,
            socket: false,
            socket_path: None,
            qr: true,
            host: None,
            port: None,
            workers: None,
            config: None,
            auth_token: None,
            insecure_no_auth: false,
            cors_origin: vec![],
        };
        let argv = app_server_serve_passthrough(&args);
        let as_str: Vec<&str> = argv.iter().map(String::as_str).collect();
        // No host/port forwarded → serve applies its own --mobile 0.0.0.0 default.
        // No auth token is injected from the environment into child argv.
        assert_eq!(as_str, vec!["serve", "--mobile", "--qr"]);
    }

    #[test]
    fn web_command_is_typed_and_delegates_without_auth_material() {
        let cli = parse_ok(&["codewhale", "web", "--port", "9091"]);
        let args = match cli.command {
            Some(Commands::Web(args)) => args,
            other => panic!("expected web command, got {other:?}"),
        };
        assert_eq!(args.port, 9091);
        let forwarded = web_serve_passthrough(&args);
        assert_eq!(forwarded, ["serve", "--web", "--port", "9091"]);
        assert!(!forwarded.iter().any(|arg| arg.contains("token")));
    }

    #[test]
    fn web_command_defaults_to_runtime_port_and_documents_bootstrap_boundary() {
        let cli = parse_ok(&["codewhale", "web"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Web(WebArgs { port: 7878 }))
        ));
        let help = help_for(&["codewhale", "web", "--help"]);
        assert!(help.contains("--port"));
        assert!(help.contains("one-time loopback bootstrap"));
        assert!(!help.contains("--auth-token"));
    }

    #[test]
    fn serve_help_documents_forwarded_runtime_modes() {
        let help = help_for(&["codewhale", "serve", "--help"]);
        for flag in ["--http", "--mobile", "--web", "--mcp", "--acp"] {
            assert!(
                help.contains(flag),
                "serve help should document forwarded flag {flag}; help was:\n{help}"
            );
        }
        assert!(help.contains("compatibility"));
    }

    #[test]
    fn parses_direct_tui_command_aliases() {
        let cli = parse_ok(&["deepseek", "doctor"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Doctor(TuiPassthroughArgs { ref args })) if args.is_empty()
        ));

        let cli = parse_ok(&["deepseek", "models", "--json"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Models(TuiPassthroughArgs { ref args })) if args == &["--json"]
        ));

        let cli = parse_ok(&["deepseek", "resume", "abc123"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Resume(TuiPassthroughArgs { ref args })) if args == &["abc123"]
        ));

        let cli = parse_ok(&["deepseek", "setup", "--skills", "--local"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Setup(TuiPassthroughArgs { ref args }))
                if args == &["--skills", "--local"]
        ));

        let cli = parse_ok(&["codewhale", "fleet", "init"]);
        assert!(cli.prompt.is_empty());
        assert!(matches!(
            cli.command,
            Some(Commands::Fleet(TuiPassthroughArgs { ref args })) if args == &["init"]
        ));

        let cli = parse_ok(&[
            "codewhale",
            "fleet",
            "run",
            "tasks.json",
            "--max-workers",
            "2",
        ]);
        assert!(cli.prompt.is_empty());
        assert!(matches!(
            cli.command,
            Some(Commands::Fleet(TuiPassthroughArgs { ref args }))
                if args == &["run", "tasks.json", "--max-workers", "2"]
        ));

        let cli = parse_ok(&[
            "codewhale",
            "workflow",
            "run",
            "stopship",
            "--fleet",
            "stopship",
            "--runtime",
            "tmux",
            "--issue",
            "4375",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Workflow(WorkflowArgs {
                command: WorkflowCommand::Run {
                    ref workflow,
                    ref fleet,
                    ref runtime,
                    ref issue,
                    ..
                }
            })) if workflow == "stopship"
                && fleet.as_deref() == Some("stopship")
                && runtime == "tmux"
                && issue.as_deref() == Some("4375")
        ));
    }

    /// Fleet is the only top-level spelling for durable runs. The retired
    /// `pod` spelling must fail to parse instead of dispatching.
    #[test]
    fn fleet_is_the_only_top_level_command_and_pod_is_rejected() {
        for tail in [
            vec!["init"],
            vec!["status"],
            vec!["run", "tasks.json", "--max-workers", "2"],
        ] {
            let fleet = parse_ok(
                &std::iter::once("codewhale")
                    .chain(["fleet"])
                    .chain(tail.iter().copied())
                    .collect::<Vec<_>>(),
            );
            let Some(Commands::Fleet(fleet_args)) = &fleet.command else {
                panic!("fleet must parse into the fleet command: {tail:?}");
            };
            assert_eq!(fleet_args.args, tail, "{tail:?}");
            assert!(fleet.prompt.is_empty(), "{tail:?}");

            let retired = parse_ok(
                &std::iter::once("codewhale")
                    .chain(["pod"])
                    .chain(tail.iter().copied())
                    .collect::<Vec<_>>(),
            );
            assert!(
                retired.command.is_none(),
                "retired pod must not dispatch to any command: {tail:?}"
            );
            assert_eq!(
                retired.prompt.first().map(String::as_str),
                Some("pod"),
                "retired pod words fall through to prompt text: {tail:?}"
            );
        }

        // Help advertises fleet only.
        let help = help_for(&["codewhale", "--help"]);
        let commands = help
            .lines()
            .map(str::trim_start)
            .filter(|line| line.starts_with("fleet"))
            .collect::<Vec<_>>();
        assert_eq!(
            commands.len(),
            1,
            "expected exactly one entry: {commands:?}"
        );
        assert!(commands[0].starts_with("fleet"), "{commands:?}");
        assert!(
            commands[0].contains("fleet"),
            "help summary should name fleet: {commands:?}"
        );
        assert!(
            !help.contains("Manage durable Agent Fleet runs"),
            "the retired Fleet-led summary must be gone from top-level help"
        );

        let fleet_help = help_for(&["codewhale", "fleet", "--help"]);
        assert!(fleet_help.contains("Manage durable Agent fleet runs"));
        assert!(fleet_help.contains("codewhale fleet run tasks.json --max-workers 4"));

        // The inner command token matches the canonical name so receipts
        // and any echoed invocation never regress to the retired name.
        let args = TuiPassthroughArgs {
            args: vec!["status".into()],
        };
        assert_eq!(
            tui_args("fleet", args.clone()),
            vec!["fleet".to_string(), "status".to_string()]
        );
        assert!(command_accepts_raw_provider(Some(&Commands::Fleet(args))));
    }

    #[test]
    fn exec_and_fleet_accept_builtin_and_raw_provider_identifiers() {
        let builtin = parse_ok(&["codewhale", "--provider", "openrouter", "exec", "Reply OK"]);
        assert_eq!(builtin.provider.as_deref(), Some("openrouter"));
        assert_eq!(
            top_level_provider_override(builtin.provider.as_deref(), builtin.command.as_ref())
                .expect("built-in Exec provider"),
            Some(ProviderKind::Openrouter)
        );

        assert_eq!(
            top_level_provider_override(
                Some("qianfan"),
                Some(&Commands::Exec(TuiPassthroughArgs {
                    args: vec!["Reply OK".into()]
                }))
            )
            .expect("qianfan is a catalog route"),
            Some(ProviderKind::Qianfan)
        );

        for (provider, command) in [
            ("lm-studio", vec!["exec", "Reply OK"]),
            ("lm-studio", vec!["fleet", "status"]),
        ] {
            let argv = std::iter::once("codewhale")
                .chain(["--provider", provider])
                .chain(command.iter().copied())
                .collect::<Vec<_>>();
            let cli = parse_ok(&argv);
            assert_eq!(cli.provider.as_deref(), Some(provider));
            assert_eq!(
                top_level_provider_override(cli.provider.as_deref(), cli.command.as_ref())
                    .expect("raw TUI provider"),
                None,
                "{argv:?} should defer the raw provider id to the TUI"
            );
        }
    }

    #[test]
    fn opencode_go_provider_aliases_parse_as_builtin() {
        for alias in ["opencode-go", "opencode_go", "opencodego"] {
            assert_eq!(builtin_provider_arg(alias), Some(ProviderKind::OpencodeGo));
        }
    }

    #[test]
    fn ollama_cloud_provider_aliases_parse_as_builtin() {
        for alias in ["ollama-cloud", "ollama_cloud"] {
            assert_eq!(builtin_provider_arg(alias), Some(ProviderKind::OllamaCloud));
        }
    }

    #[test]
    fn antigravity_provider_aliases_parse_as_builtin() {
        for alias in ["antigravity", "agy"] {
            assert_eq!(builtin_provider_arg(alias), Some(ProviderKind::Antigravity));
        }
    }

    #[test]
    fn legacy_dual_wire_provider_flag_keeps_named_table_kind() {
        // The CLI flag must resolve legacy spellings to the table-owning
        // dialect kind (mirroring TOML serde), never to the collapsed catalog
        // primary, or the user's own [providers.*] table is orphaned.
        for alias in [
            "minimax-anthropic",
            "minimax_anthropic",
            "mini-max-anthropic",
            "mini_max_anthropic",
        ] {
            assert_eq!(
                builtin_provider_arg(alias),
                Some(ProviderKind::MinimaxAnthropic),
                "{alias}"
            );
        }
        let cli = parse_ok(&[
            "codewhale",
            "--provider",
            "minimax-anthropic",
            "exec",
            "Reply OK",
        ]);
        assert_eq!(
            top_level_provider_override(cli.provider.as_deref(), cli.command.as_ref())
                .expect("legacy dual-wire provider"),
            Some(ProviderKind::MinimaxAnthropic)
        );
    }

    #[test]
    fn opencode_zen_provider_aliases_parse_as_builtin() {
        for alias in [
            "opencode-zen",
            "opencode_zen",
            "opencodezen",
            "zen",
            "opencode",
        ] {
            assert_eq!(builtin_provider_arg(alias), Some(ProviderKind::OpencodeZen));
        }
    }

    #[test]
    fn raw_provider_ids_remain_restricted_to_exec_and_fleet() {
        let cli = parse_ok(&["codewhale", "--provider", "lm-studio", "model", "list"]);
        let err = top_level_provider_override(cli.provider.as_deref(), cli.command.as_ref())
            .expect_err("model registry commands still require a built-in provider");
        assert!(
            err.to_string()
                .contains("configured custom providers are accepted only by exec and fleet")
        );

        let err = Cli::try_parse_from(["codewhale", "auth", "set", "--provider", "lm-studio"])
            .expect_err("auth keeps enum-only provider validation");
        assert_eq!(err.kind(), ErrorKind::ValueValidation);

        let err = Cli::try_parse_from([
            "codewhale",
            "--provider",
            "../../lm-studio",
            "exec",
            "Reply OK",
        ])
        .expect_err("provider ids must stay simple tokens");
        assert!(
            err.to_string()
                .contains("provider must be a simple identifier")
        );
    }

    #[test]
    fn hidden_lane_log_proxy_parses_child_argv_and_preserves_other_commands() {
        let cli = parse_ok(&[
            "codewhale",
            "lane-log-proxy",
            "--log-path",
            "/tmp/lane.ndjson",
            "--receipt-path",
            "/tmp/lane.exit.json",
            "--receipt-tmp-path",
            "/tmp/lane.exit.json.tmp",
            "--environment-path",
            "/tmp/lane.env.json",
            "--lane-id",
            "lane-proof",
            "--",
            "/bin/echo",
            "--child-flag",
            "hello",
        ]);
        let (proxy, command) = split_lane_log_proxy_command(cli.command);
        assert!(command.is_none());
        let proxy = proxy.expect("proxy args");
        assert_eq!(proxy.lane_id, "lane-proof");
        assert_eq!(
            proxy.command,
            ["/bin/echo", "--child-flag", "hello"].map(str::to_string)
        );

        let cli = parse_ok(&["codewhale", "lane", "list", "--json"]);
        let (proxy, command) = split_lane_log_proxy_command(cli.command);
        assert!(proxy.is_none());
        assert!(matches!(
            command,
            Some(Commands::Lane(LaneArgs {
                command: LaneCommand::List { json: true }
            }))
        ));
    }

    /// #1888: the CLI must expose exactly the Lane verbs the shared contract
    /// declares, under the same ids — no CLI-only verb, no missing verb.
    #[test]
    fn cli_lane_subcommands_cover_the_shared_control_contract() {
        use codewhale_lane::{ControlDomain, ControlOperation, ControlSurface};

        for descriptor in codewhale_lane::control::operations_for_domain(ControlDomain::Lane) {
            let argv = [
                "codewhale".to_string(),
                "lane".to_string(),
                descriptor.verb.to_string(),
            ];
            let mut argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            if descriptor.target.requires_identity() {
                argv.push("lane-a1b2c3d4");
            }
            let cli = parse_ok(&argv);
            let Some(Commands::Lane(args)) = cli.command else {
                panic!("`{}` must parse as a lane subcommand", descriptor.verb);
            };
            let parsed = match args.command {
                LaneCommand::List { .. } => ControlOperation::LaneList,
                LaneCommand::Status { .. } => ControlOperation::LaneStatus,
                LaneCommand::Interrupt { .. } | LaneCommand::Stop { .. } => {
                    ControlOperation::LaneInterrupt
                }
                LaneCommand::Restart { .. } => ControlOperation::LaneRestart,
                LaneCommand::Resume { .. } => ControlOperation::LaneResume,
                other => panic!(
                    "unexpected lane subcommand for {}: {other:?}",
                    descriptor.verb
                ),
            };
            assert_eq!(
                parsed, descriptor.operation,
                "`codewhale lane {}` must map to {}",
                descriptor.verb, descriptor.id
            );
            assert!(
                descriptor.offers(ControlSurface::Cli),
                "{} must be declared on the CLI surface",
                descriptor.id
            );
        }
    }

    /// `lane stop` is a compatibility spelling, not a second verb.
    #[test]
    fn lane_stop_and_interrupt_resolve_to_one_verb() {
        use codewhale_lane::{ControlDomain, ControlOperation};

        for spelling in ["stop", "interrupt", "cancel", "kill"] {
            assert_eq!(
                ControlOperation::parse_verb(ControlDomain::Lane, spelling),
                Some(ControlOperation::LaneInterrupt),
                "{spelling}"
            );
        }
        let stop = parse_ok(&["codewhale", "lane", "stop", "lane-a1b2c3d4"]);
        assert!(matches!(
            stop.command,
            Some(Commands::Lane(LaneArgs {
                command: LaneCommand::Stop { .. }
            }))
        ));
    }

    #[test]
    fn short_workflow_names_do_not_resolve_version_pinned_files() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        // A bare short name must never expand to a version-pinned script.
        // The v0868_* lane scripts are gone, but the guard stays so a future
        // vXXXX_ naming habit cannot silently become resolvable.
        let candidates = workflow_source_candidates("issue-sweep", None, &workspace);
        assert!(candidates.iter().all(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("v0868_"))
        }));
        assert!(resolve_workflow_source_path("issue-sweep", None, &workspace).is_err());

        // An explicit repo-relative path still resolves — checked against a
        // workflow that actually ships.
        let explicit =
            resolve_workflow_source_path("workflows/stopship.workflow.js", None, &workspace)
                .expect("explicit workflow path");
        assert!(explicit.ends_with("workflows/stopship.workflow.js"));
    }

    #[test]
    fn workflow_run_resolves_stopship_alias_and_payload() {
        let _lock = env_lock();
        let (_dir, _tui) = install_fake_tui_binary();
        let _provider = ScopedEnvVar::remove("DEEPSEEK_PROVIDER");
        let _model = ScopedEnvVar::remove("DEEPSEEK_MODEL");
        let _base_url = ScopedEnvVar::remove("DEEPSEEK_BASE_URL");
        let _api_key = ScopedEnvVar::remove("DEEPSEEK_API_KEY");
        let _cli_api_key = ScopedEnvVar::remove("CODEWHALE_CLI_API_KEY");
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let cli = parse_ok(&[
            "codewhale",
            "--profile",
            "workflow-profile",
            "--model",
            "explicit-workflow-model",
            "--api-key",
            "explicit-profile-key",
            "--workspace",
            workspace.to_str().expect("workspace UTF-8"),
        ]);
        let resolved = resolved_runtime_for_test(ProviderKind::Deepseek, ProviderSource::Config);
        let source = resolve_workflow_source_path("stopship", None, &workspace)
            .expect("stopship workflow source");
        assert!(source.ends_with("workflows/stopship.workflow.js"));

        let process = workflow_exec_command(WorkflowExecSpec {
            cli: &cli,
            resolved_runtime: &resolved,
            config_path: &workspace.join("config.toml"),
            source_root: &workspace,
            source_path: &source,
            workflow: "stopship",
            fleet: Some("stopship"),
            issue: Some("4375"),
            goal: Some("fix stopship"),
            token_budget: Some(25_000),
            verify: true,
        })
        .expect("command");
        let current_executable = std::env::current_exe().expect("current executable");
        assert_eq!(
            process.command.first().map(String::as_str),
            current_executable.to_str(),
            "workflow lanes must launch the exact runtime that built their process spec"
        );
        let joined = process.command.join("\n");
        assert!(joined.contains("workflow-tool"));
        assert!(joined.contains("explicit-workflow-command"));
        assert!(joined.contains("--input-json"));
        assert!(!process.command.iter().any(|arg| arg == "exec"));
        assert!(!process.command.iter().any(|arg| arg == "--workspace"));
        assert!(
            process
                .command
                .windows(2)
                .any(|pair| pair == ["--profile", "workflow-profile"])
        );
        assert!(!joined.contains("Run the CodeWhale"));
        assert!(joined.contains("\"source_path\":\"workflows/stopship.workflow.js\""));
        assert!(joined.contains("\"fleet\":\"stopship\""));
        assert!(joined.contains("\"issue\":\"4375\""));
        assert!(joined.contains("\"token_budget\":25000"));
        assert!(joined.contains("\"verify\":true"));
        assert!(
            process.environment.iter().any(|(key, value)| {
                key == "DEEPSEEK_MODEL" && value == "explicit-workflow-model"
            })
        );
        assert!(
            !process
                .environment
                .iter()
                .any(|(key, _)| key == "DEEPSEEK_PROVIDER")
        );
        assert!(
            !process
                .environment
                .iter()
                .any(|(key, _)| key == "DEEPSEEK_BASE_URL")
        );
        assert!(
            !process
                .environment
                .iter()
                .any(|(key, _)| key == "DEEPSEEK_API_KEY")
        );
        assert!(process.environment.iter().any(|(key, value)| {
            key == "CODEWHALE_CLI_API_KEY" && value == "explicit-profile-key"
        }));
        assert!(
            !process
                .command
                .iter()
                .any(|argument| argument.contains("explicit-profile-key"))
        );
        assert!(
            process
                .environment
                .iter()
                .all(|(_, value)| value != "test-model")
        );
    }

    #[test]
    fn exec_keeps_global_looking_flags_as_passthrough_args() {
        let cli = parse_ok(&[
            "codewhale",
            "exec",
            "--provider",
            "definitely-not-a-provider",
            "Reply OK",
        ]);

        let Some(Commands::Exec(args)) = cli.command else {
            panic!("expected exec command");
        };

        assert_eq!(
            args.args,
            vec![
                "--provider".to_string(),
                "definitely-not-a-provider".to_string(),
                "Reply OK".to_string(),
            ]
        );
    }

    #[test]
    fn exec_rejects_provider_after_subcommand() {
        let args = vec![
            "--provider".to_string(),
            "definitely-not-a-provider".to_string(),
            "Reply OK".to_string(),
        ];

        let err = reject_exec_global_flags(&args).expect_err("provider after exec should fail");

        assert!(
            err.to_string()
                .contains("--provider must be placed before `exec`")
        );
    }

    #[test]
    fn exec_rejects_equals_form_provider_after_subcommand() {
        let args = vec!["--provider=openmodel".to_string(), "Reply OK".to_string()];

        let err = reject_exec_global_flags(&args).expect_err("provider after exec should fail");

        assert!(
            err.to_string()
                .contains("--provider must be placed before `exec`")
        );
    }

    #[test]
    fn exec_allows_documented_forwarded_flags() {
        let args = vec![
            "--auto".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "fix tests".to_string(),
        ];

        reject_exec_global_flags(&args).expect("documented exec flags should pass");
    }

    #[test]
    fn exec_allows_literal_prompt_flags_after_separator() {
        let args = vec![
            "--".to_string(),
            "--provider".to_string(),
            "is literal prompt text".to_string(),
        ];

        reject_exec_global_flags(&args).expect("separator should stop global flag validation");
    }

    #[test]
    fn dispatcher_resume_picker_only_handles_bare_windows_resume() {
        assert!(should_pick_resume_in_dispatcher(
            &["resume".to_string()],
            true
        ));
        assert!(!should_pick_resume_in_dispatcher(
            &["resume".to_string(), "--last".to_string()],
            true
        ));
        assert!(!should_pick_resume_in_dispatcher(
            &["resume".to_string(), "abc123".to_string()],
            true
        ));
        assert!(!should_pick_resume_in_dispatcher(
            &["resume".to_string()],
            false
        ));
    }

    #[test]
    fn auth_set_uses_isolated_file_store_and_preserves_tui_defaults() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let codewhale_home = dir.path().join("codewhale-home");
        let codewhale_home_value = codewhale_home.to_string_lossy().into_owned();
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &codewhale_home_value);
        let _backend = ScopedEnvVar::set("CODEWHALE_SECRET_BACKEND", "file");
        let path = codewhale_home.join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        let secrets = Secrets::auto_detect();

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Deepseek,
                api_key: Some("sk-test".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("auth set should persist credential");

        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert_eq!(
            store.config.default_text_model.as_deref(),
            Some("deepseek-v4-pro")
        );
        let saved = std::fs::read_to_string(&path).expect("config should be written");
        assert!(!saved.contains("sk-test"), "{saved}");
        assert!(
            !saved
                .lines()
                .any(|line| line.trim_start().starts_with("api_key="))
        );
        assert!(saved.contains("default_text_model = \"deepseek-v4-pro\""));
        assert_eq!(
            secrets.get("deepseek").expect("read secret").as_deref(),
            Some("sk-test")
        );
    }

    /// `codewhale login` now means the Codewhale account device flow: the
    /// account-login flags parse through and reach the cloud path.
    #[test]
    fn login_parses_account_device_flow_flags() {
        let cli = parse_ok(&["codewhale", "login", "--no-open", "--timeout-seconds", "5"]);
        let Some(Commands::Login(args)) = cli.command else {
            panic!("expected Login");
        };
        assert!(args.no_open);
        assert_eq!(args.timeout_seconds, 5);
        assert!(args.api_key.is_none());
        assert!(args.provider.is_none());

        let cli = parse_ok(&["codewhale", "login"]);
        let Some(Commands::Login(args)) = cli.command else {
            panic!("expected Login");
        };
        assert!(!args.no_open);
        assert_eq!(args.timeout_seconds, 600);
    }

    /// The provider-key surface moved to `auth set --provider`; the hidden
    /// legacy flags must redirect loudly instead of silently configuring a key.
    #[test]
    fn login_rejects_legacy_provider_flags_with_redirect() {
        let err = reject_legacy_login_provider_args(&LoginArgs {
            no_open: false,
            timeout_seconds: 600,
            api_key: Some("sk-x".to_string()),
            provider: None,
        })
        .expect_err("legacy --api-key must be rejected");
        let rendered = err.to_string();
        assert!(
            rendered.contains("auth set --provider"),
            "redirect must name `auth set --provider`: {rendered}"
        );

        let err = reject_legacy_login_provider_args(&LoginArgs {
            no_open: false,
            timeout_seconds: 600,
            api_key: None,
            provider: Some(ProviderKind::Deepseek),
        })
        .expect_err("legacy --provider must be rejected");
        assert!(
            err.to_string().contains("auth set --provider"),
            "redirect must name `auth set --provider`"
        );

        reject_legacy_login_provider_args(&LoginArgs {
            no_open: false,
            timeout_seconds: 600,
            api_key: None,
            provider: None,
        })
        .expect("plain account login carries no legacy flags");
    }

    /// Root help keeps the `login` token, but its meaning is now the account
    /// sign-in; the subcommand help must say so.
    #[test]
    fn login_help_describes_account_signin() {
        let help = help_for(&["codewhale", "login", "--help"]);
        assert!(
            help.contains("Codewhale account"),
            "login help must describe account sign-in: {help}"
        );
        assert!(
            !help.to_lowercase().contains("api key"),
            "login help must not advertise provider API keys: {help}"
        );
    }

    #[test]
    fn auth_parses_daytona_slot_commands_as_unknown() {
        // The internal cloud-agent slot must not be a user command: parsing
        // rejects it and `auth --help` never teaches it.
        for argv in [
            vec![
                "codewhale",
                "auth",
                "set-slot",
                "daytona",
                "--api-key-stdin",
            ],
            vec!["codewhale", "auth", "clear-slot", "daytona"],
        ] {
            let error = Cli::try_parse_from(argv).expect_err("slot commands must not parse");
            assert_eq!(error.kind(), ErrorKind::InvalidSubcommand, "{error}");
        }
        let help = help_for(&["codewhale", "auth", "--help"]);
        assert!(!help.contains("set-slot"), "{help}");
        assert!(!help.contains("clear-slot"), "{help}");
        assert!(!help.to_lowercase().contains("daytona"), "{help}");
    }

    /// #5198: `auth set` shares the login resolver — provider auth markers go
    /// user-global even when the ambient config is workspace-scoped.
    #[test]
    fn auth_set_with_repo_scoped_ambient_config_writes_user_global_metadata() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("git marker");
        let repo_config_dir = repo.join(".codewhale");
        std::fs::create_dir_all(&repo_config_dir).expect("repo config dir");
        let repo_config = repo_config_dir.join("config.toml");
        std::fs::write(&repo_config, "approval_policy = \"never\"\n").expect("repo config");

        let codewhale_home = dir.path().join("codewhale-home");
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &codewhale_home.to_string_lossy());
        let _config = ScopedEnvVar::set("CODEWHALE_CONFIG_PATH", &repo_config.to_string_lossy());
        let _legacy_config = ScopedEnvVar::remove("DEEPSEEK_CONFIG_PATH");
        let _backend = ScopedEnvVar::set("CODEWHALE_SECRET_BACKEND", "file");
        let mut store = ConfigStore::load(None).expect("ambient store should load");
        let secrets = Secrets::auto_detect();

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Openrouter,
                api_key: Some("sk-or-repo-scoped".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("auth set should persist credential");

        assert_eq!(
            secrets.get("openrouter").expect("read secret").as_deref(),
            Some("sk-or-repo-scoped")
        );
        let global = std::fs::read_to_string(codewhale_home.join("config.toml"))
            .expect("user-global config");
        assert!(
            global.contains("auth_mode = \"api_key\""),
            "user-global config must carry the auth markers: {global}"
        );
        assert!(
            global.contains("openrouter"),
            "user-global config must name the provider table: {global}"
        );
        assert!(!global.contains("sk-or-repo-scoped"), "{global}");
        let repo_after = std::fs::read_to_string(&repo_config).expect("repo config");
        assert_eq!(
            repo_after, "approval_policy = \"never\"\n",
            "workspace config must stay untouched by credential metadata: {repo_after}"
        );
    }

    #[test]
    fn parses_auth_subcommand_matrix() {
        let cli = parse_ok(&["deepseek", "auth", "xai-device"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::XaiDevice
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "chatgpt"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Chatgpt
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "chatgpt-revoke"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::ChatgptRevoke
            }))
        ));

        let cli = parse_ok(&[
            "deepseek",
            "auth",
            "external-consent",
            "--provider",
            "openai-codex",
            "--mode",
            "read-only",
            "--path",
            "/tmp/codex-auth.json",
            "--yes",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::ExternalConsent {
                    provider: ProviderKind::OpenaiCodex,
                    mode: ExternalCredentialModeArg::ReadOnly,
                    path: Some(_),
                    yes: true,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "external-revoke", "--provider", "xai"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::ExternalRevoke {
                    provider: ProviderKind::Xai,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "deepseek"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Deepseek,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&[
            "deepseek",
            "auth",
            "set",
            "--provider",
            "openrouter",
            "--api-key-stdin",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Openrouter,
                    api_key: None,
                    api_key_stdin: true,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "get", "--provider", "novita"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Get {
                    provider: ProviderKind::Novita
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "clear", "--provider", "nvidia-nim"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Clear {
                    provider: ProviderKind::NvidiaNim
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "fireworks"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Fireworks,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "siliconflow"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Siliconflow,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "arcee"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Arcee,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "moonshot"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Moonshot,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "wanjie-ark"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::WanjieArk,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "get", "--provider", "sglang"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Get {
                    provider: ProviderKind::Sglang
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "get", "--provider", "vllm"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Get {
                    provider: ProviderKind::Vllm
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "ollama"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderKind::Ollama,
                    api_key: None,
                    api_key_stdin: false,
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "status", "--provider", "openai-codex"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Status {
                    provider: Some(ProviderKind::OpenaiCodex),
                    diagnostic: false,
                }
            }))
        ));

        let cli = parse_ok(&[
            "deepseek",
            "auth",
            "status",
            "--diagnostic",
            "--provider",
            "deepseek",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Status {
                    provider: Some(ProviderKind::Deepseek),
                    diagnostic: true,
                }
            }))
        ));

        for (provider, expected) in [
            ("anthropic", ProviderKind::Anthropic),
            ("openmodel", ProviderKind::Openmodel),
            ("open-model", ProviderKind::Openmodel),
            ("zai", ProviderKind::Zai),
            ("stepfun", ProviderKind::Stepfun),
            ("minimax", ProviderKind::Minimax),
            ("minimax-anthropic", ProviderKind::MinimaxAnthropic),
            ("minimax_anthropic", ProviderKind::MinimaxAnthropic),
            ("deepinfra", ProviderKind::Deepinfra),
            ("deep-infra", ProviderKind::Deepinfra),
            ("siliconflow-cn", ProviderKind::SiliconflowCN),
            ("siliconflow-CN", ProviderKind::SiliconflowCN),
            ("siliconflow_china", ProviderKind::SiliconflowCN),
        ] {
            let cli = parse_ok(&[
                "deepseek",
                "auth",
                "set",
                "--provider",
                provider,
                "--api-key-stdin",
            ]);
            assert!(matches!(
                cli.command,
                Some(Commands::Auth(AuthArgs {
                    command: AuthCommand::Set {
                        provider,
                        api_key: None,
                        api_key_stdin: true,
                    }
                })) if provider == expected
            ));
        }

        let cli = parse_ok(&["deepseek", "auth", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::List
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "migrate"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Migrate { dry_run: false }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "migrate", "--dry-run"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Migrate { dry_run: true }
            }))
        ));
    }

    #[test]
    fn auth_help_describes_runtime_effective_diagnostics() {
        let get = help_for(&["codewhale", "auth", "get", "--help"]);
        assert!(get.contains("effective credential route"), "{get}");
        assert!(get.contains("structural OAuth/repair state"), "{get}");

        let status = help_for(&["codewhale", "auth", "status", "--help"]);
        assert!(
            status.contains("runtime-effective credential route state"),
            "{status}"
        );

        let list = help_for(&["codewhale", "auth", "list", "--help"]);
        assert!(list.contains("runtime-effective auth state"), "{list}");
    }

    #[test]
    fn auth_set_writes_secret_store_and_keeps_config_credential_free() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-set-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        let inner = Arc::new(InMemoryKeyringStore::new());
        let secrets = Secrets::new(inner.clone());

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Deepseek,
                api_key: Some("sk-keyring".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("set should succeed");

        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!saved.contains("sk-keyring"), "{saved}");
        assert!(
            !saved
                .lines()
                .any(|line| line.trim_start().starts_with("api_key ="))
        );
        assert_eq!(
            inner.get("deepseek").unwrap().as_deref(),
            Some("sk-keyring")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_set_refuses_plaintext_config_when_secret_store_write_fails() {
        use codewhale_secrets::{KeyringStore, SecretsError};
        use std::sync::Arc;

        struct FailingStore;

        impl KeyringStore for FailingStore {
            fn get(&self, _key: &str) -> Result<Option<String>, SecretsError> {
                Ok(None)
            }

            fn set(&self, _key: &str, _value: &str) -> Result<(), SecretsError> {
                Err(SecretsError::Keyring("test write failure".to_string()))
            }

            fn delete(&self, _key: &str) -> Result<(), SecretsError> {
                Ok(())
            }

            fn backend_name(&self) -> &'static str {
                "failing test store"
            }
        }

        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("load config");
        let secrets = Secrets::new(Arc::new(FailingStore));

        let error = run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Openrouter,
                api_key: Some("fallback-test-credential".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect_err("secret-store failure must not downgrade to plaintext");

        let message = format!("{error:#}");
        assert!(message.contains("Secret storage write failed"), "{message}");
        assert!(message.contains("Refusing"), "{message}");
        assert!(
            message.contains(&codewhale_config::quote_os_path(store.path())),
            "{message}"
        );
        assert!(store.config.providers.openrouter.api_key.is_none());
        assert!(!path.exists(), "plaintext config must stay untouched");
    }

    #[test]
    fn auth_set_provider_key_does_not_switch_active_provider() {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-set-preserve-provider-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        let secrets = no_keyring_secrets();

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Arcee,
                api_key: Some("arcee-key".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("set should succeed");

        assert_eq!(store.config.provider, ProviderKind::Deepseek);
        assert!(store.config.providers.arcee.api_key.is_none());
        assert_eq!(
            store.config.providers.arcee.auth_mode.as_deref(),
            Some("api_key")
        );

        let reloaded = ConfigStore::load(Some(path.clone())).expect("store should reload");
        assert_eq!(reloaded.config.provider, ProviderKind::Deepseek);
        assert!(reloaded.config.providers.arcee.api_key.is_none());
        assert_eq!(
            reloaded.config.providers.arcee.auth_mode.as_deref(),
            Some("api_key")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_set_ollama_accepts_empty_key_and_records_base_url() {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-ollama-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        let secrets = no_keyring_secrets();

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Ollama,
                api_key: None,
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("ollama auth set should not require a key");

        assert_eq!(store.config.provider, ProviderKind::Deepseek);
        assert_eq!(
            store.config.providers.ollama.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(store.config.providers.ollama.api_key, None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_clear_removes_from_config() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-clear-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.api_key = Some("sk-stale".to_string());
        store.config.providers.deepseek.api_key = Some("sk-stale".to_string());
        store.save().unwrap();

        let inner = Arc::new(InMemoryKeyringStore::new());
        inner.set("deepseek", "sk-stale").unwrap();
        let secrets = Secrets::new(inner.clone());

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Clear {
                provider: ProviderKind::Deepseek,
            },
            &secrets,
        )
        .expect("clear should succeed");

        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert_eq!(inner.get("deepseek").unwrap(), None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_status_scoped_probe_and_list_all_provider_keyrings() {
        use codewhale_secrets::{KeyringStore, SecretsError};
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct RecordingStore {
            gets: Mutex<Vec<String>>,
        }

        impl KeyringStore for RecordingStore {
            fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
                self.gets.lock().unwrap().push(key.to_string());
                Ok(None)
            }

            fn set(&self, _key: &str, _value: &str) -> Result<(), SecretsError> {
                Ok(())
            }

            fn delete(&self, _key: &str) -> Result<(), SecretsError> {
                Ok(())
            }

            fn backend_name(&self) -> &'static str {
                "recording"
            }
        }

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-active-keyring-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        let inner = Arc::new(RecordingStore::default());
        let secrets = Secrets::new(inner.clone());

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Status {
                provider: Some(ProviderKind::Deepseek),
                diagnostic: false,
            },
            &secrets,
        )
        .expect("status should succeed");
        run_auth_command_with_secrets(&mut store, AuthCommand::List, &secrets)
            .expect("list should succeed");

        let probed = inner.gets.lock().unwrap();
        // Scoped status probes only the requested provider.
        assert_eq!(probed[0], "deepseek");
        // List now probes all providers (not just active) to fix the
        // stale keyring-only-for-active-provider bug.
        assert!(probed.len() > 1, "list should probe all providers");
        assert!(
            ProviderKind::ALL
                .iter()
                .all(|p| probed.contains(&provider_slot(*p).to_string())),
            "every known provider should be probed by auth list: {:?}",
            *probed
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_diagnostic_reports_paths_and_presence_without_values() {
        let _lock = env_lock();
        let fixture = tempfile::TempDir::new().expect("fixture root");
        // macOS spells /var through a /private symlink. Canonicalize the
        // fixture root so the metadata-only backend diagnostic can prove every
        // ancestor is a real directory instead of truthfully returning
        // `unknown` for the symlinked spelling.
        let home = fixture
            .path()
            .canonicalize()
            .expect("canonical fixture root")
            .join("isolated-codewhale-home");
        let config_path = home.join("config.toml");
        let settings_path = home.join("settings.toml");
        let secret_path = home.join("secrets").join("secrets.json");
        std::fs::create_dir_all(secret_path.parent().expect("secret parent"))
            .expect("create diagnostic fixture");
        std::fs::write(
            &config_path,
            "api_key = \"diagnostic-config-secret-1234\"\n",
        )
        .expect("write config fixture");
        std::fs::write(&settings_path, "default_mode = \"plan\"\n")
            .expect("write settings fixture");
        std::fs::write(
            &secret_path,
            r#"{"deepseek":"diagnostic-store-secret-5678"}"#,
        )
        .expect("write secret fixture");

        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
        let _backend = ScopedEnvVar::set("CODEWHALE_SECRET_BACKEND", "file");
        let _env = ScopedEnvVar::set("DEEPSEEK_API_KEY", "diagnostic-env-secret-9012");
        let store = ConfigStore::load(Some(config_path.clone())).expect("load config fixture");

        let output = auth_diagnostic_lines(&store, Some(ProviderKind::Deepseek)).join("\n");
        assert!(
            output.contains(&format!(
                "codewhale home: {} (source: CODEWHALE_HOME (isolated); state: present)",
                codewhale_config::quote_os_path(&home)
            )),
            "{output}"
        );
        assert!(
            output.contains(&format!(
                "config: {} (present)",
                codewhale_config::quote_os_path(&config_path)
            )),
            "{output}"
        );
        assert!(
            output.contains(&format!(
                "settings: {} (present)",
                codewhale_config::quote_os_path(&settings_path)
            )),
            "{output}"
        );
        assert!(
            output.contains("secret backend: file (inspection: metadata_only)"),
            "{output}"
        );
        assert!(
            output.contains(&format!(
                "secret store: {} (present)",
                codewhale_config::quote_os_path(&secret_path)
            )),
            "{output}"
        );
        assert!(
            output.contains("provider deepseek sources: config_literal=present, secret_backend=present (provider entry unprobed), environment=present (DEEPSEEK_API_KEY)"),
            "{output}"
        );
        assert!(
            output.contains("legacy secret store: suppressed by explicit CODEWHALE_HOME isolation"),
            "{output}"
        );
        for secret_fragment in [
            "diagnostic-config-secret",
            "diagnostic-store-secret",
            "diagnostic-env-secret",
            "1234",
            "5678",
            "9012",
            "last4",
        ] {
            assert!(
                !output.contains(secret_fragment),
                "diagnostic leaked {secret_fragment:?}: {output}"
            );
        }
    }

    #[test]
    fn auth_status_reports_all_active_provider_sources_with_last4() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let _lock = env_lock();
        let _env = ScopedEnvVar::set("DEEPSEEK_API_KEY", "sk-env-1111");

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-status-table-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        store.config.api_key = Some("sk-config-3333".to_string());
        store.config.providers.deepseek.api_key = Some("sk-config-3333".to_string());

        let inner = Arc::new(InMemoryKeyringStore::new());
        inner.set("deepseek", "sk-keyring-2222").unwrap();
        let secrets = Secrets::new(inner);

        let output =
            auth_status_lines_for_provider(&store, &secrets, ProviderKind::Deepseek).join("\n");

        assert!(output.contains("provider: deepseek"));
        assert!(output.contains("active source: config (last4: ...3333)"));
        assert!(output.contains("lookup order: config -> secret store -> env"));
        assert!(output.contains("config file: "));
        assert!(output.contains("set, last4: ...3333"));
        assert!(output.contains("secret store: in-memory (test) (set, last4: ...2222)"));
        assert!(output.contains("env var: DEEPSEEK_API_KEY (set, last4: ...1111)"));
        assert!(!output.contains("sk-config-3333"));
        assert!(!output.contains("sk-keyring-2222"));
        assert!(!output.contains("sk-env-1111"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_status_all_providers_lists_every_known_provider() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-all-status-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        store.config.providers.arcee.api_key = Some("sk-arcee-test1234".to_string());

        let inner = Arc::new(InMemoryKeyringStore::new());
        inner.set("openrouter", "sk-or-test5678").unwrap();
        let secrets = Secrets::new(inner);

        let output = auth_status_all_providers(&store, &secrets).join("\n");

        assert!(output.contains("account:"), "{output}");
        assert!(output.contains("codewhale login"), "{output}");
        // No-brand invariant: the internal cloud-agent slot is not user
        // surface, so status never names it or teaches a set-slot command.
        assert!(!output.to_lowercase().contains("daytona"), "{output}");
        assert!(!output.contains("set-slot"), "{output}");

        // Should list all known providers
        assert!(output.contains("deepseek"));
        assert!(output.contains("arcee"));
        assert!(output.contains("openrouter"));
        assert!(output.contains("huggingface"));
        assert!(output.contains("ollama"));

        // Active provider should be marked
        assert!(output.contains("deepseek") && output.contains("*"));

        // Arcee should show config source
        assert!(output.contains("config"));

        // Should NOT leak raw keys
        assert!(!output.contains("sk-arcee-test1234"));
        assert!(!output.contains("sk-or-test5678"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_status_never_probes_codex_file_and_reports_exact_consent() {
        use codewhale_secrets::InMemoryKeyringStore;
        use std::sync::Arc;

        let _lock = env_lock();
        let _access_token = ScopedEnvVar::set("OPENAI_CODEX_ACCESS_TOKEN", "");
        let _codex_token = ScopedEnvVar::set("CODEX_ACCESS_TOKEN", "");

        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let auth_path = dir.path().join("auth.json");
        std::fs::write(&auth_path, r#"{"tokens":{"access_token":"secret-token"}}"#)
            .expect("write auth file");
        let auth_path_str = auth_path.to_string_lossy().into_owned();
        let _auth_file = ScopedEnvVar::set("OPENAI_CODEX_AUTH_FILE", &auth_path_str);

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::OpenaiCodex;
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));

        let output =
            auth_status_lines_for_provider(&store, &secrets, ProviderKind::OpenaiCodex).join("\n");

        assert!(output.contains("provider: openai-codex"));
        assert!(output.contains("auth mode: codex_oauth"));
        assert!(output.contains("active source: missing"));
        assert!(output.contains(
            "lookup order: env -> Codewhale-owned ChatGPT sign-in -> consent-gated exact Codex CLI file"
        ));
        assert!(output.contains("external credentials: disabled"));
        assert!(output.contains("scope_valid=false"));
        assert!(output.contains("disabled; no external-credential probing, reading"));
        assert!(output.contains("file not probed"));
        assert!(!output.contains("secret-token"));

        store.config.providers.openai_codex.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::OpenaiCodex,
                codewhale_config::ExternalCredentialSource::CodexCli,
                auth_path.clone(),
            ));
        let output =
            auth_status_lines_for_provider(&store, &secrets, ProviderKind::OpenaiCodex).join("\n");
        assert!(
            output.contains("active source: external read-only consent (availability not probed)")
        );
        assert!(output.contains("external credentials: read_only"));
        assert!(output.contains("provider=openai-codex"));
        assert!(output.contains("source=codex_cli"));
        assert!(output.contains(&format!(
            "path={}",
            codewhale_config::quote_os_path(&auth_path)
        )));
        assert!(output.contains(&format!(
            "consent_version={}",
            codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION
        )));
        assert!(output.contains("file not probed"));
        assert!(!output.contains("secret-token"));

        let ambient_path = dir.path().join("new-ambient-auth.json");
        let ambient_path_str = ambient_path.to_string_lossy().into_owned();
        let _ambient_file = ScopedEnvVar::set("OPENAI_CODEX_AUTH_FILE", &ambient_path_str);
        let changed =
            auth_status_lines_for_provider(&store, &secrets, ProviderKind::OpenaiCodex).join("\n");
        assert!(changed.contains("state=active"), "{changed}");
        assert!(changed.contains("ambient_path_changed=true"), "{changed}");
        assert!(changed.contains("consent remains pinned"), "{changed}");
        assert!(
            changed.contains(&codewhale_config::quote_os_path(&auth_path)),
            "{changed}"
        );
        assert!(!changed.contains(&ambient_path_str), "{changed}");
    }

    #[test]
    fn xai_valid_owned_generation_blocks_external_consent_without_storage_probes() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::remove("XAI_API_KEY");
        let _xai_base = ScopedEnvVar::remove("XAI_BASE_URL");
        let _auth_mode = ScopedEnvVar::remove("DEEPSEEK_AUTH_MODE");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = "external owner bytes must not be read";
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let _grok_auth_path = ScopedEnvVar::set("GROK_AUTH_PATH", &external_path.to_string_lossy());

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        store.config.providers.xai.oauth_credential_generation =
            Some("xai-auth-0123456789abcdef0123456789abcdef.json".to_string());
        store.config.providers.xai.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                external_path.clone(),
            ));
        let keyring = Arc::new(RecordingKeyringStore::default());
        let secrets = Secrets::new(keyring.clone());

        let scoped = auth_status_lines_for_provider(&store, &secrets, ProviderKind::Xai).join("\n");
        assert!(
            scoped.contains(
                "credential route: Codewhale-owned OAuth configured/unprobed (valid generation pointer; storage unprobed)"
            ),
            "{scoped}"
        );
        assert!(scoped.contains("external credentials: blocked by the configured Codewhale-owned xAI OAuth generation"), "{scoped}");
        assert!(
            scoped.contains(
                "xAI OAuth generation: configured Codewhale-owned pointer (storage unprobed)"
            ),
            "{scoped}"
        );
        assert!(
            !scoped.contains("active source: Codewhale-owned OAuth"),
            "a valid pointer is configured/unprobed, not an active credential: {scoped}"
        );
        assert!(
            !scoped.contains("fallback"),
            "an owned generation must never advertise Grok CLI fallback: {scoped}"
        );

        let all = auth_status_all_providers(&store, &secrets).join("\n");
        let xai_row = all
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI status row");
        assert!(
            xai_row.contains("Codewhale-owned OAuth configured/unprobed"),
            "{xai_row}"
        );

        let list = auth_list_lines(&store, &secrets).join("\n");
        let xai_list_row = list
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI list row");
        assert!(
            xai_list_row.ends_with("owned-oauth-configured"),
            "{xai_list_row}"
        );

        let get = auth_get_line_with_runtime(
            &store,
            &secrets,
            ProviderKind::Xai,
            &CliRuntimeOverrides::default(),
        );
        assert!(
            get.starts_with("xai: configured (source: Codewhale-owned OAuth generation"),
            "{get}"
        );
        assert!(!get.starts_with("xai: set"), "{get}");
        assert!(!get.contains("fallback"), "{get}");
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "owned OAuth diagnostics must not query the xAI API-key store: {:?}",
            keyring.queried()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external trap unchanged"),
            external_raw
        );

        store.config.providers.xai.auth_mode = None;
        store.config.auth_mode = Some("oauth".to_string());
        assert_eq!(
            xai_auth_diagnostics(&store, &CliRuntimeOverrides::default()).route,
            XaiAuthDiagnosticRoute::ApiKey,
            "a root auth mode must not select the xAI OAuth runtime route"
        );
    }

    #[test]
    fn xai_invalid_generation_requires_repair_blocks_external_and_keeps_api_key_diagnostics() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::remove("XAI_API_KEY");
        let _xai_base = ScopedEnvVar::remove("XAI_BASE_URL");
        let _auth_mode = ScopedEnvVar::remove("DEEPSEEK_AUTH_MODE");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = "external owner bytes must remain unread";
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let _grok_auth_path = ScopedEnvVar::set("GROK_AUTH_PATH", &external_path.to_string_lossy());

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        store.config.providers.xai.api_key = Some("fake-cfg-key-1234".to_string());
        store.config.providers.xai.oauth_credential_generation = Some("../unsafe.json".to_string());
        store.config.providers.xai.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                external_path.clone(),
            ));
        let keyring = Arc::new(RecordingKeyringStore::default());
        let secrets = Secrets::new(keyring.clone());

        let scoped = auth_status_lines_for_provider(&store, &secrets, ProviderKind::Xai).join("\n");
        assert!(
            scoped.contains("credential route: xAI OAuth needs repair"),
            "{scoped}"
        );
        assert!(
            scoped.contains("API-key fallback: config (last4: ...1234)"),
            "{scoped}"
        );
        assert!(scoped.contains("external credentials: blocked by the invalid Codewhale-owned xAI OAuth generation pointer"), "{scoped}");
        assert!(
            scoped.contains("repair: run `codewhale auth xai-device`"),
            "{scoped}"
        );
        assert!(
            !scoped.contains("external read-only consent (availability not probed)"),
            "invalid owned pointers must not activate Grok CLI consent: {scoped}"
        );

        let all = auth_status_all_providers(&store, &secrets).join("\n");
        let xai_row = all
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI status row");
        assert!(xai_row.contains("needs repair"), "{xai_row}");
        assert!(xai_row.contains("API-key fallback: config"), "{xai_row}");

        let list = auth_list_lines(&store, &secrets).join("\n");
        let xai_list_row = list
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI list row");
        assert!(xai_list_row.ends_with("needs-repair"), "{xai_list_row}");

        let get = auth_get_line_with_runtime(
            &store,
            &secrets,
            ProviderKind::Xai,
            &CliRuntimeOverrides::default(),
        );
        assert!(get.contains("xai: needs repair"), "{get}");
        assert!(get.contains("API-key fallback: config-file"), "{get}");
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "an invalid owned pointer must not query the xAI API-key store: {:?}",
            keyring.queried()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external trap unchanged"),
            external_raw
        );
    }

    #[test]
    fn xai_cli_custom_endpoint_rejects_inherited_api_key_sources() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::set("XAI_API_KEY", "fake-ambient-key-3333");
        let _xai_base = ScopedEnvVar::remove("XAI_BASE_URL");
        let _auth_mode = ScopedEnvVar::remove("DEEPSEEK_AUTH_MODE");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = "external owner bytes must remain unprobed";
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let _grok_auth_path = ScopedEnvVar::set("GROK_AUTH_PATH", &external_path.to_string_lossy());

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.api_key = Some("fake-cfg-key-1111".to_string());
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        store.config.providers.xai.oauth_credential_generation =
            Some("xai-auth-0123456789abcdef0123456789abcdef.json".to_string());
        store.config.providers.xai.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                external_path.clone(),
            ));
        let keyring = Arc::new(RecordingKeyringStore::default());
        keyring.set_value("xai", "fake-store-key-2222");
        let secrets = Secrets::new(keyring.clone());
        let runtime_overrides = CliRuntimeOverrides {
            base_url: Some("https://gateway.example.test/v1".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let scoped = auth_status_lines_for_provider_with_runtime(
            &store,
            &secrets,
            ProviderKind::Xai,
            &runtime_overrides,
        )
        .join("\n");
        assert!(
            scoped.contains("route: https://gateway.example.test/v1"),
            "{scoped}"
        );
        assert!(scoped.contains("credential route: missing"), "{scoped}");
        assert!(
            scoped.contains("custom xAI endpoint; API-key-only"),
            "{scoped}"
        );
        assert!(
            scoped.contains("not eligible for this custom xAI endpoint"),
            "{scoped}"
        );
        assert!(
            scoped.contains("external credentials: unavailable on a custom xAI endpoint"),
            "{scoped}"
        );
        for redacted_tail in ["...1111", "...2222", "...3333"] {
            assert!(
                !scoped.contains(redacted_tail),
                "custom CLI route must not advertise an inherited credential: {scoped}"
            );
        }

        let all =
            auth_status_all_providers_with_runtime(&store, &secrets, &runtime_overrides).join("\n");
        let xai_row = all
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI status row");
        assert!(xai_row.contains("unset"), "{xai_row}");
        assert!(
            !xai_row.contains("config") && !xai_row.contains("keyring") && !xai_row.contains("env"),
            "xAI summary must show runtime-effective sources only: {xai_row}"
        );

        let list = auth_list_lines_with_runtime(&store, &secrets, &runtime_overrides).join("\n");
        let xai_list_row = list
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI list row");
        assert!(xai_list_row.ends_with("missing"), "{xai_list_row}");

        let get =
            auth_get_line_with_runtime(&store, &secrets, ProviderKind::Xai, &runtime_overrides);
        assert_eq!(get, "xai: not set");
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "a global custom endpoint must not query xAI keyring state: {:?}",
            keyring.queried()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external trap unchanged"),
            external_raw
        );
    }

    #[test]
    fn xai_env_custom_endpoint_rejects_inherited_api_key_sources() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::set("XAI_API_KEY", "fake-ambient-key-6666");
        let _xai_base = ScopedEnvVar::set("XAI_BASE_URL", "https://env-gateway.example.test/v1");
        let _auth_mode = ScopedEnvVar::remove("DEEPSEEK_AUTH_MODE");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = "external owner bytes must remain unprobed";
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let _grok_auth_path = ScopedEnvVar::set("GROK_AUTH_PATH", &external_path.to_string_lossy());

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.api_key = Some("fake-cfg-key-4444".to_string());
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        store.config.providers.xai.oauth_credential_generation =
            Some("xai-auth-0123456789abcdef0123456789abcdef.json".to_string());
        store.config.providers.xai.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                external_path.clone(),
            ));
        let keyring = Arc::new(RecordingKeyringStore::default());
        keyring.set_value("xai", "fake-store-key-5555");
        let secrets = Secrets::new(keyring.clone());

        let scoped = auth_status_lines_for_provider(&store, &secrets, ProviderKind::Xai).join("\n");
        assert!(
            scoped.contains("route: https://env-gateway.example.test/v1"),
            "{scoped}"
        );
        assert!(scoped.contains("credential route: missing"), "{scoped}");
        assert!(
            scoped.contains("custom xAI endpoint; API-key-only"),
            "{scoped}"
        );
        for redacted_tail in ["...4444", "...5555", "...6666"] {
            assert!(
                !scoped.contains(redacted_tail),
                "custom env route must not advertise an inherited credential: {scoped}"
            );
        }

        let all = auth_status_all_providers(&store, &secrets).join("\n");
        let xai_row = all
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI status row");
        assert!(xai_row.contains("unset"), "{xai_row}");

        let list = auth_list_lines(&store, &secrets).join("\n");
        let xai_list_row = list
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI list row");
        assert!(xai_list_row.ends_with("missing"), "{xai_list_row}");

        assert_eq!(
            auth_get_line_with_runtime(
                &store,
                &secrets,
                ProviderKind::Xai,
                &CliRuntimeOverrides::default(),
            ),
            "xai: not set"
        );
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "an XAI_BASE_URL custom route must not query xAI keyring state: {:?}",
            keyring.queried()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external trap unchanged"),
            external_raw
        );
    }

    #[test]
    fn xai_config_bound_custom_endpoint_uses_its_route_key() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::remove("XAI_API_KEY");
        let _xai_base = ScopedEnvVar::remove("XAI_BASE_URL");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.base_url =
            Some("https://bound-gateway.example.test/v1".to_string());
        store.config.providers.xai.api_key = Some("fake-bound-key-7777".to_string());
        let keyring = Arc::new(RecordingKeyringStore::default());
        keyring.set_value("xai", "fake-store-key-8888");
        let secrets = Secrets::new(keyring.clone());

        let scoped = auth_status_lines_for_provider(&store, &secrets, ProviderKind::Xai).join("\n");
        assert!(
            scoped.contains("credential route: config (last4: ...7777)"),
            "{scoped}"
        );
        assert!(
            scoped.contains("config file:") && scoped.contains("runtime-effective, last4: ...7777"),
            "{scoped}"
        );
        assert_eq!(
            auth_get_line_with_runtime(
                &store,
                &secrets,
                ProviderKind::Xai,
                &CliRuntimeOverrides::default(),
            ),
            "xai: set (source: config-file)"
        );
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "an endpoint-bound config key should resolve before the xAI keyring: {:?}",
            keyring.queried()
        );
    }

    #[test]
    fn xai_absent_generation_with_consent_is_external_configured_and_unprobed() {
        use std::sync::Arc;

        let _lock = env_lock();
        let _xai_key = ScopedEnvVar::remove("XAI_API_KEY");
        let _xai_base = ScopedEnvVar::remove("XAI_BASE_URL");
        let _auth_mode = ScopedEnvVar::remove("DEEPSEEK_AUTH_MODE");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = "external owner bytes remain unprobed";
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let _grok_auth_path = ScopedEnvVar::set("GROK_AUTH_PATH", &external_path.to_string_lossy());

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::Xai;
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        store.config.providers.xai.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::Xai,
                codewhale_config::ExternalCredentialSource::GrokCli,
                external_path.clone(),
            ));
        let keyring = Arc::new(RecordingKeyringStore::default());
        let secrets = Secrets::new(keyring.clone());

        let scoped = auth_status_lines_for_provider(&store, &secrets, ProviderKind::Xai).join("\n");
        assert!(
            scoped.contains("credential route: external read-only consent configured/unprobed"),
            "{scoped}"
        );
        assert!(
            scoped.contains("external credentials: read_only"),
            "{scoped}"
        );
        assert!(
            scoped.contains(
                "lookup order: configured consent-gated exact Grok CLI file (availability unprobed)"
            ),
            "{scoped}"
        );

        let all = auth_status_all_providers(&store, &secrets).join("\n");
        let xai_row = all
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI status row");
        assert!(
            xai_row.contains("external consent configured/unprobed"),
            "{xai_row}"
        );

        let list = auth_list_lines(&store, &secrets).join("\n");
        let xai_list_row = list
            .lines()
            .find(|line| line.starts_with("xai"))
            .expect("xAI list row");
        assert!(
            xai_list_row.ends_with("external-consent-configured"),
            "{xai_list_row}"
        );

        let get = auth_get_line_with_runtime(
            &store,
            &secrets,
            ProviderKind::Xai,
            &CliRuntimeOverrides::default(),
        );
        assert!(
            get.contains("source: external read-only consent; availability unprobed"),
            "{get}"
        );
        assert!(
            !keyring.queried().iter().any(|slot| slot == "xai"),
            "external-consent diagnostics must not query the xAI API-key store: {:?}",
            keyring.queried()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external trap unchanged"),
            external_raw
        );
    }

    #[test]
    fn auth_list_uses_persisted_consent_without_probing_codex_file() {
        use codewhale_secrets::InMemoryKeyringStore;
        use std::sync::Arc;

        let _lock = env_lock();
        let _access_token = ScopedEnvVar::set("OPENAI_CODEX_ACCESS_TOKEN", "");
        let _codex_token = ScopedEnvVar::set("CODEX_ACCESS_TOKEN", "");

        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let auth_path = dir.path().join("auth.json");
        std::fs::write(&auth_path, r#"{"tokens":{"access_token":"secret-token"}}"#)
            .expect("write auth file");
        let auth_path_str = auth_path.to_string_lossy().into_owned();
        let _auth_file = ScopedEnvVar::set("OPENAI_CODEX_AUTH_FILE", &auth_path_str);

        let mut store = ConfigStore::load(Some(config_path)).expect("store should load");
        store.config.provider = ProviderKind::OpenaiCodex;
        store.config.providers.openai_codex.external_credentials =
            Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                ProviderKind::OpenaiCodex,
                codewhale_config::ExternalCredentialSource::CodexCli,
                auth_path,
            ));
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));

        let output = auth_list_lines(&store, &secrets).join("\n");
        let row = output
            .lines()
            .find(|line| line.starts_with("openai-codex"))
            .unwrap_or_else(|| panic!("missing openai-codex row:\n{output}"));
        assert!(row.ends_with("external-consent"), "{row}");
        assert!(!output.contains("secret-token"));
    }

    #[test]
    fn auth_list_labels_each_row_by_its_own_provider() {
        // ProviderKind::secret_store_slot collapses families onto one durable
        // slot -- SiliconflowCN onto `siliconflow`, the four Model Studio
        // variants onto `modelstudio-token-plan` -- but this table has one row
        // per kind. Labelling rows by slot printed `siliconflow` twice and
        // `modelstudio-token-plan` four times, so a reader could not tell which
        // row belonged to which provider.
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store =
            ConfigStore::load(Some(dir.path().join("config.toml"))).expect("store should load");
        let secrets = Secrets::new(std::sync::Arc::new(
            codewhale_secrets::InMemoryKeyringStore::new(),
        ));

        let lines = auth_list_lines(&store, &secrets);
        let labels: Vec<&str> = lines
            .iter()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();

        assert_eq!(
            labels.len(),
            ProviderKind::ALL.len(),
            "one row per provider kind: {labels:?}"
        );
        let unique: std::collections::BTreeSet<&&str> = labels.iter().collect();
        assert_eq!(
            unique.len(),
            labels.len(),
            "every row must name its own provider, not a shared slot: {labels:?}"
        );
    }

    #[test]
    fn external_consent_persists_exact_scope_and_api_key_or_revoke_disables_it() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("codewhale-home");
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-auth.json");
        let external_raw = r#"{"secret":"must-never-be-read-or-written"}"#;
        std::fs::write(&external_path, external_raw).expect("external auth trap");
        let mut store = ConfigStore::load(Some(config_path.clone())).expect("store should load");
        let secrets = no_keyring_secrets();

        let preview = external_consent_preview_lines(
            ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
            &external_path,
        )
        .join("\n");
        assert!(preview.contains("owning CLI: Grok CLI"), "{preview}");
        assert!(
            preview.contains(&format!(
                "exact resolved path: {}",
                codewhale_config::quote_os_path(&external_path)
            )),
            "{preview}"
        );
        assert!(preview.contains("no refresh, identity-provider or discovery requests"));
        assert!(preview.contains("normal requests to the explicitly selected provider"));
        assert!(preview.contains("managed: unavailable"));

        let mut prompt = Vec::new();
        confirm_external_consent_answer(&mut "yes\n".as_bytes(), &mut prompt)
            .expect("exact yes confirms");
        assert!(
            String::from_utf8(prompt)
                .unwrap()
                .contains("exact read-only")
        );
        let cancelled = confirm_external_consent_answer(&mut "YES\n".as_bytes(), &mut Vec::new())
            .expect_err("confirmation is deliberate and case-sensitive");
        assert!(cancelled.to_string().contains("cancelled"));

        let unconfirmed = run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalConsent {
                provider: ProviderKind::Xai,
                mode: ExternalCredentialModeArg::ReadOnly,
                path: Some(external_path.clone()),
                yes: false,
            },
            &secrets,
        )
        .expect_err("non-interactive consent requires --yes");
        assert!(unconfirmed.to_string().contains("requires explicit --yes"));
        assert!(store.config.providers.xai.external_credentials.is_none());
        assert!(
            !config_path.exists(),
            "unconfirmed consent must not persist"
        );

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalConsent {
                provider: ProviderKind::Xai,
                mode: ExternalCredentialModeArg::ReadOnly,
                path: Some(external_path.clone()),
                yes: true,
            },
            &secrets,
        )
        .expect("read-only consent should persist");

        let consent = store
            .config
            .providers
            .xai
            .external_credentials
            .as_ref()
            .expect("persisted consent");
        assert_eq!(
            consent.access,
            codewhale_config::ExternalCredentialAccess::ReadOnly
        );
        assert_eq!(consent.provider, ProviderKind::Xai.as_str());
        assert_eq!(
            consent.source,
            codewhale_config::ExternalCredentialSource::GrokCli
        );
        assert_eq!(consent.path, external_path);
        assert_eq!(
            consent.consent_version,
            codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION
        );
        assert_eq!(
            store.config.providers.xai.auth_mode.as_deref(),
            Some("oauth")
        );
        assert_eq!(
            std::fs::read_to_string(&consent.path).expect("external file unchanged"),
            external_raw
        );

        let reloaded = ConfigStore::load(Some(config_path.clone())).expect("reload consent");
        let reloaded_consent = reloaded
            .config
            .providers
            .xai
            .external_credentials
            .as_ref()
            .expect("reloaded exact consent");
        assert_eq!(reloaded_consent.provider, ProviderKind::Xai.as_str());
        assert_eq!(
            reloaded_consent.source,
            codewhale_config::ExternalCredentialSource::GrokCli
        );
        assert_eq!(reloaded_consent.path, external_path);
        assert_eq!(
            reloaded_consent.consent_version,
            codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION
        );

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderKind::Xai,
                api_key: Some("xai-codewhale-owned-key".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("Codewhale-owned API key should supersede external consent");
        assert!(store.config.providers.xai.external_credentials.is_none());
        assert_eq!(
            std::fs::read_to_string(&external_path).expect("external file still unchanged"),
            external_raw
        );

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalConsent {
                provider: ProviderKind::Xai,
                mode: ExternalCredentialModeArg::ReadOnly,
                path: Some(external_path.clone()),
                yes: true,
            },
            &secrets,
        )
        .expect("consent can be granted again");
        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalRevoke {
                provider: ProviderKind::Xai,
            },
            &secrets,
        )
        .expect("revoke should persist");
        assert!(store.config.providers.xai.external_credentials.is_none());
        assert_eq!(
            std::fs::read_to_string(&external_path).expect("revoke never touches external file"),
            external_raw
        );
    }

    #[test]
    fn unsupported_managed_and_kimi_external_consent_fail_closed() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("external-auth.json");
        std::fs::write(&external_path, "must remain unchanged").expect("external fixture");
        let mut store = ConfigStore::load(Some(config_path.clone())).expect("store should load");
        let secrets = no_keyring_secrets();

        let managed = run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalConsent {
                provider: ProviderKind::OpenaiCodex,
                mode: ExternalCredentialModeArg::Managed,
                path: Some(external_path.clone()),
                yes: true,
            },
            &secrets,
        )
        .expect_err("managed access must fail without a preservation adapter");
        assert!(
            managed
                .to_string()
                .contains("schema-safe preservation adapter")
        );

        let kimi = run_auth_command_with_secrets(
            &mut store,
            AuthCommand::ExternalConsent {
                provider: ProviderKind::Moonshot,
                mode: ExternalCredentialModeArg::ReadOnly,
                path: Some(external_path.clone()),
                yes: true,
            },
            &secrets,
        )
        .expect_err("Kimi must remain API-key-only");
        assert!(kimi.to_string().contains("API-key-only"));
        assert!(
            kimi.to_string()
                .contains("https://platform.kimi.ai/console/api-keys")
        );
        assert!(
            store
                .config
                .providers
                .openai_codex
                .external_credentials
                .is_none()
        );
        assert!(
            store
                .config
                .providers
                .moonshot
                .external_credentials
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(external_path).expect("external fixture unchanged"),
            "must remain unchanged"
        );
        assert!(
            !config_path.exists(),
            "rejected consent must not write config"
        );
    }

    #[test]
    fn api_key_config_failure_restores_absent_and_existing_secret_state() {
        let _lock = env_lock();
        for prior in [None, Some("prior-xai-key")] {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let home = dir
                .path()
                .canonicalize()
                .expect("canonical temp root")
                .join("codewhale-home");
            let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
            let config_path = dir.path().join("config.toml");
            let mut store = ConfigStore::load(Some(config_path.clone())).expect("load store");
            store.config.providers.xai.auth_mode = Some("oauth".to_string());
            store.config.providers.xai.external_credentials =
                Some(codewhale_config::ExternalCredentialConsentToml::read_only(
                    ProviderKind::Xai,
                    codewhale_config::ExternalCredentialSource::GrokCli,
                    dir.path().join("external.json"),
                ));
            std::fs::create_dir(&config_path).expect("turn config target into a directory");
            let secrets = no_keyring_secrets();
            if let Some(prior) = prior {
                secrets.set("xai", prior).expect("seed prior secret");
            }

            let error = run_auth_command_with_secrets(
                &mut store,
                AuthCommand::Set {
                    provider: ProviderKind::Xai,
                    api_key: Some("new-xai-key".to_string()),
                    api_key_stdin: false,
                },
                &secrets,
            )
            .expect_err("config write must fail");
            assert!(error.to_string().contains("config"), "{error:#}");
            assert_eq!(
                secrets.get("xai").expect("restored secret"),
                prior.map(str::to_string)
            );
            assert_eq!(
                store.config.providers.xai.auth_mode.as_deref(),
                Some("oauth")
            );
            assert!(store.config.providers.xai.external_credentials.is_some());
            assert!(store.config.providers.xai.api_key.is_none());
            assert!(config_path.is_dir());
        }
    }

    #[test]
    fn auth_status_scoped_provider_shows_detailed_info() {
        use codewhale_secrets::InMemoryKeyringStore;
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-scoped-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;
        store.config.providers.arcee.api_key = Some("sk-arcee-9999".to_string());

        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));

        let output =
            auth_status_lines_for_provider(&store, &secrets, ProviderKind::Arcee).join("\n");

        assert!(output.contains("provider: arcee"));
        assert!(output.contains("active source: config (last4: ...9999)"));
        assert!(output.contains("route:"));
        assert!(output.contains("model:"));
        assert!(!output.contains("sk-arcee-9999"));

        for sentinel in [codewhale_config::API_KEYRING_SENTINEL, "  __KEYRING__  "] {
            store.config.providers.arcee.api_key = Some(sentinel.to_string());
            assert_eq!(provider_config_api_key(&store, ProviderKind::Arcee), None);
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dispatch_uses_secret_store_without_rehydrating_plaintext_config() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        // Runtime resolution reads process-global provider environment overrides.
        // Serialize with the tests that temporarily set those overrides so this
        // in-memory DeepSeek credential is not resolved against another provider.
        let _lock = env_lock();
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-dispatch-keyring-heal-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        let inner = Arc::new(InMemoryKeyringStore::new());
        inner.set("deepseek", "ring-key").unwrap();
        let secrets = Secrets::new(inner);

        let resolved = resolve_runtime_for_dispatch_with_secrets(
            &mut store,
            &CliRuntimeOverrides::default(),
            &secrets,
        );

        assert_eq!(resolved.api_key.as_deref(), Some("ring-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));
        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert!(
            !path.exists(),
            "dispatch must not create config from a stored key"
        );

        let resolved_again = resolve_runtime_for_dispatch_with_secrets(
            &mut store,
            &CliRuntimeOverrides::default(),
            &secrets,
        );
        assert_eq!(resolved_again.api_key.as_deref(), Some("ring-key"));
        assert_eq!(
            resolved_again.api_key_source,
            Some(RuntimeApiKeySource::Keyring)
        );
        assert!(
            !path.exists(),
            "repeat dispatch must remain credential-file free"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn logout_removes_plaintext_provider_keys() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("codewhale-home");
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
        let path = home.join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.api_key = Some("sk-stale".to_string());
        store.config.providers.deepseek.api_key = Some("sk-stale".to_string());
        store.config.providers.fireworks.api_key = Some("fw-stale".to_string());
        store.config.providers.xai.auth_mode = Some("oauth".to_string());
        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        store.config.providers.xai.oauth_credential_generation = Some(generation.to_string());
        store.save().unwrap();
        let credentials = home.join("credentials");
        codewhale_config::with_xai_oauth_lifecycle_lock(|owned| {
            owned.write(generation, b"xai-generation", false)?;
            owned.write(
                codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME,
                b"legacy-xai",
                false,
            )?;
            Ok(())
        })
        .expect("seed Codewhale-owned xAI credentials");
        std::fs::write(credentials.join("other-provider.json"), "preserve").unwrap();

        let secrets = no_keyring_secrets();

        run_logout_command_with_secrets(&mut store, &secrets, None).expect("logout should succeed");

        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert!(store.config.providers.fireworks.api_key.is_none());
        assert!(store.config.providers.xai.auth_mode.is_none());
        assert!(
            store
                .config
                .providers
                .xai
                .oauth_credential_generation
                .is_none()
        );
        assert!(!credentials.join(generation).exists());
        assert!(!credentials.join("xai-auth.json").exists());
        assert!(credentials.join("other-provider.json").exists());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn logout_clears_keyring_credentials_for_all_providers() {
        // Logout used to delete the keyring secret only for the *active*
        // provider, leaving credentials stored under other providers
        // behind while printing "logged out".
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("codewhale-home");
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
        let path = home.join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.provider = ProviderKind::Deepseek;

        let secrets = no_keyring_secrets();
        secrets
            .set(provider_slot(ProviderKind::Deepseek), "sk-deepseek")
            .expect("seed deepseek key");
        secrets
            .set(provider_slot(ProviderKind::Fireworks), "fw-stale")
            .expect("seed fireworks key");

        run_logout_command_with_secrets(&mut store, &secrets, None).expect("logout should succeed");

        for provider in [ProviderKind::Deepseek, ProviderKind::Fireworks] {
            assert!(
                provider_keyring_api_key(&secrets, provider).is_none(),
                "keyring credential for {provider:?} survived logout"
            );
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn logout_clears_account_session_and_daytona_slot() {
        use codewhale_secrets::account::{
            AccountAuthBundle, AccountSession, AccountSessionStore, AccountUser,
            DEFAULT_ACCOUNT_API_BASE, secure_account_session_secrets,
        };

        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("codewhale-home");
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &home.to_string_lossy());
        let path = home.join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");

        let secrets = no_keyring_secrets();
        secrets
            .set(codewhale_secrets::DAYTONA_TOKEN_SLOT, "dtn_logout")
            .expect("seed daytona token");

        let account = secure_account_session_secrets().expect("account store");
        AccountSessionStore::new(account, None, DEFAULT_ACCOUNT_API_BASE)
            .save(AccountAuthBundle {
                token_type: "Bearer".to_string(),
                access_token: "access-logout".to_string(),
                refresh_token: "refresh-logout".to_string(),
                session: Some(AccountSession {
                    id: "session-logout".to_string(),
                    ..AccountSession::default()
                }),
                user: Some(AccountUser {
                    id: "acct-logout".to_string(),
                    ..AccountUser::default()
                }),
            })
            .expect("seed account session");

        run_logout_command_with_secrets(&mut store, &secrets, None).expect("logout should succeed");

        assert!(
            secrets
                .get(codewhale_secrets::DAYTONA_TOKEN_SLOT)
                .expect("read daytona")
                .is_none(),
            "daytona slot survived logout"
        );
        let account = secure_account_session_secrets().expect("account store after logout");
        assert!(
            AccountSessionStore::new(account, None, DEFAULT_ACCOUNT_API_BASE)
                .load()
                .expect("load account")
                .is_none(),
            "account session survived logout"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_set_slot_daytona_has_no_user_surface() {
        // The internal cloud-agent credential is managed by Codewhale, not
        // users: no CLI command may write or clear it, and no help text may
        // teach it. Membership (`codewhale login`) is the only door.
        use codewhale_secrets::InMemoryKeyringStore;
        use std::sync::Arc;

        for argv in [
            vec![
                "codewhale",
                "auth",
                "set-slot",
                "daytona",
                "--api-key",
                "dtn_saved",
            ],
            vec!["codewhale", "auth", "clear-slot", "daytona"],
        ] {
            assert!(
                Cli::try_parse_from(argv).is_err(),
                "slot commands must not parse"
            );
        }

        let inner = Arc::new(InMemoryKeyringStore::new());
        let secrets = Secrets::new(inner);
        assert!(
            secrets
                .get(codewhale_secrets::DAYTONA_TOKEN_SLOT)
                .expect("read slot")
                .is_none()
        );
    }

    #[test]
    fn auth_migrate_moves_plaintext_keys_into_keyring_and_strips_file() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-migrate-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.api_key = Some("sk-deep".to_string());
        store.config.providers.deepseek.api_key = Some("sk-deep".to_string());
        store.config.providers.openrouter.api_key = Some("or-key".to_string());
        store.config.providers.novita.api_key = Some("nv-key".to_string());
        store.save().unwrap();

        let inner = Arc::new(InMemoryKeyringStore::new());
        let secrets = Secrets::new(inner.clone());

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Migrate { dry_run: false },
            &secrets,
        )
        .expect("migrate should succeed");

        assert_eq!(inner.get("deepseek").unwrap(), Some("sk-deep".to_string()));
        assert_eq!(inner.get("openrouter").unwrap(), Some("or-key".to_string()));
        assert_eq!(inner.get("novita").unwrap(), Some("nv-key".to_string()));

        // Config file must no longer contain the api keys.
        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert!(store.config.providers.openrouter.api_key.is_none());
        assert!(store.config.providers.novita.api_key.is_none());

        let saved = std::fs::read_to_string(&path).expect("config exists post-migrate");
        assert!(!saved.contains("sk-deep"), "plaintext leaked: {saved}");
        assert!(!saved.contains("or-key"), "plaintext leaked: {saved}");
        assert!(!saved.contains("nv-key"), "plaintext leaked: {saved}");

        let backup_path = path.with_file_name(format!(
            "{}.bak",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let backup = std::fs::read_to_string(&backup_path).expect("credential-free backup");
        assert!(
            !backup.contains("sk-deep"),
            "plaintext leaked in backup: {backup}"
        );
        assert!(
            !backup.contains("or-key"),
            "plaintext leaked in backup: {backup}"
        );
        assert!(
            !backup.contains("nv-key"),
            "plaintext leaked in backup: {backup}"
        );

        let resolved = resolve_runtime_for_dispatch_with_secrets(
            &mut store,
            &CliRuntimeOverrides::default(),
            &secrets,
        );
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));
        let after_dispatch = std::fs::read_to_string(&path).expect("config after dispatch");
        assert!(!after_dispatch.contains("sk-deep"), "{after_dispatch}");
        assert!(
            !after_dispatch
                .lines()
                .any(|line| line.trim_start().starts_with("api_key ="))
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_migrate_dry_run_does_not_modify_anything() {
        use codewhale_secrets::{InMemoryKeyringStore, KeyringStore};
        use std::sync::Arc;

        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-auth-migrate-dry-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.providers.openrouter.api_key = Some("or-stay".to_string());
        store.save().unwrap();

        let inner = Arc::new(InMemoryKeyringStore::new());
        let secrets = Secrets::new(inner.clone());

        run_auth_command_with_secrets(&mut store, AuthCommand::Migrate { dry_run: true }, &secrets)
            .expect("dry-run should succeed");

        assert_eq!(inner.get("openrouter").unwrap(), None);
        assert_eq!(
            store.config.providers.openrouter.api_key.as_deref(),
            Some("or-stay")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_global_override_flags() {
        let cli = parse_ok(&[
            "deepseek",
            "--provider",
            "openai",
            "--config",
            "/tmp/deepseek.toml",
            "--profile",
            "work",
            "--model",
            "deepseek-v4-pro",
            "--output-mode",
            "json",
            "--verbosity",
            "concise",
            "--log-level",
            "debug",
            "--telemetry",
            "true",
            "--approval-policy",
            "on-request",
            "--sandbox-mode",
            "workspace-write",
            "--base-url",
            "https://openai-compatible.example/v1",
            "--api-key",
            "sk-test",
            "--workspace",
            "/tmp/workspace",
            "--no-mouse-capture",
            "--skip-onboarding",
            "model",
            "resolve",
            "deepseek-v4-pro",
        ]);

        assert_eq!(cli.provider.as_deref(), Some("openai"));
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/deepseek.toml")));
        assert_eq!(cli.profile.as_deref(), Some("work"));
        assert_eq!(cli.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(cli.output_mode.as_deref(), Some("json"));
        assert_eq!(cli.verbosity.as_deref(), Some("concise"));
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
        assert_eq!(cli.telemetry, Some(true));
        assert_eq!(cli.approval_policy.as_deref(), Some("on-request"));
        assert_eq!(cli.sandbox_mode.as_deref(), Some("workspace-write"));
        assert_eq!(
            cli.base_url.as_deref(),
            Some("https://openai-compatible.example/v1")
        );
        assert_eq!(cli.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cli.workspace, Some(PathBuf::from("/tmp/workspace")));
        assert!(cli.no_mouse_capture);
        assert!(!cli.mouse_capture);
        assert!(cli.skip_onboarding);
    }

    #[test]
    fn cli_provider_helpers_follow_config_metadata() {
        let registry_kinds: Vec<ProviderKind> = codewhale_config::provider::all_providers()
            .iter()
            .map(|provider| provider.kind())
            .collect();
        // Full registry keeps legacy dialect/plan kinds; ALL is the catalog surface.
        assert_eq!(registry_kinds.len(), 48);
        assert_eq!(ProviderKind::ALL.len(), 43);
        for kind in ProviderKind::ALL {
            assert!(
                registry_kinds.contains(&kind),
                "catalog kind {kind:?} must remain in the full registry"
            );
        }

        for provider in registry_kinds {
            assert_eq!(provider_env_vars(provider), provider.provider().env_vars());
            // Shared-account families collapse onto one durable slot (see
            // ProviderKind::secret_store_slot); everything else uses its own id.
            assert_eq!(
                provider_slot(provider),
                provider.secret_store_slot(),
                "{provider:?} slot must match ProviderKind::secret_store_slot"
            );
            if provider == ProviderKind::SiliconflowCN {
                assert_eq!(
                    provider_slot(provider),
                    provider_slot(ProviderKind::Siliconflow)
                );
            } else if matches!(
                provider,
                ProviderKind::ModelstudioTokenPlan
                    | ProviderKind::ModelstudioTokenPlanAnthropic
                    | ProviderKind::ModelstudioCodingPlan
                    | ProviderKind::ModelstudioCodingPlanAnthropic
            ) {
                assert_eq!(
                    provider_slot(provider),
                    "modelstudio-token-plan",
                    "{provider:?} must share the Model Studio family slot"
                );
            } else {
                assert_eq!(provider_slot(provider), provider.provider().id());
            }
        }
    }

    #[test]
    fn the_telemetry_flag_documents_itself_in_help() {
        // A consent control nobody can find is a consent control nobody has.
        let help = Cli::command().render_long_help().to_string();
        let telemetry_line = help
            .lines()
            .position(|line| line.contains("--telemetry"))
            .map(|index| help.lines().skip(index).take(3).collect::<String>())
            .expect("--telemetry must appear in --help");
        assert!(
            telemetry_line.contains("telemetry"),
            "expected a help string beside --telemetry, got: {telemetry_line}"
        );
        assert!(
            telemetry_line.contains("default on"),
            "the help string must disclose the default: {telemetry_line}"
        );
        assert!(
            telemetry_line.contains("CODEWHALE_TELEMETRY=0 always")
                && telemetry_line.contains("wins"),
            "the help string must document the always-winning opt-out: {telemetry_line}"
        );
    }

    #[test]
    fn root_help_describes_product_actions_not_internal_tui_layers() {
        let help = Cli::command().render_long_help().to_string();
        assert!(
            !help.contains("TUI"),
            "root help must describe what Codewhale does, not its internal UI/runtime layers:\n{help}"
        );
    }

    #[test]
    fn only_one_function_may_locate_and_spawn_the_tui() {
        // Single-binary invariant: no sibling TUI discovery exists. The
        // two-process glue has been deleted; the only TUI entry is codewhale_tui::run.
        let source = include_str!("lib.rs");
        let a = format!("{}{}", "locate_sibling", "_tui_binary");
        let b = format!("{}{}", "tui_spawn", "_error");
        let c = format!("{}{}", "build_tui", "_command");
        let d = format!("{}{}", "Command::new", "(&tui)");
        assert!(
            !source.contains(&a),
            "single binary must not contain sibling TUI discovery"
        );
        assert!(
            !source.contains(&b),
            "single binary must not contain tui spawn error"
        );
        assert!(
            !source.contains(&c),
            "single binary must not contain build_tui dispatch"
        );
        assert!(
            !source.contains(&d),
            "single binary must not contain Command new tui"
        );
    }

    #[test]
    fn parses_no_project_config_before_subcommand() {
        let cli = parse_ok(&["codewhale", "--no-project-config", "exec", "list the files"]);
        assert!(cli.no_project_config);
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert_eq!(args.args, vec!["list the files".to_string()]);
            }
            other => panic!("expected exec subcommand, got {other:?}"),
        }
    }

    #[test]
    fn no_project_config_after_passthrough_subcommand_is_not_the_dispatcher_flag() {
        // `exec` captures trailing args (`trailing_var_arg`), so a misplaced
        // `--no-project-config` is NOT honored as the dispatcher flag — it must
        // appear before the subcommand, exactly like `--skip-onboarding`.
        let cli = parse_ok(&["codewhale", "exec", "--no-project-config", "hi"]);
        assert!(!cli.no_project_config);
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert!(args.args.iter().any(|a| a == "--no-project-config"));
            }
            other => panic!("expected exec subcommand, got {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_prompt_flag_for_interactive_startup_prompt() {
        let cli = parse_ok(&["deepseek", "-p", "Reply with exactly OK."]);

        assert_eq!(cli.prompt_flag.as_deref(), Some("Reply with exactly OK."));
        assert!(cli.prompt.is_empty());
        assert_eq!(
            root_tui_passthrough(&cli).unwrap(),
            vec!["--prompt".to_string(), "Reply with exactly OK.".to_string()]
        );
    }

    #[test]
    fn parses_top_level_continue_for_interactive_resume() {
        let cli = parse_ok(&["codewhale", "--continue"]);

        assert!(cli.continue_session);
        assert!(cli.prompt_flag.is_none());
        assert!(cli.prompt.is_empty());
        assert_eq!(root_tui_passthrough(&cli).unwrap(), vec!["--continue"]);
    }

    #[test]
    fn parses_top_level_resume_flags_for_interactive_resume() {
        // The operations runbook advertises `codewhale --resume <id>`. Before
        // the root flag existed, the trailing prompt positional swallowed it
        // and forwarded `--prompt "--resume <id>"` to the TUI (exit 2).
        for argv in [
            &["codewhale", "--resume", "800596e6"][..],
            &["codewhale", "--resume=800596e6"][..],
            &["codewhale", "-r", "800596e6"][..],
            &["codewhale", "--session-id", "800596e6"][..],
            &["codewhale", "--session-id=800596e6"][..],
        ] {
            let cli = parse_ok(argv);
            assert!(
                cli.prompt.is_empty(),
                "{argv:?} must not be swallowed as a prompt: {:?}",
                cli.prompt
            );
            assert!(cli.prompt_flag.is_none(), "{argv:?}");
            assert!(cli.command.is_none(), "{argv:?}");
            assert_eq!(
                root_tui_passthrough(&cli).unwrap(),
                vec!["--resume".to_string(), "800596e6".to_string()],
                "{argv:?}"
            );
        }
    }

    #[test]
    fn empty_resume_identifier_is_rejected_rather_than_starting_fresh() {
        // `codewhale --resume "$SESSION_ID"` with the variable unset used to
        // trim to empty, filter to None, and start a brand-new session while
        // looking like it resumed one. Losing the session the user asked for
        // must be loud.
        for argv in [
            &["codewhale", "--resume", ""][..],
            &["codewhale", "--session-id", "   "][..],
        ] {
            let cli = parse_ok(argv);
            let err = root_tui_passthrough(&cli)
                .expect_err("an empty resume id must not silently start a fresh session");
            assert!(
                err.to_string().contains("needs a session id"),
                "{argv:?}: {err}"
            );
        }
    }

    #[test]
    fn top_level_resume_rejects_startup_prompt_and_conflicting_flags() {
        let cli = parse_ok(&["codewhale", "--resume", "800596e6", "-p", "follow up"]);
        let err = root_tui_passthrough(&cli).expect_err("prompted resume should be rejected");
        assert!(
            err.to_string()
                .contains("codewhale exec --resume 800596e6 <PROMPT>"),
            "{err}"
        );

        assert!(Cli::try_parse_from(["codewhale", "--resume", "800596e6", "--continue"]).is_err());
        assert!(Cli::try_parse_from(["codewhale", "--resume", "a", "--session-id", "b"]).is_err());
        assert!(Cli::try_parse_from(["codewhale", "--session-id", "b", "-c"]).is_err());
    }

    #[test]
    fn parses_rc_as_the_account_owned_interactive_handoff() {
        let cli = parse_ok(&["codewhale", "rc"]);

        let Some(Commands::Rc(args)) = cli.command else {
            panic!("rc should parse as the remote-control TUI handoff");
        };
        assert!(args.args.is_empty());
    }

    #[test]
    fn top_level_continue_rejects_startup_prompt() {
        let cli = parse_ok(&["codewhale", "--continue", "-p", "follow up"]);

        let err = root_tui_passthrough(&cli).expect_err("prompted continue should be rejected");
        assert!(
            err.to_string()
                .contains("codewhale exec --continue <PROMPT>")
        );
    }

    #[test]
    fn parses_split_top_level_prompt_words_for_windows_cmd_shims() {
        let cli = parse_ok(&["deepseek", "hello", "world"]);

        assert_eq!(cli.prompt, vec!["hello", "world"]);
        assert!(cli.command.is_none());
        assert_eq!(
            root_tui_passthrough(&cli).unwrap(),
            vec!["--prompt".to_string(), "hello world".to_string()]
        );
    }

    #[test]
    fn prompt_flag_keeps_split_tail_words_for_windows_cmd_shims() {
        let cli = parse_ok(&["deepseek", "-p", "hello", "world"]);

        assert_eq!(cli.prompt_flag.as_deref(), Some("hello"));
        assert_eq!(cli.prompt, vec!["world"]);
        assert_eq!(
            root_tui_passthrough(&cli).unwrap(),
            vec!["--prompt".to_string(), "hello world".to_string()]
        );
    }

    #[test]
    fn known_subcommands_still_parse_before_prompt_tail() {
        let cli = parse_ok(&["deepseek", "doctor"]);

        assert!(cli.prompt.is_empty());
        assert!(matches!(cli.command, Some(Commands::Doctor(_))));
    }

    #[test]
    fn root_help_surface_contains_expected_subcommands_and_globals() {
        let rendered = help_for(&["deepseek", "--help"]);

        for token in [
            "run",
            "doctor",
            "models",
            "sessions",
            "resume",
            "setup",
            "login",
            "logout",
            "auth",
            "mcp-server",
            "config",
            "model",
            "providers",
            "thread",
            "sandbox",
            "app-server",
            "completion",
            "metrics",
            "--provider",
            "--model",
            "--config",
            "--profile",
            "--output-mode",
            "--log-level",
            "--telemetry",
            "--base-url",
            "--api-key",
            "--approval-policy",
            "--sandbox-mode",
            "--mouse-capture",
            "--no-mouse-capture",
            "--skip-onboarding",
            "--continue",
            "--prompt",
        ] {
            assert!(
                rendered.contains(token),
                "expected help to contain token: {token}"
            );
        }
    }

    #[test]
    fn subcommand_help_surfaces_are_stable() {
        let cases = [
            ("config", vec!["get", "set", "unset", "list", "path"]),
            ("model", vec!["list", "resolve"]),
            (
                "thread",
                vec![
                    "list",
                    "read",
                    "resume",
                    "fork",
                    "archive",
                    "unarchive",
                    "set-name",
                    "clear-name",
                ],
            ),
            ("sandbox", vec!["check"]),
            (
                "exec",
                vec![
                    "--auto",
                    "--json",
                    "--resume",
                    "--session-id",
                    "--continue",
                    "--output-format",
                    "stream-json",
                ],
            ),
            (
                "app-server",
                vec!["--host", "--port", "--config", "--stdio"],
            ),
            (
                "completion",
                vec![
                    "<SHELL>",
                    "bash",
                    "Every script completes both `codewhale` and the `codew` shorthand.",
                    "source <(codewhale completion bash)",
                    "~/.local/share/bash-completion/completions/codewhale",
                    "fpath=(~/.zfunc $fpath)",
                    "codewhale completion fish > ~/.config/fish/completions/codewhale.fish",
                    "codewhale completion powershell | Out-String | Invoke-Expression",
                    "codewhale completion elvish >> ~/.config/elvish/rc.elv",
                ],
            ),
            ("metrics", vec!["--json", "--since"]),
        ];

        for (subcommand, expected_tokens) in cases {
            let argv = ["deepseek", subcommand, "--help"];
            let rendered = help_for(&argv);
            for token in expected_tokens {
                assert!(
                    rendered.contains(token),
                    "expected help for `{subcommand}` to include `{token}`"
                );
            }
        }
    }

    #[test]
    fn cli_telemetry_start_fails_closed_on_a_corrupt_setup_state() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_path = dir.path().join("setup_state.json");
        std::fs::write(&setup_path, b"{not-json").expect("write corrupt setup state");
        let setup = telemetry::load_setup_state_for_decision_at(&setup_path);
        assert!(setup.is_none(), "corrupt privacy state must not default on");

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());
        assert!(
            resolve_cli_telemetry_consent(&resolved, None, Surface::Cli, setup).is_none(),
            "CLI startup must not obtain permission from an unreadable privacy record"
        );
    }
}
