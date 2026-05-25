# Implementation Plan: Pi-Native GENmicon Game Driver

**Branch**: `001-pi-native-game-driver` | **Date**: 2026-05-25 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-pi-native-game-driver/spec.md`

## Summary

Rebuild GENmicon as a Pi-native game driver package. The first complete cycle
ships a reviewed project-local Pi package that registers GENmicon commands,
game-safe tools, prompts, skills, renderers, and player/developer UI surfaces.
The existing `crates/game` Rust runtime remains the deterministic authority for
manifest validation, path safety, lookup, render snapshots, driver functions,
and save commits. Pi owns the agent/session/provider/tool-call/TUI harness; the
Rust runtime is called only through package-owned tool adapters.

## Technical Context

**Language/Version**: TypeScript Pi extension package loaded by current Pi;
Rust 1.88 stable for the deterministic game runtime helper.

**Primary Dependencies**: Pi package/extension APIs from
`@earendil-works/pi-coding-agent`, Pi TUI components from
`@earendil-works/pi-tui`, `typebox` for tool schemas, existing
`deepseek-game` crate for deterministic runtime behavior.

**Storage**: Pi session entries for derived conversation context;
`STATE.json` and `TURN_LOG.jsonl` for authoritative saves; project-local
`.pi/settings.json` for trusted local package loading; local cartridge files
under `examples/games/`.

**Testing**: TypeScript package/extension tests for package loading, commands,
active-tool policy, renderers, and bridge behavior; `cargo test -p
deepseek-game --all-features` for runtime authority; fixture cartridge tests;
terminal layout/render tests for compact, medium, and wide views.

**Target Platform**: Local Pi interactive TUI and Pi JSON/RPC-compatible
automation paths; local terminal first.

**Project Type**: Pi package/extension plus deterministic local game runtime
helper and terminal game UI.

**Performance Goals**: Driver validation for fixture cartridges completes in
under 30 seconds; one fixture player turn completes in under two minutes;
runtime JSON tool calls avoid unnecessary full-cartridge reads by using bounded
lookup and render snapshots.

**Constraints**: Player mode exposes only game-safe tools; no parallel agent
runtime beside Pi; package sources are local, pinned, or reviewed; all durable
state writes go through the runtime commit operation; TypeScript package code
must not bundle Pi core packages.

**Scale/Scope**: V1 supports one local GENmicon Pi package, the
`reconciliation-demo` fixture, and the `thirteen-angry-man` serious-game
scaffold. Remote package marketplace, hosted sessions, telemetry, and
unreviewed third-party package loading are out of scope.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **Pi Substrate First**: PASS. The feature uses Pi packages, extension events,
  command registration, tool registration, active-tool control, custom
  renderers, custom UI/editor surfaces, skills, prompts, themes, sessions,
  provider selection, and compaction hooks. The Rust runtime is limited to
  deterministic game authority and is not an agent harness.
- **Game Console First**: PASS. Player mode opens into a game-facing custom UI
  and renderer set. Developer diagnostics are explicit commands/views and do
  not change player active tools.
- **Save State Authority**: PASS. `STATE.json` and `TURN_LOG.jsonl` remain
  authoritative. Pi session entries, compaction summaries, render caches, and
  model output are derived context only.
- **Package Trust Boundaries**: PASS. V1 uses a project-local reviewed package
  with `.pi/settings.json` filters. Remote npm/git package loading is
  documented but blocked from player-mode use unless pinned and reviewed.
- **Tested Shipped Contracts**: PASS. The plan includes package manifest,
  command, tool, runtime bridge, save schema, renderer, active-tool, fixture,
  and docs tests before behavior is called shipped.

## Project Structure

### Documentation (this feature)

```text
specs/001-pi-native-game-driver/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── pi-package.md
│   ├── commands.md
│   ├── tools.md
│   ├── runtime-cli.md
│   └── player-ui.md
└── tasks.md
```

### Source Code (repository root)

```text
.pi/
└── settings.json

packages/genmicon-pi/
├── package.json
├── tsconfig.json
├── extensions/
│   ├── index.ts
│   ├── commands.ts
│   ├── tools.ts
│   ├── runtime-client.ts
│   ├── active-tools.ts
│   ├── renderers.ts
│   └── ui/
│       ├── game-console.ts
│       └── diagnostics.ts
├── skills/
│   ├── game-driver/SKILL.md
│   └── player-contract/SKILL.md
├── prompts/
│   ├── game-console.md
│   └── compact-game-context.md
├── themes/
│   └── genmicon.json
└── tests/
    ├── package-load.test.ts
    ├── commands.test.ts
    ├── tools.test.ts
    ├── active-tools.test.ts
    ├── runtime-client.test.ts
    ├── renderers.test.ts
    └── ui-layout.test.ts

crates/game/
├── src/
│   ├── bin/genmicon-game-runtime.rs
│   ├── cli.rs
│   ├── manifest.rs
│   ├── save.rs
│   ├── lookup.rs
│   ├── render.rs
│   ├── driver.rs
│   └── script.rs
└── tests/
    ├── runtime_cli.rs
    ├── fixture_reconciliation.rs
    └── fixture_thirteen_angry_man.rs

examples/games/
├── reconciliation-demo/
└── thirteen-angry-man/

SPEC_files/
├── 16_GENMICON_PROJECT_INTENTION_SPEC.md
└── 17_PI_BASED_REFACTOR_PLAN_SPEC.md
```

**Structure Decision**: The Pi package lives in `packages/genmicon-pi/` so it
can be loaded by `.pi/settings.json`, npm, git, or a local path without
becoming a second runtime. The deterministic runtime stays in `crates/game/`
and gains a JSON CLI helper for Pi tool adapters. The former `crates/kernel`
scaffold was removed during cleanup because it duplicated Pi's agent harness
responsibilities after the package/runtime path was test-covered.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Runtime JSON helper beside Pi package | Pi extensions need a stable way to call existing Rust validation and save authority without porting the whole runtime to TypeScript in V1 | Rewriting `crates/game` in TypeScript would risk save-authority regressions and delay player validation; embedding a second agent loop violates the constitution |
