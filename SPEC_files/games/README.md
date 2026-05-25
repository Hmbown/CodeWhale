# Single Game Spec System

Status: Active
Owner: Maintainer
Last reviewed: 2026-05-18

## Purpose

This directory owns one spec per Game TUI cartridge. A game spec describes a
specific playable package: premise, player role, action grammar, fixed facts,
content files, save schema expectations, skills, endings, and driver dependency.

Do not put reusable driver rules here unless they are overridden by the game.
Reusable driver behavior belongs under `SPEC_files/game_driver/`.

Use this directory when the change is specific to one playable cartridge. If
the same rule should apply to multiple games through a shared genre/runtime
contract, start in `SPEC_files/game_driver/` instead.

## Game Spec Index

| Spec | Game | Driver |
| --- | --- | --- |
| [GAME_SPEC_TEMPLATE.md](GAME_SPEC_TEMPLATE.md) | Template for a new game cartridge | Any |
| [reconciliation-demo.md](reconciliation-demo.md) | Rain at the Overpass | `galgame` |
| [thirteen-angry-man.md](thirteen-angry-man.md) | Thirteen Angry Man | `deliberation-drama` |

## Maintainer Prompt

```markdown
Spec: SPEC_files/games/<game-id>.md
Game:
Goal:
Player-facing change:
Current behavior:
Desired behavior:
Content/save/skill files affected:
Driver dependency impact:
Must not change:
Acceptance criteria:
Validation I expect:
```

## Per-Game Boundary Rules

- A game owns story, characters, action grammar, content, saves, endings,
  game-specific skills, and local fixtures.
- A game depends on a driver; it can constrain or override driver behavior only
  for that cartridge and must not silently redefine reusable driver internals.
- Fixed facts live in content files and authoritative saves, not only in prose.
- Runtime truth lives in save files and committed turn logs.
- New facts entering narration or state must follow the game's fact policy.
- Every game spec should name the driver spec it depends on.
- Reusable driver manifest fields, deterministic script functions, role
  templates, and version compatibility belong in `SPEC_files/game_driver/`.
- Pi package sessions, transcripts, and compaction summaries are derived
  context only. Restart/resume must reconstruct play context from
  `STATE.json`, `TURN_LOG.jsonl`, and runtime render/status snapshots.

## Completion Rules

A single-game change is complete only when:

- The game spec matches `game.toml`, `GAME.md`, content files, skills, saves,
  and driver dependency.
- Any changed driver behavior is reflected in the corresponding
  `SPEC_files/game_driver/` spec.
- Save fixtures are valid and restartable when the change touches state.
- Player-facing docs describe how to play the updated game.
- Tests or manual evidence cover load, render, choices, commit, and restart
  paths affected by the change.
