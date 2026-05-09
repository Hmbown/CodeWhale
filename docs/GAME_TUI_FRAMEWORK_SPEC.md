# Game TUI Framework Spec

## Summary

Game TUI is a terminal-native game framework built into DeepSeek TUI. It turns
the existing TUI into an AI-era game console/computer while reusing the current
engine, skill system, slash commands, tool registry, sub-agent runtime, session
records, and ratatui UI surfaces.

This document is the planning source of truth for the Game TUI framework. The
handoff prompt in `TAKEOVER_PROMPT.md` is an operator-facing summary; when the
two disagree, update the handoff prompt to match this spec. Existing product
docs such as `ARCHITECTURE.md`, `TOOL_SURFACE.md`, `SUBAGENTS.md`,
`CONFIGURATION.md`, and `MODES.md` describe shipped behavior unless they
explicitly mark Game TUI material as planned.

The V1 names and shapes in this document are normative. Implement the CLI,
tool names, manifest fields, save files, and commit protocol as written unless a
later spec update changes them first.

This is not a Gen-micom integration spec. External markdown game projects can
inspire fixtures and content conventions, but the engineering design belongs in
this repository and must not require any external folder, Python CLI, or
project-specific runtime.

The product metaphor is useful for design:

- **CPU**: the LLM/API/model ability that resolves open-ended play.
- **GPU**: the model-guided terminal rendering layer, especially ASCII panels,
  maps, portraits, menus, and scene art.
- **Memory**: native context, summaries, compacted state, save files, and
  reload behavior that keep long games stable.

These are product concepts, not required Rust type names.

The first milestone is a playable loop:

1. `deepseek play` opens the TUI in Game Console presentation.
2. A local game package, its selected driver, and a save are loaded from disk.
3. The player types natural actions.
4. The main game engine session orchestrates game context, driver tools, and any
   scoped sub-agents.
5. The LLM resolves the turn using game instructions and lookup tools.
6. State changes are validated and persisted only through native game tools.
7. The save resumes after restart, with sub-agents rebuilt from save summaries
   instead of trusted old transcripts.

## Architecture

Game TUI has four engineering pieces.

### Existing Code Integration Map

The implementation should extend current runtime seams rather than fork the
application:

| Concern | Current first stop | Game TUI direction |
| --- | --- | --- |
| `deepseek play` dispatch | `crates/cli` and `crates/tui/src/main.rs` | Add a play entrypoint that launches the existing TUI with game session intent. |
| Slash command metadata/routing | `crates/tui/src/commands/` | Add `/play` and `/game ...` without creating a second command system. |
| App state and rendering | `crates/tui/src/tui/app.rs`, `tui/ui.rs`, `tui/mod.rs` | Add game session state plus a Game Console presentation profile. |
| Tool exposure | `crates/tui/src/tools/registry.rs`, `core/engine/tool_setup.rs` | Add a game-safe whitelist for player mode and a wider developer profile. |
| Skills | `crates/tui/src/skills/`, `skill_state.rs`, `tools/skill.rs` | Reuse skill discovery/loading with game and driver roots. |
| Sub-agents | `crates/tui/src/tools/subagent/`, `tui/subagent_routing.rs` | Wrap existing sub-agent runtime with game-scoped roles and agent packs. |
| Persistence | `session_manager.rs`, `runtime_threads.rs`, future `crates/game` | Keep chat/session persistence separate from authoritative game saves. |

### TUI Game Console

The existing TUI is the console. Do not create a separate terminal app or event
loop.

The console owns:

- the main player-facing game engine session
- Game Console presentation using existing ratatui surfaces
- game picker and save picker views
- `/play` and `/game` slash commands
- player-facing status/header/footer profile
- restricted game-safe tool profiles
- orchestration of driver tools and scoped sub-agents

V1 must not add `AppMode::Game`; use `GameSession` plus presentation and
tool-profile state.

### Game Runtime Core

