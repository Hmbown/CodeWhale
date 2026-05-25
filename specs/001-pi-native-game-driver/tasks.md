# Tasks: Pi-Native GENmicon Game Driver

**Input**: Design documents from `specs/001-pi-native-game-driver/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`, `quickstart.md`

**Tests**: Required for package loading, command contracts, active-tool policy, runtime bridge, save authority, fixture cartridges, player UI, diagnostics, and resume behavior.

**Organization**: Tasks are grouped by user story so each story can be implemented and validated independently.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel because it touches different files and does not depend on incomplete tasks
- **[Story]**: User story label, used only in user-story phases
- Every task names at least one exact file path

## Phase 1: Setup

**Purpose**: Create the Pi package, project-local Pi settings, and test/build scaffolds needed by all stories.

- [X] T001 Create the Pi package manifest in `packages/genmicon-pi/package.json` with `pi.extensions`, `pi.skills`, `pi.prompts`, `pi.themes`, `pi-package` keyword, and Pi core peer dependencies
- [X] T002 Create TypeScript compiler settings in `packages/genmicon-pi/tsconfig.json`
- [X] T003 Create package test bootstrap in `packages/genmicon-pi/tests/test-harness.ts`
- [X] T004 Create the package entrypoint in `packages/genmicon-pi/extensions/index.ts`
- [X] T005 [P] Create extension module placeholders in `packages/genmicon-pi/extensions/commands.ts`
- [X] T006 [P] Create extension module placeholders in `packages/genmicon-pi/extensions/tools.ts`
- [X] T007 [P] Create extension module placeholders in `packages/genmicon-pi/extensions/runtime-client.ts`
- [X] T008 [P] Create extension module placeholders in `packages/genmicon-pi/extensions/active-tools.ts`
- [X] T009 [P] Create extension module placeholders in `packages/genmicon-pi/extensions/renderers.ts`
- [X] T010 [P] Create player UI module placeholder in `packages/genmicon-pi/extensions/ui/game-console.ts`
- [X] T011 [P] Create diagnostics UI module placeholder in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T012 [P] Create initial game driver skill in `packages/genmicon-pi/skills/game-driver/SKILL.md`
- [X] T013 [P] Create initial player contract skill in `packages/genmicon-pi/skills/player-contract/SKILL.md`
- [X] T014 [P] Create game console prompt template in `packages/genmicon-pi/prompts/game-console.md`
- [X] T015 [P] Create compaction prompt template in `packages/genmicon-pi/prompts/compact-game-context.md`
- [X] T016 [P] Create GENmicon theme file in `packages/genmicon-pi/themes/genmicon.json`
- [X] T017 Create project-local package loading settings in `.pi/settings.json`
- [X] T018 Add package build and test command documentation in `packages/genmicon-pi/README.md`
- [X] T019 Update the root workspace documentation pointer in `AGENTS.md`

## Phase 2: Foundational

**Purpose**: Build the shared runtime bridge, trust policy, and test utilities that block all user stories.

- [X] T020 Add JSON runtime helper binary entrypoint in `crates/game/src/bin/genmicon-game-runtime.rs`
- [X] T021 Add runtime request/response envelope types in `crates/game/src/cli.rs`
- [X] T022 Export the runtime CLI module from `crates/game/src/lib.rs`
- [X] T023 Add `validate` and `list_saves` runtime command handlers in `crates/game/src/cli.rs`
- [X] T024 Add `status`, `render`, and `playbook` runtime command handlers in `crates/game/src/cli.rs`
- [X] T025 Add `lookup`, `fact_check`, and `run_driver` runtime command handlers in `crates/game/src/cli.rs`
- [X] T026 Add `commit_turn` runtime command handler with revision checking in `crates/game/src/cli.rs`
- [X] T027 Add runtime helper tests for JSON envelopes in `crates/game/tests/runtime_cli.rs`
- [X] T028 Add runtime fixture tests for `reconciliation-demo` in `crates/game/tests/fixture_reconciliation.rs`
- [X] T029 Add runtime fixture tests for `thirteen-angry-man` in `crates/game/tests/fixture_thirteen_angry_man.rs`
- [X] T030 Implement runtime client process invocation in `packages/genmicon-pi/extensions/runtime-client.ts`
- [X] T031 Add runtime client tests with a fake helper process in `packages/genmicon-pi/tests/runtime-client.test.ts`
- [X] T032 Implement player and developer active-tool profiles in `packages/genmicon-pi/extensions/active-tools.ts`
- [X] T033 Add active-tool policy tests in `packages/genmicon-pi/tests/active-tools.test.ts`
- [X] T034 Define shared package state types in `packages/genmicon-pi/extensions/state.ts`
- [X] T035 Add package state tests in `packages/genmicon-pi/tests/state.test.ts`
- [X] T036 Implement package source review and filter validation in `packages/genmicon-pi/extensions/package-trust.ts`
- [X] T037 Add package trust tests in `packages/genmicon-pi/tests/package-trust.test.ts`
- [X] T038 Register extension startup wiring in `packages/genmicon-pi/extensions/index.ts`
- [X] T039 Add package discovery/load tests in `packages/genmicon-pi/tests/package-load.test.ts`
- [X] T040 Document the runtime bridge contract in `packages/genmicon-pi/README.md`

