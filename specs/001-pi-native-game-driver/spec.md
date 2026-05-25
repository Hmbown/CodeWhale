# Feature Specification: Pi-Native GENmicon Game Driver

**Feature Branch**: `001-pi-native-game-driver`

**Created**: 2026-05-25

**Status**: Draft

**Input**: User description: "Please start to working on actual plan for rebuild the pi native GENmicon game driver"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Enable a Pi-Native Game Driver (Priority: P1)

A game author can enable GENmicon as a reviewed local Pi package or
project-local Pi resource, validate that the GENmicon game driver is available,
and see a clear readiness result before trying to play a game.

**Why this priority**: The rebuild must prove that GENmicon builds on Pi rather
than beside it. A package-first driver entry point is the smallest useful
foundation for all later play, authoring, and diagnostics work.

**Independent Test**: Enable the GENmicon resource in a clean project, request
driver validation for a fixture cartridge, and verify that the result reports
loaded resources, trusted package source, driver readiness, and any blocking
warnings without launching gameplay.

**Acceptance Scenarios**:

1. **Given** a reviewed local GENmicon package and a valid fixture cartridge,
   **When** the author enables the package and runs driver validation,
   **Then** the system reports the driver as available and lists the loaded
   game resources.
2. **Given** a missing, disabled, or unreviewed package source, **When** the
   author requests driver validation, **Then** the system reports a blocking
   readiness error and does not expose player-mode game tools.

---

### User Story 2 - Start a Local Cartridge in Player Mode (Priority: P2)

A player can start a local game cartridge through the GENmicon Pi entry point,
see a player-facing game console, and take a first natural-language action
while only game-safe capabilities are available.

**Why this priority**: The core product value is terminal-native play, not
authoring setup. This scenario proves that the driver can support a playable
loop without exposing generic coding-agent machinery.

**Independent Test**: Launch a fixture cartridge from a clean Pi session, submit
a player action, and verify that the player sees a game-facing response, the
active capability list remains game-safe, and the save revision advances
exactly once.

**Acceptance Scenarios**:

1. **Given** a valid local cartridge and save, **When** the player starts the
   game, **Then** the first visible surface is a game console with scene,
   status, inventory or equivalent state, dialogue, choices, and an action
   composer.
2. **Given** an active player session, **When** the player submits a natural
   action, **Then** the turn resolves through the game driver, commits durable
   state exactly once, and refreshes the game view from the committed save.
3. **Given** player mode is active, **When** the model or game content attempts
   to use non-game capabilities, **Then** the request is blocked or ignored and
   the player-facing game remains usable.

---

### User Story 3 - Inspect Driver Diagnostics Safely (Priority: P3)

A cartridge author or maintainer can switch to developer diagnostics to inspect
driver readiness, package provenance, active capabilities, save state, render
data, and validation warnings without weakening player-mode restrictions.

**Why this priority**: Authors need enough visibility to debug cartridges and
drivers, but diagnostics must not become part of ordinary play or broaden the
trust boundary.

**Independent Test**: Start the same fixture cartridge in player mode and
developer diagnostics mode, then verify that diagnostics appear only in the
developer view and that the player capability allowlist is unchanged.

**Acceptance Scenarios**:

1. **Given** developer diagnostics are disabled, **When** gameplay is resolving,
   **Then** raw package paths, tool activity, save internals, and model/provider
   details are hidden from the player view.
2. **Given** developer diagnostics are enabled, **When** the author inspects the
   active game, **Then** the system shows package source, loaded resources,
   active capabilities, save revision, driver version, render snapshot, and
   validation warnings.
3. **Given** developer diagnostics have been turned off again, **When** the
   next player action is submitted, **Then** the session returns to the
   player-facing game console with the same game-safe capability restrictions.

---

### User Story 4 - Resume From Authoritative Save State (Priority: P4)

A player can close and restart the game, then resume from the authoritative save
state rather than relying on transcript history or generated summaries.

**Why this priority**: Long-running AI games need durable state and predictable
recovery. This scenario proves that the driver treats save files as authority
while using conversation context only as derived support.

**Independent Test**: Complete one fixture turn, restart the session, resume the
same save, and verify that the displayed state, revision, and recent outcome
match the saved data even if the previous transcript is absent.

**Acceptance Scenarios**:

1. **Given** a committed save revision, **When** the player restarts and resumes
   the cartridge, **Then** the game opens at the saved state with the recorded
   driver identity and revision.
2. **Given** transcript history or summaries are missing, **When** the save is
   valid, **Then** gameplay resumes from save data and reports only non-blocking
   context warnings if needed.

### Edge Cases

- If a Pi package is missing, disabled, unpinned, unreviewed, or shadowed by a
  project-local entry, validation reports the active source and blocks player
  tool exposure until the author resolves the package decision.
- If a package, skill, prompt, model output, or game markdown requests powers
  outside the game-safe player profile, the driver rejects the request and
  records a developer-visible warning.
- If `STATE.json`, `TURN_LOG.jsonl`, or the expected save revision is stale,
  malformed, or missing, the driver refuses to commit a turn and gives the
  author a recoverable diagnostic.
- If the rich game console cannot fit the terminal or a renderer fails, the
  player receives a compact text game view that preserves playability.
- If optional cartridge resources such as art, music, prompts, themes, or
  non-entry skills are absent, the driver warns in diagnostics and continues
  with deterministic fallback presentation.
- If the same player action is retried after a transient failure, the driver
  prevents duplicate durable commits for the same expected save revision.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a GENmicon game driver entry point that can
  be enabled as a reviewed local Pi package or project-local Pi resource.
