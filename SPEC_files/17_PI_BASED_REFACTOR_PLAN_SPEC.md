# Pi-Based Refactor Plan Spec

Status: Draft
Owner: Maintainer
Last reviewed: 2026-05-18

## Purpose

This spec defines the staged refactor plan for rebuilding GENmicon-TUI as a
Pi-native game layer while keeping only the essential elements needed for the
GENmicon game-console idea.

This is a planning and execution contract. It should guide work on a relatively
fresh branch, with small integration steps, explicit validation gates, and
clear removal decisions for heavy inherited DeepSeek TUI features.

The product intention and principles live in
`SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`.

## Definition Of Pi-Based

For this project, pi-based means building on Pi's actual runtime and resource
model:

- Pi remains the agent/session substrate for state, messages, tools, model
  selection, event streams, branch/tree sessions, and compaction.
- GENmicon game behavior is delivered first as Pi packages, extensions, skills,
  prompts, themes, commands, tools, renderers, and custom TUI/editor surfaces.
- The lifecycle uses Pi extension events around session, input, context,
  provider requests, messages, tool calls/results, compaction, and shutdown.
- Tools are registered through Pi's typed tool API and constrained by Pi active
  tool management plus GENmicon trust policy.
- Terminal UI uses Pi TUI components, overlays, dialogs, widgets, custom
  editors, message renderers, and width/focus rules.
- Skills, prompts, themes, drivers, and game resources are loaded through Pi
  package/project-local resource discovery whenever possible.

Pi-based does not mean:

- Player mode can run arbitrary third-party extension code.
- The project must keep pi's full coding-agent feature set.
- The project must become a TypeScript monorepo before the package path is
  proven.
- GENmicon should build a parallel agent/session/tool/package runtime.
- Game packages can install tools or change trust policy.
- Remote package install, telemetry, session sharing, or hosted services are
  part of V1.

## Ownership Boundary

This spec owns:

- Branch and migration strategy for the Pi-native refactor.
- Target architecture layers.
- Mapping from current DeepSeek TUI surfaces to lean GENmicon surfaces.
- Mapping from Pi features to GENmicon game-layer usage.
- Phase-by-phase implementation order.
- Validation gates and stop/go criteria.
- Removal/deferment plan for non-essential features.

This spec does not own:

- The product north star. Use
  `SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`.
- Detailed game-driver behavior. Use `SPEC_files/game_driver/`.
- Per-cartridge story/content contracts. Use `SPEC_files/games/`.
- Shipped behavior of the current Rust application unless it is explicitly
  preserved by this migration plan.

## Source Anchors

Current code to inspect before changing architecture:

- `crates/cli/`
- `crates/tui/src/main.rs`
- `crates/tui/src/tui/app.rs`
- `crates/tui/src/tui/ui.rs`
- `crates/tui/src/core/engine/`
- `crates/tui/src/tools/`
- `crates/tui/src/session_manager.rs`
- `crates/tui/src/game.rs`
- `crates/game/`
- `examples/games/`

Current docs and specs:

- `AGENTS.md`
- `docs/ARCHITECTURE.md`
- `docs/GAME_TUI_FRAMEWORK_SPEC.md`
- `docs/TOOL_SURFACE.md`
- `docs/SUBAGENTS.md`
- `SPEC_files/README.md`
- `SPEC_files/13_GAME_TUI_FRAMEWORK_SPEC.md`
- `SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`

Pi reference anchors:

- `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/agent.ts`
- `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/agent-loop.ts`
- `/Users/eric_yiru/Desktop/Github/pi/packages/agent/src/types.ts`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/src/core/agent-session.ts`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/extensions.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/packages.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/compaction.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/session-format.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/tui/src/tui.ts`
- `/Users/eric_yiru/Desktop/Github/pi/packages/tui/src/components/`

## Maintainer Prompt

Copy this block when asking the agent to work on the refactor:

```markdown
Spec: SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md
Goal:
Phase:
Current branch:
Current behavior:
Desired behavior:
Essential GENmicon feature served:
Pi feature being reused:
Code to preserve:
Code to remove or defer:
Acceptance criteria:
Validation I expect:
```

## Branch Strategy

The refactor should begin on a relatively fresh branch.

Recommended branch policy:

- Use a fresh branch for spec and architecture work before moving code.
- Keep user or maintainer edits in existing dirty files untouched unless the
  task explicitly includes them.
- Prefer small commits grouped by phase:
  - specs and decision records
  - minimal Pi package scaffold
  - game runtime preservation
  - terminal presentation
  - tool profile
  - fixture validation
  - removal of unused heavy surfaces
