# 2026-05-18 Pi-Based Refactor Phase 1: Inventory And Testing Baseline

Status: Active draft
Branch: `codex-pi-based-refactor-specs`
Parent specs:

- `SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`
- `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`

## Goal

Start the GENmicon-TUI refactor without moving code yet. Establish the first
execution plan, decide what the current codebase contributes to the fresh
pi-based direction, and set a testing baseline that protects the parts likely
to survive.

This file is the working plan for Phase 1. It should be updated before Phase 2
kernel work begins.

## Operating Rules

- Do not start broad rewrites before this inventory is accepted.
- Do not delete inherited modules until replacement behavior compiles and has
  focused tests.
- Do not build a generic extension/plugin framework in the first kernel slice.
- Avoid defensive abstraction. Add seams only when they serve an immediate
  testable GENmicon need.
- Keep existing dirty maintainer edits untouched unless the task explicitly
  includes them.
- Treat pi as an architecture reference first. Do not import pi's trust model
  into player mode.

## Phase 1 Deliverables

- A module inventory with keep, slim, defer, or remove decisions.
- A test baseline for preserved game-console behavior.
- A minimal first implementation slice for Phase 2.
- Stop/go criteria for entering kernel scaffolding.

## Current Essential Anchors

These anchors are likely to survive or be ported:

| Surface | Current anchor | Initial decision | Reason |
| --- | --- | --- | --- |
| Game runtime | `crates/game/` | Keep | Pure runtime already owns manifests, saves, lookup, render, driver calls, and commits. |
| Game state authority | `crates/game/src/save.rs` | Keep | Protects `STATE.json` and `TURN_LOG.jsonl` as authoritative save truth. |
| Game fixtures | `examples/games/` | Keep | Provides reconciliation and serious-game cartridges for regression tests. |
| Game session model | `crates/tui/src/game.rs` | Keep but slim | Useful launch/session facade; tied to old TUI and may need extraction. |
| Game tools | `crates/tui/src/tools/game.rs` | Keep ABI, move later | `game_*` tool contracts are core GENmicon behavior. |
| Game prompt | `crates/tui/src/prompts/game_console.md` | Keep content, move later | Contains controller invariants for player turns. |
| Game console widget | `crates/tui/src/tui/widgets/game_console.rs` | Keep behavior target | Existing layout tests protect player presentation. |
| Tool profile tests | `crates/tui/src/core/engine/tests.rs` | Keep test intent | Verifies game tool allowlist and excludes generic tools. |
| TUI layout helpers | `crates/tui-core/` | Keep or port | Contains ratio/art/panel helpers useful for game console. |

## Heavy Surfaces To Classify Before Code Moves

These are not needed for the first GENmicon milestone unless a later spec says
otherwise:

| Surface | Current anchor | Initial decision | Notes |
| --- | --- | --- | --- |
| General coding-agent modes | `crates/tui/src/tui/app.rs`, `docs/MODES.md` | Defer/remove from player product | Developer controls may keep a small subset. |
| Broad shell/file/git tools | `crates/tui/src/tools/` | Defer/remove from player mode | Keep out of game-safe profile. |
| Runtime API/tasks/automations | `crates/tui/src/runtime_api.rs`, `task_manager.rs`, `automation_manager.rs` | Defer/remove | Not part of local game-console V1. |
| RLM/Python REPL | `crates/tui/src/tools/rlm.rs`, `rlm/`, `repl/` | Defer/remove | Not essential to game loop. |
| LSP diagnostics | `crates/tui/src/lsp/` | Defer/remove | Coding workflow feature. |
| Broad MCP surface | `crates/tui/src/mcp.rs`, `mcp_server.rs` | Defer/remove | Reintroduce only through constrained game resource needs. |
| Repository snapshots | `crates/tui/src/snapshot/` | Defer/remove | Game saves provide recovery. |
| Broad provider/config UI | `crates/tui/src/config_ui.rs`, `commands/provider.rs` | Slim | Keep only provider settings needed for play. |
| Generic sub-agents | `crates/tui/src/tools/subagent/` | Defer, then scoped game processors | Proposal-only processors may return in Phase 8. |

## Pi Features To Port First

Port concepts, not product scope:

| Pi concept | Reference | GENmicon use |
| --- | --- | --- |
| Agent loop events | `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/agent-loop.ts` | Kernel event order and deterministic mock tests. |
| Agent session state | `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/agent.ts` | Small session state for game turns. |
| Tool execution mode | `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/types.ts` | Parallel read tools, sequential commit tools. |
| Context transform | pi `transformContext` | Inject save snapshot and compacted summary before provider calls. |
| Tool preflight/postprocess | pi `beforeToolCall` / `afterToolCall` | Enforce mode/trust/revision gates and refresh render state. |
| Component width discipline | `/Users/eric_yiru/Desktop/Github/pi/packages/tui/README.md` | Game console panels must not overflow. |
| Compaction entries | pi compaction docs | Long-session summaries separate from game truth. |

