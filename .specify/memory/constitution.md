<!--
Sync Impact Report
Version change: placeholder -> 1.0.0
Modified principles:
- Placeholder principle 1 -> I. Pi Substrate First
- Placeholder principle 2 -> II. Game Console First
- Placeholder principle 3 -> III. Save State Is Authority
- Placeholder principle 4 -> IV. Package Trust Boundaries
- Placeholder principle 5 -> V. Shipped Interfaces Are Tested Contracts
Added sections:
- Architecture Constraints
- Development Workflow
Removed sections:
- None
Templates requiring updates:
- .specify/templates/plan-template.md: updated
- .specify/templates/spec-template.md: updated
- .specify/templates/tasks-template.md: updated
- .specify/templates/checklist-template.md: no change required
- .specify/extensions/git/commands/*.md: no change required
Runtime guidance requiring updates:
- SPEC_files/16_GENMICON_PROJECT_INTENTION_SPEC.md: updated
- SPEC_files/17_PI_BASED_REFACTOR_PLAN_SPEC.md: updated
Follow-up TODOs:
- None
-->
# GENmicon-TUI Constitution

## Core Principles

### I. Pi Substrate First
GENmicon-TUI MUST build on Pi's established agent harness rather than creating
a parallel agent runtime. Features MUST first map to Pi packages, extensions,
skills, prompt templates, themes, session/tree/compaction APIs, provider/model
registry, tool registration, active-tool control, command registration, message
renderers, or TUI components. A new agent/session/tool/TUI/package subsystem is
allowed only after a written exception explains which Pi primitive is
insufficient, how compatibility will be preserved, and what will be deleted
when the exception is no longer needed.

Rationale: the project goal is a native game agent system built upon Pi's
well-established features, not another heavy coding-agent stack with Pi-like
ideas copied into it.

### II. Game Console First
The first visible experience in player mode MUST be the active game. Player
mode MUST hide coding-agent chrome, raw tool traffic, model/provider noise,
filesystem paths, cost meters, and developer diagnostics unless an error needs
player action. Game UI MUST use Pi TUI component rules for width, focus,
keyboard handling, overlays, renderers, and custom editor behavior. Developer
mode MUST be explicit and reversible.

Rationale: GENmicon is a terminal-native game console. The player sees
scene, figure, status, inventory, tasks, dialogue, choices, and an action
composer before implementation machinery.

### III. Save State Is Authority
Game progress MUST be authoritative only in save files and game-runtime commit
records such as `STATE.json` and `TURN_LOG.jsonl`. Pi sessions, transcripts,
branch summaries, compaction entries, render caches, model prose, and
sub-agent outputs are derived context. State changes MUST pass through the
native game commit path, with expected-revision checks and deterministic driver
functions for mechanics that cannot be trusted to prose.

Rationale: AI can resolve ambiguous play, narration, and dialogue, but durable
game facts must remain restartable, inspectable, and mechanically validated.

### IV. Package Trust Boundaries
Game cartridges, drivers, skills, prompts, themes, and optional extensions
SHOULD be distributed as Pi packages or project-local Pi resources. Package
sources MUST be local, pinned, or explicitly reviewed before they are loaded.
Package filtering MUST be used when only some resources are trusted. Player
mode MUST NOT let game package data expand active tools, alter approval or
sandbox policy, run arbitrary shell/network/file operations, or silently install
remote packages. Any trusted extension code is reviewed code, not game content.

Rationale: Pi packages are the right distribution primitive, but Pi documents
that package extensions run with full system access. GENmicon must preserve
Pi's power while treating unreviewed game data as untrusted input.

### V. Shipped Interfaces Are Tested Contracts
Commands, Pi package manifests, game manifests, save schemas, tool ABIs,
driver functions, player/developer renderers, and provider assumptions MUST be
small, documented, and covered by focused tests before they are treated as
shipped behavior. Rust code MUST remain stable-Rust compatible. TypeScript or
JavaScript Pi package code MUST depend on Pi peer packages instead of bundling
duplicate Pi cores. Tests MUST cover the behavior that enforces player-mode
tool restrictions, save authority, package trust, and terminal rendering.

Rationale: a game framework is only usable when authors can rely on stable file
formats, commands, tools, and UI contracts across saves and package updates.

## Architecture Constraints

- Pi is the default runtime substrate. Use Pi's extension API, package loading,
  resource discovery, sessions, context hooks, compaction, provider registry,
  command system, tool registry, active-tool management, message renderers, and
  TUI component library before adding project-owned equivalents.
- GENmicon functionality may start as a project-local Pi package or extension.
  A standalone wrapper command or Rust workspace target is an adapter, not an
  excuse to duplicate Pi's agent harness.
- A separate game runtime may exist for deterministic validation, path
  canonicalization, save commits, lookup, rendering snapshots, and driver
  functions. That runtime MUST expose typed game operations to Pi tools and
  MUST NOT own model streaming, Pi sessions, package installation, or generic
  terminal UI behavior.
- Player mode active tools MUST be restricted to game-safe tools and any
  approved game-scoped helpers. Generic shell, file edit, git, broad MCP,
  hosted service, telemetry, and package-install tools are developer-only or
  out of scope unless a later constitution-compliant spec approves them.
- Local-first is the V1 default. Remote package installation, hosted
  marketplaces, telemetry, Discord/status integrations, remote music services,
  or external branding require explicit maintainer approval and a trust review.
- Game package content is data. It may describe fictional worlds, rules, and
  prompts, but it MUST NOT override system policy, grant tools, alter sandbox
  behavior, or execute as code unless it is reviewed extension source.

## Development Workflow

- Every feature plan MUST identify the Pi primitive being reused. If none is
  reused, the plan MUST include a Pi-substrate exception with rationale,
  compatibility impact, and removal criteria.
- Feature specs MUST separate player, author, and developer scenarios. Player
  scenarios MUST describe what implementation details remain hidden.
- Plans and tasks MUST include trust-boundary checks for package sources,
  active tools, save writes, path access, and deterministic driver execution.
- Implementation MUST proceed in small vertical slices: package/resource
  declaration, game runtime operation, Pi tool or command, player/developer UI,
  tests, and docs.
- Documentation MUST distinguish shipped behavior from planned behavior. New
  commands, tools, package keys, config, manifests, and file formats are
  shipped only when code, tests, and docs agree.
- Verification MUST include targeted tests for the changed contract and a
  manual or automated terminal-render check when player-facing UI changes.

## Governance

This constitution supersedes conflicting guidance in specs, docs, templates,
and implementation plans. When project docs disagree, amend the affected docs
or mark the conflict before implementation continues.

Amendments require:

- a concrete change to this constitution,
- a Sync Impact Report entry describing affected principles and templates,
- updates to dependent Spec Kit templates and runtime guidance,
- semantic-version bump rationale,
- maintainer review before broad implementation work depends on the change.

Versioning policy:

- MAJOR: removes, renames, or materially redefines a core principle or trust
  boundary.
- MINOR: adds a principle, required section, or materially expands governance.
- PATCH: clarifies wording without changing compliance requirements.

Compliance review is required at plan creation, after design, before tasks are
accepted, and before shipped behavior is documented as complete. Any exception
must be recorded in the plan's Complexity Tracking table with the rejected Pi
primitive or simpler game-console alternative.

**Version**: 1.0.0 | **Ratified**: 2026-05-25 | **Last Amended**: 2026-05-25
