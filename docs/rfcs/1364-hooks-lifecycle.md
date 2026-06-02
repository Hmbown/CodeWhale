# RFC: Hook Lifecycle Data Flow

**Issue:** #1364
**Status:** Phase 1 landed, Phase 2/3 spec
**Date:** 2026-06-02
**Baseline:** `main` at `31f34c5df2363316f23a23398a81cf2d363cd19a`

## 1. Current State

CodeWhale has MCP support and configurable hooks. Phase 1 of #1364 landed via
#2434, which harvested the mutable `message_submit` slice from #2318.

Implemented in Phase 1:

- non-background `message_submit` hooks receive JSON on stdin
- stdout JSON with a non-empty string `text` field replaces submitted text
- exit code `2` blocks submission before a model turn starts
- multiple `message_submit` hooks run serially in config order
- background `message_submit` hooks remain observer-only
- transformed text is used by history, file mention expansion, API messages,
  and engine dispatch
- `continue_on_error = true` surfaces stderr/stdout/internal errors as a
  transient TUI warning instead of silently swallowing continued failures

Remaining #1364 work:

- Phase 2: expose a post-turn `turn_end` lifecycle hook
- Phase 3: expose observer-only subagent lifecycle hooks

Future work should start from current `main`; #2318 is now a reference branch,
not the Phase 1 merge target.

## 2. Shared Design Rules

All remaining hooks should preserve the existing `[[hooks.hooks]]` config
shape, existing env vars, and existing condition matching.

Structured payloads should be written to hook stdin as JSON and should include
the stable shared fields:

- `event`
- `session_id`
- `workspace`
- `mode`
- `model`

Observer lifecycle hooks must not:

- mutate transcript content
- mutate submitted user text
- mutate tool arguments or tool results
- block user input or subagent scheduling
- expose secrets, full tool outputs, or unbounded transcript content

For Phase 2 and Phase 3, stdout is ignored. The stdout JSON mutation contract
remains specific to `message_submit`.

## 3. PR Split

### PR 1: Mutable `message_submit`

Status: landed in #2434.

No further Phase 1 development should be based on #2318 unless the maintainer
explicitly asks for it. Follow-up behavior should build on `main`.

### PR 2: `turn_end`

Expose turn completion as a post-turn observer hook. This is the next focused
slice.

Scope:

- add `HookEvent::TurnEnd` with event name `turn_end`
- add a structured observer hook execution path that accepts a JSON payload
- fire from the `EngineEvent::TurnComplete` UI branch after core user-visible
  state has been updated
- run hooks without blocking the next user action
- treat hook failures as warn/log only
- include `stop_hook_active`, initially `false`, to reserve the re-entry guard
  contract
- document the payload and non-blocking semantics
- update `/hooks events`, `docs/CONFIGURATION.md`, and web docs

Non-goals:

- no blocking or replacement behavior
- no transcript mutation
- no model response mutation
- no tool output or full transcript payload
- no subagent lifecycle hook in this PR

### PR 3: Subagent Lifecycle Observer Hooks

Expose subagent start and completion as observer-only lifecycle hooks.

Scope:

- add `HookEvent::SubagentSpawn` with event name `subagent_spawn`
- add `HookEvent::SubagentComplete` with event name `subagent_complete`
- reuse the structured observer execution path from Phase 2
- fire from the existing `EngineEvent::AgentSpawned` and
  `EngineEvent::AgentComplete` UI branches
- pass bounded subagent metadata on stdin as JSON
- run hooks without blocking subagent scheduling or parent-turn progress
- treat hook failures as warn/log only
- document the payload and observer-only semantics

Non-goals:

- no subagent spawn gating in the first version
- no subagent prompt or result mutation
- no full prompt/result payload by default
- no changes to subagent scheduling
- no new subagent type matcher unless a later review asks for it

## 4. Phase 2 Technical Spec: `turn_end`

### 4.1 Hook Configuration

```toml
[[hooks.hooks]]
event = "turn_end"
command = "~/.codewhale/hooks/turn-end.sh"
timeout_secs = 5
continue_on_error = true
```

The hook should be treated as observer-only even when `background = false`.
The UI must not wait for it before accepting input, dispatching queued
messages, or repainting the idle state.

### 4.2 Trigger Point

The trigger point is `crates/tui/src/tui/ui.rs`, inside the
`EngineEvent::TurnComplete` branch.

The hook should fire after these state updates have happened:

- loading and streaming state cleared
- turn duration recorded
- runtime turn status set
- session token counters updated
- cache telemetry updated
- turn cost accrued
- user-facing notification/receipt state updated
- session snapshot persistence has been scheduled

