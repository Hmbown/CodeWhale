# Thirteen Angry Man

Thirteen Angry Man is a deliberation-room drama cartridge for the Game TUI
framework. The player is Juror 13, a participant who can question, slow the
room down, request votes, inspect admitted evidence summaries, and protect fair
process.

The game is not an investigation game. The background case is fixed, and the
player cannot leave the room, call witnesses, or introduce new evidence. Play
is about whether doubt can be surfaced and tested under heat, fatigue, pride,
prejudice, and civic pressure.

Runtime truth lives in `saves/default/STATE.json` and
`saves/default/TURN_LOG.jsonl`. Fixed case facts live under `content/`.

## Play

Run:

```text
deepseek play examples/games/thirteen-angry-man
```

Use `/game choices` to show the current command menu. You can type a numbered
choice, a bracket command, or a custom action:

```text
1
[PROTECT] I ask Juror 8 why he voted not guilty, then ask the room to slow down.
[INSPECT] I ask to inspect the knife evidence summary.
[VOTE] I request another vote.
```

The story uses git-like branch state and a deliberation drama style profile
inside the save. `story.active_branch` and `story.branches.<name>.head`
identify the current route, while each committed turn is appended to
`TURN_LOG.jsonl`. Normal play does not write to repository git history.