- Do not mix unrelated cleanup with migration steps.
- Do not delete large legacy surfaces until the replacement path is compiling
  and covered by focused tests.

If `codex/<name>` branch creation is blocked by local refs layout, use a flat
branch name such as `codex-pi-based-refactor-specs` and record that in the
handoff.

## Target Architecture

The target should be layered like this:

```text
Pi CLI / Pi app runtime
  -> GENmicon Pi package or project-local resources
    -> extensions: commands, tools, active-tool policy, renderers, custom UI
    -> skills/prompts/themes: game rules, voice, templates, presentation
    -> game runtime facade: manifest/load/save/lookup/render/driver/commit
    -> terminal presentation: player console, diagnostics, pickers/dialogs
```

### Layer 1: CLI

Responsibilities:

- Prefer Pi's CLI and package loading as the primary launch path.
- Register GENmicon play, validate, inspect, and developer commands through Pi
  extension commands.
- Add a thin `genmicon` wrapper only if it delegates to Pi or a documented Pi
  package entry point.
- Avoid exposing inherited DeepSeek coding-agent subcommands unless explicitly
  kept.

Initial commands:

- `play [game-or-path] [--save <id>] [--lang en|zh] [--dev]`
- `validate [game-or-path]`
- `saves [game-or-path]`
- `doctor`

Compatibility with `deepseek play` can remain during transition if it is cheap,
but the fresh project should be defined by a Pi-native package/command path, not
by `deepseek` naming.

### Layer 2: Pi Agent Session Integration

Responsibilities:

- Use Pi's session state, message stream, active tools, model settings,
  steering/follow-up behavior, branch/tree sessions, and compaction entries.
- Hook Pi extension events such as `session_start`, `resources_discover`,
  `input`, `before_agent_start`, `agent_start`, `turn_start`, `context`,
  `before_provider_request`, message events, tool events, `turn_end`,
  `agent_end`, `session_before_compact`, `session_compact`, and shutdown.
- Transform context before provider calls using Pi's context hooks rather than
  a parallel message pipeline.
- Use Pi's tool-call lifecycle for preflight, postprocessing, rendering, and
  active-tool restrictions.
- Keep game player mode restricted by construction.

GENmicon code should be small enough to read without understanding all of Pi,
because Pi owns the generic agent loop.

### Layer 3: Pi Model Provider Use

Responsibilities:

- Use Pi's provider registry, model registry, model switching, thinking-level
  support, API-key handling, streaming, and usage accounting where available.
- Register or override providers through Pi only when a game-specific provider
  path is explicitly needed.
- Avoid carrying every inherited DeepSeek provider/auth surface before the game
  loop is stable.

Initial provider scope should be one or two providers only. Additional
providers can be reintroduced through explicit specs.

### Layer 4: Tool Registry And Policy

Responsibilities:

- Register typed game tools with Pi's `registerTool`.
- Track and enforce active tool names with Pi's active-tool APIs.
- Enforce game-safe allowlists in player mode.
- Classify tools as read-only, mutating, hidden, developer-only, or
  player-visible through GENmicon policy metadata and Pi renderers.
- Render or hide tool activity through Pi custom renderers and player/developer
  presentation policy.

Initial player tools:

- `game_status`
- `game_render`
- `game_playbook`
- `game_lookup`
- `game_fact_check`
- `game_run_driver`
- `game_commit_turn`
- `load_skill` or a game-scoped skill equivalent

Deferred tools:

- Generic shell
- Generic file read/write/edit
- Generic git
- Generic task manager
- Generic MCP tools
- General sub-agent tools

### Layer 5: Game Runtime Facade

Responsibilities:

- Preserve or port the essential `crates/game` behavior:
  - manifest parse and validation
  - path canonicalization
  - driver resolution
  - save load/write
  - content lookup
  - render snapshots
  - deterministic driver functions
  - atomic commit
  - agent pack generation when needed
- Keep runtime independent from the TUI and model client.
- Treat package content as untrusted.

This is the strongest candidate for reuse from the current Rust project. It is
called by Pi tools; it is not a replacement for Pi's agent loop.

### Layer 6: Terminal Presentation

Responsibilities:

- Render the player-facing game console.
- Hide coding-agent chrome in player mode.
- Show developer diagnostics only when enabled.
- Provide input composer and picker/dialog surfaces.
- Use Pi TUI components, overlays, custom message/tool renderers, widgets, and
  custom editor hooks where practical.
- Follow strict width and focus rules from pi-tui.

The presentation layer should prefer Pi TUI. Any Rust/ratatui bridge kept
during transition must have a deletion or compatibility plan so the project
does not duplicate full UI stacks indefinitely.

