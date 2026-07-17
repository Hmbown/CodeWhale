#![allow(clippy::uninlined_format_args)]

mod metrics;
#[cfg(not(target_env = "ohos"))]
mod update;

use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use codewhale_agent::ModelRegistry;
use codewhale_app_server::{
    AppServerOptions, run as run_app_server, run_stdio as run_app_server_stdio,
};
use codewhale_config::{
    CliRuntimeOverrides, ConfigStore, ConfigToml, ProviderKind, ProviderSource,
    ResolvedRuntimeOptions, RuntimeApiKeySource,
};
use codewhale_execpolicy::{AskForApproval, ExecPolicyContext, ExecPolicyEngine};
use codewhale_mcp::{McpServerDefinition, run_stdio_server};
use codewhale_secrets::Secrets;
use codewhale_state::{StateStore, ThreadListFilters};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ProviderArg {
    Deepseek,
    NvidiaNim,
    Openai,
    Atlascloud,
    WanjieArk,
    Volcengine,
    Openrouter,
    XiaomiMimo,
    Novita,
    Fireworks,
    Siliconflow,
    #[value(
        alias = "silicon-flow-cn",
        alias = "siliconflow-CN",
        alias = "silicon_flow_cn",
        alias = "siliconflow_cn",
        alias = "siliconflow-china",
        alias = "siliconflow_china"
    )]
    SiliconflowCn,
    Arcee,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    Huggingface,
    Together,
    OpenaiCodex,
    Anthropic,
    #[value(alias = "open-model", alias = "open_model")]
    Openmodel,
    Zai,
    Stepfun,
    Minimax,
    #[value(
        alias = "minimax_anthropic",
        alias = "mini-max-anthropic",
        alias = "mini_max_anthropic"
    )]
    MinimaxAnthropic,
    #[value(alias = "deep-infra", alias = "deep_infra")]
    Deepinfra,
    #[value(alias = "fugu", alias = "sakana-ai", alias = "sakana_ai")]
    Sakana,
    #[value(alias = "long-cat", alias = "meituan-longcat", alias = "meituan")]
    LongCat,
    #[value(alias = "opencode_go", alias = "opencodego")]
    OpencodeGo,
    #[value(
        alias = "opencode_zen",
        alias = "opencodezen",
        alias = "zen",
        alias = "opencode"
    )]
    OpencodeZen,
    #[value(
        alias = "meta-ai",
        alias = "meta_ai",
        alias = "meta-model-api",
        alias = "muse",
        alias = "muse-spark"
    )]
    Meta,
    #[value(alias = "x-ai", alias = "x_ai", alias = "grok")]
    Xai,
}

impl From<ProviderArg> for ProviderKind {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::Deepseek => ProviderKind::Deepseek,
            ProviderArg::NvidiaNim => ProviderKind::NvidiaNim,
            ProviderArg::Openai => ProviderKind::Openai,
            ProviderArg::Atlascloud => ProviderKind::Atlascloud,
            ProviderArg::WanjieArk => ProviderKind::WanjieArk,
            ProviderArg::Volcengine => ProviderKind::Volcengine,
            ProviderArg::Openrouter => ProviderKind::Openrouter,
            ProviderArg::XiaomiMimo => ProviderKind::XiaomiMimo,
            ProviderArg::Novita => ProviderKind::Novita,
            ProviderArg::Fireworks => ProviderKind::Fireworks,
            ProviderArg::Siliconflow => ProviderKind::Siliconflow,
            ProviderArg::SiliconflowCn => ProviderKind::SiliconflowCN,
            ProviderArg::Arcee => ProviderKind::Arcee,
            ProviderArg::Moonshot => ProviderKind::Moonshot,
            ProviderArg::Sglang => ProviderKind::Sglang,
            ProviderArg::Vllm => ProviderKind::Vllm,
            ProviderArg::Ollama => ProviderKind::Ollama,
            ProviderArg::Huggingface => ProviderKind::Huggingface,
            ProviderArg::Together => ProviderKind::Together,
            ProviderArg::OpenaiCodex => ProviderKind::OpenaiCodex,
            ProviderArg::Anthropic => ProviderKind::Anthropic,
            ProviderArg::Openmodel => ProviderKind::Openmodel,
            ProviderArg::Zai => ProviderKind::Zai,
            ProviderArg::Stepfun => ProviderKind::Stepfun,
            ProviderArg::Minimax => ProviderKind::Minimax,
            ProviderArg::MinimaxAnthropic => ProviderKind::MinimaxAnthropic,
            ProviderArg::Deepinfra => ProviderKind::Deepinfra,
            ProviderArg::Sakana => ProviderKind::Sakana,
            ProviderArg::LongCat => ProviderKind::LongCat,
            ProviderArg::OpencodeGo => ProviderKind::OpencodeGo,
            ProviderArg::OpencodeZen => ProviderKind::OpencodeZen,
            ProviderArg::Meta => ProviderKind::Meta,
            ProviderArg::Xai => ProviderKind::Xai,
        }
    }
}