**Checkpoint**: Pi package loads locally, runtime helper responds with JSON, and player/developer active-tool policies are test-covered.

## Phase 3: User Story 1 - Enable a Pi-Native Game Driver (Priority: P1)

**Goal**: A game author can enable GENmicon as a reviewed local Pi package or project-local resource and validate driver readiness before play.

**Independent Test**: Enable the local package, run validation on `examples/games/reconciliation-demo`, and verify package source, loaded resources, driver readiness, save readiness, active-tool profile, and blocking warnings.

### Tests for User Story 1

- [X] T041 [P] [US1] Add validation command contract tests in `packages/genmicon-pi/tests/commands.test.ts`
- [X] T042 [P] [US1] Add package readiness fixture test in `packages/genmicon-pi/tests/package-load.test.ts`
- [X] T043 [P] [US1] Add runtime validation fixture test in `crates/game/tests/runtime_cli.rs`
- [X] T044 [P] [US1] Add unreviewed package blocking test in `packages/genmicon-pi/tests/package-trust.test.ts`

### Implementation for User Story 1

- [X] T045 [US1] Register `/genmicon:validate` in `packages/genmicon-pi/extensions/commands.ts`
- [X] T046 [US1] Implement validation result formatting in `packages/genmicon-pi/extensions/commands.ts`
- [X] T047 [US1] Implement package resource inventory collection in `packages/genmicon-pi/extensions/package-trust.ts`
- [X] T048 [US1] Implement game cartridge readiness checks through `packages/genmicon-pi/extensions/runtime-client.ts`
- [X] T049 [US1] Implement blocking warning mapping in `packages/genmicon-pi/extensions/commands.ts`
- [X] T050 [US1] Add validation command quickstart coverage in `specs/001-pi-native-game-driver/quickstart.md`
- [X] T051 [US1] Add shipped validation behavior notes in `packages/genmicon-pi/README.md`
- [X] T052 [US1] Verify `reconciliation-demo` validation fixture in `examples/games/reconciliation-demo/game.toml`
- [X] T053 [US1] Verify `thirteen-angry-man` validation fixture in `examples/games/thirteen-angry-man/game.toml`

**Checkpoint**: `/genmicon:validate` works as an independently useful author workflow without exposing player tools.

## Phase 4: User Story 2 - Start a Local Cartridge in Player Mode (Priority: P2)

**Goal**: A player can start a local cartridge, see a player-facing game console, take one natural action, and commit exactly one authoritative turn with only game-safe tools active.

**Independent Test**: Launch `reconciliation-demo`, submit one player action, verify player UI, verify active tools, and verify save revision advances exactly once.

### Tests for User Story 2

- [X] T054 [P] [US2] Add game-safe tool registration tests in `packages/genmicon-pi/tests/tools.test.ts`
- [X] T055 [P] [US2] Add player active-tool launch tests in `packages/genmicon-pi/tests/active-tools.test.ts`
- [X] T056 [P] [US2] Add player console layout tests in `packages/genmicon-pi/tests/ui-layout.test.ts`
- [X] T057 [P] [US2] Add game tool renderer tests in `packages/genmicon-pi/tests/renderers.test.ts`
- [X] T058 [P] [US2] Add commit-once fixture test in `crates/game/tests/runtime_cli.rs`
- [X] T059 [P] [US2] Add player turn integration test in `packages/genmicon-pi/tests/player-turn.test.ts`

