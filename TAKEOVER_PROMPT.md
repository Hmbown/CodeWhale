# Takeover Prompt: Game TUI Framework

## Current Goal

Continue building the Game TUI system inside DeepSeek TUI.

The project direction is no longer "integrate Gen-micom as a native subsystem".
Gen-micom-style markdown worlds are useful reference material, but the TUI repo
must own the engineering design. Do not require the external Gen-micom folder,
do not design around its Python CLI, and do not treat it as the core runtime.

Target spec file:

- `docs/GAME_TUI_FRAMEWORK_SPEC.md`
- `docs/games/thirteen-angry-man/SPEC.md` for the first serious-game
  cartridge track

## Current Implementation Status

The initial framework scaffold is now present:

- `crates/game` is a pure Rust runtime crate for manifests, driver resolution,
  saves, lookup, rendering, story playbooks, Starlark driver functions, atomic
  commits, and agent packs.
- `deepseek play`, `/play`, `/game ...`, native `game_*` tools, and
  player-mode game tool-profile wiring exist in the TUI.
- The native tool surface currently includes `game_status`, `game_render`,
  `game_playbook`, `game_lookup`, `game_fact_check`, `game_run_driver`, and
  `game_commit_turn`.
- `examples/games/reconciliation-demo` is the minimal galgame proof fixture.
  Its default save includes `AGENTS.json`, and the `dialogue` role expands into
  a Rei-owned `dialogue_girlfriend` NPC pack with a dedicated NPC skill.
- `examples/games/thirteen-angry-man` is the first serious-game cartridge
  scaffold, using the bundled `deliberation-drama` driver.
- Save-locked driver versions must resolve exactly; missing or mismatched
  versions produce a load notice rather than silently continuing as a loaded
  session.
- `[game]` config keys remain reserved/planned; current game launch resolution
  is explicit CLI/slash argument first, then current workspace `game.toml`.

## Source Of Truth And Sync Rules

`docs/GAME_TUI_FRAMEWORK_SPEC.md` is the authoritative planning spec. This
takeover prompt is a compact operator handoff for a fresh implementation
session. If the prompt and spec disagree, update this prompt to match the spec
rather than widening the design here.

Keep planned and shipped surfaces clearly separated:

- related docs should carry short pointers and invariants, not a second copy of
  the full blueprint
- `docs/ARCHITECTURE.md` should say where the Game Console scaffold attaches to the
  current runtime
- `docs/TOOL_SURFACE.md` should describe the active `game_*` profile without
  implying persistent `[game]` config is shipped
- `docs/SUBAGENTS.md` should describe game-scoped sub-agents as wrappers around
  the current sub-agent runtime, not a second agent system
- `docs/CONFIGURATION.md` should not present `[game]` keys as active until the
  config loader and UI use them
- `docs/MODES.md` should keep Game Console as a presentation/tool profile, not a
  fourth `AppMode`

## Product Direction

`deepseek play` should feel like opening an AI-era terminal-native game console
or game computer:

- choose or resume a local game
- read a clear player-facing game view
- type natural player actions
- let the main game engine session resolve the turn with game rules, driver
  tools, and scoped sub-agents
- persist progress through native game tools
- resume later from file-backed save state

The first milestone is a playable loop, not authoring tooling.

## Active UI Feature Goal

The next Game TUI UI branch should make player mode an immersive Game Console
screen inside the existing TUI rather than a transcript-first coding cockpit.
The authoritative details are in `docs/GAME_TUI_FRAMEWORK_SPEC.md` under
"Immersive Game Console Goal".

Implementation constraints:

- keep `GameSession` as the mode carrier; do not add `AppMode::Game`
- add `crates/tui/src/tui/widgets/game_console.rs` and branch
  `tui/ui.rs::render` into it when `game_player_presentation(app)` is true
- hide coding sidebars, raw tool/thinking cells, model/cost/context footer
  noise, raw JSON, file paths, and coding status labels in player mode