## Current To Target Mapping

| Current surface | Target decision |
| --- | --- |
| `crates/game` | Keep as core runtime candidate. Slim only after tests protect behavior. |
| `GameSession` | Keep concept during transition; long-term state should attach to Pi sessions plus authoritative game saves. |
| `GameConsoleWidget` | Keep behavior target; prefer Pi TUI custom UI/renderers for new work. |
| `game_*` tools | Keep ABIs unless the new spec updates them first; expose through Pi `registerTool`. |
| `tools/subagent` | Defer generic surface; use Pi session/package primitives or scoped game proposal helpers only if needed. |
| RLM/Python REPL | Remove or defer. Not essential to GENmicon V1. |
| Runtime API/tasks/automations | Remove or defer. Not essential to local game play. |
| LSP diagnostics | Remove or defer. Coding workflow feature. |
| MCP broad surface | Remove or defer. Reintroduce only as authored game resource loading if needed. |
| Skills | Keep game/driver skills through Pi package/resource discovery with strict trust limits. |
| Config UI | Defer broad UI. Keep minimal config needed for model and game roots. |
| Provider registry | Prefer Pi provider registry; add overrides only through Pi extension APIs. |
| Session manager | Use Pi sessions, branch/tree, and compaction while keeping game saves authoritative. |
| Snapshot/restore | Defer repository snapshots. Game saves are the recovery mechanism. |
| Plan/Agent/YOLO modes | Remove from player product. Keep only developer controls if justified. |

## Pi Feature Mapping

| Pi feature | GENmicon usage |
| --- | --- |
| Pi packages | Bundle GENmicon extensions, skills, prompts, themes, and trusted local resources. |
| Extension events | Hook game context injection, tool gates, render refresh, compaction, and shutdown. |
| `registerCommand` | Add play, validate, inspect, saves, and developer commands. |
| `registerTool` | Expose native game tools with game-safe policy. |
| `getActiveTools` / `setActiveTools` | Enforce player/developer active-tool profiles. |
| Tool renderers | Hide or summarize game tool activity in player mode and expand in developer mode. |
| Message renderers | Render player-facing game log and developer diagnostics. |
| Custom UI/editor | Build the game console, pickers, overlays, and player action composer. |
| Sessions/tree/compaction | Preserve conversation context while save files remain game truth. |
| Provider/model registry | Use Pi's model selection and provider configuration. |
| Skills/prompts/themes | Load game rules, voice, command templates, and visual style through Pi resources. |

## Implementation Phases

### Phase 0: Spec And Decision Baseline

Goal:

- Establish the fresh GENmicon direction before code churn.

Deliverables:

- `SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`
- `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`
- A short branch handoff noting dirty files and branch name.

Acceptance criteria:

- The product intention and refactor plan are explicit.
- Pi-based is defined with trust boundaries.
- Existing dirty files are not overwritten.
- No code behavior changes are made.

Validation:

- Manual docs review.
- `git status --short --branch`.

### Phase 1: Inventory And Removal Matrix

Goal:

- Decide what is essential, reusable, deferred, or deleted.

Deliverables:

- A removal matrix under `SPEC_files/goals/` or a new active spec section.
- Module ownership map for the fresh branch.
- List of features that must compile in the first lean target.

Tasks:

- Inventory all crates and high-level modules.
- Classify each as:
  - keep
  - keep but slim
  - reuse Pi feature
  - defer
  - delete after replacement
- Mark tests that protect reusable behavior.

Acceptance criteria:

- No large module is migrated without a classification.
- Every kept feature maps to a GENmicon product need.
- Every deferred feature has a reason.

Validation:

- No build required if docs-only.
- If code is moved, run targeted build/tests for affected crates.

### Phase 2: GENmicon Pi Package Scaffold

Goal:

- Add the smallest Pi-native package/extension surface for GENmicon.

Deliverables:

- `package.json` with `pi` manifest or project-local `.pi/settings.json`.
- Extension entrypoint registering one command, one read-only tool, and one
  player-facing renderer or custom UI stub.
- Initial game skill/prompt/theme resource layout.
- Tests or smoke checks proving Pi loads the package and resources.

Design notes:

- Keep this separate from the old DeepSeek TUI at first.
- Use Pi events and APIs instead of a new event loop.
- Use Pi active-tool management for player/developer profiles.
- Do not load unreviewed third-party package extensions in player mode.

Acceptance criteria:

- Pi can load the GENmicon package or project-local resources.
- `/genmicon` or the chosen command appears through Pi command registration.
- A read-only game tool can be called through Pi.
- Player active tools exclude shell/file/git and other generic coding tools.
- A minimal player-facing renderer/custom UI displays without width overflow.

