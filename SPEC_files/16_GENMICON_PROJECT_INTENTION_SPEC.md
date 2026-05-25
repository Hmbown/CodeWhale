# GENmicon-TUI Project Intention Spec

Status: Draft
Owner: Maintainer
Last reviewed: 2026-05-18

## Purpose

This spec defines the product intention, principles, and durable goals for the
fresh GENmicon-TUI direction.

GENmicon-TUI is a terminal-native AI game console and game-computer framework
built on Pi. It should keep the smallest GENmicon-specific layer needed to make
local, AI-mediated games playable, persistent, inspectable, and extensible
without rebuilding Pi's established agent harness.

This is the north-star spec for deciding what stays, what is rebuilt, and what
is removed during the pi-based refactor. The detailed migration mechanics live
in `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`.

## Naming

Use `GENmicon-TUI` as the working project name in specs and internal planning.
If the public name changes later, update this spec first, then update
commands, docs, package names, and examples in one coordinated change.

## Ownership Boundary

This spec owns:

- The project intention and product identity.
- The principles used to choose essential features.
- The target user experience for the fresh project.
- The minimum viable system surface.
- The non-goals and removal criteria for inherited features.
- The definition of done for the refactor at a product level.

This spec does not own:

- The step-by-step refactor sequence. Use
  `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`.
- One game cartridge's story, facts, endings, or content. Use
  `SPEC_files/games/`.
- Reusable game-driver internals. Use `SPEC_files/game_driver/`.
- Current shipped DeepSeek TUI behavior unless the refactor explicitly keeps it.

## Source Anchors

Current project anchors:

- `AGENTS.md`
- `README.md`
- `docs/GAME_TUI_FRAMEWORK_SPEC.md`
- `SPEC_files/13_GAME_TUI_FRAMEWORK_SPEC.md`
- `crates/game/`
- `crates/tui/src/game.rs`
- `crates/tui/src/tui/widgets/game_console.rs`
- `examples/games/`

Alternative framework anchors:

- `/Users/eric_yiru/Desktop/Github/pi/README.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/agent/README.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/README.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/extensions.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/packages.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/coding-agent/docs/compaction.md`
- `/Users/eric_yiru/Desktop/Github/pi/packages/tui/README.md`

Planning companion:

- `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`

## Maintainer Prompt

Copy this block when asking the agent to change the project direction or
product scope:

```markdown
Spec: SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md
Goal:
Why it matters:
Target user:
Essential features to keep:
Features to remove or defer:
Pi features to reuse:
Compatibility expectations:
Acceptance criteria:
Validation I expect:
```

## Product Intention

GENmicon-TUI should be an AI-era terminal game console:

- The player opens a local game cartridge from the terminal.
- The first screen is the game, not a coding-agent cockpit.
- Natural language input, choices, and game commands all resolve through one
  controlled game loop.
- Game state is durable and restartable.
- The model may improvise narration, dialogue, and interpretation, but it does
  not own authoritative state.
- Driver scripts and runtime validation keep mechanical facts deterministic.
- Game packages can add content, skills, saves, and driver bindings, but cannot
  expand the trusted tool surface.
- Developers can inspect diagnostics when authoring cartridges, while ordinary
  player mode hides implementation machinery.

The project should feel closer to a small programmable game console than to a
general software-development assistant with a game feature attached.

## Target Users

Primary users:

- Players who want terminal-native AI games that can be saved, resumed, and
  meaningfully constrained by authored game rules.
- Game authors who want to write local cartridges with markdown content,
  manifest files, skills, deterministic driver functions, and save templates.
- The maintainer, who needs a codebase small enough to reason about and evolve
  without fighting inherited complexity.

Secondary users:

- Agent developers evaluating game-focused tool orchestration.
- Researchers or writers prototyping serious games, dialogue games, mystery
  games, and deliberation games.

Non-primary users:

- Developers looking for a full replacement coding agent.
- Teams needing general project automation, CI triage, runtime APIs,
  automations, or broad MCP workflows.
- Users expecting a hosted game marketplace or cloud-backed game service.

## Core Principles

### Game First

The first visible experience is the active game. Player mode must not expose
coding sidebars, repository status, cost meters, task queues, raw tool cells,
model thinking, or filesystem paths unless an error truly requires it.

### Small Trusted Core

The trusted runtime must be small. Keep only the pieces needed for loading game
packages, running the turn loop, validating state, rendering the game console,
calling models, executing declared tools, and persisting saves.

### Build On Pi, Not Beside It