### Implementation for User Story 2

- [X] T060 [US2] Register `/genmicon:play` in `packages/genmicon-pi/extensions/commands.ts`
- [X] T061 [US2] Register `game_status` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T062 [US2] Register `game_render` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T063 [US2] Register `game_playbook` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T064 [US2] Register `game_lookup` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T065 [US2] Register `game_fact_check` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T066 [US2] Register `game_run_driver` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T067 [US2] Register sequential `game_commit_turn` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T068 [US2] Implement tool schema definitions with `typebox` in `packages/genmicon-pi/extensions/tools.ts`
- [X] T069 [US2] Implement player active-tool installation during play launch in `packages/genmicon-pi/extensions/active-tools.ts`
- [X] T070 [US2] Implement player console component in `packages/genmicon-pi/extensions/ui/game-console.ts`
- [X] T071 [US2] Implement compact fallback game view in `packages/genmicon-pi/extensions/ui/game-console.ts`
- [X] T072 [US2] Implement tool result renderers that hide raw JSON in `packages/genmicon-pi/extensions/renderers.ts`
- [X] T073 [US2] Implement player-facing message renderer in `packages/genmicon-pi/extensions/renderers.ts`
- [X] T074 [US2] Implement player action composer behavior in `packages/genmicon-pi/extensions/ui/game-console.ts`
- [X] T075 [US2] Add game console prompt instructions in `packages/genmicon-pi/prompts/game-console.md`
- [X] T076 [US2] Add player contract skill instructions in `packages/genmicon-pi/skills/player-contract/SKILL.md`
- [X] T077 [US2] Refresh game view after successful commit in `packages/genmicon-pi/extensions/tools.ts`
- [X] T078 [US2] Document player launch workflow in `packages/genmicon-pi/README.md`

**Checkpoint**: A one-turn player loop works with only game-safe tools active and one authoritative save commit.

## Phase 5: User Story 3 - Inspect Driver Diagnostics Safely (Priority: P3)

**Goal**: A cartridge author can turn diagnostics on and off to inspect package, resource, tool, save, driver, render, and warning state without weakening player mode.

**Independent Test**: Launch the same fixture in player and developer modes, verify diagnostics visibility, and verify active-tool policy remains unchanged.

### Tests for User Story 3

- [X] T079 [P] [US3] Add developer command tests in `packages/genmicon-pi/tests/commands.test.ts`
- [X] T080 [P] [US3] Add diagnostics UI tests in `packages/genmicon-pi/tests/diagnostics.test.ts`
- [X] T081 [P] [US3] Add active-tool invariance tests in `packages/genmicon-pi/tests/active-tools.test.ts`
- [X] T082 [P] [US3] Add developer renderer expansion tests in `packages/genmicon-pi/tests/renderers.test.ts`

### Implementation for User Story 3

- [X] T083 [US3] Register `/genmicon:dev` in `packages/genmicon-pi/extensions/commands.ts`
- [X] T084 [US3] Implement diagnostics state toggle in `packages/genmicon-pi/extensions/state.ts`
- [X] T085 [US3] Implement diagnostics component in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T086 [US3] Show package source and loaded resources in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T087 [US3] Show active tools and developer-only surfaces in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T088 [US3] Show save revision and driver identity in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T089 [US3] Show render snapshot and runtime warnings in `packages/genmicon-pi/extensions/ui/diagnostics.ts`
- [X] T090 [US3] Implement developer renderer expansion in `packages/genmicon-pi/extensions/renderers.ts`
- [X] T091 [US3] Preserve player active-tool profile when diagnostics toggle in `packages/genmicon-pi/extensions/active-tools.ts`
- [X] T092 [US3] Add diagnostics quickstart coverage in `specs/001-pi-native-game-driver/quickstart.md`
- [X] T093 [US3] Document diagnostics behavior in `packages/genmicon-pi/README.md`