The hook should fire before any automatically queued message starts a new turn,
so the completed turn is observable even when the queue immediately continues.
The hook itself must be fire-and-forget; it must not delay the queued message.

`turn_end` should fire for all terminal turn outcomes:

- `completed`
- `interrupted`
- `failed`

It should not fire on app startup, session startup, or session shutdown without
a completed engine turn.

### 4.3 Payload

Example payload:

```json
{
  "event": "turn_end",
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "agent",
  "model": "deepseek-chat",
  "turn_id": "turn_123",
  "status": "completed",
  "error": null,
  "duration_ms": 4200,
  "usage": {
    "input_tokens": 1000,
    "output_tokens": 250,
    "prompt_cache_hit_tokens": 128,
    "prompt_cache_miss_tokens": 872,
    "reasoning_replay_tokens": null
  },
  "totals": {
    "session_tokens": 1250,
    "conversation_tokens": 1250,
    "input_tokens": 1000,
    "output_tokens": 250
  },
  "tool_count": 2,
  "queued_message_count": 0,
  "stop_hook_active": false
}
```

Payload rules:

- `status` is one of `completed`, `interrupted`, or `failed`
- `error` is the turn error string only when already visible to the user
- `duration_ms` is wall-clock turn duration from `app.turn_started_at`
- `usage` mirrors `EngineEvent::TurnComplete.usage`
- `totals` are the post-update app/session counters
- `tool_count` is the number of tool evidence entries, not full tool output
- `queued_message_count` is informational and must not change queue behavior
- `stop_hook_active` is always `false` in this PR

Do not include:

- full transcript text
- full tool arguments or outputs
- API keys or provider headers
- raw assistant response text

### 4.4 Implementation Plan

1. Extend `HookEvent` in `crates/tui/src/hooks.rs`.

```rust
TurnEnd,
```

Map it in `HookEvent::as_str()` as `"turn_end"`.

2. Add a structured observer execution helper in `crates/tui/src/hooks.rs`.

Suggested shape:

```rust
pub fn execute_structured_observer(
    &self,
    event: HookEvent,
    context: &HookContext,
    payload: serde_json::Value,
)
```

This helper should:

- filter hooks with `hooks_for_event(event)`
- apply existing condition matching
- pass existing env vars
- write `payload` to stdin using the existing stdin-capable executor path
- ignore stdout
- log failures with `tracing::warn!`
- never return a blocking outcome to the caller

The caller should invoke this helper from a background task/thread so UI event
handling remains non-blocking. If that requires cloning `HookExecutor`, keep the
payload owned and avoid borrowing `App`.

3. Add a `turn_end_payload(...)` builder.

Keep it private to `hooks.rs` if all fields can be passed through
`HookContext`, or place it near the UI call site if it needs direct `App`
access. Prefer a small explicit payload builder over ad hoc JSON construction
inside the event branch.

4. Wire the UI event branch.

In `crates/tui/src/tui/ui.rs`, after the `TurnComplete` branch finishes the
core app-state updates, build the payload and dispatch the observer hook.

5. Update command and docs.

- `crates/tui/src/commands/hooks.rs`
- `docs/CONFIGURATION.md`
- `web/app/[locale]/docs/page.tsx`

### 4.5 Tests

Hook unit tests:

- `HookEvent::TurnEnd` serializes/deserializes as `turn_end`
- configured `turn_end` hooks receive stdin JSON
- stdout is ignored
- non-zero exit logs/warns but does not produce a blocking result
- timeout does not block the caller path

TUI tests:

- completed turn fires one `turn_end` hook
- failed turn fires one `turn_end` hook with `status = "failed"` and `error`
- interrupted turn fires one `turn_end` hook with `status = "interrupted"`
- payload contains post-update token totals
- queued messages still dispatch after `TurnComplete`

Manual smoke test:

1. Configure a `turn_end` hook that appends stdin JSON to a temp file.
2. Run one successful turn.
3. Confirm the file contains `event = "turn_end"` and `status = "completed"`.
4. Configure a slow hook with `timeout_secs = 1`.
5. Confirm the TUI returns to idle and accepts input without waiting.

## 5. Phase 3 Technical Spec: Subagent Lifecycle Hooks

### 5.1 Hook Configuration

```toml
[[hooks.hooks]]
event = "subagent_spawn"
command = "~/.codewhale/hooks/subagent-spawn.sh"
timeout_secs = 3
continue_on_error = true

[[hooks.hooks]]
event = "subagent_complete"
command = "~/.codewhale/hooks/subagent-complete.sh"
timeout_secs = 3
continue_on_error = true
```

