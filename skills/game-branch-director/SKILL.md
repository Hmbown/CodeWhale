---
name: game-branch-director
description: Advance git-like story branch nodes smoothly while keeping game saves authoritative and non-linear.
---

# Game Branch Director

Use this skill when a Game Console turn may advance the story graph.

The save's `story` block is a git-like progress model:

- `active_branch` is the current route name.
- `branches.<name>.head` points to the current story node, like a branch ref.
- `nodes.<id>.parents` records prior beats that unlock or justify a node.
- `nodes.<id>.next` lists likely forward edges, not mandatory rails.
- `TURN_LOG.jsonl` is the immutable commit log. Do not use the user's repository git history as the game save.

Advance story smoothly:

1. Read `game_playbook` or `game_render` before deciding the branch move.
2. Keep the active node unless the player satisfies a visible or diegetic gate.
3. A node can move `locked -> hinted -> available -> active -> resolved`.
4. Branch only when the player creates a meaningfully different route, such as a trust route versus a pressure route.
5. Never reveal sealed facts just to explain a branch. Use hints and panel text.
6. Commit story changes as a JSON merge patch through `game_commit_turn`.

Reliability rules:

- If `game_playbook` reports warnings, repair only the affected story fields in
  the next commit if the repair is obvious; otherwise keep the current node and
  narrate a safe fallback.
- Never delete `schema_version`, `revision`, `driver`, `interaction`, `story`,
  `world`, or `ui` from state.
- Keep `story.active_node` equal to `story.branches.<active_branch>.head`.
- Do not jump across two story nodes in one player turn unless the cartridge
  explicitly marks the skipped node as optional.
- If a target node is missing, resolve the action against the current node and
  add a brief in-world reason why the path remains uncertain.

Good state patch shape:

```json
{
  "story": {
    "active_node": "knife_weapon_doubt",
    "branches": {"mainline": {"head": "knife_weapon_doubt"}},
    "nodes": {
      "opening_ballot": {"status": "resolved"},
      "knife_weapon_doubt": {"status": "active"}
    }
  }
}
```
