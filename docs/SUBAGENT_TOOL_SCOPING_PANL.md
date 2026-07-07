# Sub-Agent Tool Scoping: Phased Implementation Plan

- **Date:** 2026-07-07
- **Repo state:** `main`, workspace version 0.8.67
- **Author:** CodeWhale architecture analysis (deepseek-v4-pro, thinking=high)

## Separation of Concerns

The plan addresses two **independent** workstreams that must ship as separate
commits/PRs:

| Workstream | Phases | Scope | Mechanism |
|---|---|---|---|
| **Tool deny-list inheritance** | Phase 1, Phase 2 | **All tools** (native + MCP) | `disallowed_tools` field threaded through `SubAgentRuntime` → `SubAgentToolRegistry`; deny-list matching on tool *names* (including `mcp_*` wildcards) |
| **MCP server assignment** | Phase 3 | **MCP tools only** | `mcp_servers` field on `agent` tool + Fleet profiles; server-level positive allowlisting via a filtered pool view |

The boundary: MCP tool names follow the convention `mcp_{server}_{tool}`. Phase
1's wildcard deny (`disallowed_tools: ["mcp_database_*"]`) already provides
coarse per-server denial through the naming convention — that's a *side effect*
of the naming scheme, not an MCP-specific feature. Phase 3's `mcp_servers` field
is the dedicated, intentional mechanism for server-level scoping.

---

## Current State (Evidence Summary)

### 1. Sub-agent spawn flow — where tool registry inheritance happens

**Entry point:** `spawn_subagent_from_input()` at
`crates/tui/src/tools/subagent/mod.rs:3852`

The flow is:

1. `parse_spawn_request(input)` → `SpawnRequest` (line 5543), which extracts
   `allowed_tools: Option<Vec<String>>` (line 5602) but **never extracts a
   `disallowed_tools` field** — it does not exist on `SpawnRequest`.
2. `spawn_background_with_assignment_options()` (line 2681) calls
   `build_allowed_tools(agent_type, allowed_tools, allow_shell)` (line 2713).
3. `build_allowed_tools()` (line 6791) returns `None` for non-Custom types
   (full parent inheritance) or `Some(vec)` for explicit lists.
4. The result is passed into `SubAgentToolRegistry::new_with_owner()` (line
   6577), which builds the full child tool registry via
   `ToolRegistryBuilder::new().with_full_agent_surface_options(...)` — identical
   to the parent surface (minus `agent` if depth is exhausted).
5. MCP pool is inherited wholesale: `registry.with_mcp_tools(Arc::clone(pool))`
   (line 6604-6606).

**Gap:** `disallowed_tools` does not appear in any of `SpawnRequest`,
`build_allowed_tools()`, `SubAgentToolRegistry`, or the spawn pipeline. The
parent's deny list is silently dropped.

### 2. The `--disallowed-tools` gate chain

**Parent side** (works correctly):

- `EngineConfig.disallowed_tools: Option<Vec<String>>` —
  `crates/tui/src/core/engine.rs:356`
- Applied at turn-loop tool execution time:
  `command_denies_tool(self.config.disallowed_tools.as_deref(), &tool_name)` —
  `turn_loop.rs:1608`
- Applied to the model-visible catalog:
  `filter_tool_catalog_for_gates()` — `engine.rs:3906`
- `command_denies_tool()` at `turn_loop.rs:3045` supports exact match,
  case-insensitive, and `prefix*` wildcards.

**Fleet side** (works correctly for headless workers):

- `FleetExecConfig.disallowed_tools` serialized to `--disallowed-tools` CLI arg
  in `fleet/executor.rs:94-97`
- The worker process picks it up into its own `EngineConfig`.

**Sub-agent side** (broken): No path from `EngineConfig.disallowed_tools` into
the child `SubAgentRuntime` or `SubAgentToolRegistry`.

### 3. Custom role and `allowed_tools` mechanism

**`SubAgentType::Custom`** (line 387): requires an explicit `allowed_tools` array
from the caller. `build_allowed_tools()` (line 6791) rejects `Custom` without
one.

**Per-type allowlists deprecated** since v0.6.6 (line 439-446). The
`SubAgentType::allowed_tools()` method is `#[deprecated]` — children now inherit
the full parent registry.

**Runtime filtering** lives in `SubAgentToolRegistry`:

