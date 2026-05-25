# GENmicon Player Contract

Use this skill for player-facing GENmicon turns.

## Player Experience

- Show the active game before implementation details.
- Keep raw JSON, save paths, provider details, and package plumbing out of player prose.
- Ask for a revised action when a proposed fact violates protected continuity.
- After a successful commit, refresh the view from the saved state.
- In player mode, keep active tools limited to `game_status`, `game_render`,
  `game_playbook`, `game_lookup`, `game_fact_check`, `game_run_driver`, and
  `game_commit_turn`.
- Treat `game_commit_turn` as sequential. Do not perform a second commit for
  the same player input.