Both events are observer-only. Their hooks must not gate spawn, cancel
subagents, or rewrite prompts/results.

### 5.2 Trigger Points

Trigger from `crates/tui/src/tui/ui.rs`:

- `EngineEvent::AgentSpawned { id, prompt }` -> `subagent_spawn`
- `EngineEvent::AgentComplete { id, result }` -> `subagent_complete`

Fire after the existing UI state/status updates for each branch have been
applied. Hook failures must not affect:

- `app.agent_progress`
- `app.status_message`
- `Op::ListSubAgents`
- subagent cards or mailbox routing
- parent turn completion

### 5.3 Payloads

Spawn payload:

```json
{
  "event": "subagent_spawn",
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "agent",
  "model": "deepseek-chat",
  "agent_id": "agent_abc",
  "prompt_preview": "Investigate failing tests",
  "prompt_truncated": false
}
```

Complete payload:

```json
{
  "event": "subagent_complete",
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "agent",
  "model": "deepseek-chat",
  "agent_id": "agent_abc",
  "status": "completed",
  "result_preview": "Found the failing assertion in parser tests",
  "result_truncated": false
}
```

Payload rules:

- use bounded previews, not full prompt/result text
- use the same truncation helper for prompt and result previews
- if agent type or assignment metadata is available without extra blocking
  lookups, include it as optional `agent_type` / `assignment` fields
- do not add blocking lookups only to enrich hook payloads
- do not include full subagent transcript

### 5.4 Implementation Plan

1. Extend `HookEvent` in `crates/tui/src/hooks.rs`.

```rust
SubagentSpawn,
SubagentComplete,
```

Map them as:

- `"subagent_spawn"`
- `"subagent_complete"`

2. Reuse the Phase 2 structured observer helper.

Subagent lifecycle hooks should not create a second observer execution path.
Reuse the `execute_structured_observer` helper and payload writing behavior.

3. Add bounded preview helpers.

Keep prompt/result payloads bounded. Reuse an existing truncation helper if one
is already available in the TUI layer; otherwise add a small private helper
that truncates by chars and returns both preview text and a boolean
`*_truncated` flag.

4. Wire the UI branches.

In the `AgentSpawned` branch, build the spawn payload from `id` and the
existing `prompt_summary`/bounded prompt preview.

In the `AgentComplete` branch, build the complete payload from `id` and a
bounded result preview.

5. Update docs and `/hooks events`.

- `crates/tui/src/commands/hooks.rs`
- `docs/CONFIGURATION.md`
- `web/app/[locale]/docs/page.tsx`

### 5.5 Tests

Hook unit tests:

- `HookEvent::SubagentSpawn` serializes/deserializes as `subagent_spawn`
- `HookEvent::SubagentComplete` serializes/deserializes as
  `subagent_complete`
- subagent observer hooks receive stdin JSON
- stdout is ignored
- non-zero exits do not affect caller state

TUI tests:

- `AgentSpawned` fires one `subagent_spawn` hook
- `AgentComplete` fires one `subagent_complete` hook
- payload previews are truncated and marked when input is long
- hook failure does not prevent `Op::ListSubAgents`
- hook failure does not alter `app.agent_progress`
- hook failure does not alter `app.status_message`

Manual smoke test:

1. Configure both subagent hooks to append stdin JSON to a temp file.
2. Trigger a subagent.
3. Confirm `subagent_spawn` appears before `subagent_complete`.
4. Confirm prompts/results are bounded previews, not full transcripts.

## 6. Contribution Workflow

For Phase 2 and Phase 3:

- create a fresh branch from current `main`
- keep each PR focused on one behavior boundary
- use `Refs #1364 (partial)` unless a maintainer reopens or re-scopes the issue
- do not use `Closes #1364` for either follow-up slice unless the maintainer
  confirms the issue should be fully closed by that PR
- include local validation evidence in the PR body
- keep PRs ready for the direct-merge path: rebased, non-draft, green CI, and
  backed by focused tests

Suggested PR body shape:

```text
Summary:
Scope:
Not in this slice:
Builds on: #2434
Issues: Refs #1364 (partial)
Validation:
```

## 7. Review Checkpoints

Phase 2 is ready for review only if:

- `turn_end` fires after post-turn app state is updated
- all terminal turn statuses are covered by tests
- hook execution is non-blocking from the UI perspective
- payload excludes transcript/tool-output content
- docs specify that stdout is ignored

Phase 3 is ready for review only if:

- subagent hooks are observer-only
- spawn and completion are both covered by tests
- failures do not affect subagent state or scheduling
- payload previews are bounded and tested
- docs clearly state that gating/mutation are out of scope