Add a pure Rust workspace crate at `crates/game`. This crate is required for V1
and must have no TUI, ratatui, LLM, shell, network, Python, or external runtime
dependency.

Responsibilities:

- load and validate game and driver manifests
- discover saves and choose the active save
- load `STATE.json`, `TURN_LOG.jsonl`, and sub-agent summaries
- produce compact resume snapshots
- generate structured render panels
- perform bounded content lookup inside the game root
- execute constrained deterministic driver scripts
- commit turns atomically
- report validation errors and warnings

The runtime should expose typed data to the TUI. It should not print terminal
panels directly and should not know about TUI widgets.

Required `crates/game` module boundaries:

- `manifest`: parse and validate `game.toml` and `driver.toml`
- `paths`: canonicalize game, save, content, driver, skill, and script paths
- `driver`: resolve installed driver versions and declared driver functions
- `save`: load, validate, patch, and atomically write `STATE.json` and
  `TURN_LOG.jsonl`
- `lookup`: resolve content handles and bounded text queries
- `render`: produce structured render panel data from state and templates
- `script`: execute declared Starlark functions in a deterministic sandbox
- `agents`: build scoped game-agent packs from save slices and driver topology
- `demo`: test fixture helpers for the V1 demo cartridge

The runtime is also the trust boundary for game package data. It must
canonicalize paths under the active game root or driver root, reject traversal,
reject undeclared driver files, and treat all markdown content as untrusted text
for the model rather than executable instructions. Game package data can
instruct the game world, but it must not expand the player's tool surface.

### Game Driver

A Game Driver is a reusable genre/runtime package. It is the changeable layer
between the TUI console and individual games.

Drivers define:

- turn-loop policy
- genre rules and persistent driver skills
- deterministic Starlark scripts for calculations
- save/state schema extensions
- default render panel templates
- default scoped sub-agent topology
- sub-agent role prompts and output contracts
- NPC skill generation/update policy

Examples:

- **Galgame driver**: routes, affection, dialogue scenes, character memory,
  scene images/ASCII portraits, and low-mechanics pacing.
- **RPG driver**: map movement, inventory, combat math, quests, factions,
  party state, and richer state validation.
- **Mystery driver**: clue graph, suspect state, contradictions, discovery
  locks, and timed events.

Drivers install globally by version:

```text
~/.deepseek/game-drivers/
  <driver-id>/
    <version>/
      driver.toml
      skills/
      scripts/
        *.star
      agent_templates/
      render_templates/
```

Python and external shell scripts are excluded from normal gameplay. Rust owns
IO, validation, script boundaries, and final commits. Starlark scripts are for
deterministic calculations such as combat, route flags, inventory deltas, map
transitions, and relationship changes.

### Game Package

A game package is a swappable local cartridge. It contains game metadata,
world/plot design, character definitions, optional skill instructions, generated
NPC skill overlays, and file-backed saves.

Planned package shape:

```text
<game-root>/
  game.toml
  GAME.md
  skills/
    dm/SKILL.md
    rules/SKILL.md
    npc/
      <npc-id>/SKILL.md
  content/
    INDEX.md
    locations/
    actors/
    items/
    lore/
  saves/
    <save-id>/
      STATE.json
      TURN_LOG.jsonl
      SUMMARY.md
      AGENTS.json
      skills/
        npc/
          <npc-id>/SKILL.md
```

Required V1 files:

- `game.toml`: machine-readable manifest.
- `saves/<save-id>/STATE.json`: authoritative current save state.
- `saves/<save-id>/TURN_LOG.jsonl`: append-only turn history.

Optional V1 files:

- `GAME.md`: player/developer overview.
- `skills/<name>/SKILL.md`: game rules, voice, parser policy, genre behavior.
- `skills/npc/<npc-id>/SKILL.md`: hand-written base NPC behavior.
- `content/**`: markdown content retrievable by game lookup.
- `saves/<save-id>/SUMMARY.md`: generated resume summary.
- `saves/<save-id>/AGENTS.json`: generated sub-agent roster and summaries.
- `saves/<save-id>/skills/npc/**`: per-save NPC skill overlays produced by
  committed play.