- `is_tool_allowed(name)` — line 6662: checks the `allowed_tools: Option<Vec<String>>` filter
- `tools_for_model()` — line 6672: filters the API catalog sent to the model
- `posture_permits_tool()` — line 6637: role-posture enforcement
- `execute()` — line 6703: combines allowlist + posture + approval gating

**No equivalent for `disallowed_tools`** exists anywhere in this chain.

### 4. MCP server registration and tool exposure

**MCP tool naming convention:** `mcp_{server}_{tool}` —
`crates/tui/src/mcp.rs:1696`

**`McpPool.all_tools()`** (line 1688) iterates all connected servers, filters by
per-tool enable flags, returns `Vec<(String, &McpTool)>` with server-prefixed
names.

**Sub-agent inheritance:** The entire pool is cloned (`Arc::clone(pool)`) and
registered via `registry.with_mcp_tools(pool)` at `subagent/mod.rs:6604-6606`.
There is **no server-level filtering** — every connected MCP server is exposed
to every sub-agent.

### 5. Fleet worker configuration and sub-agent interaction

**`FleetExecConfig`** (`config/src/lib.rs:1050`): has `allowed_tools`,
`disallowed_tools`, `max_turns`, `max_spawn_depth`, `append_system_prompt`,
`output_format`.

**`apply_exec_hardening()`** (`fleet/worker_runtime.rs:441`): applies
`FleetExecConfig` to an `AgentWorkerSpec` — caps steps, filters tool profile via
`filter_tool_profile()` (line 475), appends system prompt.

**Fleet profiles** (`FleetProfile`, line 1109): have `slot`, `role`, `loadout`,
`model`, `permissions` (allow_shell, trust, approval_required), `delegation`
hints. **No tool-scoping fields** except `allow_shell`.

**`FleetRolePreset`** (line 1384): has `tool_profile`
(read-only/read-write/custom), `tools` (explicit tool list), `capabilities`.
**No `disallowed_tools` field.**

**Fleet-to-sub-agent bridge:** Fleet workers run as `codewhale exec`
subprocesses with CLI args. The in-process sub-agent path (`agent` tool) has no
equivalent bridge to `FleetExecConfig`.

### 6. Worker profile infrastructure (already in place)

**`WorkerRuntimeProfile`** (`crates/tui/src/worker_profile.rs:128`): capability
contract with:

- `permissions: PermissionSet` (write + network booleans)
- `shell: ShellPolicy` (None / ReadOnly / Full)
- `tools: ToolScope` (Inherit / Explicit(Vec\<String\>))
- `model: ModelRoute`, `max_spawn_depth`, `background`

**`derive_child()`** (line 191): already computes parent × child intersection
for permissions, shell, tools, and depth — the non-escalation primitive. **But
`ToolScope` only has `Inherit` and `Explicit(allowlist)` — no deny-list
representation.**

**Integration note (v0.8.67):** `SubAgentRuntime` now carries
`worker_profile: WorkerRuntimeProfile` (line 1456), and
`SubAgentToolRegistry` reads it via `new_with_owner()` at line 6614 to
set `runtime_profile`. A deny-list added to `WorkerRuntimeProfile`
would automatically flow through the existing spawn pipeline without
a separate `with_disallowed_tools()` builder on `SubAgentRuntime`.

---

## Phase 1: Sub-agent `disallowed_tools` inheritance (Immediate)

### Problem

When `--disallowed-tools exec_shell,write_file` is set on the parent, sub-agents
receive the full tool registry with no filtering. The deny list is silently
dropped.

### Implementation approach

#### 1a. Add `disallowed_tools` to `SpawnRequest`

**File:** `crates/tui/src/tools/subagent/mod.rs`

```rust
struct SpawnRequest {
    // ... existing fields unchanged ...
    allowed_tools: Option<Vec<String>>,
    /// Tool deny-list from the parent runtime, merged with any caller request.
    /// Deny always wins over allow.
    disallowed_tools: Option<Vec<String>>,  // NEW
    /// When true (default), the child inherits the parent runtime's
    /// disallowed_tools. Set false to start with a clean slate.
    inherit_disallowed_tools: bool,         // NEW, default true
}
```

#### 1b. Plumb through the spawn pipeline

1. `spawn_subagent_from_input()` (line 3852) — pass
   `spawn_request.disallowed_tools` and `inherit_disallowed_tools` into
   `spawn_background_with_assignment_options()`