fn builtin_provider_arg(value: &str) -> Option<ProviderArg> {
    ProviderArg::from_str(value, false).ok()
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
    version = env!("DEEPSEEK_BUILD_VERSION"),
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
    #[arg(long)]
    telemetry: Option<bool>,
    #[arg(long)]
    approval_policy: Option<String>,
    #[arg(long)]
    sandbox_mode: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    /// Workspace directory for TUI file tools
    #[arg(short = 'C', long = "workspace", alias = "cd", value_name = "DIR")]
    workspace: Option<PathBuf>,
    #[arg(long = "no-alt-screen", hide = true)]
    no_alt_screen: bool,
    #[arg(long = "mouse-capture", conflicts_with = "no_mouse_capture")]
    mouse_capture: bool,
    #[arg(long = "no-mouse-capture", conflicts_with = "mouse_capture")]
    no_mouse_capture: bool,
    #[arg(long = "skip-onboarding")]
    skip_onboarding: bool,
    /// Legacy compatibility alias for Act + Full Access.
    #[arg(long, hide = true)]
    yolo: bool,
    /// Continue the most recent interactive session for this workspace.
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,
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
    /// Run interactive/non-interactive flows via the TUI binary.
    Run(RunArgs),
    /// Run Codewhale diagnostics.
    Doctor(TuiPassthroughArgs),
    /// List live provider API models via the TUI binary.
    Models(TuiPassthroughArgs),
    /// Generate speech audio with Xiaomi MiMo TTS models via the TUI binary.
    #[command(visible_alias = "tts")]
    Speech(TuiPassthroughArgs),
    /// List saved TUI sessions.
    Sessions(TuiPassthroughArgs),
    /// Resume a saved TUI session.
    Resume(TuiPassthroughArgs),
    /// Fork a saved TUI session.
    Fork(TuiPassthroughArgs),
    /// Create a default AGENTS.md in the current directory.
    Init(TuiPassthroughArgs),
    /// Bootstrap MCP config and/or skills directories.
    Setup(TuiPassthroughArgs),
    /// Generate a remote Codewhale agent deploy bundle (cloud + chat bridge).
    RemoteSetup(RemoteSetupArgs),
    /// Run a non-interactive prompt through the TUI runtime.
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
    /// Manage durable Agent Fleet runs via the TUI runtime.
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
  codewhale lane stop <lane-id>
  codewhale lane start --workflow stopship --fleet stopship --runtime tmux --goal verify-release-candidate -- echo hello

Lane records persist under $CODEWHALE_HOME/lanes/. tmux durability belongs to
Runtime, not Fleet.
")]
    Lane(LaneArgs),
    /// Run a Codewhale-powered code review over a git diff.
    Review(TuiPassthroughArgs),
    /// Apply a patch file or stdin to the working tree.
    Apply(TuiPassthroughArgs),
    /// Run the offline TUI evaluation harness.
    Eval(TuiPassthroughArgs),
    /// Manage TUI MCP servers.
    Mcp(TuiPassthroughArgs),
    /// Inspect TUI feature flags.
    Features(TuiPassthroughArgs),
    /// Run a local TUI server.
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
    /// Generate shell completions for the TUI binary.
    Completions(TuiPassthroughArgs),
    /// Configure provider credentials.
    Login(LoginArgs),
    /// Remove saved authentication state.
    Logout,
    /// Manage authentication credentials and provider mode.
    Auth(AuthArgs),
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
    #[command(after_help = r#"Examples:
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

The command prints the completion script to stdout; redirect it to a path your shell loads automatically."#)]
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Print a usage rollup from the audit log and session store.
    Metrics(MetricsArgs),
    /// Check for and apply updates to the `codewhale` binary.
    Update(UpdateArgs),
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
        return Ok(Some(provider.into()));
    }
    if command_accepts_raw_provider(command) {
        return Ok(None);
    }

    let expected = ProviderArg::value_variants()
        .iter()
        .filter_map(ValueEnum::to_possible_value)
        .map(|value| value.get_name().to_string())
        .collect::<Vec<_>>()
        .join(", ");
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
    Stop { lane_id: String },
    /// Start a lane under a Runtime backend (tmux|inline|vm|ci).
    Start {
        /// Workflow name (e.g. `stopship`).
        #[arg(long)]
        workflow: Option<String>,
        /// Fleet roster name (e.g. `stopship`).
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
        /// Named Fleet roster (e.g. stopship). Required for role-resolved Workflow runs.
        #[arg(long)]
        fleet: String,
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

fn run_lane_command(args: LaneArgs) -> Result<()> {
    use codewhale_lane::{LaneRegistry, backend_for};
    use std::io::{BufRead, Seek, Write};
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    match args.command {
        LaneCommand::List { json } => {
            let reg = LaneRegistry::open_default()?;
            let mut lanes = reg.list()?;
            for lane in &mut lanes {
                if let Err(err) = backend_for(lane).reconcile(&reg, lane) {
                    eprintln!("warning: could not reconcile lane `{}`: {err:#}", lane.id);
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&lanes)?);
            } else if lanes.is_empty() {
                println!("No lanes under {}", reg.root().display());
            } else {
                println!(
                    "{:<16} {:<10} {:<12} {:<16} {:<10} STARTED",
                    "ID", "STATUS", "RUNTIME", "WORKFLOW", "ISSUE"
                );
                for lane in lanes {
                    println!(
                        "{:<16} {:<10} {:<12} {:<16} {:<10} {}",
                        lane.id,
                        lane.status.as_str(),
                        lane.runtime.as_str(),
                        lane.workflow.as_deref().unwrap_or("-"),
                        lane.issue.as_deref().unwrap_or("-"),
                        lane.started_at,
                    );
                }
            }
            Ok(())
        }
        LaneCommand::Status { lane_id, json } => {
            let reg = LaneRegistry::open_default()?;
            let mut lane = reg.load(&lane_id)?;
            backend_for(&lane).reconcile(&reg, &mut lane)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&lane)?);
            } else {
                println!("lane:     {}", lane.id);
                println!("status:   {}", lane.status.as_str());
                println!("runtime:  {}", lane.runtime.as_str());
                println!("workflow: {}", lane.workflow.as_deref().unwrap_or("-"));
                println!("fleet:    {}", lane.fleet.as_deref().unwrap_or("-"));
                println!("issue:    {}", lane.issue.as_deref().unwrap_or("-"));
                println!("goal:     {}", lane.goal.as_deref().unwrap_or("-"));
                println!("started:  {}", lane.started_at);
                println!("stopped:  {}", lane.stopped_at.as_deref().unwrap_or("-"));
                println!(
                    "worktree: {}",
                    lane.worktree_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "-".into())
                );
                println!("branch:   {}", lane.branch.as_deref().unwrap_or("-"));
                println!("tmux:     {}", lane.tmux_session.as_deref().unwrap_or("-"));
                println!(
                    "socket:   {}",
                    lane.tmux_socket
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
                println!("attach:   {}", lane.attach_target.as_deref().unwrap_or("-"));
                println!("log:      {}", lane.log_path.display());
            }
            Ok(())
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
        LaneCommand::Stop { lane_id } => {
            let reg = LaneRegistry::open_default()?;
            let mut lane = reg.load(&lane_id)?;
            let backend = backend_for(&lane);
            backend.stop(&reg, &mut lane)?;
            println!("stopped {}", lane.id);
            Ok(())
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

            let roots = named_fleet_search_roots(&workspace);
            let named_fleet = codewhale_workflow::load_named_fleet(&fleet, &roots)
                .with_context(|| format!("load fleet `{fleet}` from {}", display_roots(&roots)))?;
            if workflow == "stopship" || fleet == "stopship" || fleet == "v0868-stopship" {
                named_fleet
                    .validate_stopship_roles()
                    .with_context(|| format!("validate stopship roles in fleet `{fleet}`"))?;
            }

            let process = workflow_exec_command(WorkflowExecSpec {
                cli,
                resolved_runtime,
                config_path,
                source_root: &source_root,
                source_path: &source_path,
                workflow: &workflow,
                fleet: &fleet,
                issue: issue.as_deref(),
                goal: goal.as_deref(),
                token_budget,
                verify,
            })?;
            start_lane(LaneStartRequest {
                workflow: Some(workflow),
                fleet: Some(fleet),
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
    fleet: &'a str,
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
    let command =
        build_tui_command_with_paths(cli, resolved_runtime, passthrough, Some(config_path), None)?;
    lane_process_spec_from_command(&command)
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

fn lane_process_spec_from_command(command: &Command) -> Result<WorkflowProcessSpec> {
    let mut argv = Vec::new();
    argv.push(command.get_program().to_string_lossy().into_owned());
    argv.extend(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    let mut environment = std::collections::BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        if valid_lane_environment_key(key) && !shell_owned_lane_environment(key) {
            environment.insert(key.to_string(), value.to_string());
        }
    }
    for (key, value) in command.get_envs() {
        let key = key
            .to_str()
            .context("workflow runtime environment key is not UTF-8")?
            .to_string();
        if let Some(value) = value {
            environment.insert(
                key,
                value
                    .to_str()
                    .context("workflow runtime environment value is not UTF-8")?
                    .to_string(),
            );
        } else {
            environment.remove(&key);
        }
    }
    Ok(WorkflowProcessSpec {
        command: argv,
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
    #[arg(long, value_enum, hide = true)]
    provider: Option<ProviderArg>,
    #[arg(long)]
    api_key: Option<String>,
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
    /// Show current provider and credential source state.
    /// Without `--provider`, shows all known providers.
    /// With `--provider`, shows detailed status for that provider.
    Status {
        /// Show status for a specific provider only.
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
    },
    /// Save an API key to the shared user config file. Reads from
    /// `--api-key`, `--api-key-stdin`, or prompts on stdin when
    /// neither is given. Does not echo the key.
    Set {
        #[arg(long, value_enum)]
        provider: ProviderArg,
        /// Inline value (discouraged — appears in shell history).
        #[arg(long)]
        api_key: Option<String>,
        /// Read the key from stdin instead of prompting.
        #[arg(long = "api-key-stdin", default_value_t = false)]
        api_key_stdin: bool,
    },
    /// Report whether a provider has a key configured. Never prints
    /// the value; just `set` / `not set` plus the source layer.
    Get {
        #[arg(long, value_enum)]
        provider: ProviderArg,
    },
    /// Delete a provider's key from config and secret-store storage.
    Clear {
        #[arg(long, value_enum)]
        provider: ProviderArg,
    },
    /// List all known providers with their auth state, without
    /// revealing keys.
    List,
    /// Advanced: migrate config-file keys into a platform credential store.
    #[command(hide = true)]
    Migrate {
        /// Don't actually write anything; print what would change.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Get { key: String },
    Set { key: String, value: String },
    Unset { key: String },
    List,
    Path,
}

#[derive(Debug, Args)]
struct ModelArgs {
    #[command(subcommand)]
    command: ModelCommand,
}

#[derive(Debug, Subcommand)]
enum ModelCommand {
    List {
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
    },
    Resolve {
        model: Option<String>,
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
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

fn run() -> Result<()> {
    let mut cli = Cli::parse();

    // The detached log proxy must not depend on user config parsing: its job
    // is to frame child output and publish a terminal receipt even when the
    // delegated command's own config is malformed.
    let (proxy, command) = split_lane_log_proxy_command(cli.command.take());
    if let Some(args) = proxy {
        return run_lane_log_proxy_command(args);
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
        return delegate_to_tui(&cli, &resolved_runtime, passthrough);
    }

    let mut store = ConfigStore::load(cli.config.clone())?;
    match command {
        Some(Commands::Run(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, args.args)
        }
        Some(Commands::Doctor(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("doctor", args))
        }
        Some(Commands::Models(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("models", args))
        }
        Some(Commands::Speech(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("speech", args))
        }
        Some(Commands::Sessions(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("sessions", args))
        }
        Some(Commands::Resume(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            run_resume_command(&cli, &resolved_runtime, args)
        }
        Some(Commands::Fork(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("fork", args))
        }
        Some(Commands::Init(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("init", args))
        }
        Some(Commands::Setup(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("setup", args))
        }
        Some(Commands::RemoteSetup(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, remote_setup_tui_args(args))
        }
        Some(Commands::Exec(args)) => {
            reject_exec_global_flags(&args.args)?;
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("exec", args))
        }
        Some(Commands::Fleet(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("fleet", args))
        }
        Some(Commands::WorkflowTool(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("workflow-tool", args))
        }
        Some(Commands::LaneLogProxy(_)) => unreachable!("lane log proxy dispatched above"),
        Some(Commands::Workflow(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            let config_path = store.path().to_path_buf();
            run_workflow_command(&cli, &resolved_runtime, &config_path, args)
        }
        Some(Commands::Lane(args)) => run_lane_command(args),
        Some(Commands::Review(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("review", args))
        }
        Some(Commands::Apply(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("apply", args))
        }
        Some(Commands::Eval(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("eval", args))
        }
        Some(Commands::Mcp(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("mcp", args))
        }
        Some(Commands::Features(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("features", args))
        }
        Some(Commands::Serve(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            // `serve` starts a long-running runtime API listener; supervise the
            // delegated child so it is torn down with the dispatcher (#3259).
            delegate_server_to_tui(&cli, &resolved_runtime, tui_args("serve", args))
        }
        Some(Commands::Web(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_server_to_tui(&cli, &resolved_runtime, web_serve_passthrough(&args))
        }
        Some(Commands::Completions(args)) => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            delegate_to_tui(&cli, &resolved_runtime, tui_args("completions", args))
        }
        Some(Commands::Login(args)) => run_login_command(&mut store, args),
        Some(Commands::Logout) => run_logout_command(&mut store),
        Some(Commands::Auth(args)) => match args.command {
            AuthCommand::XaiDevice => {
                let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
                delegate_to_tui(
                    &cli,
                    &resolved_runtime,
                    vec!["auth".to_string(), "xai-device".to_string()],
                )
            }
            command => run_auth_command(&mut store, command),
        },
        Some(Commands::McpServer) => run_mcp_server_command(&mut store),
        Some(Commands::Config(args)) => run_config_command(&mut store, args.command),
        Some(Commands::Model(args)) => {
            run_model_command(&mut store, args.command, runtime_overrides.provider)
        }
        Some(Commands::Thread(args)) => run_thread_command(args.command),
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
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "codewhale", &mut io::stdout());
            Ok(())
        }
        Some(Commands::Metrics(args)) => run_metrics_command(args),
        Some(Commands::Update(args)) => {
            #[cfg(not(target_env = "ohos"))]
            {
                update::run_update(args.beta, args.check, args.proxy)
            }
            #[cfg(target_env = "ohos")]
            {
                let _ = args;
                bail!("self-update is not supported on HarmonyOS/OpenHarmony yet");
            }
        }
        None => {
            let resolved_runtime = resolve_runtime_for_dispatch(&mut store, &runtime_overrides);
            let forwarded = root_tui_passthrough(&cli)?;
            delegate_to_tui(&cli, &resolved_runtime, forwarded)
        }
    }
}

fn root_tui_passthrough(cli: &Cli) -> Result<Vec<String>> {
    let mut forwarded = Vec::new();
    if cli.continue_session {
        forwarded.push("--continue".to_string());
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

fn run_login_command(store: &mut ConfigStore, args: LoginArgs) -> Result<()> {
    run_login_command_with_secrets(store, args, &Secrets::auto_detect())
}

fn run_login_command_with_secrets(
    store: &mut ConfigStore,
    args: LoginArgs,
    secrets: &Secrets,
) -> Result<()> {
    let provider: ProviderKind = args.provider.unwrap_or(ProviderArg::Deepseek).into();
    store.config.provider = provider;

    let api_key = match args.api_key {
        Some(v) => v,
        None => read_api_key_from_stdin()?,
    };
    let secret_store_saved = persist_provider_api_key(store, secrets, provider, &api_key)?;
    let destination = if secret_store_saved {
        secrets.backend_name().to_string()
    } else {
        store.path().display().to_string()
    };
    if provider == ProviderKind::Deepseek {
        println!("logged in using API key mode (deepseek); saved key to {destination}");
    } else {
        println!(
            "logged in using API key mode ({}); saved key to {destination}",
            provider.as_str(),
        );
    }
    Ok(())
}

fn run_logout_command(store: &mut ConfigStore) -> Result<()> {
    run_logout_command_with_secrets(store, &Secrets::auto_detect())
}

fn run_logout_command_with_secrets(store: &mut ConfigStore, secrets: &Secrets) -> Result<()> {
    let active_provider = store.config.provider;
    store.config.api_key = None;
    for provider in ProviderKind::ALL {
        clear_provider_api_key_from_config(store, provider);
    }
    clear_provider_api_key_from_keyring(secrets, active_provider);
    store.config.auth_mode = None;
    store.save()?;
    println!("logged out");
    Ok(())
}

/// Map [`ProviderKind`] to the canonical provider credential slot.
fn provider_slot(provider: ProviderKind) -> &'static str {
    match provider {
        // Keep the historical shared credential slot for the China endpoint.
        ProviderKind::SiliconflowCN => "siliconflow",
        _ => provider.provider().id(),
    }
}

#[cfg(test)]
fn no_keyring_secrets() -> Secrets {
    Secrets::new(std::sync::Arc::new(
        codewhale_secrets::InMemoryKeyringStore::new(),
    ))
}

fn write_provider_api_key_to_config(
    store: &mut ConfigStore,
    provider: ProviderKind,
    api_key: &str,
) {
    prepare_provider_api_key_metadata(store, provider);
    store.config.providers.for_provider_mut(provider).api_key = Some(api_key.to_string());
    if provider == ProviderKind::Deepseek {
        store.config.api_key = Some(api_key.to_string());
    }
}

fn prepare_provider_api_key_metadata(store: &mut ConfigStore, provider: ProviderKind) {
    store.config.auth_mode = Some("api_key".to_string());
    store.config.providers.for_provider_mut(provider).auth_mode = Some("api_key".to_string());
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

/// Persist a provider credential to the durable secret store first. A
/// plaintext config slot is used only when that write fails.
fn persist_provider_api_key(
    store: &mut ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
    api_key: &str,
) -> Result<bool> {
    prepare_provider_api_key_metadata(store, provider);
    let secret_store_saved = match secrets.set(provider_slot(provider), api_key) {
        Ok(()) => {
            clear_provider_api_key_from_config(store, provider);
            true
        }
        Err(err) => {
            eprintln!(
                "warning: secret-store write failed for {}; using owner-only config fallback: {err}",
                provider_slot(provider)
            );
            write_provider_api_key_to_config(store, provider, api_key);
            false
        }
    };
    store.save()?;
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(store.path())?;
    Ok(secret_store_saved)
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
            return path;
        }
    }

    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    codex_home.join("auth.json")
}

fn provider_oauth_file_path(provider: ProviderKind) -> Option<PathBuf> {
    (provider == ProviderKind::OpenaiCodex).then(openai_codex_auth_file_path)
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
    slot.or(root).filter(|v| !v.trim().is_empty())
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

fn auth_status_all_providers(store: &ConfigStore, secrets: &Secrets) -> Vec<String> {
    let active_provider = store.config.provider;
    let mut lines = Vec::new();
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
        let config_key = provider_config_api_key(store, provider);
        let keyring_key = provider_keyring_api_key(secrets, provider);
        let env_key = provider_env_value(provider);
        let oauth_file_present = provider_oauth_file_path(provider).is_some_and(|p| p.exists());

        let config_status = config_key.map(|_| "set").unwrap_or("-");
        let keyring_status = keyring_key.as_ref().map(|_| "set").unwrap_or("-");
        let env_status = env_key.as_ref().map(|_| "set").unwrap_or("-");

        let source = if provider == ProviderKind::OpenaiCodex {
            // Keep the summary consistent with `auth status`: Codex auth is
            // OAuth-file (or env token) based — config/keyring keys are not
            // consulted for it.
            if env_key.is_some() {
                "env"
            } else if oauth_file_present {
                "oauth file"
            } else {
                "unset"
            }
        } else if config_key.is_some() {
            "config"
        } else if keyring_key.is_some() {
            "keyring"
        } else if env_key.is_some() {
            "env"
        } else if oauth_file_present {
            "oauth file"
        } else {
            "unset"
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
    lines
}

fn auth_list_lines(store: &ConfigStore, secrets: &Secrets) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("provider     config store env  active".to_string());
    for provider in ProviderKind::ALL {
        let slot = provider_slot(provider);
        let file = provider_config_set(store, provider);
        let keyring = (!file).then(|| provider_keyring_set(secrets, provider));
        let env = provider_env_set(provider);
        let oauth_file = provider_oauth_file_path(provider).is_some_and(|p| p.exists());
        let active = if provider == ProviderKind::OpenaiCodex {
            if env {
                "env"
            } else if oauth_file {
                "oauth"
            } else {
                "missing"
            }
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
            "{slot:<12}  {}     {}      {}   {active}",
            yes_no(file),
            keyring_status_short(keyring),
            yes_no(env)
        ));
    }
    lines
}

fn auth_status_lines_for_provider(
    store: &ConfigStore,
    secrets: &Secrets,
    provider: ProviderKind,
) -> Vec<String> {
    let config_key = provider_config_api_key(store, provider);
    let keyring_key = provider_keyring_api_key(secrets, provider);
    let env_key = provider_env_value(provider);
    let oauth_file = provider_oauth_file_path(provider);
    let oauth_file_present = oauth_file.as_ref().is_some_and(|path| path.exists());

    let active_source = if provider == ProviderKind::OpenaiCodex {
        if env_key.is_some() {
            "env"
        } else if oauth_file_present {
            "Codex OAuth file"
        } else {
            "missing"
        }
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
    let active_label = active_last4
        .map(|last4| format!("{active_source} (last4: {last4})"))
        .unwrap_or_else(|| active_source.to_string());

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
        "lookup order: env -> Codex OAuth file".to_string()
    } else {
        "lookup order: config -> secret store -> env".to_string()
    };
    let auth_mode = if provider == ProviderKind::OpenaiCodex {
        "codex_oauth"
    } else {
        store.config.auth_mode.as_deref().unwrap_or("api_key")
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
            store.path().display(),
            source_status(config_key, "missing")
        ),
        format!(
            "secret store: {} ({})",
            secrets.backend_name(),
            source_status(keyring_key.as_deref(), "missing")
        ),
        format!("env var: {env_var_label} ({env_status})"),
    ];
    if let Some(path) = oauth_file {
        let status = if path.exists() { "present" } else { "missing" };
        lines.push(format!("Codex OAuth file: {} ({status})", path.display()));
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

fn run_auth_command(store: &mut ConfigStore, command: AuthCommand) -> Result<()> {
    run_auth_command_with_secrets(store, command, &Secrets::auto_detect())
}

fn run_auth_command_with_secrets(
    store: &mut ConfigStore,
    command: AuthCommand,
    secrets: &Secrets,
) -> Result<()> {
    match command {
        AuthCommand::XaiDevice => {
            bail!("xAI device authentication must be delegated to codewhale-tui")
        }
        AuthCommand::Status { provider } => {
            match provider {
                Some(p) => {
                    let provider: ProviderKind = p.into();
                    for line in auth_status_lines_for_provider(store, secrets, provider) {
                        println!("{line}");
                    }
                }
                None => {
                    for line in auth_status_all_providers(store, secrets) {
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
            let provider: ProviderKind = provider.into();
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
            let provider: ProviderKind = provider.into();
            let slot = provider_slot(provider);
            let in_file = provider_config_set(store, provider);
            let in_keyring = !in_file && provider_keyring_set(secrets, provider);
            let in_env = provider_env_set(provider);
            // Report the highest-priority source that has it.
            let source = if in_file {
                Some("config-file")
            } else if in_keyring {
                Some("secret-store")
            } else if in_env {
                Some("env")
            } else {
                None
            };
            match source {
                Some(source) => println!("{slot}: set (source: {source})"),
                None => println!("{slot}: not set"),
            }
            Ok(())
        }
        AuthCommand::Clear { provider } => {
            let provider: ProviderKind = provider.into();
            let slot = provider_slot(provider);
            clear_provider_api_key_from_config(store, provider);
            clear_provider_api_key_from_keyring(secrets, provider);
            store.save()?;
            println!("cleared API key for {slot} from config and secret store");
            Ok(())
        }
        AuthCommand::List => {
            for line in auth_list_lines(store, secrets) {
                println!("{line}");
            }
            Ok(())
        }
        AuthCommand::Migrate { dry_run } => run_auth_migrate(store, secrets, dry_run),
    }
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

    for provider in ProviderKind::ALL {
        let slot = provider_slot(provider);
        let from_provider_block = store
            .config
            .providers
            .for_provider(provider)
            .api_key
            .clone()
            .filter(|v| !v.trim().is_empty());
        let from_root = (provider == ProviderKind::Deepseek)
            .then(|| store.config.api_key.clone())
            .flatten()
            .filter(|v| !v.trim().is_empty());
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

fn run_config_command(store: &mut ConfigStore, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Get { key } => {
            if let Some(value) = store.config.get_display_value(&key) {
                println!("{value}");
                return Ok(());
            }
            bail!("key not found: {key}");
        }
        ConfigCommand::Set { key, value } => {
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
            for (key, value) in store.config.list_values() {
                println!("{key} = {value}");
            }
            Ok(())
        }
        ConfigCommand::Path => {
            println!("{}", store.path().display());
            Ok(())
        }
    }
}

fn model_command_provider_hint(
    command_provider: Option<ProviderArg>,
    top_level_provider: Option<ProviderKind>,
) -> Option<ProviderKind> {
    command_provider
        .map(ProviderKind::from)
        .or(top_level_provider)
}

fn run_model_command(
    store: &mut ConfigStore,
    command: ModelCommand,
    top_level_provider: Option<ProviderKind>,
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
            let provider = model_command_provider_hint(provider, top_level_provider);
            let resolved = registry.resolve(model.as_deref(), provider);
            println!("requested: {}", resolved.requested.unwrap_or_default());
            println!("resolved: {}", resolved.resolved.id);
            println!("provider: {}", resolved.resolved.provider.as_str());
            println!("used_fallback: {}", resolved.used_fallback);
            Ok(())
        }
        ModelCommand::Set { model } => {
            let trimmed = model.trim();
            if trimmed.is_empty() {
                bail!("Model name cannot be empty");
            }
            let canonical = match trimmed.to_ascii_lowercase().as_str() {
                "pro" | "deepseek-v4pro" => "deepseek-v4-pro",
                "flash" | "deepseek-v4flash" => "deepseek-v4-flash",
                _ => trimmed,
            };
            store.config.default_text_model = Some(canonical.to_string());
            store.save()?;
            println!("Default model set to '{canonical}'");
            Ok(())
        }
    }
}

fn run_thread_command(command: ThreadCommand) -> Result<()> {
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
        ThreadCommand::Resume { thread_id } => {
            let args = vec!["resume".to_string(), thread_id];
            delegate_simple_tui(args)
        }
        ThreadCommand::Fork { thread_id } => {
            let args = vec!["fork".to_string(), thread_id];
            delegate_simple_tui(args)
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
        return delegate_server_to_tui(cli, resolved_runtime, app_server_serve_passthrough(&args));
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    if args.stdio {
        return runtime.block_on(run_app_server_stdio(args.config));
    }
    // Legacy in-process app-server HTTP transport (`/healthz`, `/thread`, `/app`,
    // `/prompt`, `/tool`, `/jobs`). Kept for backward compatibility; defaults to
    // 127.0.0.1:8787 to avoid colliding with the runtime API default of :7878.
    let host = args.host.as_deref().unwrap_or("127.0.0.1");
    let port = args.port.unwrap_or(8787);
    let listen: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid app-server listen address {host}:{port}"))?;
    runtime.block_on(run_app_server(AppServerOptions {
        listen,
        config_path: args.config,
        auth_token: args.auth_token.or_else(app_server_token_from_env),
        insecure_no_auth: args.insecure_no_auth,
        cors_origins: args.cors_origin,
    }))
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
    let Some(raw) = store.config.get_value(MCP_SERVER_DEFINITIONS_KEY) else {
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

    let unwrapped: String = serde_json::from_str(raw)
        .with_context(|| format!("invalid JSON payload at key {MCP_SERVER_DEFINITIONS_KEY}"))?;
    serde_json::from_str::<Vec<McpServerDefinition>>(&unwrapped).with_context(|| {
        format!("invalid MCP server definition list in key {MCP_SERVER_DEFINITIONS_KEY}")
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

fn delegate_to_tui(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
) -> Result<()> {
    let mut cmd = build_tui_command(cli, resolved_runtime, passthrough)?;
    let tui = PathBuf::from(cmd.get_program());
    let status = cmd
        .status()
        .map_err(|err| anyhow!("{}", tui_spawn_error(&tui, &err)))?;
    exit_with_tui_status(status)
}

/// Delegate a long-running server command (`serve --http`/`--mobile`,
/// `app-server --http`/`--mobile`) to the sibling TUI binary, supervising the
/// child so its listener does not outlive the dispatcher (#3259).
///
/// Plain [`delegate_to_tui`] blocks on `Command::status()`, which reaps the
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
fn delegate_server_to_tui(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
) -> Result<()> {
    let mut std_cmd = build_tui_command(cli, resolved_runtime, passthrough)?;
    install_server_parent_death_signal(&mut std_cmd);
    let tui = PathBuf::from(std_cmd.get_program());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create server-teardown runtime")?;
    runtime.block_on(async move {
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|err| anyhow!("{}", tui_spawn_error(&tui, &err)))?;
        // Windows: hold a kill-on-job-close Job Object for the dispatcher's
        // lifetime so an uncatchable dispatcher death tears the child down.
        // Bound for the whole `block_on` scope; never dropped early because the
        // match arms below `std::process::exit`.
        #[cfg(windows)]
        let _child_job = attach_server_child_job(&child);
        match supervise_server_child(&mut child, server_shutdown_signal()).await? {
            ServerTeardown::Exited(status) => exit_with_tui_status(status),
            // The child has been killed and reaped; exit with the conventional
            // 128 + signal code for the signal that initiated the shutdown.
            ServerTeardown::Signaled(code) => std::process::exit(code),
        }
    })
}

/// On Linux, ask the kernel to terminate the delegated server if the dispatcher
/// dies before it can run the graceful shutdown supervisor. This covers the
/// hard parent-death edge of #3259 for `SIGKILL`, OOM, or abrupt process exit.
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn install_server_parent_death_signal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the child between fork and exec. The closure
    // only calls `libc::prctl` with constant arguments and does not touch heap
    // memory or parent-held locks.
    unsafe {
        cmd.pre_exec(|| {
            let result = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
            if result == -1 {
                // Best effort: the child only loses this OS-level safety net.
                let _ = std::io::Error::last_os_error();
            }
            Ok(())
        });
    }
}

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
fn install_server_parent_death_signal(_cmd: &mut Command) {}

/// Outcome of supervising a delegated server child.
#[derive(Debug)]
enum ServerTeardown {
    /// The child exited on its own; its status is carried for propagation.
    Exited(std::process::ExitStatus),
    /// A shutdown signal fired; the child was killed and reaped. Carries the
    /// conventional `128 + signal` exit code to propagate.
    Signaled(i32),
}

/// Wait for the server `child` to exit, or for `shutdown` to fire first. On
/// shutdown, kill the child and reap it so no listener is left reparented.
async fn supervise_server_child<F>(
    child: &mut tokio::process::Child,
    shutdown: F,
) -> io::Result<ServerTeardown>
where
    F: std::future::Future<Output = i32>,
{
    tokio::select! {
        status = child.wait() => Ok(ServerTeardown::Exited(status?)),
        code = shutdown => {
            // Send the kill, then wait so the PID is reaped before the
            // dispatcher returns and exits.
            let _ = child.start_kill();
            let _ = child.wait().await;
            Ok(ServerTeardown::Signaled(code))
        }
    }
}

/// Resolve when the dispatcher should tear down a delegated server child, and
/// the conventional `128 + signal` exit code to propagate: Ctrl+C on every
/// platform (130), plus SIGTERM (143) and SIGHUP (129) on Unix.
#[cfg(unix)]
async fn server_shutdown_signal() -> i32 {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate()).ok();
    let mut hangup = signal(SignalKind::hangup()).ok();
    let term = async {
        match terminate.as_mut() {
            Some(s) => {
                s.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let hup = async {
        match hangup.as_mut() {
            Some(s) => {
                s.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => 130,
        _ = term => 143,
        _ = hup => 129,
    }
}

#[cfg(not(unix))]
async fn server_shutdown_signal() -> i32 {
    let _ = tokio::signal::ctrl_c().await;
    130
}

/// Assign the delegated server `child` to a kill-on-job-close Job Object so the
/// OS terminates it when the dispatcher's handle to the job closes — which it
/// does on any dispatcher exit, including an uncatchable kill (#3259). The
/// returned guard must be held for the dispatcher's lifetime. Best-effort:
/// returns `None` if the job cannot be created or assigned. Mirrors the Job
/// Object idiom in `crates/tui/src/tools/shell.rs`.
#[cfg(windows)]
fn attach_server_child_job(child: &tokio::process::Child) -> Option<ServerChildJob> {
    let Some(child_handle) = child.raw_handle() else {
        tracing::warn!("delegated server child exited before a job object could be attached");
        return None;
    };

    match ServerChildJob::attach(child_handle) {
        Ok(job) => Some(job),
        Err(err) => {
            tracing::warn!("failed to place delegated server child in a job object: {err}");
            None
        }
    }
}

#[cfg(windows)]
struct ServerChildJob {
    handle: windows::Win32::Foundation::HANDLE,
}

// SAFETY: the wrapped value is a process-wide kernel handle; moving it across
// threads does not invalidate it, and it is only ever closed once, on drop.
#[cfg(windows)]
unsafe impl Send for ServerChildJob {}

#[cfg(windows)]
impl ServerChildJob {
    fn attach(child_handle: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows::core::PCWSTR;

        // SAFETY: FFI calls with valid arguments; results are checked via the
        // `windows` Result wrappers and the handle is stored for close-on-drop.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(win_io_error)?;
        let job = Self { handle };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(win_io_error)?;
            AssignProcessToJobObject(job.handle, HANDLE(child_handle)).map_err(win_io_error)?;
        }
        Ok(job)
    }
}

#[cfg(windows)]
impl Drop for ServerChildJob {
    fn drop(&mut self) {
        // Closing the last handle triggers KILL_ON_JOB_CLOSE. On a normal return
        // the child has already been reaped, so this is a no-op cleanup; an
        // uncatchable dispatcher death closes the handle via the OS instead.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn win_io_error(err: windows::core::Error) -> std::io::Error {
    std::io::Error::other(err)
}

#[cfg(all(test, unix))]
mod server_teardown_tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_propagates_child_exit_when_no_shutdown() {
        // `true` exits immediately with success; a never-firing shutdown must
        // let the child's own exit win.
        let mut child = tokio::process::Command::new("true")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn true");
        let outcome = supervise_server_child(&mut child, std::future::pending::<i32>())
            .await
            .expect("supervise");
        match outcome {
            ServerTeardown::Exited(status) => assert!(status.success()),
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_signal_kills_and_reaps_long_running_child() {
        // A long-lived child stands in for the delegated server listener; the
        // regression is that it outlives dispatcher teardown (#3259).
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sleep");
        assert!(
            child.id().is_some(),
            "child should be running before shutdown"
        );
        // A ready future models an immediate shutdown signal carrying the
        // SIGTERM exit code (143).
        let outcome = supervise_server_child(&mut child, async { 143 })
            .await
            .expect("supervise");
        assert!(matches!(outcome, ServerTeardown::Signaled(143)));
        // Once supervise returns the child has been killed AND reaped, so tokio
        // drops the recorded pid — no listener is left reparented.
        assert!(
            child.id().is_none(),
            "delegated child must be reaped after dispatcher teardown"
        );
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[test]
    fn parent_death_signal_hook_does_not_break_spawn() {
        let mut cmd = Command::new("true");
        install_server_parent_death_signal(&mut cmd);
        let status = cmd.status().expect("spawn true with parent-death hook");
        assert!(status.success());
    }
}

fn run_resume_command(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    args: TuiPassthroughArgs,
) -> Result<()> {
    let passthrough = tui_args("resume", args);
    if should_pick_resume_in_dispatcher(&passthrough, cfg!(windows)) {
        return run_dispatcher_resume_picker(cli, resolved_runtime);
    }
    delegate_to_tui(cli, resolved_runtime, passthrough)
}

fn run_dispatcher_resume_picker(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
) -> Result<()> {
    let mut sessions_cmd = build_tui_command(cli, resolved_runtime, vec!["sessions".to_string()])?;
    let tui = PathBuf::from(sessions_cmd.get_program());
    let status = sessions_cmd
        .status()
        .map_err(|err| anyhow!("{}", tui_spawn_error(&tui, &err)))?;
    if !status.success() {
        return exit_with_tui_status(status);
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

    delegate_to_tui(
        cli,
        resolved_runtime,
        vec!["resume".to_string(), session_id.to_string()],
    )
}

fn should_pick_resume_in_dispatcher(passthrough: &[String], is_windows: bool) -> bool {
    is_windows && passthrough == ["resume"]
}

fn build_tui_command(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
) -> Result<Command> {
    build_tui_command_with_paths(
        cli,
        resolved_runtime,
        passthrough,
        cli.config.as_deref(),
        cli.workspace.as_deref(),
    )
}

fn build_tui_command_with_paths(
    cli: &Cli,
    resolved_runtime: &ResolvedRuntimeOptions,
    passthrough: Vec<String>,
    config_path: Option<&Path>,
    workspace_path: Option<&Path>,
) -> Result<Command> {
    let tui = locate_sibling_tui_binary()?;
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

    let mut cmd = Command::new(&tui);
    if let Some(config) = config_path {
        cmd.arg("--config").arg(config);
    }
    if let Some(profile) = cli.profile.as_ref() {
        cmd.arg("--profile").arg(profile);
    }
    if let Some(workspace) = workspace_path {
        cmd.arg("--workspace").arg(workspace);
    }
    // Accepted for older scripts, but no longer forwarded: the interactive TUI
    // always owns the alternate screen to avoid host scrollback hijacking.
    let _ = cli.no_alt_screen;
    if cli.mouse_capture {
        cmd.arg("--mouse-capture");
    }
    if cli.no_mouse_capture {
        cmd.arg("--no-mouse-capture");
    }
    if cli.skip_onboarding {
        cmd.arg("--skip-onboarding");
    }
    cmd.args(passthrough);

    let uses_raw_tui_provider = cli
        .provider
        .as_deref()
        .is_some_and(|provider| builtin_provider_arg(provider).is_none());
    let keyring_bridge_provider = resolved_runtime.provider;
    let keyring_bridge_api_key = resolved_runtime.api_key.as_ref();
    let keyring_bridge_source = resolved_runtime.api_key_source;

    if let Some(provider) = cli.provider.as_deref() {
        let provider = builtin_provider_arg(provider)
            .map(ProviderKind::from)
            .map_or_else(
                || provider.to_string(),
                |provider| provider.as_str().to_string(),
            );
        // Set both names so an inherited CODEWHALE_PROVIDER cannot outrank the
        // explicit CLI pin when the TUI applies its environment overrides.
        cmd.env("CODEWHALE_PROVIDER", &provider);
        cmd.env("DEEPSEEK_PROVIDER", provider);
    }
    if !(uses_raw_tui_provider
        || (cli.profile.is_some()
            && matches!(resolved_runtime.provider_source, ProviderSource::Config)))
        && matches!(keyring_bridge_source, Some(RuntimeApiKeySource::Keyring))
        && let Some(api_key) = keyring_bridge_api_key
    {
        // TUI reloads auth_mode from config/profile, but it does not re-query the
        // platform keyring on normal startup. Bridge only the recovered secret;
        // replaying auth_mode here would turn it back into a profile override.
        cmd.env("DEEPSEEK_API_KEY", api_key);
        for var in provider_env_vars(keyring_bridge_provider) {
            if *var != "DEEPSEEK_API_KEY" {
                cmd.env(var, api_key);
            }
        }
        cmd.env(
            "DEEPSEEK_API_KEY_SOURCE",
            RuntimeApiKeySource::Keyring.as_env_value(),
        );
    }

    if let Some(model) = cli.model.as_ref() {
        cmd.env("DEEPSEEK_MODEL", model);
    }
    if let Some(output_mode) = cli.output_mode.as_ref() {
        cmd.env("DEEPSEEK_OUTPUT_MODE", output_mode);
    }
    if let Some(v) = verbosity.as_ref() {
        cmd.env("CODEWHALE_VERBOSITY", v);
        cmd.env("DEEPSEEK_VERBOSITY", v);
    }
    if let Some(log_level) = cli.log_level.as_ref() {
        cmd.env("DEEPSEEK_LOG_LEVEL", log_level);
    }
    if let Some(telemetry) = cli.telemetry {
        cmd.env("DEEPSEEK_TELEMETRY", telemetry.to_string());
    }
    if let Some(policy) = cli.approval_policy.as_ref() {
        cmd.env("DEEPSEEK_APPROVAL_POLICY", policy);
    }
    if let Some(mode) = cli.sandbox_mode.as_ref() {
        cmd.env("DEEPSEEK_SANDBOX_MODE", mode);
    }
    if cli.yolo {
        cmd.env("DEEPSEEK_YOLO", "true");
    }
    if let Some(api_key) = cli.api_key.as_ref() {
        // `--profile` is resolved by the TUI after this facade starts it, so
        // the base ConfigStore provider may not be the effective provider.
        // Carry the explicit secret through a provider-neutral, source-marked
        // slot; the TUI applies it after profile/OAuth resolution and before
        // saved API-key slots. Preserve legacy provider envs only when their
        // identity is already unambiguous here.
        cmd.env("CODEWHALE_CLI_API_KEY", api_key);
        if !uses_raw_tui_provider && (cli.profile.is_none() || cli.provider.is_some()) {
            cmd.env("DEEPSEEK_API_KEY", api_key);
            for var in provider_env_vars(resolved_runtime.provider) {
                if *var != "DEEPSEEK_API_KEY" {
                    cmd.env(var, api_key);
                }
            }
        }
        cmd.env("DEEPSEEK_API_KEY_SOURCE", "cli");
    }
    if let Some(base_url) = cli.base_url.as_ref() {
        cmd.env("DEEPSEEK_BASE_URL", base_url);
    }

    Ok(cmd)
}

fn tui_child_exit_code(status: std::process::ExitStatus) -> Option<i32> {
    if let Some(code) = status.code() {
        return Some(code);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        status.signal().map(|signal| 128 + signal)
    }

    #[cfg(not(unix))]
    {
        None
    }
}

fn exit_with_tui_status(status: std::process::ExitStatus) -> Result<()> {
    if let Some(code) = tui_child_exit_code(status) {
        std::process::exit(code);
    }
    bail!("codewhale-tui terminated without an exit code")
}

fn delegate_simple_tui(args: Vec<String>) -> Result<()> {
    let tui = locate_sibling_tui_binary()?;
    let status = Command::new(&tui)
        .args(args)
        .status()
        .map_err(|err| anyhow!("{}", tui_spawn_error(&tui, &err)))?;
    exit_with_tui_status(status)
}

fn tui_spawn_error(tui: &Path, err: &io::Error) -> String {
    format!(
        "failed to spawn companion TUI binary at {}: {err}\n\
\n\
The `codewhale` dispatcher found a `codewhale-tui` file, but the OS refused \
to execute it. Common fixes:\n\
  - Reinstall with `npm install -g codewhale`, or run `codewhale update`.\n\
  - On Windows, run `where codewhale` and `where codewhale-tui`; both should \
come from the same install directory.\n\
  - If you downloaded release assets manually, keep both `codewhale` and \
`codewhale-tui` binaries together and make sure the TUI binary is executable.\n\
  - Set DEEPSEEK_TUI_BIN to the absolute path of a working `codewhale-tui` \
binary.",
        tui.display()
    )
}

/// Resolve the sibling `codewhale-tui` executable next to the running
/// dispatcher. Honours platform executable suffix (`.exe` on Windows) so
/// the npm-distributed Windows package — which ships
/// `bin/downloads/codewhale-tui.exe` — is found by `Path::exists` (#247).
///
/// `DEEPSEEK_TUI_BIN` is consulted first as an explicit override for
/// custom installs and CI test layouts. On Windows we additionally try
/// the suffix-less name as a fallback for users who already manually
/// renamed the file before this fix landed.
fn locate_sibling_tui_binary() -> Result<PathBuf> {
    if let Ok(override_path) = std::env::var("DEEPSEEK_TUI_BIN") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Ok(candidate);
        }
        bail!(
            "DEEPSEEK_TUI_BIN points at {}, which is not a regular file.",
            candidate.display()
        );
    }

    let current = std::env::current_exe().context("failed to locate current executable path")?;
    if let Some(found) = sibling_tui_candidate(&current) {
        return Ok(found);
    }

    // Build a stable error path so the user sees the platform-correct
    // expected name, not "codewhale-tui" on Windows.
    let expected = current.with_file_name(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    bail!(
        "Companion `codewhale-tui` binary not found at {}.\n\
\n\
The `codewhale` dispatcher delegates interactive sessions to a sibling \
`codewhale-tui` binary. To fix this, install one of:\n\
  • npm:    npm install -g codewhale                (downloads both binaries)\n\
  • cargo:  cargo install codewhale-cli codewhale-tui --locked\n\
  • GitHub Releases: download BOTH `codewhale-<platform>` AND \
`codewhale-tui-<platform>` from https://github.com/Hmbown/CodeWhale/releases/latest \
and place them in the same directory.\n\
\n\
Or set DEEPSEEK_TUI_BIN to the absolute path of an existing `codewhale-tui` binary.",
        expected.display()
    );
}

/// Return the first existing sibling-binary path under any of the names
/// `codewhale-tui` might use on this platform. Pure function to keep
/// `locate_sibling_tui_binary` testable.
fn sibling_tui_candidate(dispatcher: &Path) -> Option<PathBuf> {
    // Primary: platform-correct name. EXE_SUFFIX is "" on Unix and ".exe"
    // on Windows.
    let primary =
        dispatcher.with_file_name(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
    if primary.is_file() {
        return Some(primary);
    }
    // Windows fallback: a user who manually renamed `.exe` away (per the
    // workaround in #247) still launches successfully under the new code.
    if cfg!(windows) {
        let suffixless = dispatcher.with_file_name("codewhale-tui");
        if suffixless.is_file() {
            return Some(suffixless);
        }
    }
    None
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
    use codewhale_config::ProviderSource;
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

    fn command_env(cmd: &Command, name: &str) -> Option<String> {
        let name = std::ffi::OsStr::new(name);
        cmd.get_envs().find_map(|(key, value)| {
            if key == name {
                value.map(|v| v.to_string_lossy().into_owned())
            } else {
                None
            }
        })
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    struct ScopedEnvVar {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            // Safety: tests using this helper serialize with env_lock() and
            // restore the original value in Drop.
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }

        fn remove(name: &'static str) -> Self {
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
                    std::env::set_var(self.name, previous);
                } else {
                    std::env::remove_var(self.name);
                }
            }
        }
    }

    fn install_fake_tui_binary() -> (tempfile::TempDir, ScopedEnvVar) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
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
            api_key: None,
            api_key_source: None,
            base_url: "http://localhost:8000/v1".to_string(),
            auth_mode: None,
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        }
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
                    provider: Some(ProviderArg::Openai)
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
                    provider: Some(ProviderArg::Deepseek)
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
            model_command_provider_hint(Some(ProviderArg::Minimax), Some(ProviderKind::Zai)),
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

        for argv in [
            ["deepseek", "app-server", "--http", "--mobile"].as_slice(),
            ["deepseek", "app-server", "--http", "--stdio"].as_slice(),
            ["deepseek", "app-server", "--mobile", "--stdio"].as_slice(),
        ] {
            let err = Cli::try_parse_from(argv).expect_err("conflicting transports must fail");
            assert_eq!(err.kind(), ErrorKind::ArgumentConflict, "argv={argv:?}");
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
                && fleet == "stopship"
                && runtime == "tmux"
                && issue.as_deref() == Some("4375")
        ));
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

        for (provider, command) in [
            ("qianfan", vec!["exec", "Reply OK"]),
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
            assert_eq!(builtin_provider_arg(alias), Some(ProviderArg::OpencodeGo));
        }
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
            assert_eq!(builtin_provider_arg(alias), Some(ProviderArg::OpencodeZen));
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
        assert_eq!(err.kind(), ErrorKind::InvalidValue);

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
    fn raw_provider_dispatch_defers_dynamic_config_to_the_tui() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"provider = "lm-studio"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
model = "qwen-2.5-7b"
"#,
        )
        .expect("custom provider config fixture");
        assert!(
            ConfigStore::load(Some(config_path.clone())).is_err(),
            "the enum-backed dispatcher store must not be the owner of dynamic provider config"
        );

        let config = config_path.to_string_lossy().into_owned();
        let cli = parse_ok(&[
            "codewhale",
            "--config",
            &config,
            "--provider",
            "lm-studio",
            "exec",
            "Reply OK",
        ]);
        let prepared = prepare_raw_provider_tui_dispatch(
            &cli,
            cli.command.as_ref(),
            &CliRuntimeOverrides::default(),
        )
        .expect("prepare raw provider dispatch")
        .expect("Exec with a raw provider should bypass dispatcher config resolution");
        assert_eq!(prepared.1, ["exec", "Reply OK"].map(str::to_string));
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

    #[test]
    fn short_workflow_names_do_not_resolve_historical_v0868_files() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let candidates = workflow_source_candidates("issue-sweep", None, &workspace);
        assert!(candidates.iter().all(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("v0868_"))
        }));
        assert!(resolve_workflow_source_path("issue-sweep", None, &workspace).is_err());

        let historical = resolve_workflow_source_path(
            "workflows/v0868_issue_sweep.workflow.js",
            None,
            &workspace,
        )
        .expect("explicit historical workflow path");
        assert!(historical.ends_with("workflows/v0868_issue_sweep.workflow.js"));
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
            fleet: "stopship",
            issue: Some("4375"),
            goal: Some("fix stopship"),
            token_budget: Some(25_000),
            verify: true,
        })
        .expect("command");
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
    fn deepseek_login_uses_isolated_file_store_and_preserves_tui_defaults() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let codewhale_home = dir.path().join("codewhale-home");
        let codewhale_home_value = codewhale_home.to_string_lossy().into_owned();
        let _home = ScopedEnvVar::set("CODEWHALE_HOME", &codewhale_home_value);
        let _backend = ScopedEnvVar::set("CODEWHALE_SECRET_BACKEND", "file");
        let path = codewhale_home.join("config.toml");
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        let secrets = Secrets::auto_detect();

        run_login_command_with_secrets(
            &mut store,
            LoginArgs {
                provider: Some(ProviderArg::Deepseek),
                api_key: Some("sk-test".to_string()),
            },
            &secrets,
        )
        .expect("login should persist credential");

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
                .any(|line| line.trim_start().starts_with("api_key ="))
        );
        assert!(saved.contains("default_text_model = \"deepseek-v4-pro\""));
        assert_eq!(
            secrets.get("deepseek").expect("read secret").as_deref(),
            Some("sk-test")
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

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "deepseek"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderArg::Deepseek,
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
                    provider: ProviderArg::Openrouter,
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
                    provider: ProviderArg::Novita
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "clear", "--provider", "nvidia-nim"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Clear {
                    provider: ProviderArg::NvidiaNim
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "fireworks"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderArg::Fireworks,
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
                    provider: ProviderArg::Siliconflow,
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
                    provider: ProviderArg::Arcee,
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
                    provider: ProviderArg::Moonshot,
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
                    provider: ProviderArg::WanjieArk,
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
                    provider: ProviderArg::Sglang
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "get", "--provider", "vllm"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Get {
                    provider: ProviderArg::Vllm
                }
            }))
        ));

        let cli = parse_ok(&["deepseek", "auth", "set", "--provider", "ollama"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Auth(AuthArgs {
                command: AuthCommand::Set {
                    provider: ProviderArg::Ollama,
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
                    provider: Some(ProviderArg::OpenaiCodex)
                }
            }))
        ));

        for (provider, expected) in [
            ("anthropic", ProviderArg::Anthropic),
            ("openmodel", ProviderArg::Openmodel),
            ("open-model", ProviderArg::Openmodel),
            ("zai", ProviderArg::Zai),
            ("stepfun", ProviderArg::Stepfun),
            ("minimax", ProviderArg::Minimax),
            ("minimax-anthropic", ProviderArg::MinimaxAnthropic),
            ("minimax_anthropic", ProviderArg::MinimaxAnthropic),
            ("deepinfra", ProviderArg::Deepinfra),
            ("deep-infra", ProviderArg::Deepinfra),
            ("siliconflow-cn", ProviderArg::SiliconflowCn),
            ("siliconflow-CN", ProviderArg::SiliconflowCn),
            ("siliconflow_china", ProviderArg::SiliconflowCn),
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
                provider: ProviderArg::Deepseek,
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
    fn auth_set_uses_plaintext_config_only_when_secret_store_write_fails() {
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

        run_auth_command_with_secrets(
            &mut store,
            AuthCommand::Set {
                provider: ProviderArg::Openrouter,
                api_key: Some("fallback-test-credential".to_string()),
                api_key_stdin: false,
            },
            &secrets,
        )
        .expect("config fallback");

        assert_eq!(
            store.config.providers.openrouter.api_key.as_deref(),
            Some("fallback-test-credential")
        );
        let saved = std::fs::read_to_string(path).expect("config fallback file");
        assert!(saved.contains("fallback-test-credential"));
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
                provider: ProviderArg::Arcee,
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
                provider: ProviderArg::Ollama,
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
                provider: ProviderArg::Deepseek,
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
                provider: Some(ProviderArg::Deepseek),
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
    fn auth_status_openai_codex_reports_codex_oauth_file() {
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
        assert!(output.contains("active source: Codex OAuth file"));
        assert!(output.contains("lookup order: env -> Codex OAuth file"));
        assert!(output.contains(&format!(
            "Codex OAuth file: {} (present)",
            auth_path.display()
        )));
        assert!(!output.contains("secret-token"));
    }

    #[test]
    fn auth_list_treats_openai_codex_oauth_file_as_active() {
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

        let output = auth_list_lines(&store, &secrets).join("\n");
        let row = output
            .lines()
            .find(|line| line.starts_with("openai-codex"))
            .unwrap_or_else(|| panic!("missing openai-codex row:\n{output}"));
        assert!(row.ends_with("oauth"), "{row}");
        assert!(!output.contains("secret-token"));
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
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "deepseek-cli-logout-test-{}-{nanos}.toml",
            std::process::id()
        ));
        let mut store = ConfigStore::load(Some(path.clone())).expect("store should load");
        store.config.api_key = Some("sk-stale".to_string());
        store.config.providers.deepseek.api_key = Some("sk-stale".to_string());
        store.config.providers.fireworks.api_key = Some("fw-stale".to_string());
        store.save().unwrap();

        let secrets = no_keyring_secrets();

        run_logout_command_with_secrets(&mut store, &secrets).expect("logout should succeed");

        assert!(store.config.api_key.is_none());
        assert!(store.config.providers.deepseek.api_key.is_none());
        assert!(store.config.providers.fireworks.api_key.is_none());

        let _ = std::fs::remove_file(path);
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
            "--no-alt-screen",
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
        assert!(cli.no_alt_screen);
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
        assert_eq!(registry_kinds, ProviderKind::ALL);

        for provider in ProviderKind::ALL {
            assert_eq!(provider_env_vars(provider), provider.provider().env_vars());
            if provider == ProviderKind::SiliconflowCN {
                assert_eq!(
                    provider_slot(provider),
                    provider_slot(ProviderKind::Siliconflow)
                );
            } else {
                assert_eq!(provider_slot(provider), provider.provider().id());
            }
        }
    }

    #[test]
    fn build_tui_command_forwards_raw_exec_and_fleet_provider_without_secret_bridge() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();
        let _ambient_provider = ScopedEnvVar::set("CODEWHALE_PROVIDER", "openrouter");

        let cases = [
            (
                parse_ok(&["codewhale", "--provider", "lm-studio", "exec", "Reply OK"]),
                vec!["exec".to_string(), "Reply OK".to_string()],
            ),
            (
                parse_ok(&["codewhale", "--provider", "lm-studio", "fleet", "status"]),
                vec!["fleet".to_string(), "status".to_string()],
            ),
        ];

        for (cli, passthrough) in cases {
            let mut resolved =
                resolved_runtime_for_test(ProviderKind::Openrouter, ProviderSource::Config);
            resolved.api_key = Some("unrelated-keyring-secret".to_string());
            resolved.api_key_source = Some(RuntimeApiKeySource::Keyring);

            let cmd = build_tui_command(&cli, &resolved, passthrough.clone())
                .expect("raw provider should dispatch to the TUI");
            assert_eq!(
                command_env(&cmd, "CODEWHALE_PROVIDER").as_deref(),
                Some("lm-studio")
            );
            assert_eq!(
                command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
                Some("lm-studio")
            );
            for secret_var in [
                "CODEWHALE_CLI_API_KEY",
                "DEEPSEEK_API_KEY",
                "OPENROUTER_API_KEY",
                "DEEPSEEK_API_KEY_SOURCE",
            ] {
                assert_eq!(
                    command_env(&cmd, secret_var),
                    None,
                    "raw provider dispatch must not bridge {secret_var}"
                );
            }
            assert_eq!(
                cmd.get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                passthrough
            );
        }
    }

    #[test]
    fn build_tui_command_allows_openai_and_forwards_provider_key() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&[
            "deepseek",
            "--provider",
            "openai",
            "--workspace",
            "/tmp/codewhale-workspace",
        ]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::Openai,
            provider_source: ProviderSource::Cli,
            model: "glm-5".to_string(),
            api_key: Some("resolved-openai-key".to_string()),
            api_key_source: Some(RuntimeApiKeySource::Keyring),
            base_url: "https://openai-compatible.example/v4".to_string(),
            auth_mode: Some("api_key".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, Vec::new()).expect("command");
        assert_eq!(
            command_env(&cmd, "CODEWHALE_PROVIDER").as_deref(),
            Some("openai")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("openai")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY").as_deref(),
            Some("resolved-openai-key")
        );
        assert_eq!(
            command_env(&cmd, "OPENAI_API_KEY").as_deref(),
            Some("resolved-openai-key")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY_SOURCE").as_deref(),
            Some("keyring")
        );
        assert_eq!(command_env(&cmd, "DEEPSEEK_AUTH_MODE"), None);
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--workspace", "/tmp/codewhale-workspace"]),
            "expected workspace forwarding in args: {args:?}"
        );
    }

    #[test]
    fn build_tui_command_allows_openai_codex_from_resolved_runtime() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&["codewhale", "doctor"]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::OpenaiCodex,
            provider_source: ProviderSource::Config,
            model: "gpt-5.5".to_string(),
            api_key: None,
            api_key_source: None,
            base_url: "https://chatgpt.com/backend-api".to_string(),
            auth_mode: Some("oauth".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, vec!["doctor".to_string()])
            .expect("openai-codex should be accepted by the facade");
        assert_eq!(command_env(&cmd, "DEEPSEEK_PROVIDER"), None);
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["doctor"]);
    }

    #[test]
    fn build_tui_command_forwards_explicit_openai_codex_provider() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&["codewhale", "--provider", "openai-codex", "doctor"]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::OpenaiCodex,
            provider_source: ProviderSource::Cli,
            model: "gpt-5.5".to_string(),
            api_key: None,
            api_key_source: None,
            base_url: "https://chatgpt.com/backend-api".to_string(),
            auth_mode: Some("oauth".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, vec!["doctor".to_string()])
            .expect("openai-codex should be accepted by the facade");
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("openai-codex")
        );
    }

    #[test]
    fn build_tui_command_allows_anthropic_cli_provider() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();

        let cli = parse_ok(&["codewhale", "--provider", "anthropic", "doctor"]);
        let resolved = resolved_runtime_for_test(ProviderKind::Anthropic, ProviderSource::Cli);

        let cmd = build_tui_command(&cli, &resolved, vec!["doctor".to_string()])
            .expect("anthropic should be accepted by the facade");
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("anthropic")
        );
    }

    #[test]
    fn build_tui_command_allows_anthropic_env_provider() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();

        let cli = parse_ok(&["codewhale", "doctor"]);
        let resolved = resolved_runtime_for_test(
            ProviderKind::Anthropic,
            ProviderSource::Env("DEEPSEEK_PROVIDER"),
        );

        build_tui_command(&cli, &resolved, vec!["doctor".to_string()])
            .expect("anthropic from provider env should be accepted by the facade");
    }

    #[test]
    fn build_tui_command_bridges_anthropic_keyring_secret() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();

        let cli = parse_ok(&["codewhale", "doctor"]);
        let mut resolved =
            resolved_runtime_for_test(ProviderKind::Anthropic, ProviderSource::Config);
        resolved.api_key = Some("anthropic-keyring-secret".to_string());
        resolved.api_key_source = Some(RuntimeApiKeySource::Keyring);

        let cmd = build_tui_command(&cli, &resolved, vec!["doctor".to_string()])
            .expect("config-sourced anthropic provider should be accepted");

        assert_eq!(command_env(&cmd, "DEEPSEEK_PROVIDER"), None);
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY").as_deref(),
            Some("anthropic-keyring-secret")
        );
        assert_eq!(
            command_env(&cmd, "ANTHROPIC_API_KEY").as_deref(),
            Some("anthropic-keyring-secret")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY_SOURCE").as_deref(),
            Some("keyring")
        );
    }

    #[test]
    fn build_tui_command_does_not_export_default_runtime_overrides_for_profiles() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&["deepseek", "--profile", "google"]);
        let mut resolved_headers = std::collections::BTreeMap::new();
        resolved_headers.insert("X-From-Base".to_string(), "base".to_string());
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::Deepseek,
            provider_source: ProviderSource::Config,
            model: "deepseek-v4-pro".to_string(),
            api_key: Some("config-file-key".to_string()),
            api_key_source: Some(RuntimeApiKeySource::ConfigFile),
            base_url: "https://api.deepseek.com/beta".to_string(),
            auth_mode: Some("api_key".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: Some("normal".to_string()),
            http_headers: resolved_headers,
        };

        let cmd = build_tui_command(&cli, &resolved, Vec::new()).expect("command");

        assert_eq!(command_env(&cmd, "DEEPSEEK_PROVIDER"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_MODEL"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_BASE_URL"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_API_KEY"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_API_KEY_SOURCE"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_AUTH_MODE"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_HTTP_HEADERS"), None);
        assert_eq!(command_env(&cmd, "CODEWHALE_VERBOSITY"), None);
        assert_eq!(command_env(&cmd, "DEEPSEEK_VERBOSITY"), None);
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2).any(|pair| pair == ["--profile", "google"]),
            "expected profile forwarding in args: {args:?}"
        );
    }

    #[test]
    fn build_tui_command_defaults_noninteractive_to_concise_verbosity() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();

        let cli = parse_ok(&["codewhale"]);
        let resolved = resolved_runtime_for_test(ProviderKind::Deepseek, ProviderSource::Config);

        let cmd = build_tui_command(
            &cli,
            &resolved,
            vec!["exec".to_string(), "summarize".to_string()],
        )
        .expect("command");

        assert_eq!(
            command_env(&cmd, "CODEWHALE_VERBOSITY").as_deref(),
            Some("concise")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_VERBOSITY").as_deref(),
            Some("concise")
        );
    }

    #[test]
    fn build_tui_command_respects_resolved_verbosity_override() {
        let _lock = env_lock();
        let (_dir, _bin) = install_fake_tui_binary();

        let cli = parse_ok(&["codewhale"]);
        let mut resolved =
            resolved_runtime_for_test(ProviderKind::Deepseek, ProviderSource::Config);
        resolved.verbosity = Some("normal".to_string());

        let cmd = build_tui_command(&cli, &resolved, vec!["exec".to_string()]).expect("command");

        assert_eq!(
            command_env(&cmd, "CODEWHALE_VERBOSITY").as_deref(),
            Some("normal")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_VERBOSITY").as_deref(),
            Some("normal")
        );
    }

    #[test]
    fn build_tui_command_allows_moonshot_and_forwards_kimi_key() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&[
            "codewhale",
            "--provider",
            "moonshot",
            "--model",
            "kimi-k2.7-code",
            "--workspace",
            "/tmp/codewhale-workspace",
        ]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::Moonshot,
            provider_source: ProviderSource::Cli,
            model: "kimi-k2.7-code".to_string(),
            api_key: Some("resolved-kimi-key".to_string()),
            api_key_source: Some(RuntimeApiKeySource::Keyring),
            base_url: "https://api.moonshot.ai/v1".to_string(),
            auth_mode: Some("api_key".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, Vec::new()).expect("command");
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("moonshot")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_MODEL").as_deref(),
            Some("kimi-k2.7-code")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY").as_deref(),
            Some("resolved-kimi-key")
        );
        assert_eq!(
            command_env(&cmd, "MOONSHOT_API_KEY").as_deref(),
            Some("resolved-kimi-key")
        );
        assert_eq!(
            command_env(&cmd, "KIMI_API_KEY").as_deref(),
            Some("resolved-kimi-key")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY_SOURCE").as_deref(),
            Some("keyring")
        );
        assert_eq!(command_env(&cmd, "DEEPSEEK_AUTH_MODE"), None);
    }

    #[test]
    fn build_tui_command_allows_volcengine_and_forwards_ark_keys() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&[
            "codewhale",
            "--provider",
            "volcengine",
            "--model",
            "DeepSeek-V4-Pro",
            "--workspace",
            "/tmp/codewhale-workspace",
        ]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::Volcengine,
            provider_source: ProviderSource::Cli,
            model: "DeepSeek-V4-Pro".to_string(),
            api_key: Some("resolved-ark-key".to_string()),
            api_key_source: Some(RuntimeApiKeySource::Keyring),
            base_url: "https://ark.cn-beijing.volces.com/api/coding/v3".to_string(),
            auth_mode: Some("api_key".to_string()),
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, Vec::new()).expect("command");
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("volcengine")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_MODEL").as_deref(),
            Some("DeepSeek-V4-Pro")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_API_KEY").as_deref(),
            Some("resolved-ark-key")
        );
        assert_eq!(
            command_env(&cmd, "VOLCENGINE_API_KEY").as_deref(),
            Some("resolved-ark-key")
        );
        assert_eq!(
            command_env(&cmd, "VOLCENGINE_ARK_API_KEY").as_deref(),
            Some("resolved-ark-key")
        );
        assert_eq!(
            command_env(&cmd, "ARK_API_KEY").as_deref(),
            Some("resolved-ark-key")
        );
    }

    #[test]
    fn build_tui_command_exports_explicit_provider_model_and_base_url() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let cli = parse_ok(&[
            "deepseek",
            "--profile",
            "google",
            "--provider",
            "openai",
            "--model",
            "glm-5",
            "--base-url",
            "https://openai-compatible.example/v4",
        ]);
        let resolved = ResolvedRuntimeOptions {
            provider: ProviderKind::Openai,
            provider_source: ProviderSource::Cli,
            model: "glm-5".to_string(),
            api_key: None,
            api_key_source: None,
            base_url: "https://openai-compatible.example/v4".to_string(),
            auth_mode: None,
            insecure_skip_tls_verify: false,
            output_mode: None,
            log_level: None,
            telemetry: false,
            approval_policy: None,
            sandbox_mode: None,
            yolo: None,
            verbosity: None,
            http_headers: std::collections::BTreeMap::new(),
        };

        let cmd = build_tui_command(&cli, &resolved, Vec::new()).expect("command");

        assert_eq!(
            command_env(&cmd, "DEEPSEEK_PROVIDER").as_deref(),
            Some("openai")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_MODEL").as_deref(),
            Some("glm-5")
        );
        assert_eq!(
            command_env(&cmd, "DEEPSEEK_BASE_URL").as_deref(),
            Some("https://openai-compatible.example/v4")
        );
    }

    #[test]
    fn build_tui_command_forwards_provider_keyring_env_vars_for_all_providers() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        for provider in ProviderKind::ALL {
            let cli = parse_ok(&["codewhale", "--workspace", "/tmp/codewhale-workspace"]);
            let resolved = ResolvedRuntimeOptions {
                provider,
                provider_source: ProviderSource::Config,
                model: "test-model".to_string(),
                api_key: Some("test-key".to_string()),
                api_key_source: Some(RuntimeApiKeySource::Keyring),
                base_url: "http://localhost:8000/v1".to_string(),
                auth_mode: Some("api_key".to_string()),
                insecure_skip_tls_verify: false,
                output_mode: None,
                log_level: None,
                telemetry: false,
                approval_policy: None,
                sandbox_mode: None,
                yolo: None,
                verbosity: None,
                http_headers: std::collections::BTreeMap::new(),
            };

            let cmd = build_tui_command(&cli, &resolved, Vec::new())
                .unwrap_or_else(|e| panic!("{}: {e}", provider.as_str()));

            assert_eq!(
                command_env(&cmd, "DEEPSEEK_API_KEY").as_deref(),
                Some("test-key"),
                "{}: DEEPSEEK_API_KEY not forwarded",
                provider.as_str()
            );
            for var in provider_env_vars(provider)
                .iter()
                .filter(|var| **var != "DEEPSEEK_API_KEY")
            {
                assert_eq!(
                    command_env(&cmd, var).as_deref(),
                    Some("test-key"),
                    "{}: {var} not forwarded",
                    provider.as_str()
                );
            }
            assert_eq!(
                command_env(&cmd, "DEEPSEEK_API_KEY_SOURCE").as_deref(),
                Some("keyring"),
                "{}: expected keyring source bridge",
                provider.as_str()
            );
            assert_eq!(
                command_env(&cmd, "DEEPSEEK_AUTH_MODE"),
                None,
                "{}: auth mode should come from config/profile, not env handoff",
                provider.as_str()
            );
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
                    "source <(codewhale completion bash)",
                    "~/.local/share/bash-completion/completions/codewhale",
                    "fpath=(~/.zfunc $fpath)",
                    "codewhale completion fish > ~/.config/fish/completions/codewhale.fish",
                    "codewhale completion powershell | Out-String | Invoke-Expression",
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

    /// Regression for issue #247: on Windows the dispatcher must find the
    /// sibling `codewhale-tui.exe`, not bail out looking for an
    /// extension-less `codewhale-tui`. The candidate resolver also accepts
    /// the suffix-less name on Windows so users who manually renamed the
    /// file as a workaround keep working after the upgrade.
    #[test]
    fn sibling_tui_candidate_picks_platform_correct_name() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let dispatcher = dir
            .path()
            .join("codewhale")
            .with_extension(std::env::consts::EXE_EXTENSION);
        // Touch the dispatcher so its parent dir is the lookup root.
        std::fs::write(&dispatcher, b"").unwrap();

        // No sibling yet — resolver returns None.
        assert!(sibling_tui_candidate(&dispatcher).is_none());

        let target =
            dispatcher.with_file_name(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&target, b"").unwrap();

        let found = sibling_tui_candidate(&dispatcher).expect("must locate sibling");
        assert_eq!(found, target, "primary platform-correct name wins");
    }

    #[test]
    fn dispatcher_spawn_error_names_path_and_recovery_checks() {
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "access is denied");
        let message = tui_spawn_error(Path::new("C:/tools/codewhale-tui.exe"), &err);

        assert!(message.contains("C:/tools/codewhale-tui.exe"));
        assert!(message.contains("access is denied"));
        assert!(message.contains("where codewhale"));
        assert!(message.contains("DEEPSEEK_TUI_BIN"));
    }

    #[cfg(unix)]
    #[test]
    fn tui_child_exit_code_maps_unix_signal_to_shell_status() {
        use std::os::unix::process::ExitStatusExt;

        let status = std::process::ExitStatus::from_raw(libc::SIGPIPE);

        assert_eq!(tui_child_exit_code(status), Some(141));
    }

    /// Windows-only fallback: the user from #247 manually renamed the
    /// file to drop `.exe`. After the fix lands, that workaround must
    /// still resolve via the suffix-less fallback so they don't have to
    /// rename it back.
    #[cfg(windows)]
    #[test]
    fn sibling_tui_candidate_windows_falls_back_to_suffixless() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let dispatcher = dir.path().join("codewhale.exe");
        std::fs::write(&dispatcher, b"").unwrap();

        // Only the suffixless name exists — emulates the manual rename.
        let suffixless = dispatcher.with_file_name("codewhale-tui");
        std::fs::write(&suffixless, b"").unwrap();

        let found = sibling_tui_candidate(&dispatcher)
            .expect("Windows fallback must locate suffixless codewhale-tui");
        assert_eq!(found, suffixless);
    }

    /// `DEEPSEEK_TUI_BIN` overrides the discovery path. Useful for
    /// custom Windows install layouts and CI test rigs.
    #[test]
    fn locate_sibling_tui_binary_honours_env_override() {
        let _lock = env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let custom = dir
            .path()
            .join(format!("custom-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&custom, b"").unwrap();
        let custom_str = custom.to_string_lossy().into_owned();
        let _bin = ScopedEnvVar::set("DEEPSEEK_TUI_BIN", &custom_str);

        let resolved = locate_sibling_tui_binary().expect("override must resolve");
        assert_eq!(resolved, custom);
    }
}