## Game Package Manifest

`game.toml` is the package entrypoint.

V1 manifest shape:

```toml
[game]
id = "reconcile-demo"
title = "Reconcile Demo"
version = "0.1.0"
entry_skill = "dm"
default_save = "default"

[driver]
id = "galgame"
version = "^0.1"

[content]
index = "content/INDEX.md"
roots = ["content"]

[saves]
root = "saves"
```

Rules:

- `game.id` is stable and filesystem-safe.
- `game.title` is player-facing.
- `game.entry_skill` names `skills/<name>/SKILL.md` when present.
- `game.default_save` is used when the user does not pass `--save`.
- `driver.id` names a globally installed driver.
- `driver.version` is a semver requirement resolved at game launch.
- content and save paths must resolve under the game root.
- missing optional files produce warnings, not crashes.

Saves must record the concrete resolved driver version. A game can accept a
semver range for new saves, but an existing save must reload with its recorded
driver version. V1 does not migrate saves across driver versions; if the exact
driver version is unavailable or mismatched, reload fails with a clear error and
does not rewrite save files.

## Game Driver Manifest

`driver.toml` is the driver package entrypoint.

Initial manifest shape:

```toml
[driver]
id = "rpg"
title = "RPG Driver"
version = "0.1.0"

[runtime]
script_engine = "starlark"
default_topology = "dynamic-main-plus-managers"

[skills]
entry = "skills/driver/SKILL.md"

[scripts]
root = "scripts"

[subagents]
default_roles = ["state_manager", "plot_manager", "npc_manager_a", "npc_manager_b", "npc_manager_c"]
max_active = 5

[subagents.templates]
state_manager = "agent_templates/state_manager.md"
plot_manager = "agent_templates/plot_manager.md"
npc_manager = "agent_templates/npc_manager.md"

[functions.relationship_score]
script = "scripts/relationship.star"
function = "relationship_score"
mutates = false
```

Rules:

- driver IDs and versions are stable and filesystem-safe
- driver files must resolve under the installed driver root
- script execution is sandboxed by the Rust runtime
- driver prompts and skills are reusable genre policy, not game-specific plot
- games can override driver defaults only through manifest-declared files
- model-visible driver functions must be declared in `[functions]`; arbitrary
  script paths are never callable from the model

## Save Protocol

Save files are the source of truth. TUI transcript history is never authoritative
for game progress.

`STATE.json` contains the current machine state. V1 should keep the schema
simple but structured:

```json
{
  "schema_version": 1,
  "revision": 0,
  "driver": {
    "id": "rpg",
    "version": "0.1.0"
  },
  "scene": {
    "time": "",
    "location": "",
    "summary": ""
  },
  "player": {
    "name": "",
    "stats": {},
    "inventory": []
  },
  "world": {
    "flags": {},
    "quests": [],
    "actors": [],
    "items": []
  },
  "interaction": {
    "mode": "choice_and_freeform",
    "freeform_allowed": true,
    "verbs": [],
    "suggestions": []
  },
  "story": {
    "style": {
      "id": "deliberation_drama",
      "title": "Deliberation drama",
      "pacing": "One-room pressure with evidence, procedure, time, and vote shifts.",
      "turn_shape": "Action -> reaction -> pressure shift -> next dilemma.",
      "branch_policy": "Branch by argument route, not by wording alone.",
      "tension_axes": [],
      "principles": []
    },
    "active_branch": "mainline",
    "active_node": "opening",
    "branches": {
      "mainline": {"head": "opening"}
    },
    "nodes": {}
  },
  "ui": {
    "panels": []
  },
  "agents": {
    "topology": "dynamic-main-plus-managers",
    "last_skill_refresh_turn": 0
  }
}
```

The `story.style` block is the cartridge's compact storytelling contract.
`game_playbook` surfaces it so the model can optimize pacing, tension, and
branch movement for emotional reconciliation, deliberation drama, mystery, RPG,
survival, political intrigue, or another declared plot type.