- keep `/game dev on` as the diagnostics/raw-state/tool-activity escape hatch
- extend `crates/game::render` additively with player-view data,
  `AsciiArtFrame`, `AsciiArtVariant`, and `GameViewSnapshot`
- store accepted model-authored ASCII art under `STATE.json.ui.ascii`, committed
  only through `game_commit_turn`
- add a game-scoped `render_artist` role that can use only game-safe read tools
  and returns JSON ASCII proposals; it cannot call `game_commit_turn`
- fall back to deterministic text panels when art is missing, stale, invalid,
  or oversized
- add optional designer/agent-controlled scene music: cues are local cartridge
  data, selected through committed game state, and may use a locally installed
  `kew` process adapter; player mode must not expose a music control panel

## Design Center

Game TUI is a TUI-owned framework with four engineering pieces:

1. **TUI game console**
   - The existing DeepSeek TUI is the game console/computer.
   - Reuse the current app, engine, skill discovery, slash commands, tool
     registry, session persistence, sub-agent runtime, and ratatui surfaces.
   - Add a player-facing presentation profile and a restricted game tool
     profile.

2. **Game runtime core**
   - Required pure Rust crate at `crates/game`.
   - Loads game/driver manifests and saves, validates state, executes
     constrained deterministic scripts, renders structured panels, performs
     bounded lookup, and commits turns atomically.
   - Must have no ratatui, TUI, LLM, shell, network, Python, or external
     Gen-micom runtime dependency.

3. **Game Driver**
   - Reusable genre runtime for galgame, RPG, mystery, simulation, etc.
   - Owns turn-loop policy, genre skills, Starlark deterministic scripts,
     save/schema extensions, render templates, and sub-agent role templates.
   - Installed globally by version and selected by each game manifest.

4. **Game package**
   - Local swappable cartridge with manifest, markdown content, world/plot
     design, character definitions, optional skills, NPC skills, and saves.
   - The package is data, not executable code.

Hardware metaphor for product direction:

- CPU = LLM/API/model ability.
- GPU = model-driven terminal rendering and ASCII art.
- Memory = context, summaries, compaction, save files, and reload stability.

Use this metaphor to explain the product, not as mandatory Rust API naming.

## Relationship To Skills

The current TUI skill system is relevant and should be reused.

Existing behavior:

- skills are discovered from `SKILL.md`
- skills are listed in the system prompt
- the model can load a skill body with `load_skill`
- `/skill <name>` injects a skill into the next normal user turn

Game TUI should extend this idea for games:

- a game driver can include persistent genre skills
- a game package can include `skills/<name>/SKILL.md`
- the game manifest declares a persistent entry skill for game rules, voice,
  parser policy, and genre conventions
- important NPCs can have `skills/npc/<npc-id>/SKILL.md`
- saves can contain generated NPC skill overlays that evolve through play
- optional skills can still be loaded for specific game systems
- skills provide instructions only; they are not the source of save truth
- native game tools own validation, state lookup, rendering, and persistence
- manifest-declared game and driver skills auto-load for the active game session
- auto-loaded game skills cannot expand the player tool profile, escape game or
  driver roots, override save authority, or change approval/sandbox policy

Pi-native transition:

- Treat `packages/genmicon-pi` as the active integration point for GENmicon
  commands, tools, prompts, skills, renderers, and diagnostics.
- Treat `crates/game` and `genmicon-game-runtime` as deterministic runtime
  authority only.
- Do not rebuild a separate GENmicon agent/session kernel beside Pi.

## Sub-Agent Runtime

Use current TUI sub-agents as scoped game processors.

Baseline serious-game topology:

1. **Main Game Engine Session**
   - Talks to the player.
   - Coordinates the turn.
   - Calls driver tools and sub-agents.
   - Produces the final player-facing response.
   - Commits authoritative state through native game tools.

2. **State Manager Sub-Agent**
   - Tracks inventory, flags, quests, location, map position, relationship
     values, combat state, and other save-relevant facts.

3. **Plot Manager Sub-Agent**
   - Tracks designer intent, route direction, pacing, foreshadowing, escalation,
     and whether the story is drifting.

