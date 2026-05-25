# GENmicon Pi Package

`genmicon-pi` is the project-local Pi package for the Pi-native GENmicon game
driver rebuild. It bundles extension code, skills, prompt templates, and a
theme while keeping deterministic game authority in `crates/game`.

## Local Loading

The repository loads this package through `.pi/settings.json` with explicit
resource filters:

```json
{
  "source": "./packages/genmicon-pi",
  "extensions": ["extensions/index.ts"],
  "skills": ["skills/**/SKILL.md"],
  "prompts": ["prompts/*.md"],
  "themes": ["themes/genmicon.json"]
}
```

## Commands

- `/genmicon:validate <game-path>` checks package, cartridge, driver, and save readiness.
- `/genmicon:play <game-path> --save <id>` starts player mode.
- `/genmicon:dev on|off|status` controls diagnostics.
- `/genmicon:saves <game-path>` lists available saves.

### Validation Behavior

`/genmicon:validate` calls the runtime helper with the `validate` command. A
passing result reports the game id, resolved driver id, save revision, and
warning count. Runtime failures and blocking warnings are surfaced before
player mode widens to any game tools.

### Player Launch Workflow

`/genmicon:play <game-path> --save <id>` first runs the same validation path as
`/genmicon:validate`. If validation fails or produces blocking warnings, player
mode does not start. When validation passes, the package installs only the
game-safe tool profile, opens `genmicon.gameConsole`, and keeps the action
composer focused on player input. The first turn should inspect game state,
check facts when needed, call `game_commit_turn` once, and render the refreshed
view returned by the runtime helper.

### Diagnostics Behavior

`/genmicon:dev on|off|status` controls developer diagnostics without changing
the player active-tool profile or writing save state. Diagnostics show package
source and review status, loaded resources, active tools, save revision, driver
identity, render availability, last runtime command, and warnings.

### Save Authority And Resume

`/genmicon:saves <game-path>` lists saves through the runtime helper. Restart
and resume use `/genmicon:play <game-path> --save <id>`, which reloads status
and render snapshots from save files before injecting derived context into the
Pi session. Transcript history and compaction summaries are never treated as
authoritative save state, and `game_commit_turn` rejects transcript-derived
state fields.

## Build And Test

```bash
npm install
npm run typecheck
npm test
```

Latest local evidence:

- `npm run typecheck`: passed.
- `npm test`: passed, 51 package tests.
- `cargo test -p deepseek-game --all-features`: passed for the runtime helper
  and fixture cartridges.

The package calls the Rust runtime helper `genmicon-game-runtime` for
authoritative manifest, save, lookup, render, driver, and commit operations.

## Runtime Bridge

The runtime helper reads one JSON request from stdin and writes one JSON
response to stdout:

```json
{
  "command": "validate",
  "game_root": "examples/games/reconciliation-demo",
  "save_id": "default",
  "developer": false,
  "payload": {}
}
```

Responses always use the same envelope:

```json
{
  "ok": true,
  "data": {},
  "warnings": [],
  "error": null
}
```

Supported commands are `validate`, `status`, `render`, `playbook`, `lookup`,
`fact_check`, `run_driver`, `commit_turn`, and `list_saves`. `commit_turn` is
the only command that writes save files, and it requires the expected revision
inside the payload consumed by the Rust runtime.