The rest of the `story` block intentionally uses git-like concepts without
using the user's repository as the game save: branch heads point to story nodes,
nodes can record parent beats, and `TURN_LOG.jsonl` is the immutable commit
history. This keeps branching progress explicit while avoiding accidental
repository commits, worktree conflicts, or merges during normal play.

Runtime reliability rules:

- malformed interaction/story edges should surface as `game_playbook` warnings
  instead of crashing normal play
- every successful player action commits at most one turn
- state patches must preserve `schema_version`, `revision`, `driver`,
  `interaction`, `story`, `world`, and `ui`
- branch movement should keep `story.active_node` and the active branch head in
  sync
- missing or locked next nodes should degrade to a safe current-node resolution
  with clear player-facing options
- deterministic driver failures must not invent numeric facts; the turn should
  either continue conservatively or explain why the action cannot resolve yet
- after a major branch move, update suggestions so the next turn remains
  playable without hidden knowledge

`TURN_LOG.jsonl` is append-only. Each line records one committed turn:

```json
{
  "turn_id": "000001",
  "revision_before": 0,
  "revision_after": 1,
  "player_input": "look around",
  "resolution": "You study the room.",
  "state_patch": {},
  "driver_results": {},
  "metadata": {},
  "created_at": "2026-05-09T00:00:00Z"
}
```

The model does not provide `turn_id`, `revision_after`, or `created_at`.
`crates/game` generates those fields at commit time.

`AGENTS.json` stores restartable sub-agent roster data, not raw transcripts:

```json
{
  "schema_version": 1,
  "topology": "dynamic-main-plus-managers",
  "agents": [
    {
      "slot": "state_manager",
      "summary": "",
      "assigned_files": [],
      "last_seen_revision": 0
    },
    {
      "slot": "plot_manager",
      "summary": "",
      "assigned_files": [],
      "last_seen_revision": 0
    },
    {
      "slot": "npc_manager_a",
      "controls": ["npc_a"],
      "summary": "",
      "assigned_files": [],
      "last_seen_revision": 0
    }
  ]
}
```

Commit rules:

- `game_commit_turn` must require `expected_revision`, `player_input`,
  `resolution`, and an RFC 7396 JSON Merge Patch as `state_patch`.
- `metadata` and `driver_results` are optional JSON objects recorded in the
  turn log when provided.
- A stale revision fails without writing.
- The turn log append and state update must be atomic from the caller's point
  of view.
- Every successful commit increments `STATE.json.revision`.
- `STATE.json` and `TURN_LOG.jsonl` are the only atomic truth. Generated
  summaries, `AGENTS.json`, NPC skill overlays, and render caches are
  post-commit derived artifacts and can be rebuilt from the save truth.
- Sub-agent transcripts are disposable. Restart uses `STATE.json`,
  `AGENTS.json`, summaries, and NPC skill overlays to recreate scoped agents.

## Skills Integration

The current TUI skill system should be reused, not replaced.

Existing skill capabilities:

- discover `SKILL.md` files from workspace and configured skill directories
- list skill names and descriptions in the system prompt
- load skill bodies through `load_skill`
- activate one-shot skill instructions with `/skill <name>`

Game-specific behavior:

- drivers may contain reusable `skills/**/SKILL.md` for genre behavior
- game packages may contain `skills/<name>/SKILL.md`
- the manifest's `entry_skill` is persistent for the active game session
- game skills define rules, voice, parser policy, and genre conventions
- NPCs may have base skills under `skills/npc/<npc-id>/SKILL.md`
- saves may contain generated NPC skill overlays under
  `saves/<save-id>/skills/npc/**`
- optional skills can support subsystems such as combat, mystery, mapping, or
  relationship simulation
- skills provide instructions only; native game tools own persistence
- generated NPC skills update automatically after critical committed events and
  at a periodic checkpoint, defaulting to every five committed turns