Explicitly do not port first:

- Public extension package install.
- Arbitrary extension code execution.
- General coding-agent tools.
- Session sharing.
- Remote package workflows.
- Full provider registry.

## Testing Baseline

### Required Narrow Gates

Run these before Phase 2 changes and after every Phase 2 change touching the
relevant surface:

```bash
cargo test -p deepseek-game --all-features
cargo test -p deepseek-tui game_prompt_injects_single_turn_controller
cargo test -p deepseek-tui game_turn_controller_pins_commit_and_player_mode_invariants
cargo test -p deepseek-tui game_console_renders_player_panels_without_coding_chrome
cargo test -p deepseek-tui game_console_renders_representative_terminal_sizes
```

### Required Kernel Tests For Phase 2

The first new pi-style kernel scaffold must add tests for:

- `prompt_emits_session_and_turn_events_in_order`
- `parallel_read_tools_complete_without_reordering_tool_results`
- `sequential_tool_forces_ordered_execution`
- `preflight_can_block_disallowed_player_tool`
- `postprocess_can_refresh_game_view_after_commit`
- `player_profile_exposes_only_game_safe_tools`

### Required Runtime Tests For Phase 3

The runtime facade must preserve tests for:

- manifest load and path traversal rejection
- driver version resolution and exact reload
- save load, commit, merge patch, stale revision rejection
- lookup bounds and package-root containment
- render panels and `GameViewSnapshot`
- declared driver function execution only
- restart from committed save state

### Required UI Tests For Phase 5

The terminal presentation must preserve tests for:

- 60x20, 90x28, and 140x40 layouts
- player mode hides coding chrome, tool cells, model/cost text, and thinking
- developer mode restores diagnostics
- scene and portrait art fit bounded panes
- dialogue strips raw markdown-only markers

## First Implementation Slice After This File

Phase 2 should start with a small kernel crate or module that has no TUI,
provider, filesystem, shell, or game-package dependency.

Minimum types:

- `KernelState`
- `KernelMessage`
- `KernelEvent`
- `KernelTool`
- `ToolExecutionMode`
- `ToolResult`
- `MockProvider`

Minimum behavior:

- Accept one user/player prompt.
- Emit deterministic lifecycle events.
- Let the mock provider request tool calls.
- Execute parallel tools concurrently but preserve result message order.
- Execute a sequential tool in source order.
- Allow preflight to block a tool by name.
- Allow postprocess to attach derived state.

Do not add:

- Real model provider code.
- Public extension loading.
- Game cartridge loading.
- Terminal UI.
- Generic config system.
- Compatibility aliases.

Initial scaffold:

- `crates/kernel/`
- package name: `genmicon-kernel`
- current dependencies: none beyond the Rust standard library
- current scope: event order, messages, mock provider output, typed tools,
  parallel/sequential execution, preflight, postprocess, and generic allowlist
  profiles

## Stop/Go Criteria For Phase 2

Proceed to Phase 2 only when:

- This file exists and names the test baseline.
- The narrow baseline tests have been run or failures are recorded.
- The implementation target is a small kernel scaffold, not a full app rewrite.
- Existing dirty maintainer edits remain untouched.

Pause if:

- `deepseek-game` tests fail before changes.
- Existing game-console tests fail before changes.
- The branch has unrelated source edits.
- The proposed kernel starts depending on TUI, shell, network, or game package
  paths before those phases are reached.

## Baseline Results

Record the first baseline run here.

- `cargo test -p deepseek-game --all-features`: pass, 18 tests.
- `cargo test -p deepseek-tui game_prompt_injects_single_turn_controller`: pass.
- `cargo test -p deepseek-tui game_turn_controller_pins_commit_and_player_mode_invariants`: pass.
- `cargo test -p deepseek-tui game_console_renders_player_panels_without_coding_chrome`: pass.
- `cargo test -p deepseek-tui game_console_renders_representative_terminal_sizes`: pass.
- `cargo fmt --all --check`: pass after adding `genmicon-kernel`.
- `cargo test -p genmicon-kernel`: pass, 6 tests.

The second TUI layout rerun used `cargo test -p deepseek-tui
game_console_renders`, which also covered
`game_console_renders_scene_art_in_scene_panel`.

## Open Decisions

- Whether the first kernel scaffold should be a new crate or an isolated module
  under an existing crate.
- Whether the fresh binary name is `genmicon`, `genmicon-tui`, or a transition
  alias.
- Whether the first provider adapter should preserve DeepSeek only or define a
  provider trait before binding to DeepSeek.
- How soon to update `SPEC_files/README.md`; it is currently dirty from prior
  maintainer edits, so this branch should not rewrite it without explicit
  confirmation.
