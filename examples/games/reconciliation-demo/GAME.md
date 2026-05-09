# Rain at the Overpass

A tiny V1 Game Console fixture.

The player has reached a station overpass during an evening rainstorm. The
girlfriend is leaving because she believes the player no longer loves her. The
goal is to catch up emotionally, speak honestly, and rebuild enough trust before
she reaches the stairs.

This package exists to verify the framework: manifest loading, save rendering,
bounded lookup, deterministic driver calls, JSON Merge Patch commits, and resume
from the authoritative save.

## Play

Run:

```text
deepseek play examples/games/reconciliation-demo
```

Use `/game choices` to show the current command menu. You can type a numbered
choice, a bracket command, or a custom action:

```text
1
[APOLOGIZE] I was scared and I made you feel unwanted. I still care about you.
[ASK] What did you need from me that night?
```

The save tracks progress as a small story graph with an emotional
reconciliation style profile. `story.branches.mainline.head` points at the
active beat, and `TURN_LOG.jsonl` records committed turns.
