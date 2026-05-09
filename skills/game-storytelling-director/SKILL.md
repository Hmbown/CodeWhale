---
name: game-storytelling-director
description: Adapt Game Console narration, pacing, tension, and branch movement to the active plot style profile.
---

# Game Storytelling Director

Use this skill when a Game Console turn needs better story pacing, more
attractive narration, or style-specific branch movement.

Read `game_playbook` first. The playbook may include `story_style`, which is the
cartridge's compact storytelling contract.

General turn shape:

1. Identify the player's concrete intent.
2. Pick the relevant tension axis from the active style.
3. Narrate one vivid consequence, not a generic summary.
4. Move one meaningful state track: trust, pressure, clue access, vote, resource, location, or branch head.
5. End with a changed situation and a few clear next options.

Reliability rules:

- One player input should produce one committed turn.
- Prefer a partial success with a cost over a dead end.
- If a command is unclear, interpret it conservatively and continue rather than
  stopping play.
- If a deterministic driver call fails, do not invent a numeric result. Resolve
  the narrative conservatively and commit metadata noting the missing driver
  result only when the game can still continue.
- Keep visible choices updated after a major branch move so the next prompt is
  playable without hidden knowledge.
- Preserve the player's agency: do not make irreversible failure from a single
  ambiguous line unless the cartridge clearly warns that the action is risky.

Style optimization:

- Emotional reconciliation: make the consequence emotional before it is mechanical; reward accountability, specificity, restraint, and listening.
- Deliberation drama: move evidence, character, procedure, time, or vote pressure every turn; hints should appear through juror behavior and room friction.
- Mystery: let clues sharpen questions before they solve answers; preserve sealed facts; wrong theories should still create pressure or cost.
- Adventure RPG: make choices concrete and stateful; show risks and costs; let inventory, location, and faction state matter.
- Survival: foreground scarcity, threat clocks, body condition, and tradeoffs; every safety gain should cost time, supplies, or exposure.
- Political intrigue: track leverage, promises, reputation, secrets, and timing; make victories create future obligations.

Branch movement:

- Keep the current node if the player did not satisfy a gate.
- Advance when the action changes the situation in a way the player can perceive.
- Branch when the player's method changes the story route, not just wording.
- Keep `story.active_node` and `story.branches.<name>.head` aligned in the state patch.

Avoid:

- Long exposition blocks.
- Solving hidden nodes in narration.
- Treating menu choices as the only valid actions when `freeform_allowed` is true.
- Updating story state outside `game_commit_turn`.