- **FR-002**: System MUST expose driver validation that reports package
  provenance, loaded game resources, active capability profile, save readiness,
  and blocking warnings before gameplay starts.
- **FR-003**: System MUST start a local cartridge in player mode from a trusted
  package/resource configuration and show a game-facing console as the first
  visible surface.
- **FR-004**: Player mode MUST expose only the documented game-safe capability
  allowlist and MUST exclude generic shell, file editing, git, broad external
  service, package-install, and unrestricted automation capabilities.
- **FR-005**: System MUST resolve a player turn through the GENmicon driver and
  commit durable game state exactly once through the authoritative save commit
  path.
- **FR-006**: System MUST treat save files and turn logs as the source of truth
  for game progress; transcripts, summaries, renderer caches, and model prose
  are derived context only.
- **FR-007**: System MUST provide developer diagnostics for package source,
  loaded resources, active capabilities, save revision, driver identity, render
  snapshot, and validation warnings without changing player-mode restrictions.
- **FR-008**: System MUST prevent game content, prompts, skills, package
  metadata, and model output from granting new capabilities or overriding
  trust policy.
- **FR-009**: System MUST support restart and resume from an authoritative save
  even when prior transcript context is unavailable.
- **FR-010**: System MUST document the shipped driver entry point, package
  expectations, player capability allowlist, save authority rule, and
  diagnostic behavior before the feature is considered complete.

### Pi Surface & Trust Requirements *(mandatory)*

- **Pi Primitive**: The default integration boundary is a Pi package or
  project-local Pi resource that contributes GENmicon commands, game-safe
  callable tools, skills, prompts, themes, custom renderers, and game console
  UI surfaces. Deterministic save and driver authority may live in a small
  local runtime that is called by those Pi surfaces.
- **Package Source**: V1 accepts reviewed local packages and project-local
  resources by default. Remote npm or git packages are allowed only when pinned
  and explicitly reviewed before player-mode use.
- **Loaded Resources**: The feature may load GENmicon extension code, driver
  skills, game prompts, themes, renderer definitions, and cartridge resources.
  Resource filters must be available when a package contains more than the
  trusted GENmicon surface.
- **Active Tools in Player Mode**: The player allowlist is limited to game
  status, render, playbook, lookup, fact check, declared driver functions,
  authoritative turn commit, skill loading for approved game skills, and scoped
  game proposal helpers when explicitly enabled by the driver.
- **Developer-Only Surfaces**: Generic shell, file editing, git, package
  install/update, provider configuration, raw session manipulation, raw state
  editing, and broad external integrations are developer-only or out of scope
  for this milestone.
- **Save Authority**: Durable writes go through one authoritative game commit
  operation that checks the expected save revision, validates the proposed
  state change, appends a turn record, and refreshes the view from saved data.
- **Untrusted Inputs**: Game markdown, package metadata, issue text, model
  output, generated skills, prompt text, render proposals, and optional assets
  are treated as data. They cannot alter active capabilities, trust policy, or
  save authority.

### Key Entities *(include if feature involves data)*

- **GENmicon Driver Package**: The reviewed Pi package or project-local
  resource set that makes the game driver available to a Pi session.
- **Game Cartridge**: A local game package containing manifest metadata,
  content, skills, prompts, themes or presentation data, assets, and saves.
- **Game Driver**: The reusable genre/runtime contract that validates cartridge
  readiness, exposes game-safe capabilities, resolves deterministic driver
  functions, and governs turn shape.
- **Game Save**: The authoritative state and append-only turn history used to
  resume play and validate commits.
- **Player Capability Profile**: The exact set of game-safe actions available
  during player mode.
- **Developer Diagnostic View**: The author-facing view of package provenance,
  resource loading, active capabilities, save state, render data, and warnings.
- **Game View**: The player-facing scene, status, inventory or equivalent state,
  tasks, dialogue, choices, and action composer derived from the current save.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A game author can enable the reviewed GENmicon package/resource
  and receive a driver readiness result for a fixture cartridge in under 30
  seconds.
- **SC-002**: In all validated player launch paths, the active capability list
  contains only documented game-safe capabilities.
- **SC-003**: A player can launch a fixture cartridge, submit one natural
  action, receive a game-facing response, and see the refreshed state in under
  two minutes.
- **SC-004**: A valid player turn advances the save revision exactly once and
  appends exactly one durable turn record.
- **SC-005**: Restarting after a committed turn restores the same save revision
  and displayed game state without requiring prior transcript history.
- **SC-006**: Developer diagnostics identify package source, loaded resources,
  active capabilities, save revision, driver identity, and validation warnings
  for 100% of fixture validation runs.
- **SC-007**: Terminal presentation remains usable at compact, medium, and wide
  terminal sizes by falling back to a compact game view when rich presentation
  cannot fit.

## Assumptions

- The first milestone targets local play and local authoring; hosted
  marketplaces, telemetry, remote music services, and unreviewed remote package
  loading are out of scope.
- Pi package/resource integration is the preferred GENmicon distribution path;
  any standalone wrapper is an adapter around Pi, not a replacement harness.
- Existing game runtime ideas and fixtures may be reused when they serve save
  authority, validation, lookup, render snapshots, or deterministic driver
  behavior.
- The first serious validation target is a small fixture cartridge plus one
  deliberation-style serious-game scaffold, not a full game marketplace.
- Developer diagnostics are intended for cartridge authors and maintainers, not
  ordinary players.
