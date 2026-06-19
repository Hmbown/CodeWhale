# Spec Delta: Sub-Agent Control

This change adds a first-class TUI control surface for enabling and disabling
sub-agents.

## ADDED Requirements

### Requirement: Persistent Sub-Agent Toggle
WHEN a user changes the sub-agent enabled state from the TUI with persistence,
the system SHALL save the state as the canonical `features.subagents` feature
flag in the active CodeWhale config file.

#### Scenario: Disable sub-agents persistently
GIVEN sub-agents are currently enabled
WHEN the user runs `/config subagents off --save`
THEN the system saves `subagents = false` under `[features]`
AND subsequent turns use the disabled sub-agent state.

#### Scenario: Re-enable sub-agents persistently
GIVEN sub-agents were previously disabled in config
WHEN the user runs `/config subagents on --save`
THEN the system saves `subagents = true` under `[features]`
AND subsequent turns use the enabled sub-agent state.

### Requirement: Session-Only Sub-Agent Toggle
WHEN a user changes the sub-agent enabled state from the TUI without
persistence, the system SHALL apply the state to subsequent turns in the current
session without writing the config file.

#### Scenario: Disable for the current session
GIVEN sub-agents are currently enabled
WHEN the user runs `/config subagents off`
THEN the current session treats `features.subagents` as disabled
AND the status message states that the change is session-only.

#### Scenario: Query current state
GIVEN the current session has an effective sub-agent state
WHEN the user runs `/config subagents status`
THEN the system reports whether sub-agents are enabled or disabled
AND the config file is not modified.

### Requirement: Model-Facing Tool Gating
IF the effective sub-agent feature flag is disabled,
the system SHALL not expose the model-facing `agent` tool for new turns.

#### Scenario: Agent tool hidden
GIVEN `features.subagents` is disabled
WHEN the next Agent or YOLO turn builds its tool registry
THEN the registry does not include the `agent` tool.

#### Scenario: Other tools remain available
GIVEN `features.subagents` is disabled
WHEN the next Agent or YOLO turn builds its tool registry
THEN non-sub-agent tools continue to follow their existing feature and approval
rules.

### Requirement: Deterministic Sub-Agent Control Precedence
WHERE sub-agent feature flags and sub-agent depth or concurrency limits are all
configured, the system SHALL treat `features.subagents` as the global on/off
switch and treat `[subagents]` limits as controls that only matter while the
global switch is enabled.

#### Scenario: Disabled feature wins over depth
GIVEN `features.subagents = false`
AND `[subagents] max_depth = 1`
WHEN a new Agent or YOLO turn builds its tool registry
THEN the `agent` tool is not exposed.

#### Scenario: Depth zero remains distinct
GIVEN `features.subagents = true`
AND `[subagents] max_depth = 0`
WHEN sub-agent spawning is evaluated
THEN the global feature remains enabled
AND spawning is blocked by the depth limit rather than by the global toggle.

### Requirement: Discoverable Config View Toggle
WHEN the native `/config` view displays experimental feature rows,
the system SHALL make `features.subagents` editable and keep unrelated
experimental feature rows read-only.

#### Scenario: Sub-agent row editable
GIVEN the native `/config` view is open
WHEN the user navigates to `features.subagents`
THEN the row is marked editable
AND saving the row updates the canonical sub-agent feature flag.

#### Scenario: Other experimental rows remain read-only
GIVEN the native `/config` view is open
WHEN the user navigates to an unrelated experimental feature row
THEN that row remains read-only unless a separate contract makes it editable.