4. **NPC Manager Sub-Agent A/B/C**
   - Each controls one or several NPCs.
   - Generates dialogue, reactions, emotional state, memories, and proposed
     character actions.

Sub-agent rules:

- sub-agents propose; they do not commit authoritative state
- the main session is the only final narrator
- save files are the source of truth
- sub-agents receive scoped agent packs, not the whole game
- player-mode sub-agents must use game-safe tools, not full coding tools
- game sub-agents are accessed through `game_agent_*` helpers, not generic
  `agent_spawn` in player mode
- reload reconstructs sub-agents from save summaries and NPC skills

The driver declares allowed roles, default roles, and maximum active count. The
main game engine session activates only the needed driver-bounded subset per
scene and turn; V1 must not force all five manager roles to run every turn.

Agent packs should contain only:

```text
role.md
output_contract.md
allowed_files.md
current_scene.md
relevant_save_slice.json
recent_turns.md
assigned_skills/
callable_driver_functions.md
```

Generated NPC skills should update after critical committed events and at a
periodic checkpoint, defaulting to every five committed turns.

## File-Backed Save Principle

Game progress must not depend on the chat transcript.

The save files are authoritative. The transcript is useful for display,
debugging, and recovery context, but never the source of truth for game state.

Planned save shape:

```text
<game-root>/
  game.toml
  GAME.md
  skills/
    dm/SKILL.md
    npc/
      <npc-id>/SKILL.md
  content/
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

Game drivers install globally:

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

Games bind drivers by semver in `game.toml`, and saves record the concrete
resolved driver version. Existing saves must reload with that exact concrete
driver version; V1 has no migration path, so unavailable or mismatched driver
versions fail clearly without rewriting the save. Normal gameplay must not
depend on Python or external shell scripts; deterministic game math belongs in
constrained Starlark scripts executed by the Rust runtime.

`STATE.json` and `TURN_LOG.jsonl` are the only atomic truth. `SUMMARY.md`,
`AGENTS.json`, NPC skill overlays, and render caches are post-commit derived
artifacts and must be rebuildable.

## Active And Reserved Public Surface

CLI:

```text
deepseek play [game-or-path] [--save <id>] [--dev]
```

Slash commands:

```text
/play [game-or-path]
/game status
/game render
/game rules
/game choices
/game saves
/game dev
/game exit
```

Reserved config shape:

```toml
[game]
roots = []
default_game = ""
default_save = ""
developer_mode = false