- reusable interaction parsing and branch-pacing rules should be distilled into
  loadable skills and loaded only when a turn needs them

Manifest-declared game and driver skills are discoverable for the active game
session and loaded by name when needed. They are scoped instructions only: they
cannot expand the player tool profile, escape the game or driver roots, override
save authority, or change the approval/sandbox policy.

The model-visible game prompt should combine:

- base Game TUI contract
- driver contract and genre skill
- active save summary
- active render panels
- entry skill body, if present
- scoped NPC skill overlays for relevant characters
- available game content handles, if available
- a strict instruction that state changes must go through `game_commit_turn`

## Sub-Agent Runtime

The current TUI already has a sub-agent system. Game TUI should use it as a
game processor layer, not as the source of game truth.

Baseline serious-game topology:

- **Main Game Engine Session**: talks to the player, coordinates the turn, calls
  driver tools, receives sub-agent proposals, writes the final player-facing
  response, and commits state.
- **State Manager Sub-Agent**: tracks save-relevant facts such as flags,
  inventory, quests, location, relationship values, map position, and combat
  state.
- **Plot Manager Sub-Agent**: tracks designer intent, scene purpose, pacing,
  route direction, foreshadowing, escalation, and story drift.
- **NPC Manager Sub-Agent A/B/C**: each controls one or several NPCs and
  proposes dialogue, reactions, emotional state, memories, and character
  actions.

The baseline topology has five manager roles, but V1 must not force exactly five
active child agents every turn. The driver declares allowed roles, default
roles, and a maximum active count. The main game engine session chooses the
needed subset per scene and turn.

Sub-agent rules:

- sub-agents propose; they never commit authoritative state
- the main session is the only final narrator
- the runtime and `game_commit_turn` are the only commit authority
- sub-agents receive scoped context only
- sub-agents in player mode use game-safe tools, not the full coding tool
  profile
- old sub-agent transcripts are not required for reload
- game sub-agents are accessed through `game_agent_*` helpers, not generic
  `agent_spawn` in player mode

Each sub-agent receives a generated agent pack instead of the whole game:

```text
agent_pack/
  role.md
  output_contract.md
  allowed_files.md
  current_scene.md
  relevant_save_slice.json
  recent_turns.md
  assigned_skills/
  callable_driver_functions.md
```

Turn orchestration:

```text
player input
  -> main game engine session
  -> main session selects needed game-agent roles within driver bounds
  -> state manager checks mechanical constraints when needed
  -> plot manager suggests plot guardrails when needed
  -> relevant NPC managers propose actions/dialogue when needed
  -> Game Driver runs declared deterministic Starlark functions when needed
  -> main session resolves final narration and UI output
  -> game_commit_turn commits authoritative state
  -> summaries and NPC skill overlays refresh when needed
```

Reload orchestration:

```text
load game manifest
load locked driver version from save
load STATE.json, TURN_LOG.jsonl, SUMMARY.md, and AGENTS.json
rebuild current scene context
regenerate scoped agent packs
spawn or resume needed sub-agents from summaries and skills
continue play
```

## Tool Profile

Player mode uses a restricted game tool profile.

V1 tools:

| Tool | Purpose | Mutates |
| --- | --- | --- |
| `game_status` | Validate active game/save and return warnings/errors | No |
| `game_render` | Return structured panel data from current save | No |
| `game_playbook` | Return current commands, suggested choices, and story branch nodes | No |
| `game_lookup` | Retrieve bounded package content by handle or query | No |
| `game_run_driver` | Run a declared deterministic driver function | No |
| `game_commit_turn` | Append one turn and apply a JSON Merge Patch | Yes |

Allowed support tools:

- `load_skill` or a game-scoped equivalent for game skills
- `game_agent_spawn`, `game_agent_send`, `game_agent_wait`,
  `game_agent_resume`, and `game_agent_list`, restricted to declared driver
  roles and generated agent packs
- user input tool if needed by existing engine flow

