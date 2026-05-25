# Contract: GENmicon Pi Commands

## `/genmicon:validate [game-path]`

Validates package, cartridge, driver, and save readiness without starting a
player turn.

**Output**:

- package source and review status
- loaded resources
- driver id/version
- save id/revision
- warnings and blocking errors

## `/genmicon:play [game-path] [--save <id>] [--lang en|zh]`

Starts player mode for a local cartridge.

**Behavior**:

- verifies reviewed package source
- loads cartridge and save
- sets player active tools
- opens player-facing game console
- hides developer diagnostics

## `/genmicon:dev [on|off|status]`

Toggles or reports developer diagnostics.

**Behavior**:

- `on` shows package/resource/tool/save/render diagnostics
- `off` restores player-facing presentation
- `status` reports diagnostic state without changing it

## `/genmicon:saves [game-path]`

Lists available saves for a local cartridge.

**Output**:

- save id
- revision
- driver id/version
- last turn id when available
- warnings for malformed saves

## Implementation Cross-Check

- Command registration and formatting live in
  `packages/genmicon-pi/extensions/commands.ts`.
- `/genmicon:validate` calls runtime validation and blocks unsafe warnings.
- `/genmicon:play` validates, loads a resume snapshot, installs player tools,
  injects derived context, and opens `genmicon.gameConsole`.
- `/genmicon:dev` toggles presentation-only diagnostics.
- `/genmicon:saves` lists saves through the runtime helper.
- `packages/genmicon-pi/tests/commands.test.ts`,
  `packages/genmicon-pi/tests/player-turn.test.ts`, and
  `packages/genmicon-pi/tests/resume.test.ts` cover the shipped command
  behavior.
