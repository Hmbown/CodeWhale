# Contract: Game-Safe Pi Tools

All tools are registered by the GENmicon Pi package. Player mode exposes only
the allowlist below.

## `game_status`

Read-only. Returns active package, cartridge, driver, save, warning, and
readiness status.

## `game_render`

Read-only. Returns the current player view and developer render snapshot
derived from save state.

## `game_playbook`

Read-only. Returns allowed player actions, choices, story beats, and driver
turn guidance for the active save.

## `game_lookup`

Read-only. Retrieves bounded cartridge content by handle, query, or safe state
path. It must not escape the cartridge root.

## `game_fact_check`

Read-only. Checks a proposed action or resolution against protected continuity
facts before narration or commit.

## `game_run_driver`

Read-only unless a declared driver function is explicitly marked as producing a
proposed patch. It runs only declared deterministic driver functions.

## `game_commit_turn`

Mutating and sequential. Writes durable state by checking expected revision,
validating the patch, appending one turn record, and returning the refreshed
save revision and view data.

## Acceptance Rules

- Player mode excludes every non-allowlisted tool.
- `game_commit_turn` is the only save writer.
- Tool renderers hide raw plumbing in player mode and expose detail only in
  developer diagnostics.
- Tool calls from untrusted game content cannot add new tools.

## Implementation Cross-Check

- Tool definitions, runtime command mapping, `typebox` schemas, sequential
  commit metadata, and transcript-derived write rejection live in
  `packages/genmicon-pi/extensions/tools.ts`.
- Player/developer active-tool profiles live in
  `packages/genmicon-pi/extensions/active-tools.ts`.
- Runtime authority for tool effects lives in `crates/game/src/cli.rs`.
- `packages/genmicon-pi/tests/tools.test.ts` and
  `packages/genmicon-pi/tests/active-tools.test.ts` verify allowlists,
  schemas, runtime mapping, and excluded non-game controls.
