# Research: Pi-Native GENmicon Game Driver

## Decision: Use a Pi Package as the Integration Boundary

The GENmicon driver will ship first as `packages/genmicon-pi`, a reviewed local
Pi package that declares extensions, skills, prompts, and themes in
`package.json`.

**Rationale**: Pi packages are the established way to bundle and share
extensions, skills, prompt templates, and themes. They support local paths,
project settings, npm, git, conventional directories, and resource filters.
This matches GENmicon's need to combine commands, tools, UI, prompts, and
game-driver guidance without forking Pi.

**Alternatives considered**:

- Standalone Rust TUI: rejected because it recreates a parallel agent and TUI
  harness.
- Copying Pi concepts into `crates/kernel`: rejected because the constitution
  requires building on Pi, not beside it.
- Loose `.pi/extensions` files only: useful for experiments, but packages give
  versioning, filtering, and distribution structure.

## Decision: Keep `crates/game` as Deterministic Authority

The existing `deepseek-game` crate remains the source of deterministic game
authority and gains a JSON helper command for Pi tools.

**Rationale**: The crate already owns manifest loading, driver resolution, save
loading, lookup, rendering, driver script execution, atomic commits, and
fixture behavior. Keeping it limits risk and makes save authority testable.

**Alternatives considered**:

- Port game runtime logic to TypeScript immediately: rejected for V1 because it
  risks regressions in path canonicalization, merge patch commits, driver
  function validation, and fixture saves.
- Let the model manage state through Pi sessions: rejected because transcripts
  and compaction summaries are derived context, not game truth.

## Decision: Add a JSON Runtime Helper for Pi Tool Adapters

Pi tools will call a local helper, `genmicon-game-runtime`, with JSON input and
JSON output for validation, status, render, playbook, lookup, fact check,
driver calls, and turn commits.

**Rationale**: A CLI-style JSON boundary is simple to test, language-neutral,
and keeps the Pi extension package small. It avoids linking TypeScript directly
to Rust internals while still using the existing runtime.

**Alternatives considered**:

- FFI/native Node module: rejected as too much packaging complexity for V1.
- HTTP server: rejected because local gameplay does not need a background
  service or network surface.
- Direct file writes from TypeScript: rejected because it bypasses save
  authority.

## Decision: Enforce Player Mode With Pi Active Tools

The package will set and verify a player capability profile using Pi active
tool controls. Generic shell, file editing, git, package install, provider
configuration, and broad external integrations stay developer-only or out of
scope.

**Rationale**: Pi already owns tool registration and active-tool selection.
GENmicon should constrain those capabilities through Pi rather than adding a
parallel policy engine.

**Alternatives considered**:

- Prompt-only restrictions: rejected because untrusted game content and model
  output can ignore prompt policy.
- Separate player process: rejected because it duplicates Pi session/runtime
  ownership.

## Decision: Use Pi Custom UI and Renderers for Player/Developer Views

The package will provide game-facing message/tool renderers plus a custom UI
surface for the player console and a separate diagnostics view.

**Rationale**: Pi supports custom message renderers, tool result renderers,
widgets, overlays, custom editor behavior, and TUI components. This is the
right place to hide tool plumbing in player mode while preserving developer
inspection.

**Alternatives considered**:

- Keep the current ratatui `GameConsoleWidget` as the long-term UI: useful as a
  behavior reference, but it would keep GENmicon tied to the inherited TUI.
- Plain transcript rendering only: rejected because the first visible
  experience must be the game console.

## Decision: Project-Local Reviewed Package for V1

V1 uses a project-local package entry in `.pi/settings.json` that points to
`./packages/genmicon-pi` and filters loaded resources explicitly.

**Rationale**: Project-local loading supports fast iteration, reproducible
fixtures, and trust review before any remote package distribution. Pi package
filtering lets the project load only the resources intended for player mode.

**Alternatives considered**:

- Global install in `~/.pi/agent/settings.json`: rejected for V1 because it is
  harder to reproduce in the repo.
- Remote npm/git package: deferred until the local package and trust policy are
  tested.
