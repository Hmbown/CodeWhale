# Capacity Controller

`codewhale-tui` includes an opt-in capacity-aware context controller. In the
default V4 path it is disabled, because its active interventions can rewrite
the live prompt and break prefix-cache affinity. Treat it as telemetry or an
experimental guardrail unless `capacity.enabled = true` is set explicitly.

## Policy Overview

Each checkpoint computes:

- `H_hat` (runtime pressure proxy)
- `C_hat` (model capacity prior)
- `slack = C_hat - H_hat`
- dynamic slack profile over last `N=8` observations

### Runtime Pressure Proxy (`H_hat`)

- `action_complexity_bits = log2(1 + action_count_this_turn)`
- `tool_complexity_bits = log2(1 + tool_calls_recent_window)`
- `ref_complexity_bits = log2(1 + unique_reference_ids_recent_window)`
- `context_pressure_bits = 6.0 * context_used_ratio`

Formula:

`H_hat = 0.35*action_complexity_bits + 0.30*tool_complexity_bits + 0.20*ref_complexity_bits + 0.15*context_pressure_bits`

### Capacity Prior (`C_hat`)

Per-model priors:

- `deepseek_v3_2_chat = 3.9`
- `deepseek_v3_2_reasoner = 4.1`
- `deepseek_v4_pro = 3.5`
- `deepseek_v4_flash = 4.2`
- fallback `3.8` (used for other DeepSeek IDs, including future releases)

### Failure Probability

Using rolling profile fields:

- `final_slack`
- `min_slack`
- `violation_ratio`
- `slack_volatility`
- `slack_drop`

Formula:

`z = -1.65*final_slack -0.85*min_slack +1.35*violation_ratio +0.70*slack_volatility +0.28*slack_drop -0.12`

`p_fail = sigmoid(z)` clamped to `[0,1]`.

Risk bands:

- low: `p_fail <= low_risk_max`
- medium: `p_fail <= medium_risk_max`
- high: otherwise

Action mapping when the controller is explicitly enabled:

- low -> `NoIntervention`
- medium -> `TargetedContextRefresh`
- high + severe dynamics (`min_slack <= severe_min_slack` or `violation_ratio >= severe_violation_ratio`) -> `VerifyAndReplan`
- otherwise high -> `VerifyWithToolReplay`

## Checkpoints

When enabled, the engine evaluates controller policy at:

1. Pre-request checkpoint (before `MessageRequest` assembly).
2. Post-tool checkpoint (after tool result append).
3. Error-escalation checkpoint (tool error streak path).

## Interventions

Interventions are not part of the default v0.7.5 V4 path. The default path is:
append messages, preserve prefix-cache reuse, suggest manual `/compact` near
real model pressure, and use overflow recovery only if the request would exceed
the model input budget.

### `TargetedContextRefresh`

- Runs compaction (`compact_messages_safe`) when possible.
- Falls back to local trim if compaction path fails.
- Persists canonical state.
- Replaces long-tail active context with compact canonical prompt + memory pointer.

### `VerifyWithToolReplay`

- Replays one read-only critical tool call from recent turn context.
- Appends verification note with pass/fail + diff summary.
- On replay conflict/error, marks escalation candidate and disables replay for current turn.

### `VerifyAndReplan`

- Persists canonical snapshot.
- Clears volatile prompt tail while preserving latest user ask and latest verification note.
- Injects canonical replan instruction into system prompt.
- Continues turn loop from compact canonical state.

## Safety Controls

- Max one intervention per turn.
- Cooldowns for refresh and replan.
- Replay budget per turn (`max_replay_per_turn`).
- Fail-open behavior when controller inputs are unavailable.
- Compaction/replay failures are logged; turn continues.

## Memory Store

Path:

- `DEEPSEEK_CAPACITY_MEMORY_DIR` (if set)
- otherwise `~/.codewhale/memory/<session_id>.jsonl`
- fallback: existing `~/.deepseek/memory/<session_id>.jsonl` or workspace-local `.codewhale` / legacy `.deepseek` memory paths when needed

Record fields:

- `id`, `ts`, `turn_index`, `action_trigger`
- `h_hat`, `c_hat`, `slack`, `risk_band`
- `canonical_state`
- `source_message_ids`
- optional `replay_info`

Loader utility supports fetching last `K` snapshots for rehydration.

## Configuration

`[capacity]` keys:

- `enabled` (default `false`)
- `low_risk_max` (default `0.50`)
- `medium_risk_max` (default `0.62`)
- `severe_min_slack` (default `-0.25`)
- `severe_violation_ratio` (default `0.40`)
- `refresh_cooldown_turns` (default `6`)
- `replan_cooldown_turns` (default `5`)
- `max_replay_per_turn` (default `1`)
- `min_turns_before_guardrail` (default `4`)
- `profile_window` (default `8`)
- `deepseek_v3_2_chat_prior` (default `3.9`)
- `deepseek_v3_2_reasoner_prior` (default `4.1`)
- `deepseek_v4_pro_prior` (default `3.5`)
- `deepseek_v4_flash_prior` (default `4.2`)
- `fallback_default_prior` (default `3.8`)

Equivalent environment overrides are available with `DEEPSEEK_CAPACITY_*`.