2. `spawn_background_with_assignment_options()` signature (line 2681) — add
   `disallowed_tools: Option<Vec<String>>` parameter
3. Pass into `SubAgentTask` (line 2813) and then into `run_subagent_task()`
4. Pass into `SubAgentToolRegistry::new_with_owner()` (line 6577)

#### 1c. Add deny-list filtering to `SubAgentToolRegistry`

Add `disallowed_tools: Vec<String>` field to the struct. Add method:

```rust
fn is_tool_denied(&self, name: &str) -> bool {
    let tool_name = name.to_ascii_lowercase();
    self.disallowed_tools.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        if let Some(prefix) = rule.strip_suffix('*') {
            tool_name.starts_with(prefix)
        } else {
            rule == tool_name
        }
    })
}
```

Modify `is_tool_allowed()` (line 6662) — check deny list first:

```rust
fn is_tool_allowed(&self, name: &str) -> bool {
    if name == "agent" && !self.can_spawn_child { return false; }
    if self.is_tool_denied(name) { return false; }        // NEW
    match &self.allowed_tools {
        None => true,
        Some(list) => list.iter().any(|t| t == name),
    }
}
```

Modify `tools_for_model()` (line 6672) — filter denied tools from the
model-visible catalog.

#### 1d. Thread `disallowed_tools` from parent engine into child runtime

**File:** `crates/tui/src/core/engine.rs`

Add `disallowed_tools: Option<Vec<String>>` to `SubAgentRuntime` with a builder:

```rust
pub fn with_disallowed_tools(mut self, tools: Option<Vec<String>>) -> Self {
    self.disallowed_tools = tools;
    self
}
```

At the two engine construction sites (lines 1451 and 2532), add:

```rust
.with_disallowed_tools(self.config.disallowed_tools.clone())
```

The `child_runtime()` and `background_runtime()` methods must propagate
`disallowed_tools`.

#### 1e. Merge runtime + caller deny lists

In `spawn_subagent_from_input()`, after building `SpawnRequest`, merge:

```rust
let effective_disallowed = merge_deny_lists(
    spawn_request.inherit_disallowed_tools.then(|| runtime.disallowed_tools.clone()).flatten(),
    spawn_request.disallowed_tools,
);
```

Union logic: parent runtime deny list AND caller deny list both apply. Neither
can remove the other's entries. The `inherit_disallowed_tools: false` flag
allows dropping only the *parent runtime's* list — explicit caller
`disallowed_tools` always applies.

### Files touched

| File | Change |
|------|--------|
| `crates/tui/src/tools/subagent/mod.rs` | `SpawnRequest` + fields, `SubAgentTask` + field, `SubAgentToolRegistry` + field + `is_tool_denied()`, updates to `is_tool_allowed()` and `tools_for_model()`, `spawn_background_with_assignment_options` signature, `parse_spawn_request` |
| `crates/tui/src/core/engine.rs` | Thread `self.config.disallowed_tools` into `SubAgentRuntime`; add `with_disallowed_tools()` builder |
| `crates/tui/src/tools/subagent/tests.rs` | Tests for deny-list inheritance, deny-wins-over-allow, wildcard matching, opt-out flag, `tools_for_model()` catalog filtering |

### Verification

- New test: parent with `--disallowed-tools exec_shell` → child `tools_for_model()` excludes `exec_shell`
- New test: parent disallow + child explicit allow → deny wins
- New test: wildcard deny `mcp_*` → all MCP tools excluded from child catalog
- New test: `inherit_disallowed_tools: false` → parent deny list not applied, but caller deny list is
- Confirm existing tests pass unchanged

---

## Phase 2: Per-sub-agent tool scoping (Builds on Phase 1)

### Problem

Currently, `allowed_tools` on the model-facing `agent` tool only affects
`Custom` roles. There's no caller-facing way to scope tools for non-Custom
roles, express a deny list at the spawn call site, or attach tool restrictions to
Fleet profiles.

### Implementation approach

#### 2a. Extend `agent` tool schema to accept `disallowed_tools`

Add `disallowed_tools` to the JSON schema of the `agent` tool and parse it in
`parse_spawn_request()` alongside `allowed_tools`. The model can express "spawn
an agent that can do anything except X."

Also parse `inherit_disallowed_tools: true/false`.

