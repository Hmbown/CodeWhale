# DeepSeek TUI Architecture

This document provides an overview of the DeepSeek TUI architecture for developers and contributors.

Current boundary note (v0.8.6):
- `crates/tui` is still the live end-user runtime for the TUI, runtime API, task manager, and tool execution loop.
- Other workspace crates are being split out incrementally, but they are not yet the sole runtime source of truth.
- The LSP subsystem (`crates/tui/src/lsp/`) is fully wired into the engine's post-tool-execution path
  (`core/engine/lsp_hooks.rs`), providing inline diagnostics after every edit_file/apply_patch/write_file.
- The swarm agent system was removed in v0.8.5 in favour of sub-agents (agent_spawn) and RLM (rlm_query).
  No model-visible swarm tool remains in the active codebase.
- Game TUI is implemented as a TUI-owned Game Console scaffold. Its
  authoritative planning spec is `docs/GAME_TUI_FRAMEWORK_SPEC.md`; it must stay
  TUI-owned and must not depend on an external Gen-micom repo or Python CLI.

## High-Level Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Interface                          │
│  ┌─────────────────┐  ┌─────────────────┐  ┌────────────────┐  │
│  │   TUI (ratatui) │  │  One-shot Mode  │  │  Config/CLI    │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬───────┘  │
└───────────┼─────────────────────┼────────────────────┼──────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Core Engine                              │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                    Agent Loop (core/engine.rs)           │   │
│  │  ┌─────────┐  ┌─────────────┐  ┌──────────────────────┐ │   │
│  │  │ Session │  │ Turn Mgmt   │  │ Tool Orchestration   │ │   │
│  │  └─────────┘  └─────────────┘  └──────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Tool & Extension Layer                      │
│  ┌──────────┐  ┌──────────┐  ┌─────────┐  ┌────────────────┐   │
│  │  Tools   │  │  Skills  │  │  Hooks  │  │  MCP Servers   │   │
│  │ (shell,  │  │ (plugins)│  │ (pre/   │  │  (external)    │   │
│  │  file)   │  │          │  │  post)  │  │                │   │
│  └──────────┘  └──────────┘  └─────────┘  └────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Runtime API + Task Management                  │
│  ┌─────────────────────────────┐  ┌──────────────────────────┐  │
│  │ HTTP/SSE Runtime API        │  │ Persistent Task Manager  │  │
│  │ (runtime_api.rs)            │  │ (task_manager.rs)        │  │
│  └─────────────────────────────┘  └──────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
            │                     │
            ▼                     ▼
┌─────────────────────────────────────────────────────────────────┐
│                        LLM Layer                                │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              LLM Client Abstraction (llm_client.rs)       │  │
│  │  ┌─────────────────┐  ┌─────────────────────────────┐    │  │
│  │  │  DeepSeek Client │  │  Compatible Client (DeepSeek)│    │  │
│  │  │   (client.rs)   │  │       (client.rs)           │    │  │
│  │  └─────────────────┘  └─────────────────────────────┘    │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Module Organization

### Entry Point

- **`main.rs`** - CLI argument parsing (clap), configuration loading, entry point routing

### Core Components

- **`core/`** - Main engine components
  - `engine.rs` - Engine state, operation handling, message processing
  - `engine/turn_loop.rs` - Streaming turn loop and tool execution orchestration
  - `engine/capacity_flow.rs` - Capacity guardrail checkpoints and interventions
  - `session.rs` - Session state management
  - `turn.rs` - Turn-based conversation handling
  - `events.rs` - Event system for UI updates
  - `ops.rs` - Core operations

### Configuration

- **`config.rs`** - Configuration loading, profiles, environment variables
- **`settings.rs`** - Runtime settings management

### Workspace Crates