**Checkpoint**: Diagnostics are explicit, reversible, and do not alter player-mode trust policy.

## Phase 6: User Story 4 - Resume From Authoritative Save State (Priority: P4)

**Goal**: A player can restart and resume from authoritative save files without prior transcript history.

**Independent Test**: Commit one turn, restart a clean Pi session, resume the save, and verify the view reflects `STATE.json` and `TURN_LOG.jsonl`.

### Tests for User Story 4

- [X] T094 [P] [US4] Add resume runtime fixture test in `crates/game/tests/runtime_cli.rs`
- [X] T095 [P] [US4] Add transcript-missing resume test in `packages/genmicon-pi/tests/resume.test.ts`
- [X] T096 [P] [US4] Add compaction context tests in `packages/genmicon-pi/tests/compaction.test.ts`
- [X] T097 [P] [US4] Add save list command tests in `packages/genmicon-pi/tests/commands.test.ts`

### Implementation for User Story 4

- [X] T098 [US4] Register `/genmicon:saves` in `packages/genmicon-pi/extensions/commands.ts`
- [X] T099 [US4] Implement save listing through `packages/genmicon-pi/extensions/runtime-client.ts`
- [X] T100 [US4] Implement resume state loading in `packages/genmicon-pi/extensions/commands.ts`
- [X] T101 [US4] Implement Pi session context injection from save snapshot in `packages/genmicon-pi/extensions/index.ts`
- [X] T102 [US4] Implement game-aware compaction prompt usage in `packages/genmicon-pi/prompts/compact-game-context.md`
- [X] T103 [US4] Prevent transcript-derived state writes in `packages/genmicon-pi/extensions/tools.ts`
- [X] T104 [US4] Add restart/resume quickstart coverage in `specs/001-pi-native-game-driver/quickstart.md`
- [X] T105 [US4] Document save authority and resume behavior in `packages/genmicon-pi/README.md`

**Checkpoint**: Restart/resume is correct from save files even when previous Pi transcript context is absent.

## Phase 7: Rebuild Cleanup And Migration

**Purpose**: Remove or reclassify inherited duplicate runtime surfaces after the Pi-native package path is test-covered.

- [X] T106 Add migration note for `crates/kernel` in `SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md`
- [X] T107 Remove `crates/kernel` from workspace members in `Cargo.toml` after Pi package tests cover equivalent policy
- [X] T108 Delete duplicate kernel crate files in `crates/kernel/Cargo.toml`
- [X] T109 Delete duplicate kernel implementation in `crates/kernel/src/lib.rs`
- [X] T110 Update `Cargo.lock` after removing `crates/kernel`
- [X] T111 Update package/runtime ownership notes in `AGENTS.md`
- [X] T112 Update project intention notes in `SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md`
- [X] T113 Update game driver spec index in `SPEC_files/game_driver/README.md`
- [X] T114 Update game cartridge spec index in `SPEC_files/games/README.md`
- [X] T115 Update existing Game TUI bridge notes in `docs/GAME_TUI_FRAMEWORK_SPEC.md`
- [X] T116 Update transition alias notes in `TAKEOVER_PROMPT.md`
- [X] T117 Add package installation notes in `README.md`
- [X] T118 Add release evidence checklist for the Pi-native rebuild in `SPEC_files/WORKFLOW.md`

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Validate contracts, docs, tests, and release readiness across the full rebuild.