#### 2b. Allow `allowed_tools` for non-Custom roles

Change `build_allowed_tools()` logic:

- If `allowed_tools` is provided and non-empty → use it for **any** role type
  (not just `Custom`)
- If `allowed_tools` is `None` or empty for `Custom` → error as before
- If `allowed_tools` is `None` for non-`Custom` roles → full inheritance as
  before

#### 2c. Add tool scoping fields to Fleet profiles

**File:** `crates/config/src/lib.rs`

Add to `FleetProfile`:

```rust
pub struct FleetProfile {
    // ... existing fields unchanged ...
    /// Optional tool allowlist for workers using this profile.
    /// Applies to ALL tools — native and MCP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Optional tool deny-list for workers using this profile.
    /// Applies to ALL tools — native and MCP. Deny wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
}
```

Resolution path: `FleetProfile.allowed_tools`/`disallowed_tools` → merge into
`FleetExecConfig` → CLI args → worker's `EngineConfig`. This covers **all**
tools — MCP tool names like `mcp_github_*` are just strings in the list.

#### 2d. Add `denied_tools` to `WorkerRuntimeProfile`

**File:** `crates/tui/src/worker_profile.rs`

Add alongside existing `tools: ToolScope`:

```rust
pub struct WorkerRuntimeProfile {
    // ... existing fields unchanged ...
    /// Tools explicitly denied to this worker. Deny always wins over allow.
    /// Merged (union) during parent→child derivation.
    pub denied_tools: Vec<String>,
}
```

Update `derive_child()` to union deny lists (parent deny + child deny both
apply).

### Fleet workers and sub-agent tool restrictions — three separate concerns

| Concern | Status | Fixed by |
|---|---|---|
| Headless Fleet worker gets `--disallowed-tools` | Already works | `build_worker_exec_command()` passes CLI args → worker's own `EngineConfig` |
| In-process sub-agent spawned FROM a Fleet worker doesn't inherit | Broken | Phase 1: `EngineConfig.disallowed_tools` → `SubAgentRuntime` → child registry |
| Fleet profiles don't carry tool restrictions | Not implemented | Phase 2c: `FleetProfile.allowed_tools`/`disallowed_tools` → `FleetExecConfig` |

All three cover **all** tools — MCP tool names are just strings. The
MCP-specific `mcp_servers` field is Phase 3.

### Files touched

| File | Change |
|------|--------|
| `crates/tui/src/tools/subagent/mod.rs` | `parse_spawn_request` — parse `disallowed_tools`, `inherit_disallowed_tools`; `build_allowed_tools` — accept explicit lists for any role |
| `crates/config/src/lib.rs` | Add `allowed_tools`, `disallowed_tools` to `FleetProfile` |
| `crates/tui/src/worker_profile.rs` | Add `denied_tools: Vec<String>` to `WorkerRuntimeProfile`; update `derive_child()` |
| `crates/tui/src/fleet/worker_runtime.rs` | Resolve profile tool scoping into `FleetExecConfig` |

---

## Phase 3: Per-sub-agent MCP server assignment (Depends on Phase 1 + 2)

### Problem

All connected MCP servers are exposed to every sub-agent. There is no way to give
a sub-agent access to only the `github` MCP server but not `database`, or to
prevent a sub-agent from accessing any MCP tools at all.

Note: Phase 1's `disallowed_tools: ["mcp_database_*"]` already provides coarse
per-server denial through the naming convention. Phase 3 adds the dedicated,
intentional mechanism for positive server-level allowlisting.

### Implementation approach

#### 3a. Add `mcp_servers` field to the `agent` tool schema

```json
{
  "action": "start",
  "prompt": "...",
  "mcp_servers": ["github", "filesystem"]
}
```

Parse in `parse_spawn_request()` into `SpawnRequest`. `None` / absent = all
servers (current behavior). `Some(["github"])` = only the github server.

#### 3b. Create a filtered MCP pool wrapper

```rust
// crates/tui/src/mcp.rs (or subagent/mod.rs)
struct ScopedMcpPool {
    inner: Arc<Mutex<McpPool>>,
    allowed_servers: Option<Vec<String>>,  // None = all servers
}
```

`ScopedMcpPool::tools()` delegates to `inner.all_tools()` but filters by server
prefix, only returning tools from allowed servers.

#### 3c. Apply MCP scoping in `SubAgentToolRegistry::new_with_owner()`