- **`crates/tools`** - Shared tool invocation primitives, including tool result/error/capability types used by the TUI runtime.
- **`crates/agent`** - Model/provider registry (ModelRegistry) for resolving model IDs to provider endpoints.
- **`crates/app-server`** - HTTP/SSE + JSON-RPC app server transport for headless agent workflows.
- **`crates/config`** - Config loading, profiles, environment variable precedence, CLI runtime overrides.
- **`crates/core`** - Agent loop, session management, turn orchestration, capacity flow guardrails.
- **`crates/execpolicy`** - Approval/sandbox policy engine for tool execution decisions.
- **`crates/hooks`** - Lifecycle hooks (stdout, jsonl, webhook) for pre/post tool events.
- **`crates/mcp`** - MCP client + stdio server for Model Context Protocol tool servers.
- **`crates/protocol`** - Request/response framing and protocol types.
- **`crates/secrets`** - OS keyring integration for API key storage.
- **`crates/state`** - SQLite thread/session persistence layer.
- **`crates/tui-core`** - Event-driven TUI state machine scaffold.

### LLM Integration

- **`client.rs`** - HTTP client for DeepSeek's documented OpenAI-compatible Chat Completions API
- **`llm_client.rs`** - Abstract LLM client trait with retry logic
- **`models.rs`** - Data structures for API requests/responses

#### DeepSeek API Endpoints

DeepSeek exposes OpenAI-compatible endpoints. The CLI uses:
- `https://api.deepseek.com/beta/chat/completions` - default v0.8.16 DeepSeek model turns
- `https://api.deepseek.com/beta/models` - default v0.8.16 live model discovery and health checks

`https://api.deepseek.com/v1` is accepted for OpenAI SDK compatibility, and
can still be configured explicitly to opt out of beta-only features such as
strict tool mode, chat prefix completion, and FIM completion. The public
DeepSeek docs do not document a Responses API path for this workflow; the engine
drives turns through Chat Completions.

### Tool System

- **`tools/`** - Built-in tool implementations
  - `mod.rs` - Tool registry and common types
  - `shell.rs` - Shell command execution
  - `file.rs` - File read/write operations
  - `todo.rs` - Checklist tools plus legacy todo aliases
  - `tasks.rs` - Model-visible durable task, gate, background shell, and PR-attempt tools
  - `github.rs` - Read-only GitHub context and guarded comment/closure tools backed by `gh`
  - `automation.rs` - Model-visible scheduling tools over `AutomationManager`
  - `plan.rs` - Planning tools
  - `subagent.rs` - Sub-agent spawning (replaces the removed `agent_swarm` surface)
  - `spec.rs` - Tool specifications
  - `rlm.rs` - Recursive Language Model (RLM) tool — sandboxed Python REPL with `llm_query()` helpers

### Extension Systems

- **`mcp.rs`** - Model Context Protocol client for external tool servers
- **`skills.rs`** - Plugin/skill loading and execution
- **`hooks.rs`** - Pre/post execution hooks with conditions

### User Interface

- **`tui/`** - Terminal UI components (ratatui-based)
  - `app.rs` - Application state and message handling
  - `ui.rs` - Event handling, streaming state, and rendering logic
  - `approval.rs` - Tool approval dialog
  - `clipboard.rs` - Clipboard handling
  - `streaming.rs` - Streaming text collector

- **`ui.rs`** - Legacy/simple UI utilities

### Game TUI Framework

`deepseek play` is implemented as a Game Console presentation inside the
existing TUI, not as a separate terminal application or event loop. The
authoritative plan is
[`GAME_TUI_FRAMEWORK_SPEC.md`](GAME_TUI_FRAMEWORK_SPEC.md); this section only
records architectural attachment points.

The implementation includes a required pure Rust `crates/game` runtime for
manifests, path validation, bounded lookup, constrained Starlark driver
functions, structured render data, git-like story branch/playbook data, and
atomic save commits. That crate must not depend on ratatui, the TUI, the LLM
client, shell/network execution, Python, or external game repos.

The TUI side wires `crates/cli`, `crates/tui/src/main.rs`, `commands/`,
`tui/app.rs`, `tui/ui.rs`, `core/engine/tool_setup.rs`,
`tools/registry.rs`, `skills/`, and `tools/subagent/` into a `GameSession`
presentation/tool-profile state. V1 must not add `AppMode::Game`.

