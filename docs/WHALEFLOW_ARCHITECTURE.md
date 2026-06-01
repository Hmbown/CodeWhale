# WhaleFlow Architecture

> Declarative multi-agent workflow orchestration for CodeWhale.
> Inspired by Claude Code's Dynamic Workflows (May 2026, Opus 4.8).

## Design

WhaleFlow is a **declarative JSON-config-driven workflow orchestrator**.
The DeepSeek model generates a `WorkflowConfig` describing phases, tasks,
and dependencies. The Rust scheduler executes the config: topologically
sorting phases, fanning out sub-agents with concurrency control, piping
results between dependent tasks, and returning an integrated structured
result.

This is **not** a JavaScript scripting runtime. Claude's Dynamic Workflows
use a Node `vm` sandbox where the model writes JS scripts calling `agent()`,
`parallel()`, `pipeline()`. WhaleFlow achieves the same capability through
a declarative config — the model writes JSON (DeepSeek V4's strongest
format) and the Rust runtime handles parallelism and dependency management.
This is simpler to build, easier to debug, and plays to DeepSeek's strengths.

## Crate: `crates/whaleflow`

Pure orchestration logic. No TUI, network, or filesystem dependencies.
Depends only on `serde`, `tokio`, and CodeWhale workspace libraries.

### Module map

| Module | Purpose |
|--------|---------|
| `config.rs` | `WorkflowConfig`, `Phase`, `Task`, `FailurePolicy` structs + JSON Schema + validation |
| `spawner.rs` | `AgentSpawner` trait — abstract interface for spawning sub-agents |
| `scheduler.rs` | `Scheduler` — topological sort, concurrency, result plumbing, failure handling |
| `result.rs` | `WorkflowResult`, `TaskResult` — structured output returned to the model |
| `lib.rs` | Public API re-exports |

### AgentSpawner trait (the seam)

```rust
#[async_trait]
pub trait AgentSpawner: Send + Sync {
    async fn spawn(
        &self,
        task_id: String,
        prompt: String,
        agent_type: Option<String>,
    ) -> Result<AgentResult, SpawnError>;
}
```

The `whaleflow` crate never spawns agents directly. The embedding
application (CodeWhale TUI crate) implements `AgentSpawner` using the
existing `SubAgentManager` / `SubAgentRuntime` infrastructure. This keeps
`whaleflow` decoupled from the TUI stack (ratatui, crossterm, etc.).

### Workflow config shape

```json
{
  "goal": "Security audit and remediation",
  "max_concurrent": 6,
  "phases": [
    {
      "name": "discovery",
      "parallel": true,
      "on_failure": "skip_continue",
      "tasks": [
        {
          "id": "scan-auth",
          "prompt": "Audit src/auth/ for vulnerabilities...",
          "agent_type": "review"
        }
      ]
    },
    {
      "name": "triage",
      "depends_on": ["discovery"],
      "parallel": false,
      "tasks": [
        {
          "id": "rank-findings",
          "prompt": "Rank all findings by severity...",
          "depends_on_results": ["scan-auth", "scan-api"]
        }
      ]
    }
  ]
}
```

### Execution flow

1. Model generates `WorkflowConfig` JSON
2. Model calls `workflow_run` tool with the config
3. Scheduler validates config (unique IDs, no cycles, valid deps)
4. Scheduler topologically sorts phases by `depends_on`
5. For each phase:
   - If `parallel`: fan out all tasks, limited by `max_concurrent` semaphore
   - If sequential: run tasks one at a time
   - On failure: skip-continue (default) or abort (per-phase policy)
6. Results from completed tasks are injected into dependent tasks' prompts
7. Structured `WorkflowResult` returned to the model

### Failure handling

| Policy | Behavior |
|--------|----------|
| `skip_continue` (default) | Failed tasks are marked failed. Remaining tasks continue. Downstream tasks depending on failed results get skipped. |
| `abort` | First failure stops the entire workflow immediately. |

### Concurrency model

- `max_concurrent` in config controls the semaphore (default 6, max TBD)
- Applies globally across all phases
- Within a parallel phase, all tasks are spawned and acquire permits
- Sequential phases hold one permit while executing each task

## Integration point: `crates/tui`

### AgentSpawner implementation

`WhaleFlowSpawner` implements `AgentSpawner` using the existing
`SubAgentManager` / `SubAgentRuntime`. For `isolation: "worktree"` tasks:

1. Creates worktree via `WorktreeManager::create()`
2. Passes `cwd` to `agent_open` so the child runs in the isolated checkout
3. After completion, extracts patch via `WorktreeManager::extract_changes()`
4. Applies patch to main workspace via `WorktreeManager::apply_patch()`
5. Cleans up via `WorktreeManager::remove()`

### TUI surfaces

**Side panel (agents pane):**
- whaleFlow agents appear under a "🐋 Swarm" group header
- Shows: swarm goal, current phase, agent count, overall progress
- Per-agent: task ID, status icon (⏳/✓/✗), last completed checkpoint
- Global stats: total tokens, cost (USD/CNY), elapsed time
- Non-whaleFlow agents appear below, ungrouped

**Pop-up dashboard (`/whaleflow dashboard` or `Ctrl+W`):**
- Toggleable overlay — floats above conversation, does not block input
- Top: workflow goal + global progress bar (N/M tasks complete)
- Per-phase progress bars with ✓/⏳/○ status
- Middle table: agent ID | task | status/progress | tokens | cost | elapsed
- Bottom: total cost, tokens, time across all agents
- Expand agent row to see tool calls and outputs

**Commands:**
- `/workflow on/off` — toggle feature (off = suppress auto-detection, hide tool)
- `/whaleflow dashboard` or `Ctrl+W` — open/close the dashboard overlay

### Per-agent cost tracking

`AgentResult` extended with `tokens_used` and `cost_usd` fields.
Populated from `SubAgentSessionProjection` returned by `agent_eval`.
Aggregated in the scheduler for the global stats panel.

### Progress model

Agents report checkpoints (not live tool calls). Each checkpoint:
- Phase name, task ID, status (running/completed/failed)
- Last completed tool call name + file path
- Step count

The scheduler aggregates checkpoints into phase-level and workflow-level
progress for the TUI to render.

## The name "WhaleFlow"

Whale = CodeWhale. Flow = workflow. Also a nod to "pod" — a group of
whales working together, like a swarm of sub-agents.

## Splitting back into existing crates

If the CodeWhale maintainer prefers not to add a new crate, the modules
map cleanly into the existing structure:

| Module | Destination |
|--------|------------|
| `config.rs`, `result.rs` | `crates/tools/src/workflow/` (schema + tool definition) |
| `spawner.rs` | `crates/tools/src/workflow/` (or a trait in `crates/agent`) |
| `scheduler.rs` | `crates/agent/src/workflow/` (orchestration logic) |
| TUI integration | `crates/tui/src/tools/workflow.rs` (already the plan) |

The crate boundary is thin enough that extraction or inlining is a
mechanical refactor — no logic changes required.

## Prior art

- **Claude Dynamic Workflows** (May 2026): JS scripts in a Node `vm`
  sandbox. Model writes `agent()`, `parallel()`, `pipeline()` calls.
  Up to 16 concurrent, 1000 total agents. Resumable runs.
- **pi-dynamic-workflows** (Michaelliv, May 2026): TypeScript clone
  for Pi. Same API surface. AST-validated parser + sandbox.
- **codex-dynamic-workflows** (DannyMac180, May 2026): Skill-based
  approach for Codex. No programmable runner — simulates subagents
  with isolated packet notes.