Replace the wholesale clone with a scoped variant:

```rust
if let Some(pool) = runtime.mcp_pool.as_ref() {
    let scoped = ScopedMcpPool::new(Arc::clone(pool), spawn_request.mcp_servers);
    registry = registry.with_mcp_tools_scoped(scoped);
}
```

#### 3d. Add MCP server scoping to Fleet profiles

```rust
pub struct FleetProfile {
    // ...
    /// MCP servers available to workers using this profile.
    /// None/empty = all connected servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
}
```

### Files touched

| File | Change |
|------|--------|
| `crates/tui/src/tools/subagent/mod.rs` | `SpawnRequest` + `mcp_servers` field; `SubAgentToolRegistry` — scoped MCP registration; `parse_spawn_request` — parse `mcp_servers` |
| `crates/tui/src/mcp.rs` | `ScopedMcpPool` or `McpPool::filtered_view()` |
| `crates/tui/src/tools/registry.rs` | `with_mcp_tools_scoped()` variant that accepts a filtered view |
| `crates/config/src/lib.rs` | Add `mcp_servers` to `FleetProfile` |

---

## Open Design Questions

### 1. Should sub-agents inherit `--disallowed-tools` by default, or opt-in?

**Inherit by default, with an opt-out flag.** Rationale: the parent's deny list
is a security boundary — escaping it via sub-agent spawn is privilege
escalation. The `WorkerRuntimeProfile::derive_child()` precedent already
establishes capability intersection as the safe default.

The `inherit_disallowed_tools: false` flag on the `agent` call allows a parent
to explicitly spawn a child with a clean slate (e.g., a sandboxed worker with
its own policy). The flag only controls the *parent runtime's* deny list — any
explicit `disallowed_tools` on the `agent` call itself always applies.

### 2. Should MCP scoping be part of custom role or a separate mechanism?

**Separate mechanism.** The `mcp_servers` field operates on server names;
`allowed_tools`/`disallowed_tools` operate on tool names. Mixing them creates
ambiguity: does `allowed_tools: ["mcp_github_*"]` mean "allow the github server"
or "allow the literal tool named `mcp_github_*`"? Phase 3's `mcp_servers` is the
positive allowlist for servers; Phase 1's `disallowed_tools: ["mcp_database_*"]`
is the negative path through the naming convention. Both are useful and
orthogonal.

### 3. How should Fleet workers interact with sub-agent tool restrictions?

Three separate sub-concerns, none of which are MCP-specific:

- **Headless Fleet worker CLI path** (already works):
  `FleetExecConfig.disallowed_tools` → `--disallowed-tools` CLI arg → worker's
  `EngineConfig`. Covers **all** tools including MCP names.
- **In-process sub-agents spawned FROM Fleet workers** (Phase 1 fix): the
  worker's `EngineConfig.disallowed_tools` threads into `SubAgentRuntime`, then
  into child registries via Phase 1's inheritance chain. Covers **all** tools.
- **Fleet profile resolution** (Phase 2): `FleetProfile.allowed_tools` /
  `disallowed_tools` resolve into `FleetExecConfig`, feeding the headless CLI
  path. Covers **all** tools — not just MCP.

Phase 3 adds `FleetProfile.mcp_servers` for the MCP-specific positive
allowlist, but that's a separate concern and a separate commit.

### 4. `ToolScope` representation?

**Add a separate `denied_tools: Vec<String>` to `WorkerRuntimeProfile`** rather
than overloading the `ToolScope` enum. This keeps allow/deny semantics clear
(deny always wins), is backward-compatible, and the `derive_child()` union
semantics (parent deny + child deny both apply) are simpler than nested enum
variants.

---

## Summary

| Phase | Scope | Key deliverable | Ships with |
|---|---|---|---|
| 1 | **All tools** | `disallowed_tools` inherits to in-process sub-agents; deny-list enforcement in `SubAgentToolRegistry` | Phase 1 only |
| 2 | **All tools** | Per-sub-agent `allowed_tools`/`disallowed_tools` on `agent` call; Fleet profile tool fields; `WorkerRuntimeProfile.denied_tools` | Phase 2 only |
| 3 | **MCP only** | `mcp_servers` field for per-server positive allowlisting; `ScopedMcpPool`; Fleet profile `mcp_servers` | Phase 3 only |