The player tool profile is a whitelist around native `game_*` tools,
game-scoped `game_agent_*` helpers, and required skill-loading support. Normal
coding tools stay out of player mode unless developer mode is explicitly active.
See
[`GAME_TUI_FRAMEWORK_SPEC.md`](GAME_TUI_FRAMEWORK_SPEC.md).

### LSP Integration

- **`lsp/`** - Post-edit diagnostics injection (#136)
  - `mod.rs` - `LspManager` — lazy per-language transport pool + config
  - `client.rs` - `StdioLspTransport` — JSON-RPC over stdio with `didOpen`/`didChange`/`publishDiagnostics`
  - `diagnostics.rs` - Diagnostic types, severity, and HTML-block renderer
  - `registry.rs` - Language detection and default server map (rust-analyzer, pyright, gopls, clangd, typescript-language-server)
  - Wired into the engine via `core/engine/lsp_hooks.rs` — called after every successful edit

### Security

- **`sandbox/`** - platform sandbox policy preparation and denial reporting
  - `mod.rs` - Sandbox type definitions
  - `policy.rs` - Sandbox policy configuration
  - `seatbelt.rs` - macOS Seatbelt profile generation
  - `landlock.rs` - Linux Landlock detection and future helper contract
  - `windows.rs` - Windows helper contract; not advertised until a Job
    Object process-containment helper exists

### Utilities

- **`utils.rs`** - Common utilities
- **`logging.rs`** - Logging infrastructure
- **`compaction.rs`** - Context compaction for long conversations
- **`pricing.rs`** - Cost estimation
- **`prompts.rs`** - System prompt templates
- **`project_doc.rs`** - Project documentation handling
- **`session.rs`** - Session serialization
- **`runtime_api.rs`** - HTTP/SSE runtime API (`deepseek serve --http`)
- **`runtime_threads.rs`** - Durable thread/turn/item store + replayable event timeline
- **`task_manager.rs`** - Durable queue, worker pool, task timelines and artifacts

## Data Flow

### Interactive Session

1. User input received in TUI
2. Input processed by `core/engine.rs`
3. Message sent to LLM via `llm_client.rs`
4. Response streamed back, parsed in `client.rs`
5. Tool calls extracted and executed via `tools/`
6. Hooks triggered before/after tool execution
7. Results aggregated and sent back to LLM
8. Final response rendered in TUI

### Crash Recovery + Offline Queue

1. Before sending user input, the TUI writes a checkpoint snapshot to `~/.deepseek/sessions/checkpoints/latest.json`
2. Startup remains fresh by default; prior sessions are resumed explicitly via `--resume`/`--continue` (or `Ctrl+R` in TUI)
3. While degraded/offline, new prompts are queued in-memory and mirrored to `~/.deepseek/sessions/checkpoints/offline_queue.json`
4. Queue edits (`/queue ...`) are persisted continuously so drafts and queued prompts survive restarts
5. Successful turn completion clears the active checkpoint and writes a durable session snapshot
6. Agent/Yolo turns also take pre/post-turn side-git workspace snapshots under `~/.deepseek/snapshots/<project_hash>/<worktree_hash>/.git`; `/restore N` and `revert_turn` restore file state without changing conversation history or the user's `.git`

### Tool Execution

1. LLM requests tool via `tool_use` content block
2. Tool registry looks up handler
3. Pre-execution hooks run
4. Approval requested if needed (non-yolo mode)
5. Tool executed (possibly sandboxed on macOS)
6. Post-execution hooks run
7. Result metadata is retained on runtime item records
8. **LSP post-edit hook** (v0.8.6): if the tool was `edit_file`/`apply_patch`/`write_file` and LSP is enabled, the engine runs `run_post_edit_lsp_hook()` to collect diagnostics
9. **Diagnostics flush** (v0.8.6): before the next API request, `flush_pending_lsp_diagnostics()` injects any collected errors as a synthetic user message
10. Result returned to agent loop

### Background Tasks

1. Client enqueues task (`/task add ...` or `POST /v1/tasks`)
2. `task_manager.rs` persists task + queue entry under `~/.deepseek/tasks`
3. Worker picks queued task (bounded pool), transitions to `running`
4. Task creates/uses a runtime thread and starts a runtime turn
5. `runtime_threads.rs` persists thread/turn/item records + monotonic event sequence
6. Timeline/tool summaries/artifact references are persisted incrementally
7. Checklist state, verifier gates, PR attempts, and guarded GitHub events are applied from tool metadata to the active task
8. Final state (`completed|failed|canceled`) is durable and queryable via TUI/API

Model-visible durable task tools are a surface over this same manager. They do
not introduce a parallel work system: `task_create` enqueues normal tasks,
`checklist_*` updates task-local progress, `task_gate_run` and completed
`task_shell_wait` attach verification evidence, and automation runs enqueue
ordinary durable tasks.

### Runtime Thread/Turn Timeline

1. API/TUI creates or resumes a thread (`/v1/threads*`)
2. Turn starts on the thread (`/v1/threads/{id}/turns`)
3. Engine events are mapped to item lifecycle events (`item.started|item.delta|item.completed`)
4. Interrupt/steer operations apply to the active turn only
5. Compaction (auto/manual) is emitted as `context_compaction` item lifecycle
6. Clients replay history and resume with `/v1/threads/{id}/events?since_seq=<n>`

### Durable Schema Gates

- `session_manager.rs`, `runtime_threads.rs`, and `task_manager.rs` embed `schema_version` on persisted records.
- On load, newer schema versions are rejected with explicit errors instead of silently truncating/overwriting data.
- This allows safe forward migrations and prevents corruption when binaries and stored state are out of sync.

## Extension Points

### Adding a New Tool

1. Create handler in `tools/`
2. Register in `tools/registry.rs`
3. Add tool specification (name, description, input schema)

### Adding an MCP Server

1. Configure in `~/.deepseek/mcp.json`
2. Server auto-discovered at startup
3. Tools exposed to LLM automatically

### Creating a Skill

1. Create skill directory with `SKILL.md`
2. Define skill prompt and optional scripts
3. Place in `~/.deepseek/skills/`

### Adding Hooks

Configure in `~/.deepseek/config.toml`:

```toml
[[hooks]]
event = "tool_call_before"
command = "echo 'Running tool: $TOOL_NAME'"
```

## Key Design Decisions

1. **Streaming-first**: All LLM responses stream for responsiveness
2. **Tool safety**: Non-YOLO mode requires approval for destructive operations, including side-effectful MCP tools
3. **Extensibility**: MCP, skills, and hooks allow customization without code changes
4. **Cross-platform**: Core works on Linux/macOS/Windows. Sandbox guarantees
   are platform-specific: macOS Seatbelt is the active policy path; Linux and
   Windows require helper enforcement before they should be treated as full OS
   sandboxing.
5. **Minimal dependencies**: Careful dependency selection for build speed
6. **Local-first runtime API**: HTTP/SSE endpoints are intended for trusted localhost access and are currently served by the `crates/tui` runtime

## Configuration Files

- `~/.deepseek/config.toml` - Main configuration
- `/etc/deepseek/managed_config.toml` - Optional managed defaults layer (Unix)
- `/etc/deepseek/requirements.toml` - Optional allowed-policy constraints (Unix)
- `~/.deepseek/mcp.json` - MCP server configuration
- `~/.deepseek/skills/` - User skills directory
- `~/.deepseek/sessions/` - Session history
- `~/.deepseek/sessions/checkpoints/` - Crash checkpoint + offline queue persistence
- `~/.deepseek/snapshots/` - Side-git pre/post-turn workspace snapshots for `/restore` and `revert_turn`
- `~/.deepseek/tasks/` - Background task records, queue, timelines, artifacts
- `~/.deepseek/audit.log` - Append-only audit events for credential + approval/elevation actions
