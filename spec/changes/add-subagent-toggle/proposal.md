# Proposal: Add First-Class Sub-Agent Toggle

## Why

Users need an obvious way to turn sub-agents on or off from the TUI. Today the
runtime already has a `subagents` feature flag and a one-run CLI override, but
the native `/config` surface shows `features.subagents` as read-only. Users who
want a single-agent session have to know either the feature-flag key or indirect
limits such as `[subagents] max_depth = 0`.

**Background**:
- Issue #3305 targets a narrow v0.8.63 slice of the broader config editability
  audit in #3303.
- The engine already uses `Feature::Subagents` to decide whether to expose
  sub-agent runtime state and the model-facing `agent` tool.

**Current state**: Users can set `[features] subagents = false` manually or run
`codewhale-tui --disable subagents`, but cannot persistently toggle this from
the TUI.

**Expected state**: Users can inspect, disable, and re-enable sub-agents through
clear TUI config commands and the `/config` view, with deterministic behavior
for existing depth and concurrency settings.

## What Changes

- Treat the existing `[features] subagents` feature flag as the canonical
  first-class on/off switch for sub-agents.
- Add `/config subagents on`, `/config subagents off`, and `/config subagents
  status` commands that update or report `features.subagents`.
- Allow `/config features.subagents true|false [--save]` to update the same
  value.
- Make the `features.subagents` row editable in the native `/config` view while
  keeping unrelated experimental feature rows read-only.
- Keep `[subagents] max_depth = 0` as a recursion-depth control, not a second
  global enable flag.
- Update docs to explain persistent TUI toggles, per-session CLI overrides, and
  the difference between disabling all sub-agents and setting max depth to zero.

## Impact

### Affected Specs
- `spec/changes/add-subagent-toggle/specs/subagent-control/spec-delta.md` -
  adds requirements for TUI sub-agent enable/disable behavior.

### Affected Code
- `crates/tui/src/commands/groups/config/config.rs` - parse and apply
  sub-agent toggle commands, including persistence.
- `crates/tui/src/tui/views/mod.rs` - make only `features.subagents` editable
  in the native config view.
- `crates/tui/src/config_persistence.rs` - add or reuse TOML persistence for
  nested feature booleans.
- `crates/tui/src/core/engine.rs` and tests - verify disabled sub-agents hide
  the `agent` tool on the next turn.
- `docs/CONFIGURATION.md` and possibly `docs/SUBAGENTS.md` - document the
  control surface and precedence.

### User Impact
- Users get a direct TUI path to opt out of sub-agent delegation.
- Existing configs using `[features] subagents = false` continue to work.
- Existing `--disable subagents` one-run behavior continues to work.

### API Changes
- No network API changes.
- Adds slash-command aliases under the existing `/config` command surface.

### Migration Needed
- [ ] Database migration
- [ ] API version bump
- [ ] User communication
- [x] Documentation update

## Timeline Estimate

Small to medium. The runtime gate exists; most work is command parsing,
persistence, view editability, documentation, and regression tests.

## Risks

- Risk: Introducing `[subagents] enabled` would create conflicting controls.
  Mitigation: reuse `[features] subagents` as the single canonical on/off flag.
- Risk: Session-only and persistent settings could be confused.
  Mitigation: status messages must say whether a change is session-only or
  saved, matching existing `/config` copy.
- Risk: Existing experimental feature rows become accidentally editable.
  Mitigation: tests must assert only `features.subagents` changes editability.