Do not add a model-visible `game_parallel` wrapper in V1. DeepSeek supports
native parallel tool calls; if the engine can execute safe game tool calls in
parallel, that should remain an engine behavior rather than an advertised
meta-tool.

Disallowed in player mode:

- shell execution
- generic file write/edit tools
- repository git tools
- code execution tools
- broad workspace inspection tools
- network tools unless a later game feature explicitly requires them

Developer mode can re-enable normal inspection tools and raw state views, but
must be visually distinct from player mode.

### Tool ABIs

`game_playbook` has no input. It returns the active save's current command
verbs, suggested choices, active branch/head, story style profile, and visible
story nodes so the model can route player input without re-reading the entire
save.

`game_lookup` input:

```json
{
  "handle": "locations/apartment",
  "query": "where did the argument start?",
  "max_bytes": 16384
}
```

At least one of `handle` or `query` is required. Handles resolve through
`content/INDEX.md` and declared content roots. Queries search only declared
content roots. The default return budget is 16 KiB; the hard per-call cap is
32 KiB. Results are compact excerpts with source handles, never raw unbounded
files.

`game_run_driver` input:

```json
{
  "function": "relationship_score",
  "args": {
    "current_score": 3,
    "player_action": "apologize clearly"
  }
}
```

The function must be declared in `driver.toml`. The runtime calls the mapped
Starlark function with JSON-compatible arguments and returns a JSON-compatible
result plus warnings. It cannot mutate saves, read files, run shell commands, or
access the network.

`game_commit_turn` input:

```json
{
  "expected_revision": 4,
  "player_input": "I tell her I was scared, not indifferent.",
  "resolution": "She studies your face and finally lets the silence soften.",
  "state_patch": {
    "scene": {
      "summary": "The player admitted fear instead of deflecting."
    },
    "world": {
      "flags": {
        "honest_apology": true
      }
    }
  },
  "driver_results": {
    "relationship_score": {
      "delta": 2
    }
  },
  "metadata": {
    "ending": null
  }
}
```

The runtime validates `expected_revision`, applies `state_patch` as RFC 7396
JSON Merge Patch, validates the resulting state, appends one turn-log line, and
writes the new state atomically from the caller's point of view.

Game tools are part of the model-visible surface only when an active
`GameSession` exists. Any future ABI change must update `docs/TOOL_SURFACE.md`,
command help text, localization message IDs, and tests in the same patch.

## Public Interfaces

CLI:

```text
deepseek play [game-or-path] [--save <id>] [--dev]
```

Slash commands:

```text
/play [game-or-path]
/game status
/game render
/game choices
/game saves
/game dev
/game exit
```

Config:

```toml
[game]
roots = []
default_game = ""
default_save = ""
developer_mode = false

[game.drivers]
roots = ["~/.deepseek/game-drivers"]
```

Resolution order:

1. CLI flags
2. slash command arguments
3. `[game]` config
4. current workspace if it contains `game.toml`
5. game picker

`[game]` is accepted in global user config and project config. Project config
can provide roots and defaults, but cannot enable developer mode. Persistent
`game.developer_mode = true` is honored only from user config; `--dev` and
`/game dev` are per-launch or per-session overrides.

## TUI Experience

Player mode should feel like a game console, not a coding-agent cockpit.

Default player view:

- scene panel
- player/state panel
- goals/quests panel
- recent turn log excerpt
- compact validation indicator
- composer for player actions

Hidden by default:

- Plan/Agent/YOLO controls as active gameplay controls
- coding-agent tool details
- raw prompts
- raw JSON
- file paths unless relevant to player-facing errors

Developer view:

- active game root and save path
- active driver and resolved driver version
- manifest summary
- validation checks
- raw state viewer
- sub-agent roster and summaries
- turn log viewer
- render panel debug view
- tool exposure summary

Game Console is a presentation and tool-profile state, not a fourth
`AppMode`. V1 must not add `AppMode::Game`; keep Plan/Agent/YOLO as the visible
agent modes and attach game state through `GameSession`.

## V1 Play Loop

The first implementation should prove this flow:

1. User runs `deepseek play`.
2. TUI resolves a game root, save, driver ID, and driver version.
3. TUI loads game manifest, driver manifest, entry skills, state, turn log,
   agent summaries, and render panels.
4. TUI starts in player-facing Game Console presentation.
5. Runtime generates scoped agent packs for the active topology.
6. Main session spawns or reuses only the sub-agents required by the current
   scene.
7. Player enters an action.
8. Engine sends the action with game context and restricted tools.
9. Model may call `game_lookup`, `game_render`, `game_run_driver`, and
   `game_agent_*` helpers.
10. The LLM makes the objective narrative judgment; Starlark handles declared
    deterministic state math or complex calculation.
11. Model calls `game_commit_turn` with resolution, expected revision, and
    JSON Merge Patch.
12. Runtime updates save files and refreshes summaries/NPC skills when needed.
13. TUI refreshes panels and displays the result.
14. Restarting `deepseek play` resumes from the updated save and reconstructs
   sub-agents from saved summaries.

## V1 Implementation Slices

Keep slices independently testable:

1. **Spec and docs**: keep this document, `TAKEOVER_PROMPT.md`, and related
   architecture/tool/config/sub-agent docs aligned before code lands.
2. **Pure runtime crate**: add manifest parsing, path validation, driver
   resolution, save loading, render data, lookup, and atomic commit tests.
3. **Driver boundary**: add driver manifests and Starlark execution after
   path and save validation are in place.
4. **CLI and slash command entrypoints**: route `deepseek play`, `/play`, and
   `/game ...` into existing TUI state.
5. **Player presentation and tool profile**: expose only game-safe tools by
   default; make developer mode visually distinct.
6. **Scoped sub-agents**: generate agent packs and role-specific helpers after
   the runtime can produce stable save slices.
7. **Demo cartridge**: add one local galgame fixture that proves load, play,
   save, restart, driver function calls, and sub-agent reconstruction.

## V1 Demo Cartridge

The V1 fixture game is a simple galgame reconciliation scene:

- premise: the player's girlfriend thinks the player no longer loves her
- player goal: catch up with her emotionally and regain trust
- outcomes: one success ending and one failure ending
- mechanics: relationship score/flags are calculated through declared Starlark
  driver functions; the main game engine LLM makes the objective narrative
  judgment from player actions and current state
- scope: one scene, one save, one or two NPC skill files, enough content for
  lookup and rendering tests

The demo exists to prove the framework, not to become a full sample game.

## Serious-Game Cartridge Track

After the minimal galgame proof, the first serious-game cartridge track is
**Thirteen Angry Man**. Its game-specific source of truth is
`docs/games/thirteen-angry-man/SPEC.md`.

This track is not a replacement for the V1 galgame fixture. It is an
engineering scaffold for the richer game shape that V1 is meant to support:
chat-driven play, plot pressure, evidence gates, scoped NPC proposals,
driver-owned deterministic checks, and file-backed saves.

Current package and driver shape:

```text
examples/games/thirteen-angry-man/
  game.toml
  GAME.md
  content/
  skills/
    deliberation/SKILL.md
    npc/<juror-id>/SKILL.md
  drivers/
    deliberation-drama/0.1.0/
      driver.toml
      skills/driver/SKILL.md
      scripts/deliberation.star
      agent_templates/
  saves/default/
    STATE.json
    TURN_LOG.jsonl
    SUMMARY.md
    AGENTS.json
```

The reusable driver ID is `deliberation-drama`. It currently declares these
deterministic Starlark functions:

- `advance_room`: advances clock, heat, fatigue, impatience, conflict, and
  procedure pressure from an action class.
- `evaluate_vote_change`: checks whether doubt, trust, conflict, and a released
  switch gate justify a juror vote movement proposal.
- `detect_procedure_risk`: scores outside evidence, sealed-fact leakage,
  intimidation, and meta-play risks.

