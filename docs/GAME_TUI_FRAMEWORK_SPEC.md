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

## Current Implementation Status

As of 2026-05-09, the repository has the Game Console scaffold needed for the
next feature branch:

- `crates/game` exists as the pure Rust runtime crate for manifests, driver
  resolution, saves, lookup, render panels, Starlark driver functions, story
  playbooks, atomic commits, and scoped agent packs.
- `deepseek play`, `/play`, `/game status`, `/game render`, `/game choices`,
  `/game rules`, `/game saves`, `/game dev`, and `/game exit` are wired into
  the existing TUI through `GameSession`.
- Player-mode tool registration exposes native `game_*` tools, `load_skill`,
  and game-scoped `game_agent_*` helpers while excluding normal coding tools.
- The native game tool surface includes `game_status`, `game_render`,
  `game_playbook`, `game_lookup`, `game_fact_check`, `game_run_driver`, and
  `game_commit_turn`.
- `examples/games/reconciliation-demo` is the minimal galgame proof fixture;
  `examples/games/thirteen-angry-man` is the first serious-game cartridge
  scaffold.
- The reconciliation demo now carries a restartable `AGENTS.json` roster and a
  Rei NPC skill; the driver's generic `dialogue` default role expands into an
  individual active-NPC pack such as `dialogue_girlfriend`.

Still reserved or incomplete:

- `[game]` config keys are documented as a future surface but are not yet read by
  the loader or `/config` UI.
- The game picker and globally managed driver installation UX are not part of
  the active scaffold.
- V1 should not be called complete until the acceptance criteria below have
  focused tests and the player-facing hiding/dev diagnostics behavior is
  verified end to end.

## Immersive Game Console Goal

The next player-facing UI branch should rework player-mode Game Console into an
immersive game screen inside the existing TUI, not a coding/chat cockpit.
`GameSession` remains the mode carrier; do not add `AppMode::Game`.

The current gap is that `game_render` already returns structured panels, but
player mode still mostly displays them as transcript text with coding-oriented
header, footer, and sidebar chrome. The new default player view should project
game state into a dedicated `GameConsoleWidget` with fixed-ratio ASCII scene
and figure art, game status, items, tasks or quests, dialogue, choices, and a
player-action composer.

Key UI behavior:

- render the dedicated `GameConsoleWidget` when
  `game_player_presentation(app)` is true
- hide normal coding UI in player mode, including Plan/Todos/Tasks/Agents
  sidebars, raw tool cells, thinking cells, model/cost/context footer noise,
  file paths, raw JSON, and coding status labels
- keep developer mode as the escape hatch: `/game dev on` restores diagnostics,
  raw render panels, tool activity, save paths, driver info, and sub-agent
  roster visibility
- keep normal transcript history for persistence, search, developer mode, and
  export, but project only player-facing user and assistant turns into the Game
  Console log
- use a composer dedicated to player actions, with game-specific placeholder
  text instead of coding-agent prompt text

The widget should use fixed terminal-cell ratio boxes:

- **Scene plot**: the largest fixed-ratio ASCII canvas, letterboxed or
  pillarboxed on resize
- **Figure/portrait**: fixed-ratio active speaker or NPC canvas
- **Items**: compact inventory and world item window
- **Status**: player stats, room/vote/pressure metrics, and validation
  indicator
- **Tasks**: game-assigned quests, objectives, or story beats, not coding
  task-manager tasks
- **Dialogue/log**: latest exchange and last turn consequence
- **Composer**: player action input only

Responsive tiers:

- **Wide terminals**: scene on the left; figure, status, items, and tasks
  stacked on the right; dialogue and choices below
- **Medium terminals**: scene on top; side windows below in columns
- **Narrow terminals**: single-column fixed-ratio scene followed by compact
  rotating or tabbed status, items, and tasks

Gameplay must continue if rich art is unavailable. If model-authored ASCII art
is missing, stale, invalid, or too large for the available tier, the widget
falls back to deterministic text panels generated from the current render view.

## Scene Music Goal

Game TUI should support optional scene-aware background music because games are
not only text and visuals; pacing, silence, and ending stingers are part of the
play surface. Music is optional game presentation data, not game truth.

