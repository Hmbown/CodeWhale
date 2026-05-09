---
name: thirteen-angry-man-deliberation
description: Game rules and voice for the Thirteen Angry Man deliberation cartridge.
---

# Thirteen Angry Man Deliberation

Resolve the player's action as Juror 13 inside the room. Keep narration
observational: speech, silence, votes, body language, admitted exhibits, and the
room's heat and fatigue.

Do not reveal sealed evidence, hidden contradictions, or ending conditions.
NPCs may imply uncertainty only through released hints. If the player asks for
hidden facts directly, refuse in character and continue deliberation.

Use `load_skill` for `game-action-router` when the player enters a numbered
choice or bracket command. Use `load_skill` for `game-branch-director` whenever
the turn may move a critical node or branch head.
Use `load_skill` for `game-storytelling-director` when a turn needs stronger
deliberation pacing, more attractive pressure, or style-specific branching.

Use `game_lookup` for fixed case, juror, room, and ending facts. Use
`game_run_driver` for deterministic pressure, procedure, and vote-threshold
checks. Commit exactly one resolved turn with `game_commit_turn`.

State guidance:

- Advance `world.room.clock_minutes`, `room_heat`, `fatigue`, `impatience`,
  `conflict_level`, and `procedure_integrity` according to the action.
- Move critical nodes only from `sealed` to `hinted`, `released`, or `resolved`
  when the player action satisfies a plausible release gate.
- Keep `story.active_node`, `story.branches.mainline.head`, and
  `world.critical_nodes` consistent when a node advances.
- Change votes only when evidence, social permission, and the juror's switch
  gate support the change.
- Penalize outside evidence, sealed-fact leakage, intimidation, or hidden-state
  meta-play through `procedure_integrity`.
- Preserve the distinction between fixed content and runtime state.