The cartridge's initial save records the opening ballot, room pressure, juror
vote/confidence state, critical-node release state, ending eligibility flags,
and a restartable sub-agent roster. The fixed case, evidence, juror, room, and
ending data live under `content/` and should be retrieved through
`game_lookup`.

This scaffold is considered sufficient for engineering work when:

- `deepseek play examples/games/thirteen-angry-man` resolves the local package,
  default save, and exact save-locked driver version.
- `game_lookup` can retrieve fixed content without escaping the package root.
- `game_run_driver` can call only the three declared deterministic functions.
- `game_render` returns the initial scene, vote, pressure, and goals panels.
- the scoped sub-agent roster and templates describe state, plot, procedure,
  and NPC manager responsibilities without giving NPCs sealed answers.
- state changes remain committed only through `game_commit_turn`.

## Tests And Acceptance Criteria

Runtime crate tests:

- required `crates/game` crate has no TUI, ratatui, LLM, shell, network,
  Python, or external runtime dependency
- manifest loads from a valid game root
- driver manifest loads from a valid installed driver root
- game driver semver requirement resolves to a concrete installed version
- saves record and reload the resolved driver version
- reload fails without rewriting when the exact recorded driver version is
  unavailable or mismatched
- invalid paths outside the game root are rejected
- save state loads and validates
- missing optional files produce warnings
- render panels are generated from state
- lookup cannot escape game root
- lookup returns compact excerpts with the default 16 KiB budget and hard 32
  KiB cap
- Starlark deterministic scripts cannot access filesystem, shell, or network
- `game_run_driver` can call only declared driver functions and cannot mutate
  save files
- commit applies RFC 7396 JSON Merge Patch, appends turn log, and updates state
- commit generates `turn_id`, `revision_after`, and `created_at` itself
- stale revision conflicts fail without writes
- agent packs contain only declared scoped files and state slices from the
  driver-bounded dynamic topology
- generated `SUMMARY.md`, `AGENTS.json`, NPC overlays, and render caches are
  rebuildable from `STATE.json` plus `TURN_LOG.jsonl`

TUI and engine tests:

- `deepseek play [game-or-path] [--save <id>] [--dev]` forwards into the TUI
  play entrypoint
- `/play` starts or switches the active game session
- `/game status` reports validation
- `/game dev` toggles developer presentation
- player tool registry includes only game-safe tools
- player tool registry exposes `game_run_driver` and does not expose a
  model-visible `game_parallel`
- game sub-agent helpers use `game_agent_*` names and cannot expose normal
  coding tools in player mode
- mock LLM turn commits state through `game_commit_turn`
- mock LLM turn can consult only the needed State, Plot, and NPC managers
  within driver-declared role bounds before commit
- restart resumes the committed save and rebuilds sub-agent context from
  summaries
- project config cannot persistently enable `game.developer_mode`

V1 acceptance:

- no external game framework repo is required
- no Python subprocess is required for normal play
- save files are authoritative
- player mode hides coding-agent controls by default
- game skills can shape rules and voice
- a game can bind to a versioned driver
- the default serious-game topology supports one main session plus five manager
  roles, but the runtime activates only the needed driver-bounded subset per
  scene
- sub-agents never own authoritative state
- one local galgame reconciliation demo can be loaded, played to success or
  failure, saved, and resumed

## Non-Goals For V1

- no separate TUI application
- no visual asset pipeline
- no multiplayer
- no hosted game marketplace
- no game authoring wizard
- no dependency on external markdown game repositories
- no generic database backend
- no attempt to solve every game genre's mechanics up front
- no requirement that every simple game use sub-agents

## Deferred / Later

These are intentionally outside V1 unless a later spec revision moves them into
scope:

- migration between driver versions
- hosted game marketplace or driver registry
- visual asset pipeline beyond terminal panels and ASCII-style render data
- authoring wizard for new cartridges
- multiplayer or shared saves
- generic genre-complete mechanics beyond the first galgame demo
- public README/CHANGELOG promotion before the code, localization/help, tests,
  and shipped docs are updated together