The first adapter candidate is [`kew`](https://github.com/ravachol/kew), a
terminal music player described by its project as "Music for the Shell." It
supports local playback from the command line, `play <path>`, `--noui`, and
`--quitonstop`, which make it plausible as a background player for scene and
ending cues. The GitHub project is GPL-2.0 and notes that active development has
moved to Codeberg, so implementation must treat `kew` as an optional
locally installed process adapter, not vendored source, linked Rust code, or a
default shipped dependency.

Music behavior:

- cartridges may declare local music cues for scenes, pressure states, and
  endings
- cue selection is driven by designer-authored cue rules, game state, render
  snapshot, or committed turn metadata
- the TUI may start, stop, fade, or replace a cue when scene, active story node,
  pressure tier, or ending changes
- player mode hides music controls and adapter logs; music is part of the
  authored presentation, not a player-operated subsystem
- gameplay, save loading, and rendering must continue when no local audio
  adapter is installed, when audio is disabled, or when playback fails
- audio playback is local only in V1: no streaming services, remote URLs,
  downloads, telemetry, Discord status integration, or hosted music providers

Architecture constraints:

- `crates/game` may validate cue declarations and return cue IDs in render/view
  data, but it must not spawn audio processes or depend on `kew`
- the TUI owns adapter lifecycle, process cleanup, cue-level volume application,
  and developer diagnostics; it must not expose a player-facing music control
  panel
- `game_commit_turn` remains the only save writer; accepted cue changes are
  committed as save-state patch data, while external process state is not
  authoritative
- player mode does not expose a model-visible generic music-control tool in V1;
  cue changes flow through state, render, and commit data
- developer mode may show adapter name, executable path, current cue, last
  command status, and playback errors

## Architecture

Game TUI has four engineering pieces.

### Existing Code Integration Map

The implementation should extend current runtime seams rather than fork the
application:

| Concern | Current first stop | Current Game TUI shape |
| --- | --- | --- |
| `deepseek play` dispatch | `crates/cli` and `crates/tui/src/main.rs` | Launches the existing TUI with game session intent. |
| Slash command metadata/routing | `crates/tui/src/commands/` | `/play` and `/game ...` reuse the normal slash-command system. |
| App state and rendering | `crates/tui/src/tui/app.rs`, `tui/ui.rs`, `tui/mod.rs` | Uses `GameSession` plus a Game Console presentation/tool profile. |
| Tool exposure | `crates/tui/src/tools/registry.rs`, `core/engine/tool_setup.rs` | Player mode uses a game-safe whitelist; developer mode can use the wider profile. |
| Skills | `crates/tui/src/skills/`, `skill_state.rs`, `tools/skill.rs` | Game and driver skill roots feed the existing discovery/loading path. |
| Sub-agents | `crates/tui/src/tools/subagent/`, `tui/subagent_routing.rs` | `game_agent_*` helpers wrap the existing sub-agent runtime with scoped packs. |
| Persistence | `session_manager.rs`, `runtime_threads.rs`, `crates/game` | Keep chat/session persistence separate from authoritative game saves. |

### TUI Game Console

The existing TUI is the console. Do not create a separate terminal app or event
loop.

The console owns:

- the main player-facing game engine session
- Game Console presentation using existing ratatui surfaces
- a dedicated player widget at `crates/tui/src/tui/widgets/game_console.rs`
- optional scene-aware background music adapter lifecycle
- game picker and save picker views
- `/play` and `/game` slash commands
- player-facing status/header/footer profile
- restricted game-safe tool profiles
- orchestration of driver tools and scoped sub-agents

V1 must not add `AppMode::Game`; use `GameSession` plus presentation and
tool-profile state.

The TUI render path should branch in `crates/tui/src/tui/ui.rs` so player-mode
Game Console uses `GameConsoleWidget` instead of the normal `ChatWidget` plus
coding sidebar. After successful `game_render` and `game_commit_turn` tool
results, the app should refresh `app.game_session` panels and player-view data
by parsing the returned JSON from `ToolResult.content`.

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
- `render`: produce structured render panel data and additive player-view
  snapshots from state and templates, including optional music cue IDs
- `script`: execute declared Starlark functions in a deterministic sandbox
- `agents`: build scoped game-agent packs from save slices and driver topology
- `demo`: test fixture helpers for the V1 demo cartridge

The runtime is also the trust boundary for game package data. It must
canonicalize paths under the active game root or driver root, reject traversal,
reject undeclared driver files, and treat all markdown content as untrusted text
for the model rather than executable instructions. Game package data can
instruct the game world, but it must not expand the player's tool surface.

The `render` module should keep the existing structured panel output and add a
player-view snapshot for the immersive widget. Additive render interfaces:

- new panel kinds: `figure`, `items`, `status`, and `tasks`
- `AsciiArtFrame`: one validated terminal-cell art frame with declared columns,
  rows, lines, and a fixed cell ratio
- `AsciiArtVariant`: a size or tier-specific frame candidate for scene or
  figure art
- `GameViewSnapshot`: player-facing scene, figure, items, status, tasks,
  dialogue, choices, validation, optional ASCII art data, and optional music
  cue data for the current save revision

`game_render` should return both the existing panels and the new view object so
developer mode, compatibility tests, and future exporters can continue reading
the original panel model while player mode reads the richer snapshot.

### Game Driver

A Game Driver is a reusable genre/runtime package. It is the changeable layer
between the TUI console and individual games.

Drivers define:

- turn-loop policy
- genre rules and persistent driver skills
- deterministic Starlark scripts for calculations
- save/state schema extensions
- default render panel templates
- default music cue policy
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
  assets/
    music/
  saves/
    <save-id>/
      STATE.json
      TURN_LOG.jsonl
      SUMMARY.md
      AGENTS.json
      skills/
        npc/
          <npc-id>/SKILL.md
  save_templates/
    <save-id>/
      STATE.json
      TURN_LOG.jsonl
      AGENTS.json
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
- `save_templates/<save-id>/**`: immutable starting saves used when creating a
  missing explicit `--save <id>`. Templates are read-only package data, not live
  play state.

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
template_root = "save_templates"

[audio]
enabled = false
assets = "assets/music"
adapter = "kew"

[[audio.cues]]
id = "scene_default"
file = "assets/music/scene_default.flac"
scope = "scene"
loop = true
volume = 60

[[audio.cues]]
id = "ending_success"
file = "assets/music/ending_success.flac"
scope = "ending"
loop = false
volume = 70
```

Rules:

- `game.id` is stable and filesystem-safe.
- `game.title` is player-facing.
- `game.entry_skill` names `skills/<name>/SKILL.md` when present.
- `game.default_save` is used when the user does not pass `--save`.
- When the user passes a filesystem-safe `--save <id>` that does not exist,
  player mode creates it from `saves.template_root/game.default_save` when that
  template root is configured, otherwise from `saves.root/game.default_save`.
  The created live save gets an empty `TURN_LOG.jsonl`, so
  `deepseek play ... --save new1` starts a separate run without mutating the
  template.
- `driver.id` names a globally installed driver.
- `driver.version` is a semver requirement resolved at game launch.
- content and save paths must resolve under the game root.
- audio asset paths must resolve under the game root and must be local files,
  not URLs.
- cue IDs are stable game data. Missing optional audio files warn and disable
  the affected cue rather than failing the game.
- cue volume and loop behavior are designer-authored cue properties, not
  player-facing controls.
- `audio.adapter = "kew"` declares a preferred adapter only. The runtime must
  still work when no adapter is installed or configured.
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
  "plot": {
    "premise": "",
    "background": "",
    "opening_conflict": "",
    "player_role": "",
    "genre": ""
  },
  "scene": {
    "time": "",
    "location": "",
    "summary": "",
    "what_happened": "",
    "immediate_stakes": "",
    "mood": "",
    "sensory": []
  },
  "cast": [],
  "conversation": {
    "current_speaker": "",
    "prompt": "",
    "available_topics": [],
    "last_exchange": []
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
    "skills": [
      {
        "id": "chat",
        "label": "Chat",
        "skill": "game-action-chat",
        "description": "Player speech and dialogue.",
        "freeform": true,
        "aliases": ["say", "ask", "tell"]
      }
    ],
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
    "panels": [],
    "ascii": {
      "source_revision": 0,
      "scene_art": [],
      "figure_art": [],
      "ratios": {
        "scene": {"cols": 4, "rows": 3},
        "figure": {"cols": 1, "rows": 1}
      },
      "variants": []
    },
    "music": {
      "source_revision": 0,
      "active_cue": "",
      "scene_cue": "",
      "ending_cue": null
    }
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

- each playable save should carry enough `plot`, `cast`, and `conversation`
  data to establish what happened, who is present, what they want, and what was
  just said before showing choices
- game entry is language-gated before the TUI session starts: launch accepts
  `--lang en|zh`, interactive launches may prompt before opening the TUI, and
  the first player-facing beat should begin directly in the selected language
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
- update recommended/suggested choices only when the story is drifting in the
  wrong direction or the player needs a slight reorientation; never expose
  hidden gates, exact scores, best routes, or decisive route-solving hints
- `ui.ascii` stores accepted model-authored ASCII art under save state with the
  source revision, scene art, figure art, fixed terminal-cell ratios, and one or
  more size variants
- ASCII art is a render cache owned by the save state, not separate authority;
  if it is stale or invalid, gameplay continues with deterministic render
  panels
- `ui.music` records the intended cue state for resume and rendering, but the
  external audio process is never authoritative and can be restarted or ignored
  without changing game truth
- scene and ending cue changes should be committed with the same turn patch that
  changes the story node, pressure tier, or ending status

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

- `game_commit_turn` appends only when it has `player_input` and `resolution`.
  `expected_revision` is inferred from the active save when omitted, and
  `state_patch` may be omitted for an empty patch.
- `game_commit_turn` must run the same continuity fact gate before writing and
  refuse blocked claims without mutating the save.
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

ASCII validation rules:

- line count must match the declared row count
- each line must fit the declared column count in terminal cells
- ANSI escape sequences are forbidden
- excessive Unicode width falls back to sanitized ASCII
- oversized variants are rejected rather than wrapped
- invalid or missing variants fall back to deterministic text panels without
  blocking the turn loop

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

### Game Turn Controller

Player-mode Game Console uses an explicit controller contract instead of relying
on many prompt layers to independently steer the same behavior. The controller
owns the turn order and invariants; skills, driver prompts, and sub-agents are
scoped modules that supply local policy or proposals. The prompt text lives in
`crates/tui/src/prompts/game_console.md` and is composed by `prompts.rs` as a
single Game Console prompt file.

Closed-loop turn sequence:

```text
observe -> classify -> estimate -> constrain -> commit -> render
```

Priority order:

```text
controller > save invariants > action skill > driver skill > NPC proposal > storytelling style
```

Controller responsibilities:

- observe the active save revision, render view, playbook, and needed facts
- classify the exact player input into one declared action skill when action
  skills are present
- estimate consequence with only needed skills, scoped game sub-agents, and
  deterministic driver calls
- constrain output through language, fact gates, branch consistency, and
  player-mode hiding before narration or commit
- commit exactly one authoritative turn with `game_commit_turn`
- render only player-facing scene, dialogue, visible consequence, and concise
  choices

Required invariants:

- `story.active_node` stays aligned with
  `story.branches[story.active_branch].head`
- `game_commit_turn.expected_revision` matches the current save revision
- sub-agents propose only; the main game session remains final narrator and
  commit authority
- normal player mode never reveals tool calls, waits, routing, hidden scores,
  branch gates, or controller trace text

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
- **Render Artist Sub-Agent**: proposes fixed-size JSON ASCII art variants for
  the current scene and active speaker or NPC. It is spawned through the
  existing `game_agent_spawn` surface, never writes save state, and returns
  proposals for the main session to validate and commit.

The baseline topology has five manager roles, but V1 must not force exactly five
active child agents every turn. The driver declares allowed roles, default
roles, and a maximum active count. The main game engine session chooses the
needed subset per scene and turn.

For small character scenes, a driver may declare a generic `dialogue` role. The
runtime expands that role into per-active-NPC packs, for example
`dialogue_girlfriend` for Rei in the reconciliation demo. These packs include
only the NPC's cast slice, relevant scene/conversation/backstory facts, allowed
content files, and NPC skill path. This keeps a named character from being
handled by a generic role with no character-specific memory.

Sub-agent rules:

- sub-agents propose; they never commit authoritative state
- the main session is the only final narrator
- normal player mode never narrates model analysis, routing, rules loading,
  tool calls, waits, sub-agent status, hidden scores, branch/gate evaluation, or
  praise/scoring of the player's choice
- the runtime and `game_commit_turn` are the only commit authority
- sub-agents receive scoped context only
- sub-agents in player mode use game-safe tools, not the full coding tool
  profile
- old sub-agent transcripts are not required for reload
- game sub-agents are accessed through `game_agent_*` helpers, not generic
  `agent_spawn` in player mode
- player mode prewarms declared packs when the Game Console opens: each
  prewarmed processor uses `deepseek-v4-flash` with thinking disabled, returns
  a minimal ready handoff, and stays running across `game_agent_send`
  assignments
- `game_agent_wait` and blocking `game_agent_result` reads use short,
  player-facing timeouts and preserve still-running waited processors on
  timeout; those processors continue in parallel and may later resume the main
  turn with a proposal, while the main session must not repeatedly wait on a
  slow processor
- `game_agent_spawn` must bind to a declared generated pack such as
  `dialogue_girlfriend` when packs are available, so active NPC dialogue does
  not fall back to an unscoped generic worker
- `render_artist` may use only game-safe read tools: `game_status`,
  `game_render`, `game_lookup`, `game_fact_check`, `game_run_driver`, and
  `load_skill`
- `render_artist` returns JSON ASCII proposals only and cannot call
  `game_commit_turn`
- the main game session automatically asks `render_artist` for proposals when
  art is missing, stale, or the scene or active speaker changes
- accepted art is stored by the main session through the same
  `game_commit_turn` state patch as the turn resolution

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
  -> render artist proposes ASCII scene/figure variants when needed
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
| `game_fact_check` | Check an action or proposed narration against continuity facts | No |
| `game_run_driver` | Run a declared deterministic driver function | No |
| `game_commit_turn` | Append one turn and apply a JSON Merge Patch | Auto in player mode |

Allowed support tools:

- `load_skill` or a game-scoped equivalent for game skills
- `game_agent_spawn`, `game_agent_wait`, `game_agent_result`,
  `game_agent_send`, `game_agent_resume`, `game_agent_assign`,
  `game_agent_cancel`, and `game_agent_list`, restricted to declared driver
  roles and generated agent packs
- `render_artist` as a game-scoped role, restricted to game-safe read tools and
  JSON ASCII proposal output
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

On game entry, the TUI must show a player-facing language-preference prompt and
the basic how-to-play guide before choices. The guide should make clear that the
player may type natural language actions, dialogue, numbered choices, or bracket
commands, while the cartridge framework preserves state, branch gates, and
consequences. Players can repeat the guide at any time with `/skill rule-repeat`
or `/game rules`.

Normal player mode must hide model thinking and game tool-call transcript cells.
The player should see the scene, dialogue, choices, and consequences, not the
engine's lookup, driver, fact-check, or commit plumbing. `/game dev on` may
expose diagnostics for cartridge authors.

Free-form actions are allowed, but they do not get to rewrite established
continuity. When a player action introduces a new biology, identity, family,
legal, location, or backstory fact, the agent must run the fact gate before
narrating or committing the turn. If the gate blocks the claim, the game should
ask for revision or handle it as an impossible/false in-world statement, not
make it true.

### Tool ABIs

`game_playbook` has no input. It returns the active save's declared action
skills, current command verbs, suggested choices, active branch/head, story
style profile, and visible story nodes so the model can route player input
without re-reading the entire save. When action skills are present, every
player action must be distilled to one declared action skill; free-form wording
is allowed inside a skill, not outside the skill set.

`game_lookup` input:

```json
{
  "handle": "locations/apartment",
  "query": "where did the argument start?",
  "state_path": "world.flags",
  "max_bytes": 16384
}
```

At least one of `handle`, `query`, or `state_path` is expected. `key` is accepted
as a repair alias for `state_path`. Handles resolve through `content/INDEX.md`
and declared content roots. Queries search only declared content roots. State
paths read bounded active-save JSON values using dot paths such as `world.flags`
or JSON pointers such as `/world/flags`. Empty calls return a compact usage guide
instead of failing. The default return budget is 16 KiB; the hard per-call cap is
32 KiB. Content results are compact excerpts with source handles, never raw
unbounded files.

`game_fact_check` input:

```json
{
  "player_action": "其实我怀了你的孩子。",
  "resolution": "optional proposed narration"
}
```

The tool checks the action and optional proposed narration against active-save
continuity facts and cartridge-defined `/facts/fact_gate/rules`. It returns
`allowed`, `hard_block`, flags, a reason, and a correction. It should run before
narrating or committing free-form actions that introduce new biology, identity,
family, legal, location, or backstory facts. Blocked claims must not be
committed as truth. A cartridge can define generic rule patterns, `block_if`
state predicates, and `unless_path` exceptions in save state; this is a
workflow-level gate, not a one-off lie detector.

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

The function must be declared in `driver.toml`. When a driver declares exactly
one function, the runtime may infer it for repair, but cartridges should still
teach agents to pass the explicit function. Top-level `player_action` or
`action` is copied into `args.player_action` when the model omits the nested
arg. The runtime calls the mapped Starlark function with JSON-compatible
arguments and returns a JSON-compatible result plus warnings. It cannot mutate
saves, read files, run shell commands, or access the network.

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

The runtime infers `expected_revision` when omitted, runs the continuity fact
gate, applies `state_patch` as RFC 7396 JSON Merge Patch, validates the
resulting state, appends one turn-log line, and writes the new state atomically
from the caller's point of view. If the fact gate blocks the turn, the tool
returns a non-mutating block result instead of writing.

Driver outputs are ordinary state estimates, not the only legal save values.
If a cartridge uses a terminal sentinel outside a driver range, the sentinel
must be documented in the game save contract, tied to explicit terminal state
flags/nodes, and covered by a focused commit-normalization test.

Game tools are part of the model-visible surface only when an active
`GameSession` exists. Any future ABI change must update `docs/TOOL_SURFACE.md`,
command help text, localization message IDs, and tests in the same patch.

## Public Interfaces

CLI:

```text
deepseek play [game-or-path] [--save <id>] [--lang en|zh] [--dev]
```

Slash commands:

```text
/play [game-or-path] [--save <id>] [--lang en|zh] [--dev]
/game status
/game render
/game choices
/game saves
/game dev
/game exit
```

Reserved config:

```toml
[game]
roots = []
default_game = ""
default_save = ""
developer_mode = false

[game.drivers]
roots = ["~/.deepseek/game-drivers"]

[game.audio]
enabled = false
adapter = "none"
kew_path = ""
```

The config keys are part of the V1 target surface but are not yet read by the
current loader or `/config` UI. Current launch resolution is explicit CLI/slash
argument first, then the current workspace if it contains `game.toml`; driver
lookup checks the game package's local `drivers/` directory and
`~/.deepseek/game-drivers`.

`[game.audio]` is an adapter capability setting only. It must not become a
player-facing music-control surface; cue choice, loop behavior, and cue volume
belong to the cartridge designer and committed game state.

Target resolution order:

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

Default player view is `GameConsoleWidget`:

- game language is selected before TUI startup and is limited to English or
  Chinese; scene, dialogue, choices, and panel labels align to that selection
- scene/background panel for the current scene and opening context; the opening
  must always restate the background story in the selected language
- fixed-ratio active figure or portrait ASCII canvas
- compact status panel with player stats, room/vote/pressure metrics, and
  validation indicator
- compact inventory/world items panel
- game tasks panel for quests, objectives, and story beats
- scrollable dialogue/log panel for the current chat history; the dialogue
  panel remains visible during tool use and turn resolution
- dialogue/log content should stay limited to live chat, immediate narration,
  and the latest in-character response; status, tasks, items, choices, scene,
  and story context belong in their own visible panels and should not be
  duplicated in Dialogue
- if the dialogue panel is rendered as plain text, game narration must not emit
  Markdown-only headings, horizontal rules, bold markers, or raw Markdown lists
  into that panel
- choice list when available
- composer for player actions only

Hidden by default:

- Plan/Agent/YOLO controls as active gameplay controls
- Plan/Todos/Tasks/Agents sidebar content
- coding-agent tool details and raw tool cells
- model thinking cells
- model/cost/context footer noise
- coding status labels
- raw prompts, raw render JSON, and other raw JSON
- music controls and music cue picker
- audio adapter command output
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
- raw render panels and player-view JSON
- tool activity and hidden transcript cells
- audio adapter diagnostics, executable path, current cue, and last playback
  error
- designer-authored cue rules and committed cue state
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
10. If scene or speaker art is missing or stale, the main session may ask the
    `render_artist` game agent for JSON ASCII proposals.
11. The LLM makes the objective narrative judgment; Starlark handles declared
    deterministic state math or complex calculation.
12. Model calls `game_commit_turn` with resolution, expected revision, and
    JSON Merge Patch, including accepted ASCII art and music cue updates when
    applicable.
13. Runtime updates save files and refreshes summaries/NPC skills when needed.
14. TUI refreshes panels, player-view snapshot, ASCII art, status, items, and
    tasks from tool results.
15. If audio is enabled, the TUI reconciles the active local music adapter with
    the committed cue state.
16. Restarting `deepseek play` resumes from the updated save and reconstructs
   sub-agents from saved summaries.

## V1 Implementation Slices

Keep slices independently testable. The current scaffold has landed the core
runtime, entrypoints, native tools, player profile wiring, and demo cartridges;
future branches should verify and complete the remaining gaps rather than
recreate those pieces.

1. **Spec and docs**: keep this document, `TAKEOVER_PROMPT.md`, and related
   architecture/tool/config/sub-agent docs aligned before code lands.
2. **Pure runtime crate**: add manifest parsing, path validation, driver
   resolution, save loading, render data, lookup, and atomic commit tests.
3. **Driver boundary**: add driver manifests and Starlark execution after
   path and save validation are in place.
4. **CLI and slash command entrypoints**: route `deepseek play`, `/play`, and
   `/game ...` into existing TUI state.
5. **Immersive player presentation and tool profile**: add
   `crates/tui/src/tui/widgets/game_console.rs`, branch `tui/ui.rs::render`
   into it for player mode, expose only game-safe tools by default, and make
   developer mode visually distinct.
6. **Render snapshot and ASCII cache**: extend `crates/game::render` with
   player-view DTOs, validate ASCII variants, and refresh `GameSession` from
   `game_render` and `game_commit_turn` tool results.
7. **Scoped sub-agents**: generate agent packs and role-specific helpers after
   the runtime can produce stable save slices.
8. **Render artist role**: add `render_artist` as a game-scoped sub-agent role
   that can propose JSON ASCII art but cannot commit state.
9. **Optional music adapter**: add manifest cue validation, player-view cue
   projection, TUI-side adapter lifecycle, and a `kew` process adapter behind
   disabled-by-default config.
10. **Demo cartridge**: add one local galgame fixture that proves load, play,
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

Pi-native transition note:

- The rebuild path now lives in `packages/genmicon-pi/` with Pi package
  commands, game-safe tools, renderers, prompts, skills, and diagnostics. The
  Rust `crates/game` runtime remains the deterministic save/driver authority
  through `genmicon-game-runtime`; it does not own Pi sessions or model loops.

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
- `render_panels` and `game_render` produce scene, figure, status, items,
  tasks, dialogue, and choices from the galgame demo and the serious-game
  cartridge scaffold
- `game_render` returns both existing render panels and the additive
  `GameViewSnapshot`
- ASCII art validation accepts exact-size variants and rejects wrong row counts,
  over-wide lines, ANSI escapes, excessive Unicode width, and oversized variants
  without wrapping
- audio cue validation accepts only local paths under the game root and rejects
  traversal, remote URLs, and missing cue IDs
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
- fixed-ratio layout fitting covers small, medium, and wide `Rect`s with no
  overlap and no overflow
- `GameConsoleWidget` buffer tests cover representative terminal sizes:
  60x20, 90x28, and 140x40
- player-mode UI tests assert coding sidebar/header/footer details are hidden
  and game panels are visible
- developer-mode UI tests assert diagnostics, raw state, raw render panels,
  tool activity, save paths, driver info, and sub-agent roster visibility return
- tool-result refresh tests assert `game_commit_turn` updates displayed
  revision, panels, ASCII art, status, items, and tasks
- audio refresh tests assert scene and ending cue changes are read from
  committed state and do not require transcript scraping
- adapter tests assert the `kew` integration is disabled by default, uses only a
  configured local executable, hides UI with `--noui` when available, cleans up
  child processes, and degrades cleanly when playback fails
- player-mode UI tests assert there is no music control panel, cue picker,
  player mute toggle, or player volume control
- sub-agent tests assert `render_artist` can use only game-safe read tools and
  cannot call `game_commit_turn`

V1 acceptance:

- no external game framework repo is required
- no Python subprocess is required for normal play
- save files are authoritative
- player mode hides coding-agent controls by default
- game skills can shape rules and voice
- a game can bind to a versioned driver
- default player mode is the immersive Game Console view, with developer mode as
  the diagnostics escape hatch
- invalid or missing ASCII art never blocks gameplay; deterministic text panels
  remain usable
- invalid or unavailable music never blocks gameplay; audio is an optional local
  presentation layer
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
- no bundled commercial music or remote music service integration
- no required background music player dependency for normal play
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
- cross-platform audio abstraction beyond the optional local adapter contract
- authoring wizard for new cartridges
- multiplayer or shared saves
- generic genre-complete mechanics beyond the first galgame demo
- public README/CHANGELOG promotion before the code, localization/help, tests,
  and shipped docs are updated together
