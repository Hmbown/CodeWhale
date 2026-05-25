# Implementation Plan: [FEATURE]

**Branch**: `[###-feature-name]` | **Date**: [DATE] | **Spec**: [link]

**Input**: Feature specification from `/specs/[###-feature-name]/spec.md`

**Note**: This template is filled in by the `/speckit-plan` command. See `.specify/templates/plan-template.md` for the execution workflow.

## Summary

[Extract from feature spec: primary requirement + technical approach from research]

## Technical Context

<!--
  ACTION REQUIRED: Replace the content in this section with the technical details
  for the project. The structure here is presented in advisory capacity to guide
  the iteration process.
-->

**Language/Version**: [e.g., Pi package/extension TypeScript on current Pi, Rust 1.88 for deterministic runtime code, or NEEDS CLARIFICATION]

**Primary Dependencies**: [e.g., Pi extension/package APIs, @earendil-works/pi-tui, crates/game, Starlark, or NEEDS CLARIFICATION]

**Storage**: [if applicable, e.g., Pi session entries, STATE.json, TURN_LOG.jsonl, local package files, or N/A]

**Testing**: [e.g., package/extension tests, cargo test, fixture cartridge tests, terminal render tests, or NEEDS CLARIFICATION]

**Target Platform**: [e.g., Pi interactive TUI, Pi JSON/RPC mode, local terminal, or NEEDS CLARIFICATION]

**Project Type**: [Pi package/extension + local game runtime + terminal game UI, or NEEDS CLARIFICATION]

**Performance Goals**: [domain-specific, e.g., 1000 req/s, 10k lines/sec, 60 fps or NEEDS CLARIFICATION]

**Constraints**: [domain-specific, e.g., <200ms p95, <100MB memory, offline-capable or NEEDS CLARIFICATION]

**Scale/Scope**: [domain-specific, e.g., 10k users, 1M LOC, 50 screens or NEEDS CLARIFICATION]

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **Pi Substrate First**: Identify the Pi primitive used for each affected
  surface: package, extension event, command, tool, active-tool policy, session,
  compaction, provider, message renderer, custom UI/editor, skill, prompt, or
  theme. Any new parallel subsystem must have an exception in Complexity
  Tracking.
- **Game Console First**: Confirm player mode opens to game-facing UI and hides
  implementation machinery; list developer-mode diagnostics separately.
- **Save State Authority**: Confirm all durable game mutations pass through the
  game commit path with revision checks; Pi sessions/compaction/render caches
  remain derived context.
- **Package Trust Boundaries**: List package sources, pins, filters, reviewed
  extension code, and active tools. Confirm game package data cannot expand
  player tools or policy.
- **Tested Shipped Contracts**: List tests/docs required before any command,
  package key, manifest field, tool ABI, save schema, driver function, or
  renderer is called shipped.

## Project Structure

### Documentation (this feature)

```text
specs/[###-feature]/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)
<!--
  ACTION REQUIRED: Replace the placeholder tree below with the concrete layout
  for this feature. Delete unused options and expand the chosen structure with
  real paths (e.g., apps/admin, packages/something). The delivered plan must
  not include Option labels.
-->

```text
# [REMOVE IF UNUSED] Option 1: Pi package or project-local Pi resources
.pi/
└── settings.json

packages/genmicon/
├── package.json          # pi manifest: extensions, skills, prompts, themes
├── extensions/
├── skills/
├── prompts/
├── themes/
└── tests/

# [REMOVE IF UNUSED] Option 2: Deterministic Rust game runtime
crates/game/
├── src/
│   ├── manifest/
│   ├── save/
│   ├── lookup/
│   ├── render/
│   └── driver/
└── tests/

tests/
├── fixtures/
├── integration/
└── render/

# [REMOVE IF UNUSED] Option 3: Game cartridge or driver package
examples/games/[game-id]/
├── game.toml
├── GAME.md
├── content/
├── skills/
├── prompts/
├── assets/
└── saves/
```

**Structure Decision**: [Document the selected structure and reference the real
directories captured above]

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| [e.g., 4th project] | [current need] | [why 3 projects insufficient] |
| [e.g., Repository pattern] | [specific problem] | [why direct DB access insufficient] |