- [X] T119 Run and document `cargo test -p deepseek-game --all-features` results in `specs/001-pi-native-game-driver/quickstart.md`
- [X] T120 Run and document package test results in `packages/genmicon-pi/README.md`
- [X] T121 Add contract cross-check notes for Pi package loading in `specs/001-pi-native-game-driver/contracts/pi-package.md`
- [X] T122 Add contract cross-check notes for commands in `specs/001-pi-native-game-driver/contracts/commands.md`
- [X] T123 Add contract cross-check notes for tools in `specs/001-pi-native-game-driver/contracts/tools.md`
- [X] T124 Add contract cross-check notes for runtime CLI in `specs/001-pi-native-game-driver/contracts/runtime-cli.md`
- [X] T125 Add contract cross-check notes for player UI in `specs/001-pi-native-game-driver/contracts/player-ui.md`
- [X] T126 Verify all shipped behavior has matching docs in `packages/genmicon-pi/README.md`
- [X] T127 Verify no unreviewed remote package source is enabled in `.pi/settings.json`
- [X] T128 Verify player-mode tool allowlist excludes shell/file/git/package/provider controls in `packages/genmicon-pi/tests/active-tools.test.ts`
- [X] T129 Verify compact, medium, and wide render fixtures in `packages/genmicon-pi/tests/ui-layout.test.ts`
- [X] T130 Run final Spec Kit consistency review across `specs/001-pi-native-game-driver/plan.md`
- [X] T131 Run final Spec Kit consistency review across `specs/001-pi-native-game-driver/spec.md`
- [X] T132 Run final Spec Kit consistency review across `specs/001-pi-native-game-driver/tasks.md`

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: Starts immediately.
- **Foundational (Phase 2)**: Depends on package scaffold and `.pi/settings.json`.
- **US1 (Phase 3)**: Depends on runtime helper, runtime client, package trust, and command scaffold.
- **US2 (Phase 4)**: Depends on US1 readiness validation and active-tool policy.
- **US3 (Phase 5)**: Depends on US2 player launch state and renderer hooks.
- **US4 (Phase 6)**: Depends on US2 commit path and US3 diagnostics state visibility.
- **Cleanup (Phase 7)**: Depends on US1-US4 tests passing; do not remove `crates/kernel` before equivalent Pi package tests pass.
- **Polish (Phase 8)**: Depends on all intended story phases and cleanup decisions.

### User Story Dependencies

- **US1**: Independent MVP for author validation.
- **US2**: Requires US1 package validation so player launch starts from a trusted state.
- **US3**: Can begin after US2 has player state and renderers, but must not change US2 active tools.
- **US4**: Requires US2 commit behavior so resume has a committed turn to reload.

### Within Each User Story

- Tests precede implementation.
- Runtime authority precedes Pi tool exposure.
- Active-tool policy precedes player launch.
- Player/developer rendering follows command/tool behavior.
- Docs update before a story is marked complete.

## Parallel Opportunities

- Setup placeholders T005-T016 can run in parallel after T001-T004.
- Runtime CLI tests T027-T029 can run in parallel after T020-T026.
- Package policy tests T031, T033, T035, T037, and T039 can run in parallel after T030-T038.
- US1 tests T041-T044 can run in parallel.
- US2 tests T054-T059 can run in parallel.
- US3 tests T079-T082 can run in parallel.
- US4 tests T094-T097 can run in parallel.
- Documentation cross-check tasks T121-T125 can run in parallel.

## Parallel Example: User Story 2

```bash
Task: "T054 [P] [US2] Add game-safe tool registration tests in packages/genmicon-pi/tests/tools.test.ts"
Task: "T055 [P] [US2] Add player active-tool launch tests in packages/genmicon-pi/tests/active-tools.test.ts"
Task: "T056 [P] [US2] Add player console layout tests in packages/genmicon-pi/tests/ui-layout.test.ts"
Task: "T057 [P] [US2] Add game tool renderer tests in packages/genmicon-pi/tests/renderers.test.ts"
Task: "T058 [P] [US2] Add commit-once fixture test in crates/game/tests/runtime_cli.rs"
Task: "T059 [P] [US2] Add player turn integration test in packages/genmicon-pi/tests/player-turn.test.ts"
```

## Implementation Strategy

### MVP First

1. Complete Phase 1 and Phase 2.
2. Complete US1 only.
3. Validate package loading and `/genmicon:validate` on `reconciliation-demo`.
4. Stop and confirm the Pi package path before implementing player turns.

### Full Cycle

1. Package scaffold and runtime helper.
2. Author validation.
3. Player launch and one-turn commit.
4. Developer diagnostics.
5. Restart/resume from save authority.
6. Remove or reclassify duplicate kernel scaffold.
7. Run final docs and contract validation.

### Release Readiness

The feature is not complete until package load, active-tool policy, save
authority, player render, diagnostics, resume, and documentation checks all
have matching tests or recorded manual evidence.
