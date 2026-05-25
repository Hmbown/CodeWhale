---

description: "Task list template for feature implementation"
---

# Tasks: [FEATURE NAME]

**Input**: Design documents from `/specs/[###-feature-name]/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Tests are REQUIRED for shipped game contracts, trust boundaries,
save mutations, active-tool policy, package loading, and player/developer UI
rendering. Tests may be omitted only for docs-only or exploratory planning
tasks, and that omission must be stated in the task file.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Pi package/extension**: `packages/<name>/extensions/`, `skills/`,
  `prompts/`, `themes/`, `tests/`
- **Project-local Pi config/resources**: `.pi/settings.json`, `.pi/`
- **Rust game runtime**: `crates/game/src/`, `crates/game/tests/`
- **Game cartridge fixtures**: `examples/games/<game-id>/`
- **Terminal/UI tests**: package tests, TUI component tests, or Rust buffer
  tests as selected by plan.md

<!--
  ============================================================================
  IMPORTANT: The tasks below are SAMPLE TASKS for illustration purposes only.

  The /speckit-tasks command MUST replace these with actual tasks based on:
  - User stories from spec.md (with their priorities P1, P2, P3...)
  - Feature requirements from plan.md
  - Entities from data-model.md
  - Endpoints from contracts/

  Tasks MUST be organized by user story so each story can be:
  - Implemented independently
  - Tested independently
  - Delivered as an MVP increment

  DO NOT keep these sample tasks in the generated tasks.md file.
  ============================================================================
-->

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, Pi package wiring, and basic structure

- [ ] T001 Create project/Pi package structure per implementation plan
- [ ] T002 Add or update `package.json` Pi manifest for extensions, skills, prompts, and themes
- [ ] T003 [P] Configure linting, formatting, and test commands for the selected Pi/Rust surfaces
- [ ] T004 [P] Add fixture cartridge or save data needed by the first user story

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

Examples of foundational tasks (adjust based on your project):

- [ ] T005 Define Pi active-tool allowlists for player and developer mode
- [ ] T006 [P] Register base Pi command/tool/resource entrypoints
- [ ] T007 [P] Implement package source, pin, and filter validation
- [ ] T008 Implement save authority checks and expected-revision handling
- [ ] T009 Implement path canonicalization for cartridge, driver, save, and asset roots
- [ ] T010 Configure developer diagnostics without exposing them in player mode

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 3: User Story 1 - [Title] (Priority: P1) 🎯 MVP

**Goal**: [Brief description of what this story delivers]

**Independent Test**: [How to verify this story works on its own]

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T011 [P] [US1] Contract test for [Pi command/tool/package key/manifest field] in [test path]
- [ ] T012 [P] [US1] Integration test for [player/game author/developer journey] in [test path]
- [ ] T013 [P] [US1] Trust-boundary test for [active tools/save write/package input/path access] in [test path]

### Implementation for User Story 1

- [ ] T014 [P] [US1] Add package/resource declaration in [package/config path]
- [ ] T015 [P] [US1] Implement deterministic game/runtime operation in [path]
- [ ] T016 [US1] Register Pi command/tool/renderer/custom UI in [path]
- [ ] T017 [US1] Implement player-mode behavior and hidden developer detail policy
- [ ] T018 [US1] Add validation, error handling, and developer diagnostics
- [ ] T019 [US1] Update relevant docs/specs for the shipped contract

**Checkpoint**: At this point, User Story 1 should be fully functional and testable independently

---

## Phase 4: User Story 2 - [Title] (Priority: P2)

**Goal**: [Brief description of what this story delivers]

**Independent Test**: [How to verify this story works on its own]

### Tests for User Story 2

- [ ] T020 [P] [US2] Contract test for [Pi command/tool/package key/manifest field] in [test path]
- [ ] T021 [P] [US2] Integration test for [user journey] in [test path]
- [ ] T022 [P] [US2] Trust-boundary or persistence regression test in [test path]

### Implementation for User Story 2

- [ ] T023 [P] [US2] Add package/resource declaration in [package/config path]
- [ ] T024 [US2] Implement [runtime operation/Pi extension behavior] in [path]
- [ ] T025 [US2] Integrate with User Story 1 components without widening player-mode tools
- [ ] T026 [US2] Update docs/specs for the shipped contract

**Checkpoint**: At this point, User Stories 1 AND 2 should both work independently

---

## Phase 5: User Story 3 - [Title] (Priority: P3)

**Goal**: [Brief description of what this story delivers]

**Independent Test**: [How to verify this story works on its own]

### Tests for User Story 3

- [ ] T027 [P] [US3] Contract test for [Pi command/tool/package key/manifest field] in [test path]
- [ ] T028 [P] [US3] Integration test for [user journey] in [test path]

### Implementation for User Story 3

- [ ] T029 [P] [US3] Add package/resource declaration in [package/config path]
- [ ] T030 [US3] Implement [runtime operation/Pi extension behavior] in [path]
- [ ] T031 [US3] Update docs/specs for the shipped contract

**Checkpoint**: All user stories should now be independently functional

---

[Add more user story phases as needed, following the same pattern]

---

## Phase N: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [ ] TXXX [P] Documentation updates in docs/
- [ ] TXXX Code cleanup and refactoring
- [ ] TXXX Performance optimization across all stories
- [ ] TXXX [P] Additional unit, package, fixture, or render tests in [test path]
- [ ] TXXX Security hardening
- [ ] TXXX Verify Pi package source pins, filters, peerDependencies, and loaded resources
- [ ] TXXX Run quickstart.md validation

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion - BLOCKS all user stories
- **User Stories (Phase 3+)**: All depend on Foundational phase completion
  - User stories can then proceed in parallel (if staffed)
  - Or sequentially in priority order (P1 → P2 → P3)
- **Polish (Final Phase)**: Depends on all desired user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) - No dependencies on other stories
- **User Story 2 (P2)**: Can start after Foundational (Phase 2) - May integrate with US1 but should be independently testable
- **User Story 3 (P3)**: Can start after Foundational (Phase 2) - May integrate with US1/US2 but should be independently testable

### Within Each User Story

- Contract, trust-boundary, and persistence tests MUST be written and FAIL before implementation
- Pi package/resource declaration before extension behavior that consumes it
- Deterministic runtime operation before Pi tool/command exposure
- Player active-tool policy before player-facing turn loop exposure
- Core implementation before integration
- Story complete before moving to next priority

### Parallel Opportunities

- All Setup tasks marked [P] can run in parallel
- All Foundational tasks marked [P] can run in parallel (within Phase 2)
- Once Foundational phase completes, all user stories can start in parallel (if team capacity allows)
- All tests for a user story marked [P] can run in parallel
- Models within a story marked [P] can run in parallel
- Different user stories can be worked on in parallel by different team members

---

## Parallel Example: User Story 1

```bash
# Launch all tests for User Story 1 together:
Task: "Contract test for [Pi command/tool/package key/manifest field] in [test path]"
Task: "Integration test for [player/game author/developer journey] in [test path]"
Task: "Trust-boundary test for [active tools/save write/package input/path access] in [test path]"

# Launch independent implementation pieces together:
Task: "Add package/resource declaration in [package/config path]"
Task: "Implement deterministic game/runtime operation in [path]"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL - blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Test User Story 1 independently
5. Deploy/demo if ready

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 → Test independently → Deploy/Demo (MVP!)
3. Add User Story 2 → Test independently → Deploy/Demo
4. Add User Story 3 → Test independently → Deploy/Demo
5. Each story adds value without breaking previous stories

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1
   - Developer B: User Story 2
   - Developer C: User Story 3
3. Stories complete and integrate independently

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify required tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Avoid: vague tasks, same file conflicts, cross-story dependencies that break independence
- Avoid: rebuilding Pi primitives without an approved constitution exception