[game.drivers]
roots = ["~/.deepseek/game-drivers"]
```

Native game tools:

- `game_status`
- `game_render`
- `game_playbook`
- `game_lookup`
- `game_fact_check`
- `game_run_driver`
- `game_commit_turn`

`game_lookup` returns bounded content excerpts from declared content roots only
(default 16 KiB, hard cap 32 KiB), and can read bounded active-save JSON state
paths through `state_path` / `key`. `game_playbook` exposes current choices,
story nodes, and the active story style. `game_fact_check` gates free-form
actions or proposed narration against active continuity facts before narration
or commit. `game_run_driver` calls only driver-declared Starlark functions and
cannot mutate saves or access files, shell, or network.

`game_commit_turn` appends only when it has `player_input` and `resolution`.
`expected_revision` is inferred from the active save when omitted, and
`state_patch` may be omitted for an empty patch. The runtime runs the same fact
gate before writing, applies an RFC 7396 JSON Merge Patch, and generates
`turn_id`, `revision_after`, and `created_at`.

Game-scoped sub-agent helpers are `game_agent_spawn`, `game_agent_send`,
`game_agent_wait`, `game_agent_resume`, and `game_agent_list`. They wrap the
existing sub-agent runtime for declared driver roles only and must not expose
generic coding-agent tools in player mode.

Do not add a model-visible `game_parallel` wrapper in V1. Rely on native
parallel tool-call execution when available.

Player mode should expose only game-safe tools plus required skill-loading
support. Developer mode can expose validation, raw paths, raw state, and normal
inspection tools.

`[game]` config is planned for user/global and project config, but it is not
active yet. Current launch resolution is explicit CLI/slash argument first, then
the current workspace if it contains `game.toml`. Driver lookup checks the game
package's local `drivers/` directory and `~/.deepseek/game-drivers`.

Future project config may provide roots and defaults, but must not persistently
enable developer mode; only user config may honor
`game.developer_mode = true`.

## Next Branch Guidance

The initial scaffold exists. Keep the next feature branch narrow and close the
remaining V1 gaps instead of redoing landed work:

1. Verify the active scaffold with focused tests before widening behavior.
2. Keep `GameSession` as presentation/tool-profile state; do not add
   `AppMode::Game`.
3. Keep player mode restricted to native game tools, skill loading, and scoped
   `game_agent_*` helpers.
4. Implement `[game]` config only with loader, `/config` UI, docs, and tests in
   the same patch.
5. Expand game picker or driver-install UX only after exact save-locked driver
   reload remains covered.
6. Persist turns only through `game_commit_turn` using JSON Merge Patch and the
   continuity fact gate.
7. Rebuild sub-agents from save summaries and NPC skills on reload; do not rely
   on old child transcripts.
8. Keep the reconciliation demo and Thirteen Angry Man scaffold loadable after
   every feature slice.

Start code discovery from the current repo seams:

- `crates/cli` and `crates/tui/src/main.rs` for `deepseek play` dispatch.
- `crates/tui/src/commands/` for `/play` and `/game ...`.
- `crates/tui/src/tui/app.rs`, `tui/ui.rs`, and `tui/mod.rs` for Game Console
  presentation and game session state.
- `crates/tui/src/core/engine/tool_setup.rs` and
  `crates/tui/src/tools/registry.rs` for the restricted player tool profile.
- `crates/tui/src/skills/`, `skill_state.rs`, and `tools/skill.rs` for game and
  driver skill loading.
- `crates/tui/src/tools/subagent/` and `tui/subagent_routing.rs` for scoped game
  sub-agent wrappers.

## Acceptance Criteria For V1

- `deepseek play` opens the existing TUI directly into a player-facing game
  console.
- A local game package can be loaded without any external repo dependency.
- A game can bind to a globally installed, versioned Game Driver.
- A save can be rendered, played, committed, and resumed after restart.
- The player does not see coding-agent controls by default.
- Game skills can provide persistent instructions for rules and voice.
- Default serious games can use one main game engine session plus five manager
  roles: State, Plot, and three NPC managers; only the needed driver-bounded
  subset has to be active each turn.
- Sub-agents can propose state/plot/dialogue, but only native game tools commit.
- Save files remain the source of truth.
- `game_run_driver` handles deterministic declared Starlark calculations.
- `game_commit_turn` uses RFC 7396 JSON Merge Patch, infers the active revision
  when omitted, and rejects stale revisions without writing.
- One local galgame reconciliation demo can reach success and failure endings.
- No Python subprocess is required for normal play.

## Verification Targets

Before claiming V1 complete, verification should include:

- crate tests for manifest load, save load, validation, render data, lookup,
  exact driver-version reload, Starlark script sandboxing, `game_run_driver`,
  JSON Merge Patch commit, generated commit fields, lookup caps, and revision
  conflicts
- TUI command tests for `deepseek play [game-or-path] [--save <id>] [--dev]`,
  `/play`, and `/game`
- tool registry tests proving player mode exposes only game-safe tools,
  excludes shell/file/git/code tools, and does not expose `game_parallel`
- sub-agent tests proving `game_agent_*` helpers receive scoped packs and cannot
  access full coding tools in player mode
- mock LLM test proving a player action leads to `game_commit_turn` and a
  resumable save with reconstructable sub-agent context
- demo fixture test proving the galgame scene can load, play, save, resume, and
  reach both endings
- full workspace check with `cargo test --workspace --all-features`

## Constraints

- Stable Rust only.
- No new terminal app or separate event loop.
- Do not depend on the external Gen-micom repository.
- Treat external markdown game examples as data or inspiration, not authority.
- Keep normal play file-backed and local-first.