Validation:

- Package load smoke test.
- Targeted extension/tool/UI tests where available.

Migration note:

- `crates/kernel` was removed after the Pi package path covered the equivalent
  policy surface: package loading, active-tool allowlists, command registration,
  runtime bridge calls, player view, diagnostics, commit-once behavior, and
  resume from authoritative saves. Future GENmicon work should extend Pi
  package resources and the deterministic `crates/game` helper, not revive a
  separate agent kernel.

### Phase 3: Game Runtime Preservation

Goal:

- Connect Pi-registered tools to the essential game runtime.

Deliverables:

- Runtime facade callable from Pi tools for load, status, render, playbook,
  lookup, fact check, driver call, and commit.
- Game package path validation.
- Fixture load tests.
- Commit/restart tests.

Design notes:

- Prefer preserving `crates/game` behavior before rewriting it.
- Keep `game_commit_turn` sequential and state-authoritative.
- Keep render snapshots derived from save state.

Acceptance criteria:

- The reconciliation fixture loads.
- The serious-game fixture loads enough to render initial panels.
- Lookup cannot escape game root.
- Driver calls can only use declared functions.
- Commit increments revision and appends a turn log.
- Restart reloads committed state.

Validation:

- `cargo test -p deepseek-game --all-features` or renamed equivalent.
- Targeted fixture tests.

### Phase 4: Restricted Tool Profile

Goal:

- Expose only game-safe tools through Pi active-tool policy in player mode.

Deliverables:

- Pi tool registrations.
- Player/developer tool profile policy.
- Tool ABIs for the native game tools.
- Hidden rendering policy for player mode.

Acceptance criteria:

- Player mode exposes only game-safe tools.
- `game_commit_turn` is the only save writer.
- Generic shell/file/git tools are absent in player mode.
- Developer mode can show diagnostics without changing player policy.

Validation:

- Tool registry tests.
- Player profile tests.
- Commit authority tests.

### Phase 5: Terminal Game Console

Goal:

- Build the first lean player-facing terminal UI.

Deliverables:

- Game console layout with scene, figure, status, items, tasks, dialogue,
  choices, and composer.
- Developer diagnostics view.
- Game picker or explicit path launch.
- Width-safe panel tests.

Design notes:

- Use Pi TUI component discipline and prefer Pi custom UI/renderers.
- Lines must fit their render width.
- Player mode hides tool plumbing and thinking.
- Dialogue should contain live exchange and consequences, not duplicated status
  or raw markdown headings.

Acceptance criteria:

- 60x20, 90x28, and 140x40 layouts do not overflow or overlap.
- Player mode hides coding-agent chrome.
- Developer mode shows raw state/render/tool details.
- Composer placeholder and input behavior are game-specific.

Validation:

- Buffer/layout tests.
- Manual terminal smoke test if a dev binary exists.

### Phase 6: Provider And Model Integration

Goal:

- Attach the game loop to Pi's real model/provider path.

Deliverables:

- Pi provider/model configuration for the selected initial provider.
- Any required Pi provider override extension.
- Reasoning/thinking handling policy using Pi capabilities.
- Usage/error behavior documented for player and developer modes.

Acceptance criteria:

- Deterministic tests do not require real credentials.
- Real provider path can stream text and tool calls through Pi.
- Provider errors become user/developer appropriate events.
- DeepSeek-specific behavior is handled only if DeepSeek remains in scope.

Validation:

- Mock or fake provider integration tests through Pi where practical.
- One opt-in real-provider smoke test only if credentials are available.

### Phase 7: Context And Compaction

Goal:

- Add bounded long-session behavior without making transcript the game truth.

Deliverables:

- Pi context hook for current save snapshot and recent turns.
- Pi compaction entry usage and summary policy.
- Branch/tree policy if kept.

Design notes:

- Game state summary is derived from save files.
- Pi compaction must not mutate game saves.
- Do not trust old sub-agent transcripts after restart.

Acceptance criteria:

- Context includes current save snapshot and recent turns.
- Old transcript can compact without changing `STATE.json`.
- Restart uses save truth plus summaries, not transcript authority.

Validation:

- Compaction unit tests.
- Restart after compaction test.

### Phase 8: Scoped Game Processors

Goal:

- Reintroduce processor-like helpers only for game proposals and only on top of
  Pi sessions/packages or a documented scoped helper.

Deliverables:

- Scoped processor pack format.
- Role allowlist from driver/game runtime.
- `game_agent_*` or renamed scoped helper tools.
- Short timeout behavior.
- Render artist role if needed.