Use Pi's established package, extension, session, compaction, provider, tool,
command, renderer, skill, prompt, theme, and TUI component features directly
where they fit. Do not create a separate GENmicon agent runtime in parallel to
Pi. New infrastructure is justified only for deterministic game authority such
as manifest validation, save commits, content lookup, render snapshots, and
driver functions.

### State Is Authority

`STATE.json` and `TURN_LOG.jsonl` are authoritative for game progress. TUI
transcripts, sub-agent transcripts, render caches, and summaries are derived
or disposable.

### Model As Resolver

The model resolves open-ended player intent, narration, dialogue, and ambiguous
consequences. It must commit state only through the native game commit tool.
Deterministic calculations belong in driver functions, not in model prose.

### Packages Are Content, Not Trust

Game packages, issue text, external docs, and generated cartridge files are
untrusted input. They may describe the game world but must not grant shell,
network, arbitrary filesystem, or generic coding-agent powers.

### Developer Mode Is Explicit

Diagnostics are valuable for cartridge authors, but they must be behind an
explicit developer-mode switch. Developer mode can show manifests, save paths,
driver details, sub-agent rosters, raw panels, tool activity, and validation
warnings.

### Local First

V1 is local-first. Games, saves, drivers, skills, and assets live on local disk.
No hosted marketplace, remote content downloads, telemetry, Discord status,
or remote music services belong in the minimal target.

### Stable, Testable Interfaces

Commands, file formats, tool ABIs, and driver manifests should be small,
documented, and covered by focused tests before they are treated as shipped.

## Essential Product Surface

The fresh project should keep or rebuild only these essential surfaces first:

- A canonical Pi package or command entry point for player mode.
- A local game cartridge/package format with manifest, content, skills,
  prompts, themes, drivers, and saves.
- A pure game runtime that validates paths, manifests, saves, lookup, driver
  calls, render snapshots, and commits.
- Pi extension glue that registers game commands, tools, renderers, custom UI,
  custom editor behavior, and package resources.
- Pi provider/model, session, branch/tree, and compaction behavior reused as
  the agent substrate.
- Native game tools:
  - `game_status`
  - `game_render`
  - `game_playbook`
  - `game_lookup`
  - `game_fact_check`
  - `game_run_driver`
  - `game_commit_turn`
- A player-facing terminal game console with scene, figure, status, items,
  tasks, dialogue, choices, and composer regions.
- Explicit developer-mode diagnostics.
- Minimal skills support for game and driver instructions.
- Optional scoped sub-agent proposals only after the main loop is stable.
- Focused test fixtures for a small dialogue game and one serious-game
  cartridge scaffold.

Current rebuild ownership:

- `packages/genmicon-pi/` owns Pi package resources: commands, game-safe tools,
  active-tool profiles, renderers, player/diagnostic UI models, prompts,
  skills, and local package trust checks.
- `crates/game` owns deterministic runtime authority through the
  `genmicon-game-runtime` JSON helper: manifest validation, save load/list,
  render/playbook, lookup, fact checks, declared driver calls, and sequential
  `commit_turn`.
- There is no GENmicon-owned parallel agent kernel. Pi remains the session,
  provider, model, command, tool, compaction, and TUI substrate.

## Features To Remove Or Defer

The fresh project should not carry these by default unless a later spec
reintroduces them with a narrow game-specific reason:

- A second general-purpose coding-agent mode as the main product.
- A parallel agent/session/tool/package runtime that duplicates Pi.
- Broad shell and file-edit tools in player mode.
- Repository-oriented tasks, PR flows, CI workflows, and release automation.
- General runtime HTTP/SSE API.
- Recurring automations.
- Broad MCP server/client management.
- LSP diagnostics for coding workflows.
- General memory systems unrelated to game saves.
- Hosted package marketplaces beyond Pi's package mechanisms.
- Telemetry or session sharing.
- Visual asset pipelines beyond terminal-friendly render data and Pi themes.
- Required external game runtimes, Python subprocesses, or separate terminal
  applications.
- Unreviewed third-party branding, hosted analytics, sponsorship lines, or
  external service integrations.

Deferred does not mean impossible. It means the feature must prove that it
serves GENmicon-TUI as a game console, not that it existed in the inherited
codebase.

## Target Product Shape

### Player Mode

Player mode should:

- Open directly into the game console.
- Ask only necessary launch questions, such as game selection, save selection,
  and language when needed.
- Hide implementation details while the game resolves a turn.
- Keep a responsive dialogue/log region visible during tool use.
- Show choices and free-form composer affordances.
- Save progress only through the game commit path.
- Resume from save state after restart.

### Developer Mode

Developer mode should:

