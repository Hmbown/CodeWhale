# Game Driver Spec System

Status: Active
Owner: Maintainer
Last reviewed: 2026-05-18

## Purpose

This directory owns reusable Game TUI driver specifications. A driver is the
genre/runtime contract used by one or more games: manifest shape, deterministic
functions, reusable skills, sub-agent topology, validation rules, and driver
versioning.

Do not put a single game's plot, characters, endings, or save facts here. Those
belong under `SPEC_files/games/`.

Use this directory when the same driver behavior can apply to more than one
game cartridge. If the change only alters one game's content, fixed facts,
starting save, or player-facing story contract, start in `SPEC_files/games/`
instead.

## Driver Spec Index

| Spec | Owns |
| --- | --- |
| [00_DRIVER_SYSTEM_SPEC.md](00_DRIVER_SYSTEM_SPEC.md) | Shared driver architecture and boundaries |
| [01_MANIFEST_RESOLUTION_SPEC.md](01_MANIFEST_RESOLUTION_SPEC.md) | `driver.toml`, version resolution, install roots, save driver lock |
| [02_SCRIPT_FUNCTIONS_SPEC.md](02_SCRIPT_FUNCTIONS_SPEC.md) | Declared deterministic driver functions and Starlark execution |
| [03_AGENT_TOPOLOGY_SPEC.md](03_AGENT_TOPOLOGY_SPEC.md) | Driver-declared sub-agent roles, templates, and scoped packs |
| [DRIVER_SPEC_TEMPLATE.md](DRIVER_SPEC_TEMPLATE.md) | Template for adding a new concrete driver spec |
| [drivers/galgame.md](drivers/galgame.md) | Minimal galgame driver used by `reconciliation-demo` |
| [drivers/deliberation-drama.md](drivers/deliberation-drama.md) | Deliberation drama driver used by `thirteen-angry-man` |

## Maintainer Prompt

```markdown
Spec: SPEC_files/game_driver/<file>.md
Driver:
Goal:
Current driver behavior:
Desired driver behavior:
Games affected:
Manifest/script/agent changes:
Must not change:
Acceptance criteria:
Validation I expect:
```

## Driver Boundary Rules

- Drivers are reusable genre/runtime packages, not game content packages.
- Drivers own reusable manifest fields, script function contracts, role
  templates, validation rules, and version compatibility.
- Driver prompts and skills can define genre policy, mechanics, and reusable
  role behavior.
- Drivers do not own cartridge-only plot beats, character canon, endings,
  fixed facts, or save fixture contents.
- Driver files must resolve under the installed driver root.
- Model-visible driver functions must be declared in `driver.toml`.
- Driver functions must be deterministic and cannot mutate saves directly.
- Save files lock the concrete resolved driver version.
- V1 does not migrate saves across driver versions unless a future spec adds a
  migration contract.

When driver behavior changes, update both the shared driver spec and every
affected concrete driver spec. If the change affects a cartridge's player
contract or save expectations, update that game's spec too.

## Concrete Driver File Layout

The runtime expects this install shape:

```text
<driver-root>/
  <driver-id>/
    <version>/
      driver.toml
      skills/
      scripts/
      agent_templates/
```

Local example games can carry local drivers under their own `drivers/`
directory. Future globally managed driver installation remains a planned
surface, not a shipped marketplace.

## Pi-Native Package Integration

The Pi-native rebuild consumes drivers through `packages/genmicon-pi` game
tools and the deterministic `crates/game` runtime helper. Drivers may provide
skills, prompts, declared functions, and templates as game/driver resources,
but they do not install Pi tools, change active-tool policy, or own session
state. Save files continue to lock the exact resolved driver version.

## Completion Rules

A driver change is complete only when:

- The shared driver system spec and concrete driver spec agree.
- Affected game specs list any changed driver dependency or behavior.
- `driver.toml`, scripts, skills, and agent templates are updated together.
- Tests cover manifest loading, version resolution, declared function calls,
  and role bounds where affected.
- Player mode remains restricted to game-safe tools.
