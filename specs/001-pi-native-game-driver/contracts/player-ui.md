# Contract: Player And Developer UI

## Player Console

The player console shows:

- scene
- status
- inventory or equivalent state
- tasks or objectives
- dialogue/log
- choices
- action composer

It hides:

- raw tool calls and JSON
- package paths
- save paths
- provider/model/cost details
- shell/file/git/package controls
- developer warnings that do not require player action

## Developer Diagnostics

Diagnostics show:

- package source and review status
- loaded resources
- active tool profile
- cartridge root
- save id/revision
- driver id/version
- render snapshot
- last runtime request/response summary
- validation warnings

## Layout Rules

- Compact, medium, and wide terminal sizes must remain non-overlapping.
- If rich rendering cannot fit, the UI falls back to a compact text game view.
- Turning diagnostics off restores player presentation without changing game
  state or player active tools.

## Implementation Cross-Check

- Player console model, width modes, compact fallback rendering, and action
  composer state live in `packages/genmicon-pi/extensions/ui/game-console.ts`.
- Diagnostic panel rows live in
  `packages/genmicon-pi/extensions/ui/diagnostics.ts`.
- Player/developer result renderers live in
  `packages/genmicon-pi/extensions/renderers.ts`.
- `packages/genmicon-pi/tests/ui-layout.test.ts`,
  `packages/genmicon-pi/tests/renderers.test.ts`, and
  `packages/genmicon-pi/tests/diagnostics.test.ts` verify layout, fallback,
  raw JSON hiding, and developer expansion.