- Preserve normal play while making diagnostics visible.
- Show active game root, save path, driver ID/version, warnings, raw state,
  raw render view, tool activity, and scoped processor status.
- Help authors debug cartridges without making those diagnostics part of the
  player-facing fiction.

### Authoring Mode

Authoring support should begin as file formats and validation, not a full
authoring wizard. A game author should be able to:

- Create a cartridge directory.
- Write `game.toml`, `GAME.md`, content markdown, skills, and save templates.
- Bind to a local driver.
- Run validation and start the cartridge.
- Inspect warnings in developer mode.

## Pi Concepts To Adopt

Use these Pi features directly before building project-owned alternatives:

- Pi packages for distributing GENmicon extensions, skills, prompts, themes,
  and cartridge/driver resources.
- Extension lifecycle events for session, input, context, provider, message,
  tool, compaction, and shutdown behavior.
- `registerTool`, `registerCommand`, active-tool management, custom message and
  tool renderers, and provider/model registry APIs.
- Pi session storage, tree navigation, branch summaries, and compaction entries
  as the conversation substrate.
- Pi TUI components, overlays, dialogs, widgets, custom editor hooks, and
  strict width/focus rules for player and developer presentation.
- Package filtering and project-local `.pi/settings.json` for loading only the
  resources that are trusted for a game.

Do not automatically adopt:

- Arbitrary third-party extension code execution in player mode.
- Full coding-agent tool defaults.
- Remote package installation at game launch.
- Session sharing or telemetry.
- Non-Pi plugin systems before the game runtime is stable.

## Relationship To Current DeepSeek TUI

The current DeepSeek TUI project is a useful source of working code and
lessons, but it is not the desired final shape. Treat it as a donor codebase
while Pi is the target substrate:

- Keep concepts that directly serve GENmicon-TUI.
- Extract, port, or retire modules when Pi already owns the generic behavior.
- Prefer Pi-native extension/package interfaces over compatibility with
  internal legacy abstractions.
- Preserve public behavior only when it remains part of the GENmicon product.
- Do not let old Plan/Agent/YOLO, coding tasks, or repository workflows define
  the new user experience.

## Minimum Viable Refactor Outcome

The first complete refactor milestone is reached when:

- A fresh branch builds a lean GENmicon Pi package, extension, or clearly named
  adapter target.
- A local cartridge launches into player mode.
- The player can take a natural language action.
- The model can use only game-safe tools.
- A turn commits exactly once through `game_commit_turn`.
- The game view refreshes from committed save state.
- Restart resumes the save.
- Developer mode exposes diagnostics.
- The codebase has a smaller, clearer module map than the inherited project.
- Removed features are documented as intentionally removed or deferred.

## Acceptance Criteria Checklist

- [ ] Product identity and intended audience are clear.
- [ ] Essential features are separated from inherited non-essential features.
- [ ] Pi adoption is defined as building on Pi's package and extension
      substrate, not a parallel runtime.
- [ ] Player mode, developer mode, and authoring needs are distinct.
- [ ] State authority and game package trust boundaries are explicit.
- [ ] Removal and deferment criteria exist for heavy inherited features.
- [ ] The companion refactor plan can be evaluated against this spec.

## Validation Gates

For this spec itself:

- Read-through review for contradictions with
  `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`.
- Confirm links and file paths still exist, or mark future paths clearly.
- No build or test gate is required for docs-only edits.

For implementation that claims this spec is satisfied:

- `cargo build` or the new equivalent build command for the target workspace.
- Targeted runtime tests for game load, render, lookup, driver call, and commit.
- Targeted TUI or terminal-render tests for player/developer presentation.
- A restart/resume test using a local fixture cartridge.

## Risks

- "Pi-based" can be misread as copying Pi concepts instead of building on Pi.
- Pi packages and extensions are powerful enough to violate game trust
  boundaries if unreviewed resources are loaded in player mode.
- Removing too much at once can strand useful code before replacement seams
  exist.
- Keeping too much can reproduce the current heavy product under a new name.
- Player-mode hiding can become cosmetic if restricted tools and save authority
  are not enforced at the runtime level.
- A fresh branch can diverge from existing specs unless spec ownership is
  updated continuously.

## Open Decisions

- Final public project name and binary name.
- Whether the long-term runtime remains Rust, moves substantial logic to
  TypeScript, or uses a hybrid boundary.
- Which parts ship as Pi package resources, which remain local game runtime
  code, and whether any standalone wrapper is needed.
- Which model providers are required for the first lean milestone.
- How much of the current `crates/game` runtime should be reused unchanged.