Acceptance criteria:

- Processors receive only scoped game context.
- Processors cannot call `game_commit_turn`.
- Main session remains final narrator and commit authority.
- Timeout does not block gameplay indefinitely.

Validation:

- Scoped tool profile tests.
- Processor pack content tests.
- Timeout behavior tests.

### Phase 9: Removal And Slimming

Goal:

- Remove inherited heavy surfaces after replacements are working.

Candidate removals:

- General coding-agent command set.
- Generic task manager and automations.
- LSP diagnostics.
- Broad MCP runtime.
- Repository snapshot and restore flows.
- RLM/Python REPL.
- Broad provider/config UI.
- General PR/review/apply/eval flows.

Acceptance criteria:

- Removed features are listed in docs or changelog as intentionally removed
  from GENmicon scope.
- Build no longer pulls unused heavy dependencies.
- Player/game tests still pass.
- Public docs no longer advertise removed surfaces.

Validation:

- Full build for the lean workspace.
- Targeted game tests.
- Search docs for removed command names.

## Stop/Go Gates

Do not proceed from one phase to the next if:

- The previous phase does not compile.
- Player mode can access generic shell/file/git tools.
- Save state can be mutated outside the commit path.
- The runtime trusts game package paths without canonicalization.
- The UI only hides tool details visually while still exposing unsafe tools.
- The branch starts mixing unrelated cleanup or legacy feature fixes.

It is acceptable to pause a phase with a written blocker and continue with a
non-overlapping docs or test task.

## Compatibility Policy

Compatibility is not a default requirement for internal legacy behavior.

Keep compatibility only for:

- Existing example game data while it remains useful as a fixture.
- `game_*` tool ABIs until a spec explicitly changes them.
- `deepseek play` as a temporary transition alias if cheap.

Do not preserve compatibility for:

- Internal TUI state names.
- Coding-agent modes.
- Broad tool surface.
- General automation APIs.
- Old docs that no longer match the GENmicon target.

## Documentation Plan

Docs should move through three states:

- Legacy reference: useful context from DeepSeek TUI.
- Active GENmicon spec: current target behavior.
- Shipped GENmicon docs: only behavior implemented in the lean branch.

During migration:

- Keep planned behavior clearly labeled as planned.
- Avoid public promotion before code, tests, docs, and examples align.
- Keep specs shorter than implementation docs; link to source anchors instead
  of duplicating code details.

## Testing Strategy

Use a narrow-to-wide test ladder:

- Unit tests for Pi extension hooks, tool policy, path validation, merge patch,
  render snapshots, and component layout.
- Fixture tests for reconciliation and serious-game cartridges.
- Mock model integration tests for a full player turn.
- Restart/resume tests.
- Developer-mode visibility tests.
- Full build/test only after a phase changes cross-cutting behavior.

Required early fixtures:

- A minimal reconciliation dialogue cartridge.
- A serious-game deliberation scaffold with fixed facts, evidence, jurors, and
  pressure state.

## Acceptance Criteria Checklist

- [ ] The branch starts from clear specs and a recorded plan.
- [ ] Pi-based architecture is defined as a Pi-native package/extension layer,
      not a parallel runtime or unbounded feature import.
- [ ] Essential GENmicon features are mapped to target layers.
- [ ] Current heavy surfaces have keep/defer/remove decisions.
- [ ] Each phase has deliverables, acceptance criteria, and validation gates.
- [ ] Player-mode trust boundaries are enforced by tool policy and runtime
      authority.
- [ ] Game saves remain authoritative across compaction and restart.
- [ ] Developer mode is explicit and does not weaken player-mode restrictions.

## Risks

- A full rewrite can lose working game-runtime behavior before tests protect it.
- A partial rewrite can leave Pi and legacy DeepSeek runtime paths competing
  for ownership and increase complexity.
- Loading unreviewed Pi package extensions too early can violate game package
  trust.
- Keeping broad DeepSeek TUI compatibility can recreate the current heavy
  product.
- Provider/auth scope can expand before the core game loop is stable.
- UI migration can absorb time before runtime authority is proven.

## Open Decisions

- Final binary and package naming.
- Rust-only, TypeScript-only, or hybrid game-runtime implementation direction.
- Which parts ship as Pi package resources and which remain local runtime code.
- First supported provider set.
- Whether to keep `deepseek play` as a transition alias.
- Exact Pi session entries and custom message formats GENmicon should add.
- Whether public extension APIs are ever allowed, and if so, whether they are
  developer-only, signed, local-only, or never active in player mode.
