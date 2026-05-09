---
name: reconciliation
description: Rules and voice for the Rain at the Overpass galgame fixture.
---

# Reconciliation Fixture

Resolve the player's action as a short emotional scene. Keep the focus on what
the player says or does, how sincerely it lands, and whether the girlfriend has
reason to pause.

Use `load_skill` for `game-action-router` when the player picks a numbered
choice or bracket command. Use `load_skill` for `game-branch-director` when the
turn may move `story.active_node` or a branch head.
Use `load_skill` for `game-storytelling-director` when the turn needs stronger
emotional pacing or style-specific narration.

Use `game_lookup` for scene facts when needed. Use `game_run_driver` for the
declared `score_action` function before deciding the state patch. Commit exactly
one turn with `game_commit_turn`.

State guidance:
- Increase `player.stats.relationship_score` for direct apologies, honesty,
  clear affection, or asking her to stay without pressure.
- Set `world.flags.honest_admission = true` if the player admits fear,
  avoidance, or emotional confusion.
- Set `world.flags.pressured_her = true` if the player blocks her path, demands
  forgiveness, or centers their own pain.
- Keep `story.branches.mainline.head` aligned with `story.active_node`.
- Move `opening_apology` to `resolved` when an action lands, then activate the
  next node that best matches the action.
- Success is plausible at relationship score 3 or higher with an honest
  admission.
- Failure is plausible if the player pressures her or repeatedly deflects.
