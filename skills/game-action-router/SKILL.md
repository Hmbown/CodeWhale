---
name: game-action-router
description: Parse player game input into bracket commands, choices, and custom actions without losing free-form intent.
---

# Game Action Router

Use this skill when a Game Console turn needs help interpreting player input.

Treat the active save's `interaction` block as the player-facing command menu:

- A bare number selects the matching `interaction.suggestions` entry.
- A bracket command such as `[ASK]`, `[INSPECT]`, `[VOTE]`, or `[APOLOGIZE]` sets the action class.
- Free-form text after the command is still important player intent.
- If the player types only natural language, infer the closest command but preserve the original text in `player_input`.

Do not reject creative actions only because they are not listed. The menu is an affordance, not a parser cage, unless `freeform_allowed` is false.

For each turn, produce one clear resolved action before narration:

```text
action_class: <command or inferred class>
target_node: <suggestion target or plausible story node>
player_intent: <short restatement>
```

Then use the normal game tools. State is authoritative only after `game_commit_turn`.

Reliability rules:

- If a numbered choice is out of range, treat the text as free-form intent when
  `freeform_allowed` is true; otherwise ask for a valid choice in character.
- If a bracket command is unknown, map it to the nearest declared verb and keep
  the raw command in `player_input`.
- Never discard the player's exact wording. Use it for tone, target, and risk
  even when the action class is inferred.
- Do not let parser failure block the game loop. Fall back to the current
  active node and offer two or three concrete next actions.
