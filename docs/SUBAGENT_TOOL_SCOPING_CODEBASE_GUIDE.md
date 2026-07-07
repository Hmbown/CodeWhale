# CodeWhale Codebase Guide for Sub-Agent Tool Scoping

> **Audience:** A developer new to both Rust and the CodeWhale codebase, preparing to implement the three-phase sub-agent tool scoping plan (see `SUBAGENT_TOOL_SCOPING_PLAN.md` and issue #4042).
>
> **Version:** CodeWhale 0.8.66, workspace edition 2024, Rust stable 1.88+.
>
> **Date:** 2026-07-07

---

## Table of Contents

1. [Codebase Overview](#section-1-codebase-overview)
2. [Rust Conventions in CodeWhale](#section-2-rust-conventions-in-codewhale)
3. [Architectural Patterns](#section-3-architectural-patterns)
4. [Sub-Agent Tool Scoping Deep Dive](#section-4-sub-agent-tool-scoping-deep-dive)
5. [Implementation Considerations](#section-5-implementation-considerations)
6. [Open Questions and Recommendations](#section-6-open-questions-and-recommendations)

---

## Section 1: Codebase Overview

### 1.1 Repository Structure

CodeWhale is a **Cargo workspace** with 15 crates, defined at the root `Cargo.toml` (line 1–16). The default build members are `crates/cli`, `crates/app-server`, and `crates/tui` — these are the ones you build with a bare `cargo build`.

```
crates/
├── tui/            ★ The monolith (~10k lines). Most live runtime logic lives here,
│                   │  including the sub-agent system, tool registry, engine, and MCP integration.
│                   │  Binary: `codewhale-tui`.
├── cli/            Thin CLI dispatcher. Parses args (Clap), delegates to TUI binary.
│                   Binary: `codewhale` + legacy `codew` shim.
├── app-server/     HTTP/SSE + JSON-RPC transport for headless / remote use.
├── core/           Agent loop boundaries: thread lifecycle, job management.
├── protocol/       Request/response framing, shared types (zero internal deps).
├── config/         Config loading, profiles, TOML schema, env precedence, FleetExecConfig.
├── state/          SQLite thread/session persistence.
├── tools/          Typed tool specs: ToolSpec, ToolRegistry, ApprovalRequirement, ToolError.
├── mcp/            MCP client + stdio server transport.
├── hooks/          Lifecycle hooks (stdout, JSONL, webhook).
├── execpolicy/     Approval/sandbox policy engine.
├── agent/          Model/provider registry, family detection, route resolution.
├── secrets/        OS keyring + file fallback for API keys.
├── release/        Update checker (GitHub Releases + CNB mirror).
├── whaleflow/      WhaleFlow workflow IR (Starlark + JS/TS compilation).
```

**The monolith caveat:** The architecture doc (`docs/ARCHITECTURE.md`) notes that the lean subsystem crates (core, tools, protocol, etc.) are being extracted incrementally, but **the actual runtime lives in `crates/tui/`**. When you trace a code path, you'll often find both the "crate" abstraction (e.g., `crates/tools/src/lib.rs`'s `ToolRegistry`) and the "runtime" implementation (e.g., `crates/tui/src/tools/registry.rs`'s `ToolRegistryBuilder`) — the runtime copies/extends the lean type. This is intentional refactoring-in-progress, not duplication.

### 1.2 Key Entry Points

| Entry Point | File | What Happens |
|---|---|---|
| CLI binary | `crates/cli/src/main.rs:1-3` | 3-line shim → `codewhale_cli::run_cli()` |
| CLI `run_cli()` | `crates/cli/src/lib.rs:629-680` | Parses Clap → builds `CliRuntimeOverrides` → delegates to TUI binary |
| TUI binary | `crates/tui/src/main.rs:1-2` | `#[tokio::main] async fn main()` |
| TUI startup | `crates/tui/src/main.rs` (rest of file) | Initializes tracing, config, ratatui UI, engine loop |
| Engine entry | `crates/tui/src/core/engine.rs:245` | `EngineConfig` struct created, then `engine.run()` drives the turn loop |
| Turn loop | `crates/tui/src/core/engine/turn_loop.rs` | Orchestrates each model request/response pair, tool execution |
| Agent tool entry | `crates/tui/src/tools/subagent/mod.rs:3852` | `spawn_subagent_from_input()` — where sub-agent spawns begin |

### 1.3 Configuration and Provider Management

Configuration flows through `crates/config/src/lib.rs`. The key struct is `ConfigStore` which layers:
1. **Defaults** (embedded in code)
2. **config.toml** (global `~/.codewhale/config.toml` or project `.codewhale/config.toml`)
3. **Environment variables** (e.g., `DEEPSEEK_API_KEY`)
4. **CLI overrides** (`codewhale_config::CliRuntimeOverrides`)

Provider routing is in `crates/agent/`. The `ModelRegistry` maps provider IDs to endpoint URLs, API key locations, and wire protocols (mostly OpenAI-compatible JSON; Anthropic uses Anthropic Messages; some use Google AI Studio). 29 providers are supported.

### 1.4 CLI vs TUI vs Headless

- **`codewhale` (CLI binary):** Thin facade. For most commands (`run`, `doctor`, `fleet`, `exec`, etc.), it delegates to the TUI binary via `delegate_to_tui()` which spawns `codewhale-tui` as a subprocess.
- **`codewhale-tui` (TUI binary):** The full application. When invoked interactively, opens a ratatui-based terminal UI. Also handles headless modes via `--exec`, `--headless`, `--fleet-run`.
- **Headless mode (Fleet):** `codewhale exec` or `codewhale fleet run` — the TUI binary runs in headless mode, writes results to stdout/JSON, and exits. This is how Fleet workers run. Key: headless workers have their own `EngineConfig` (including `allowed_tools`/`disallowed_tools`) and can spawn in-process sub-agents just like the interactive session.

---

## Section 2: Rust Conventions in CodeWhale

### 2.1 Formatting and Linting

- **`cargo fmt`** — mandatory before commits. The workspace has no custom `rustfmt.toml` (uses Rust defaults).
- **`cargo clippy`** — must pass without warnings. Run: `cargo clippy --workspace --all-targets --all-features`.
- **Naming:** Rust standard: `snake_case` for functions/variables/fields, `CamelCase` for types and enums, `SCREAMING_SNAKE` for constants.

```rust
// Example from crates/tui/src/tools/subagent/mod.rs:78-79
const DEFAULT_MAX_STEPS: u32 = u32::MAX;
pub const SUBAGENT_LIST_CLEANUP_MIN_INTERVAL: Duration = Duration::from_secs(2);
```

### 2.2 Documentation

- Public APIs require `///` doc comments (CONTRIBUTING.md §"Code Style", line 43).
- Crate-level documentation: `//!` at the top of `lib.rs` / `mod.rs`.

```rust
// Example: crates/tui/src/tools/subagent/mod.rs:1-9
//! Sub-agent spawning system.
//!
//! Provides tools to spawn background sub-agents, query their status,
//! and retrieve results. Sub-agents run with a filtered toolset and
//! inherit the workspace configuration from the main session.
```

```rust
// Example: crates/tui/src/tools/subagent/mod.rs:1433-1438
/// Runtime configuration for spawning sub-agents.
///
/// Carries everything a child needs to (a) build its own tool registry —
/// including the manager so grandchildren can spawn — and (b) cooperate with
/// lifecycle cancellation and depth caps.
pub struct SubAgentRuntime { ... }
```

### 2.3 Error Handling

CodeWhale uses **two error strategies** side by side:

**`anyhow` — application-level errors (ubiquitous):**
Every crate uses `anyhow::Result` as its primary return type. Use `.context()` / `.with_context()` to add information as errors propagate upward.

```rust
// Example: crates/tui/src/tools/subagent/mod.rs:19
use anyhow::{Result, anyhow};
```

```rust
// Example: crates/config/src/persistence.rs:46-69 — chained .with_context() calls
let contents = std::fs::read_to_string(&path)
    .with_context(|| format!("Failed to read config from {}", path.display()))?;
```

**`thiserror` — typed domain errors (selective):**
Used for structured error types where callers need to match on variants.

```rust
// Example: crates/tools/src/lib.rs:47-63
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    #[error("Invalid tool input: {0}")]
    InvalidInput(String),
    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),
    // ...
}
```

Pattern for using `thiserror` factories — always `#[must_use]`:

```rust
impl ToolError {
    #[must_use]
    pub fn execution_failed(message: impl Into<String>) -> Self { ... }
}
```

**Hand-implemented Error (rare):**
`crates/config/src/route/errors.rs` hand-implements `Display + Error` for `RouteError` because `thiserror` is not a dependency of that crate:

```rust
// crates/config/src/route/errors.rs:3-4 — explicit justification
// Note: `thiserror` is not a dependency of this crate, so this is hand-written
```

### 2.4 Async Patterns

- **Runtime:** `tokio` (v1.50.0, `features = ["full"]`).
- **Async traits:** `#[async_trait]` from the `async-trait` crate (v0.1.89).

```rust
// Example: crates/tools/src/lib.rs:325-348
#[async_trait]
pub trait ToolHandler {
    async fn handle(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;
}
```

- **`async fn`** is used extensively in the engine, sub-agent system, and TUI. Functions should be marked `async` only when they contain `.await` points.
- **Spawn vs block:** Long-running background work (sub-agents, builds) uses `tokio::spawn` or the project's `spawn_supervised()` wrapper (from `crates/tui/src/utils.rs`). Short synchronous operations stay inline.
- **Cancellation:** The `CancellationToken` from `tokio_util` is used for cooperative cancellation. `child_runtime()` derives a child token; `background_runtime()` detaches it.

### 2.5 Testing Conventions

**Unit tests:** Colocated in `#[cfg(test)] mod tests` blocks at the bottom of each source file.

```rust
// Pattern: crates/tui/src/tools/subagent/mod.rs (has tests at bottom, ~7000 lines)
#[cfg(test)]
mod tests {
    use super::*;
    // ... test functions ...
}
```

**Integration tests:** In the crate's own `tests/` directory (e.g., `crates/protocol/tests/`, `crates/state/tests/`, `crates/tools/tests/`). The repo root `tests/` is **not** used.

**Test helpers:** Factory functions that return minimal valid struct instances are common:

```rust
// Example pattern (simplified)
fn test_tool_context() -> ToolContext { ... }
fn test_subagent_runtime() -> SubAgentRuntime { ... }
```

**Running tests:**
```bash
# All tests
cargo test --workspace --all-features

# A specific crate
cargo test -p codewhale-tui

# A specific test function
cargo test -p codewhale-tui -- build_allowed_tools_general
```

### 2.6 Serde Conventions

- `#[serde(rename_all = "snake_case")]` on enums
- `#[serde(default)]` for backward-compatible config fields
- `#[serde(skip_serializing_if = "Option::is_none")]` for optional fields
- `#[serde(alias = "...")]` for renamed fields (e.g., `alias = "concurrency"` on `max_concurrency`)

```rust
// Example: crates/tui/src/worker_profile.rs:66-67
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShellPolicy { None, ReadOnly, Full }
```

### 2.7 Builder Pattern

CodeWhale uses a fluent builder pattern extensively for constructing complex structs:

```rust
// Example: SubAgentRuntime construction
let runtime = SubAgentRuntime::new(client, model, context, allow_shell, event_tx, manager)
    .with_role_models(self.config.subagent_model_overrides.clone())
    .with_auto_model(self.session.auto_model)
    .with_reasoning_effort(effort, auto)
    .with_agent_tool_surface_options(options)
    .with_max_spawn_depth(self.config.max_spawn_depth)
    .with_mcp_pool(mcp_pool)
    .background_runtime();
```

Each builder method is `#[must_use]`, takes `mut self`, and returns `Self`.

### 2.8 Logging

Uses the `tracing` crate with `tracing-subscriber` for structured output:

```rust
tracing::info!(target: "subagent", agent_id = %id, "sub-agent started");
tracing::warn!(target: "mcp", server = %name, "connection dropped");
```

There's **no widespread use** of `#[tracing::instrument]` — spans are created manually where needed.

---

## Section 3: Architectural Patterns

### 3.1 Overall Architecture

CodeWhale follows a **layered but pragmatically centralized** architecture:

```
┌──────────────────────────────────────────────────┐
│  User Interface                                   │
│  ┌──────┐  ┌──────────┐  ┌──────────────────┐    │
│  │  TUI  │  │ One-shot │  │  CLI (dispatcher) │   │
│  └──┬───┘  └────┬─────┘  └────────┬─────────┘   │
└─────┼───────────┼─────────────────┼──────────────┘
      │           │                 │
┌─────┴───────────┴─────────────────┴──────────────┐
│  Core Engine (turn loop, tool dispatch, MCP)      │
│  crates/tui/src/core/engine.rs                    │
└─────────────────────┬────────────────────────────┘
                      │
┌─────────────────────┴────────────────────────────┐
│  Tool & Extension Layer                           │
│  ┌────────┐ ┌───────┐ ┌───────┐ ┌─────────────┐ │
│  │ Tools  │ │Skills │ │Hooks  │ │MCP Client    │ │
│  └────────┘ └───────┘ └───────┘ └─────────────┘ │
└──────────────────────────────────────────────────┘
```

The engine holds a `ToolRegistry`, an `McpPool` (via `Arc<Mutex<McpPool>>`), and a `SubAgentManager` (via `SharedSubAgentManager`). Each turn: build catalog → send to model → parse response → dispatch tool calls → execute → collect results → repeat.

### 3.2 Crate Dependency Graph

```
config ← execpolicy, secrets
agent  ← config
protocol (zero internal deps)
tools  ← protocol
state  ← (external: rusqlite)
hooks  ← protocol, config
mcp    ← protocol, config
execpolicy ← config
core   ← agent, config, execpolicy, hooks, mcp, protocol, state, tools
tui    ← config, core, execpolicy, hooks, mcp, protocol, tools, ...
cli    ← agent, app-server, config, execpolicy, mcp, release, secrets, state, ...
app-server ← core, agent, config, execpolicy, hooks, mcp, protocol, state, tools
```

Build order (bottom-up): protocol → config/secrets/tools/state → agent/execpolicy/hooks/mcp → core → tui/cli/app-server.

### 3.3 Key Architectural Patterns

#### Pattern 1: Tool Registry

**Location:** `crates/tui/src/tools/registry.rs` (the runtime implementation), `crates/tools/src/lib.rs` (the lean crate interface).

The `ToolRegistry` maps tool **names** (`String`) → tool **specs** (`Arc<dyn ToolSpec>`). Each `ToolSpec` declares:
- `name()` — the canonical tool name exposed to the model
- `description()` — the prose sent in the API schema
- `json_schema()` — the JSON Schema for the tool input
- `approval_requirement()` — Auto / Suggest / Required
- `capability()` — Read / Write / Shell / Network

```rust
// crates/tui/src/tools/registry.rs:30-38
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolSpec>>,
    context: ToolContext,
    api_cache: OnceLock<Vec<Tool>>,  // Memoised catalog for prefix-cache stability
}
```

**How tools are registered:** `ToolRegistryBuilder` (same file) accumulates `Vec<Arc<dyn ToolSpec>>` and flushes them into the registry at `.build(context)`. This is how custom tools, MCP tools, and native tools all enter the same pool.

```rust
// crates/tui/src/tools/registry.rs:938-955 — MCP tools are registered as adapters
pub fn with_mcp_tools(mut self, mcp_pool: Arc<Mutex<McpPool>>) -> Self {
    if let Ok(pool) = mcp_pool.try_lock() {
        for (name, tool) in pool.all_tools() {
            let adapter = Arc::new(McpToolAdapter { name, tool, pool: mcp_pool.clone() });
            self.tools.push(adapter);
        }
    }
    self
}
```

#### Pattern 2: Gate Chain (Approval + Deny + Allow)

**Location:** `crates/tui/src/core/engine.rs:3906-3915`, `crates/tui/src/core/engine/turn_loop.rs:3045-3058`

The gate chain filters which tools the model can **see** (catalog) and **call** (execution):

```rust
// engine.rs:3906-3915 — filter model-visible catalog
fn filter_tool_catalog_for_gates(
    catalog: &mut Vec<Tool>,
    allowed_tools: Option<&[String]>,
    disallowed_tools: Option<&[String]>,
) {
    catalog.retain(|tool| {
        !turn_loop::command_denies_tool(disallowed_tools, &tool.name)
            && turn_loop::command_allows_tool(allowed_tools, &tool.name)
    });
}
```

The `command_denies_tool()` function supports three match modes:
1. **Exact match** (case-insensitive): `"exec_shell"` denies only `exec_shell`
2. **Prefix wildcard:** `"mcp_*"` denies everything starting with `mcp_`
3. **Server-level wildcard:** `"mcp_database_*"` denies all tools from the `database` MCP server

```rust
// turn_loop.rs:3045-3058
pub(super) fn command_denies_tool(disallowed_tools: Option<&[String]>, tool_name: &str) -> bool {
    let Some(disallowed_tools) = disallowed_tools else { return false };
    let tool_name = tool_name.to_ascii_lowercase();
    disallowed_tools.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        if let Some(prefix) = rule.strip_suffix('*') {
            tool_name.starts_with(prefix)
        } else {
            tool_name == rule
        }
    })
}
```

#### Pattern 3: Sub-Agent Pattern

**Location:** `crates/tui/src/tools/subagent/mod.rs` (7025 lines — this is the heart of it)

Each sub-agent:
1. Gets a **clone** of the parent's `SubAgentRuntime` (via `background_runtime()` or `child_runtime()`)
2. Builds its own `ToolRegistry` from `ToolRegistryBuilder::with_full_agent_surface_options()` — the SAME full surface as the parent
3. Wraps it in a `SubAgentToolRegistry` that applies **posture**, **allowlist**, and **approval** filters
4. Runs its own engine loop (`run_subagent()` / `run_subagent_task()`)
5. Reports results back via a `SubAgentCompletion` sentinel

Key structs:
- `SubAgentRuntime` (line 1440) — the parent's runtime state, cloned for each child
- `SubAgentToolRegistry` (line 6538) — the child's filtered view of the tool registry
- `SubAgent` (line 1741) — the running instance with its own model, prompt, status
- `SubAgentManager` — the global manager that tracks all active sub-agents

#### Pattern 4: Provider/Model Routing

**Location:** `crates/agent/src/` (registry), `crates/tui/src/core/engine.rs` (resolution)

Models are resolved through:
1. The provider (e.g., `deepseek`, `anthropic`, `openrouter`)
2. A model ID (e.g., `deepseek-v4-pro`, `claude-sonnet-4-20250514`)
3. Model strength (`same` vs `faster`) — for sub-agents, `faster` maps known families (DeepSeek V4 Pro → Flash, GLM-5.2 → Turbo)
4. Thinking budget (`inherit`, `off`, `low`, `medium`, `high`, `max`)

The `ModelRoute` enum (`crates/tui/src/worker_profile.rs:114-124`) captures the routing decision:
```rust
pub enum ModelRoute { Inherit, Faster, Auto, Fixed(String) }
```

#### Pattern 5: Configuration Layer

**Location:** `crates/config/src/lib.rs`

Config is loaded as TOML, deserialized into `ConfigToml`, then merged with CLI overrides and env vars. Crucial for this work: `EngineConfig` (runtime, not serialized) holds `allowed_tools: Option<Vec<String>>` and `disallowed_tools: Option<Vec<String>>` — these are set from CLI args (`--allowed-tools`, `--disallowed-tools`) or from Fleet profiles.

`FleetExecConfig` (`crates/config/src/lib.rs:1050`) is the serialized config for headless workers. It has `allowed_tools: Vec<String>` and `disallowed_tools: Vec<String>` that flow to CLI args.

#### Pattern 6: Worker Runtime Profiles (The New Contract)

**Location:** `crates/tui/src/worker_profile.rs` (363 lines)

Introduced in PRs #3217/#3211/#3213 for Whaleflow, `WorkerRuntimeProfile` is the capability contract that every detached worker operates under:

```rust
pub struct WorkerRuntimeProfile {
    pub role: SubAgentType,
    pub permissions: PermissionSet,    // { write, network }
    pub shell: ShellPolicy,            // None / ReadOnly / Full
    pub tools: ToolScope,              // Inherit / Explicit(Vec<String>)
    pub model: ModelRoute,
    pub provider: Option<String>,
    pub max_spawn_depth: u32,
    pub background: bool,
}
```

The critical method is `derive_child()` (line 191) which computes the **intersection** of parent and child profiles — a child can never escalate beyond its parent. **Note:** `ToolScope` currently only has `Inherit` and `Explicit(allowlist)` — there is **no deny-list representation**. Phase 2 of the plan proposes adding `denied_tools: Vec<String>` as a separate field.

---

## Section 4: Sub-Agent Tool Scoping Deep Dive

### 4.1 Sub-Agent Spawn Flow (The Full Trace)

Here's the complete path from "model calls `agent` tool" to "child has its tool registry." All line numbers refer to `crates/tui/src/tools/subagent/mod.rs` unless stated otherwise.

```
User/Model calls agent(action="start", prompt="...")
    │
    ▼
AgentTool::execute()                                    [line ~3540]
    │  Parses the JSON input, determines action
    │  For action="start":
    ▼
spawn_subagent_from_input(input, manager, runtime)      [line 3852]
    │  1. parse_spawn_request(&input) → SpawnRequest     [line 5543]
    │     └ Parses: type, role, allowed_tools ✅
    │     └ Does NOT parse: disallowed_tools ❌          [GAP]
    │
    │  2. runtime.background_runtime() → child_runtime   [line 1702/1685]
    │     └ Clones SubAgentRuntime, increments depth
    │     └ Does NOT carry disallowed_tools ❌            [GAP]
    │
    │  3. Model resolution (configured_model_for_role_or_type)
    │
    │  4. manager_guard.spawn_background_with_assignment_options()
    ▼
spawn_background_with_assignment_options(...)            [line 2681]
    │  1. build_allowed_tools(agent_type, allowed_tools, ...)
    │     └ Returns None (inherit all) or Some(allowlist)
    │     └ No disallowed_tools input ❌                 [GAP]
    │
    │  2. SubAgent::new(...) — stores allowed_tools on the agent struct
    │
    │  3. SubAgentTask { allowed_tools: tools, ... } — stored for execution
    │
    │  4. spawn_supervised("subagent-task", run_subagent_task(task))
    ▼
run_subagent_task(task)                                  [line 4100]
    │  Calls run_subagent(...)
    ▼
run_subagent(...)                                         [line 4741]
    │  1. Builds SubAgentToolRegistry::new_with_owner()
    ▼
SubAgentToolRegistry::new_with_owner(...)                [line 6577]
    │  1. ToolRegistryBuilder::new()
    │     .with_full_agent_surface_options(...)  ← builds FULL parent surface
    │     .with_mcp_tools(Arc::clone(pool))      ← clones ENTIRE MCP pool
    │     .build(context)
    │
    │  2. Stores allowed_tools filter (Option<Vec<String>>)
    │     └ No disallowed_tools field ❌                 [GAP]
    │
    │  3. The child's SubAgentToolRegistry now operates:
    │     - is_tool_allowed(name) → checks allowlist only
    │     - tools_for_model() → filters model catalog by allowlist + posture
    │     - execute(name, input) → checks allowlist + posture + approval
    │     └ None of these check a deny list ❌            [GAP]
```

**The Gap Summary:** At **five** distinct points in the spawn flow, `disallowed_tools` is absent:
1. `SpawnRequest` struct (line 1247) — no field
2. `parse_spawn_request()` (line 5543) — doesn't extract it from JSON
3. `SubAgentRuntime` (line 1440) — no field, no `with_disallowed_tools()` builder
4. `build_allowed_tools()` (line 6791) — no parameter
5. `SubAgentToolRegistry` (line 6538) — no field, no `is_tool_denied()` method

### 4.2 Tool Registry and Gate Chain

**Parent side (works correctly):**
```
EngineConfig.disallowed_tools                           [engine.rs:356]
    │
    ├─→ filter_tool_catalog_for_gates()                  [engine.rs:3906]
    │   └ Filters model-visible catalog: deny wins
    │
    └─→ command_denies_tool()                            [turn_loop.rs:1608]
        └ Blocks at execution time: deny wins
```

**Fleet headless workers (works correctly):**
```
FleetExecConfig.disallowed_tools                         [config/src/lib.rs:1055]
    │
    └─→ build_worker_exec_command_from_prompt()          [fleet/executor.rs:94-97]
        └ Serialized to --disallowed-tools CLI arg
            │
            └─→ Worker picks up into its EngineConfig     [verified]
```

**Sub-agent side (broken):**
```
EngineConfig.disallowed_tools                             [engine.rs:356]
    │
    └─→ ???  [NO CONNECTION]  ???                         [GAP]
        └ SubAgentRuntime has no field
        └ SubAgentToolRegistry has no field
        └ Child tools_for_model() includes denied tools
        └ Child execute() allows denied tools
```

### 4.3 Custom Role and allowed_tools

**`SubAgentType` enum** (`subagent/mod.rs:367-389`):
Seven variants: `General` (default), `Explore`, `Plan`, `Review`, `Implementer`, `Verifier`, `Custom`.

**`Custom` is special:** It starts locked down — `WorkerRuntimeProfile::for_role(Custom)` returns `PermissionSet::read_only()` + `ShellPolicy::None` (worker_profile.rs:164). The caller must provide an explicit `allowed_tools` list.

**`build_allowed_tools()`** (line 6791):
- Non-Custom + no explicit list → `None` (full parent inheritance)
- Non-Custom + explicit list → `Some(deduped_list)`
- Custom + non-empty list → `Some(deduped_list)`
- Custom + None or empty → **error** ("Custom sub-agent requires a non-empty allowed_tools list")

**Per-type deprecated lists** (line 437-446): The old `SubAgentType::allowed_tools()` method that returned per-type static lists (e.g., `Explore` got `[list_dir, read_file, grep_files, ...]`) is `#[deprecated since = "0.6.6"]`. Children now inherit the full parent registry.

**`allowed_tools` on the model-facing `agent` tool:**
The `agent` tool's JSON schema already accepts `allowed_tools` (array of strings). This is parsed in `parse_spawn_request()` at line 5602. It flows into `build_allowed_tools()` and ultimately into `SubAgentToolRegistry.allowed_tools`.

**What's missing:** There's **no** `disallowed_tools` field on the `agent` tool schema, no parsing for it, and no mechanism to express a deny list at the spawn call site. The `Custom` role requires an allowlist but cannot express a deny list.

### 4.4 MCP Server Registration and Tool Exposure

**MCP tool naming convention:** `mcp_{server}_{tool}` (confirmed: `crates/tui/src/mcp.rs:1696`).

**`McpPool` struct** (`crates/tui/src/mcp.rs:1454`):
```rust
pub struct McpPool {
    connections: HashMap<String, McpConnection>,
    config: McpConfig,
    // ...
}
```

**`McpPool::all_tools()`** (line 1688):
```rust
pub fn all_tools(&self) -> Vec<(String, &McpTool)> {
    for (server, conn) in &self.connections {
        for tool in conn.tools() {
            if !conn.config().is_tool_enabled(&tool.name) { continue; }
            tools.push((format!("mcp_{}_{}", server, tool.name), tool));
        }
    }
}
```

Each MCP server connection has its own `enabled_tools` / `disabled_tools` lists (per-server config in `mcp.json`), checked via `is_tool_enabled()` (line 367). This is **per-server per-tool**, not cross-server.

**How MCP tools reach sub-agents:**
1. Engine creates `McpPool` once, wraps in `Arc<Mutex<McpPool>>`
2. Passed to `SubAgentRuntime::with_mcp_pool(pool)` → stored on runtime
3. `SubAgentToolRegistry::new_with_owner()` clones the Arc and calls `with_mcp_tools(Arc::clone(pool))` (line 6604-6606)
4. `ToolRegistryBuilder::with_mcp_tools()` snapshots `pool.all_tools()` and wraps each as `McpToolAdapter`

**The MCP scoping gap:**
- The **entire** MCP pool is cloned into every sub-agent
- There is **no server-level filtering** — all connected MCP servers are exposed to all sub-agents
- There is **no `McpPool::filtered_view(&["server1", "server2"])`** method
- Phase 3 proposes adding `mcp_servers: Vec<String>` to scope which servers are visible

### 4.5 Fleet Worker Integration

**`FleetExecConfig`** (`crates/config/src/lib.rs:1050`):
```rust
pub struct FleetExecConfig {
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub max_turns: u32,
    pub max_spawn_depth: u32,
    pub append_system_prompt: String,
    pub output_format: String,
}
```

**How it reaches workers:**
1. `FleetExecConfig` serialized to `--disallowed-tools` CLI arg in `fleet/executor.rs:94-97`
2. Worker process picks it up into its own `EngineConfig.disallowed_tools`
3. **Note:** The worker's `allow_shell` is set to `false` by default and is **not** derived from `FleetExecConfig`

**`FleetProfile`** (line 1109): has `slot`, `role`, `loadout`, `model`, `permissions` (allow_shell, trust, approval_required), `delegation` hints. **No tool-scoping fields** beyond `allow_shell`.

**`FleetRolePreset`** (line 1384): has `tool_profile`, `tools`, `capabilities`. **No disallowed_tools field.**

**The Fleet-to-sub-agent gap:**
A Fleet worker runs as `codewhale exec` — it gets its own `EngineConfig` with `disallowed_tools`. If that worker spawns an in-process sub-agent (via the `agent` tool), the deny list is **still lost** — same Phase 1 gap. The Phase 1 fix (threading `EngineConfig.disallowed_tools` through `SubAgentRuntime`) closes this for ALL in-process children, whether the parent is interactive or a Fleet worker.

---

## Section 5: Implementation Considerations

### Phase 1: Fix Sub-Agent Inheritance (Immediate)

**Problem:** `--disallowed-tools exec_shell,write_file` on the parent → sub-agents see all tools. The parent's deny list is silently dropped.

**Files that need changes (from the plan):**
| File | Change |
|---|---|
| `crates/tui/src/tools/subagent/mod.rs` | `SpawnRequest` + `disallowed_tools`/`inherit_disallowed_tools` fields; `SubAgentToolRegistry` + `disallowed_tools` field + `is_tool_denied()`; update `is_tool_allowed()` + `tools_for_model()` + `execute()` |
| `crates/tui/src/core/engine.rs` | Thread `self.config.disallowed_tools` into `SubAgentRuntime` via new `with_disallowed_tools()` builder |
| `crates/tui/src/tools/subagent/tests.rs` | Tests for deny inheritance, deny-wins-over-allow, wildcard, opt-out flag |

**Approach A: Thread `disallowed_tools` through SubAgentRuntime (recommended by the plan)**

This follows the existing pattern — `SubAgentRuntime` already carries `allow_shell`, `agent_tool_surface_options`, `mcp_pool`, etc. via builder methods. Adding `disallowed_tools` is consistent.

1. Add `disallowed_tools: Option<Vec<String>>` to `SubAgentRuntime` (line 1440)
2. Add `with_disallowed_tools(mut self, tools: Option<Vec<String>>) -> Self` builder
3. In `child_runtime()` (line 1702), clone the field: `disallowed_tools: self.disallowed_tools.clone()`
4. In engine construction (lines 1451, 2532), add `.with_disallowed_tools(self.config.disallowed_tools.clone())`
5. Add `disallowed_tools: Option<Vec<String>>` to `SpawnRequest` (line 1247)
6. Add `inherit_disallowed_tools: bool` (default `true`) to `SpawnRequest`
7. In `parse_spawn_request()` (line 5543), parse `disallowed_tools` from JSON input
8. Add `disallowed_tools: Vec<String>` to `SubAgentToolRegistry` (line 6538)
9. Add `is_tool_denied(&self, name: &str) -> bool` — exact + `*` wildcard matching (reuse `command_denies_tool` logic)
10. Modify `is_tool_allowed()` (line 6662): check deny list FIRST, then allow list ("deny wins")
11. Modify `tools_for_model()` (line 6672): filter out denied tools from API catalog
12. Modify `execute()` (line 6703): check deny list before execution

**Rust-specific considerations for Phase 1:**
- `disallowed_tools` should be `Vec<String>` on `SubAgentToolRegistry` (owned, not borrowed) because the registry outlives the runtime
- `command_denies_tool()` is in `crates/tui/src/core/engine/turn_loop.rs` — either move it to a shared utility module or duplicate the logic in the subagent module
- The `is_tool_denied` check at line 6662 (where `is_tool_allowed` is called) is synchronous — no async complications
- `tools_for_model()` builds the model-visible catalog; filtering there ensures the model never even sees denied tools (defense in depth)

**Pros of this approach:**
- Consistent with existing patterns (builder, field threading)
- Follows principle of least surprise: deny lists are a security boundary
- The `inherit_disallowed_tools: false` flag preserves opt-out flexibility

**Risks:**
- If `command_denies_tool` logic is duplicated instead of shared, future wildcard enhancements could drift
- `parse_spawn_request` already handles many optional fields (line 5543-5696); adding more increases the parsing surface

---

### Phase 2: Per-Sub-Agent Tool Scoping

**Problem (builds on Phase 1):** The `agent` tool only supports `allowed_tools` for `Custom` roles. There's no caller-facing `disallowed_tools` parameter, and no way to attach tool restrictions to Fleet profiles.

**Files that need changes:**
| File | Change |
|---|---|
| `crates/tui/src/tools/subagent/mod.rs` | `SpawnRequest` already gets `disallowed_tools` in Phase 1 — Phase 2 adds parsing from the `agent` tool schema |
| `crates/tui/src/tools/subagent/mod.rs` | `agent` tool schema — add `disallowed_tools` to the JSON input schema |
| `crates/tui/src/worker_profile.rs` | Add `denied_tools: Vec<String>` to `WorkerRuntimeProfile` |
| `crates/config/src/lib.rs` | Add `allowed_tools`/`disallowed_tools` to `FleetProfile` and `FleetRolePreset` |

**Key design decisions:**

**Q1: `allowed_tools` vs `disallowed_tools` vs both?**

The recommendation is **both**: `allowed_tools` for positive scoping (narrow roles like "you can only use read_file and grep_files"), `disallowed_tools` for negative scoping ("you inherit everything except exec_shell"). They're orthogonal. The `Custom` role already requires an allowlist; Phase 2 extends other roles to optionally accept either or both.

**Q2: How to integrate with `WorkerRuntimeProfile`?**

The plan says: add a **separate** `denied_tools: Vec<String>` field to `WorkerRuntimeProfile` rather than overloading the `ToolScope` enum. This is cleaner because:
- Deny semantics (union of parent + child) are different from allow semantics (intersection)
- `derive_child()` already handles allowlist intersection — a separate field for deny lists makes the merge logic obvious
- Backward compatible: existing profiles without the field default to empty deny list

```rust
// Proposed addition to worker_profile.rs:
pub struct WorkerRuntimeProfile {
    // ... existing fields ...
    pub denied_tools: Vec<String>,  // NEW
}
```

**Configuration surface:**

For Fleet profiles, the user sets them in `config.toml`:
```toml
[fleet.profiles.code-reviewer]
role = "reviewer"
allowed_tools = ["read_file", "grep_files", "file_search"]
disallowed_tools = ["exec_shell"]

[fleet.profiles.worker]
role = "builder"
disallowed_tools = ["mcp_production_*"]
```

For the `agent` tool call, the model passes:
```json
{
  "action": "start",
  "type": "general",
  "prompt": "...",
  "disallowed_tools": ["exec_shell", "mcp_database_*"]
}
```

**Rust-specific considerations for Phase 2:**
- Adding fields to `WorkerRuntimeProfile` is straightforward — it's `Clone + Serialize + Deserialize`
- `derive_child()` union semantics for deny lists: `child_denied = parent.denied_tools ∪ child.denied_tools` (both apply)
- The `WorkerRuntimeProfile` is already threaded through the spawn flow (line 2773 in `spawn_background_with_assignment_options`) — no new threading needed

---

### Phase 3: Per-Sub-Agent MCP Server Assignment

**Problem (builds on Phase 1 & 2):** Every sub-agent gets the entire MCP pool. There's no way to say "this sub-agent can use the `github` MCP server but not `production_database`."

**This is a separate concern from tool deny-lists.** Phase 1's `disallowed_tools: ["mcp_production_*"]` already works as coarse MCP denial through the naming convention. Phase 3 adds **positive server-level allowlisting** as a dedicated mechanism.

**Files that need changes:**
| File | Change |
|---|---|
| `crates/tui/src/tools/subagent/mod.rs` | `SpawnRequest` + `mcp_servers` field; `SubAgentToolRegistry` — scoped MCP registration |
| `crates/tui/src/mcp.rs` | `McpPool::filtered_view(&[String])` — returns a subset of connections |
| `crates/tui/src/tools/registry.rs` | `with_mcp_tools_scoped()` variant that accepts a filtered pool view |
| `crates/config/src/lib.rs` | Add `mcp_servers` to `FleetProfile` |

**Approach options:**

**Option A: ScopedMcpPool — create a filtered wrapper**

```rust
// New type in mcp.rs
pub struct ScopedMcpPool {
    pool: Arc<Mutex<McpPool>>,
    allowed_servers: Vec<String>,
}

impl ScopedMcpPool {
    pub fn all_tools(&self) -> Vec<(String, &McpTool)> {
        // Only iterates connections in allowed_servers
    }
}
```

Pro: Clean separation of concerns. Con: New type to maintain, and the locked pool snapshot pattern needs to be replicated.

**Option B: McpPool::filtered_view() — add a method to McpPool**

```rust
impl McpPool {
    pub fn filtered_view(&self, servers: &[String]) -> Vec<(String, &McpTool)> {
        self.connections
            .iter()
            .filter(|(name, _)| servers.contains(name))
            .flat_map(|(server, conn)| {
                conn.tools().iter()
                    .filter(|tool| conn.config().is_tool_enabled(&tool.name))
                    .map(|tool| (format!("mcp_{}_{}", server, tool.name), tool))
            })
            .collect()
    }
}
```

Pro: Simpler, no new type. Con: Callers must remember to use `filtered_view` instead of `all_tools`; the pool itself has no "current scope" concept.

**Integration with the spawn flow:**
1. `SpawnRequest` gains `mcp_servers: Option<Vec<String>>`
2. In `SubAgentToolRegistry::new_with_owner()`, instead of `pool.all_tools()`, call `pool.filtered_view(&mcp_servers)` when `mcp_servers` is `Some`
3. If `mcp_servers` is `None`, inherit the full pool (current behavior)

**Rust-specific considerations for Phase 3:**
- `McpPool` is behind `Arc<Mutex<McpPool>>` — snapshotting for `filtered_view` needs the lock (same as current `all_tools`)
- The lifecycle: MCP tools are snapshot at registry build time, not dynamically re-scanned — consistent with current behavior
- Error handling: if `mcp_servers` references a server that doesn't exist, it should be a warning, not a hard error (servers may be temporarily disconnected)

---

## Section 6: Open Questions and Recommendations

### 6.1 Open Design Questions (from the plan)

**Q1: Should sub-agents inherit `--disallowed-tools` by default or opt-in?**

**Recommendation: Inherit by default, with opt-out.** The parent's deny list is a security boundary — escaping it via sub-agent spawn is privilege escalation. The `WorkerRuntimeProfile::derive_child()` precedent already establishes capability intersection as the safe default.

The `inherit_disallowed_tools: false` flag on the `agent` call allows a parent to explicitly spawn a child with a clean slate (e.g., a sandboxed worker with its own policy). The flag only controls the *parent runtime's* deny list — any explicit `disallowed_tools` on the `agent` call itself always applies.

**Q2: Should MCP scoping be part of custom role or a separate mechanism?**

**Recommendation: Separate mechanism.** The `mcp_servers` field operates on server names; `allowed_tools`/`disallowed_tools` operate on tool names. Mixing them creates ambiguity: does `allowed_tools: ["mcp_github_*"]` mean "allow the github server" or "allow the literal tool named `mcp_github_*`"? Phase 3's `mcp_servers` is the positive allowlist for servers; Phase 1's `disallowed_tools: ["mcp_database_*"]` is the negative path through the naming convention. Both are useful and orthogonal.

**Q3: How should Fleet workers interact with sub-agent tool restrictions?**

Three separate sub-concerns, none of which are MCP-specific:
1. **Headless Fleet CLI path** (already works): `FleetExecConfig.disallowed_tools` → `--disallowed-tools` CLI arg → worker's `EngineConfig`
2. **In-process sub-agents from Fleet workers** (Phase 1 fix): The worker's `EngineConfig.disallowed_tools` threads into `SubAgentRuntime` — same Phase 1 inheritance chain. Covers ALL tools.
3. **Fleet profile resolution** (Phase 2): `FleetProfile.allowed_tools`/`disallowed_tools` resolve into `FleetExecConfig`, feeding the headless CLI path. Covers ALL tools.

**Q4: `ToolScope` representation?**

**Recommendation: Add separate `denied_tools: Vec<String>` to `WorkerRuntimeProfile`** rather than overloading the `ToolScope` enum. This keeps allow/deny semantics clear (deny always wins), is backward-compatible, and the `derive_child()` union semantics (parent deny + child deny both apply) are simpler than nested enum variants.

### 6.2 Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Phase 1 breaks existing sub-agent behavior | Low | High | Comprehensive tests + backward-compatible defaults |
| Wildcard matching diverges between parent and child | Medium | Medium | Share `command_denies_tool` logic between `turn_loop.rs` and subagent module |
| Phase 2 `inherited` Fleet profiles work differently than direct profiles | Medium | Medium | Clear documentation; the `ToolScope::Inherited` → cannot filter comment in `filter_tool_profile()` already flags this |
| Phase 3 scoped MCP pool breaks existing MCP tool registration | Low | Medium | Keep `None` mcp_servers → full pool as the default |

### 6.3 Testing Strategy Summary

**Phase 1 tests (minimum):**
- Parent with `--disallowed-tools exec_shell` → child `tools_for_model()` excludes `exec_shell`
- Parent disallow + child explicit allow → deny wins
- Wildcard deny `mcp_*` → all MCP tools excluded from child catalog
- `inherit_disallowed_tools: false` → parent deny list not applied, but caller deny list is
- Confirm existing tests pass unchanged

**Phase 2 tests:**
- `agent` call with `disallowed_tools` → child's `is_tool_allowed` rejects denied tools
- `FleetProfile` with `allowed_tools` + `disallowed_tools` → `FleetExecConfig` has both
- `derive_child()` with parent `denied_tools` + child `denied_tools` → union (both apply)

**Phase 3 tests:**
- Sub-agent with `mcp_servers: ["github"]` → only github MCP tools visible
- Sub-agent with `mcp_servers: []` → no MCP tools visible
- Sub-agent without `mcp_servers` → full pool (backward compatible)

### 6.4 PR Structure Recommendation

The plan explicitly states these are separate workstreams that must ship as separate commits/PRs:

```
PR 1: Phase 1 — Thread disallowed_tools through SubAgentRuntime → SubAgentToolRegistry
PR 2: Phase 2 — Per-sub-agent allowed_tools/disallowed_tools on agent call + Fleet profiles
PR 3: Phase 3 — mcp_servers field for per-server MCP allowlisting
```

Each PR should be: rebased onto `main`, focused on one behavior boundary, backed by tests.

---

## Quick Reference: Key Files

| File | What's There |
|---|---|
| `crates/tui/src/tools/subagent/mod.rs` | **The sub-agent system:** `SubAgentType`, `SubAgentRuntime`, `SubAgentToolRegistry`, `parse_spawn_request`, `spawn_subagent_from_input`, `spawn_background_with_assignment_options`, `build_allowed_tools`, `run_subagent` |
| `crates/tui/src/core/engine.rs` | **The engine:** `EngineConfig` (with `disallowed_tools`), `filter_tool_catalog_for_gates`, SubAgentRuntime construction sites |
| `crates/tui/src/core/engine/turn_loop.rs` | **The turn loop:** `command_denies_tool`, `command_allows_tool`, tool execution gating |
| `crates/tui/src/worker_profile.rs` | **Worker capability contracts:** `WorkerRuntimeProfile`, `derive_child()`, `ToolScope`, `ShellPolicy`, `PermissionSet` |
| `crates/tui/src/tools/registry.rs` | **Tool registry builder:** `ToolRegistryBuilder`, `with_mcp_tools`, `with_full_agent_surface_options` |
| `crates/tui/src/mcp.rs` | **MCP integration:** `McpPool`, `all_tools()`, `is_tool_enabled()`, MCP naming convention |
| `crates/config/src/lib.rs` | **Config types:** `FleetExecConfig`, `FleetProfile`, `FleetRolePreset` |
| `crates/tui/src/fleet/worker_runtime.rs` | **Fleet hardening:** `apply_exec_hardening()`, `filter_tool_profile()` |
| `crates/tui/src/fleet/executor.rs` | **Fleet execution:** `build_worker_exec_command_from_prompt()` — serializes to CLI args |
| `crates/tools/src/lib.rs` | **Lean tool types:** `ToolSpec`, `ToolError`, `ApprovalRequirement` |

---

*This guide was compiled from live code on the `main` branch, workspace version 0.8.66, plus the existing `SUBAGENT_TOOL_SCOPING_PLAN.md` (2026-07-07). All line numbers are accurate as of this writing but may shift with future changes.*
