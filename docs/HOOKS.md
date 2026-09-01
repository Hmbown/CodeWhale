# Hooks
> 阅读简体中文版：[zh_hans/HOOKS.md](zh_hans/HOOKS.md)

Hooks run a shell command when the Codewhale **TUI** reaches a lifecycle
point. They are plain processes: they receive context through environment
variables, some receive a JSON payload on stdin, and three of them can steer
what Codewhale does next.

This page is the authoritative reference for what is implemented today.
Configuration syntax that overlaps with the rest of `config.toml` lives in
[CONFIGURATION.md](CONFIGURATION.md); this file is the event-by-event
contract.

## Scope

Hooks are a **TUI runtime feature**. Every firing point lives in the
interactive TUI and in the engine turn loop it drives.

| Surface | Fires hooks |
| --- | --- |
| `codewhale` / `codew` interactive TUI | yes |
| `codewhale exec` (headless one-shot) | no |
| the `codewhale` CLI dispatcher and its subcommands | no |
| app-server / ACP | no |
| the `workflow` tool and sub-agent *internals* | no — but the TUI fires `subagent_spawn` / `subagent_complete` around them |
| public API | there is none |

The `crates/hooks` event-sink crate in this repository is an unrelated
internal mechanism. It shares no configuration, no event names, and no
contract with the hooks described here.

## Quick start

```toml
# ~/.codewhale/config.toml
[hooks]
enabled = true

[[hooks.hooks]]
name = "announce"
event = "session_start"
command = "echo 'Codewhale session started'"
```

Run `/hooks` in the TUI to list what is configured, whether the global switch
is on, and any entry that was rejected at load. Run `/hooks events` for the
event names.

## Configuration

```toml
[hooks]
enabled = true                 # global switch; false suppresses every hook
default_timeout_secs = 30      # see the timeout note below
working_dir = "/path/to/dir"   # default: the session workspace

[[hooks.hooks]]
event = "tool_call_before"     # required; one of the 11 names below
command = "~/.codewhale/hooks/gate.sh"  # required; `sh -c` on Unix, `cmd /C` on Windows
name = "gate"                  # optional label for /hooks and log lines
timeout_secs = 30              # optional, default 30
background = false             # optional; foreground inside the hook worker
continue_on_error = true       # optional, default true
condition = { type = "tool_name", name = "exec_shell" }  # optional
```

`timeout_secs` note, stated as implemented: when `[hooks].default_timeout_secs`
is set it **overrides** every hook's own `timeout_secs`, it does not merely
supply a default for hooks that omit one. Leave it unset if you want per-hook
timeouts to apply. `/hooks list` shows the timeout the runtime will actually
apply, and names the override when one is in force.

`default_timeout_secs = 0` is **rejected at load**. Because the value replaces
every hook's own `timeout_secs`, a zero there would expire every hook in the
config immediately — including a `tool_call_before` gate, which then denies
every matching tool call. The override is ignored, per-hook `timeout_secs`
applies, the hooks themselves still load, and the rejection is reported by
`/hooks list` under *configuration problems*. Per-hook `timeout_secs = 0` is
rejected too, but that only drops the one hook that wrote it.

Hooks run with the workspace (or `working_dir`) as the current directory.

### Timeouts

The timeout applies to **foreground and background hooks alike**. When it
expires:

- the hook's whole process group is killed — Unix process groups, Windows Job
  Objects — so a hook that spawns children does not outlive its budget;
- the child is then reaped, so nothing is normally left detached or zombied;
- a foreground hook's result is `success = false`, `exit_code = None`, empty
  `stdout`/`stderr`, and `error = "Hook timed out after Ns"`;
- a background hook's timeout is logged at `warn` under the `hooks` target.
  Nothing is reported to the caller, because the caller stopped waiting the
  moment it submitted the hook.

**Termination is best-effort, and the bound that is guaranteed is Codewhale's,
not the OS's.** The kill can fail to land — a process wedged in an
uninterruptible state on Unix, a `TerminateJobObject` a protected process
survives on Windows — and no user-space program can promise otherwise. What
Codewhale does guarantee is that it stops waiting: the containment handle is
released (which re-signals the Unix process group and closes the kill-on-close
Windows Job Object) and the reap gets one short bounded window. If the child
still cannot be confirmed dead, that is logged at `warn` and the foreground
result says so — `error = "hook could not be reaped after its timeout"` rather
than the stronger timeout wording. So a timed-out hook never blocks the turn,
but treat "killed" as best-effort rather than absolute.

### Background hooks

`background = true` describes real scheduling, not just a config flag. A
background hook is **submitted, never awaited**:

- it is enqueued without blocking into a fixed 32-entry supervisor queue,
  drained by two persistent workers that apply the timeout above; saturation
  or supervisor loss is a failed submission, and no invocation creates its
  own detached supervisor thread;
- it receives the same environment variables and the same stdin JSON payload
  as the foreground form of that event — the payload contract does not change,
  only the steering does;
- its stdout and stderr are discarded (`Stdio::null()`), so it can never
  return a verdict;
- the `HookResult` the runtime hands its caller is flagged as a background
  submission and carries no exit code. Steering code reads
  `observed_exit_code()`, which is `None` for a background hook, so a
  background hook can never allow, deny, ask, or rewrite anything.

`shell_env` ignores `background` entirely — its stdout *is* the contract, so
it always runs in the foreground. `/hooks list` reports that as a
configuration warning, and does not label the hook `[bg]`.

Observer-only UI events are submitted with non-blocking `try_send` to one
32-entry queue drained by two persistent workers. A configured foreground
observer is still awaited in config order inside a worker, but the terminal
event loop never waits on its process and never creates a thread per event.
Queue saturation or dispatcher loss drops that observer event and produces an
event-specific error toast that survives an agent's ordinary progress-status
update. Steering events retain their gate or transform semantics:
fresh/queued `message_submit` dispatch reports through a bounded result
channel, same-turn steering runs that transform on the blocking worker before
calling the engine steer path, and `tool_call_before` / `shell_env` execute on
the engine or tool worker rather than the terminal event loop.

### The hook process environment

A hook command inherits the environment of the Codewhale process, plus the
`DEEPSEEK_*` variables for its event. Codewhale does not filter that
inheritance, so treat a hook exactly as you would treat any command you type
in the same shell that launched Codewhale: whatever is exported there is
visible to it.

This is *not* true of the command a `shell_env` hook feeds — see
[`shell_env`](#shell_env) for the bounded allowlist that governs a **local**
`exec_shell`, and for what changes when an external sandbox backend is
configured instead (the backend owns its base environment, and your
`shell_env` values are transmitted to it).

### Conditions

| Condition | Matches | Supported on |
| --- | --- | --- |
| `{ type = "always" }` | every invocation (also the default when omitted) | every event |
| `{ type = "tool_name", name = "exec_shell" }` | exact tool name; `*` globs are supported, e.g. `mcp__*` | `tool_call_before`, `tool_call_after`, `shell_env`, `on_error` |
| `{ type = "tool_category", category = "shell" }` | tool category | `tool_call_before`, `tool_call_after`, `shell_env`, `on_error` |
| `{ type = "mode", mode = "plan" }` | the context's mode string, case-insensitive | every event **except** `shell_env` |
| `{ type = "exit_code", code = 1 }` | the exit code the tool actually reported | `tool_call_after`, `on_error` |
| `{ type = "all", conditions = [...] }` | every nested condition | every event |
| `{ type = "any", conditions = [...] }` | at least one nested condition | every event |

Three rules keep conditions from lying:

- **`exit_code` needs a real exit code.** It matches only when the event
  actually observed a process exit code — `tool_call_after`, or `on_error` for
  a tool failure, in both cases for a process-backed tool such as `exec_shell`.
  A tool that reports no exit code never matches an `exit_code` condition; the
  condition is not satisfied by a default, a zero, or a success flag. The value
  is a 64-bit integer, so a Windows crash code such as `3221225477`
  (`0xC0000005`) is matchable.
- **Tool-scoped `on_error` hooks are supported.** `on_error` fires for
  transport and capacity errors *and* for tool failures; the tool-failure
  firing carries the tool name, call id, result, and reported exit code. A
  `tool_name` / `tool_category` / `exit_code` condition on `on_error` is
  therefore a valid configuration. An `on_error` firing with no tool behind it
  simply does not match such a condition — it is skipped at dispatch, not
  rejected at load.
- **Unsupported conditions are rejected at load.** A condition that references
  context its event never carries can never match, and a hook wearing one is
  silently inert — the dangerous form of that is a `deny` gate the operator
  believes is armed. Codewhale drops those hooks at load, logs the reason
  under the `hooks` tracing target, and shows them in `/hooks list` as
  `rejected:`. Nested predicates inside `all` / `any` are checked too. A hook
  with `timeout_secs = 0` or an empty `command` is rejected the same way.
  Rejection is **per entry**: a broken hook never takes another one with it,
  even when the two share a `name` or are both unnamed.

### Project-local hooks

A repository may ship `<workspace>/.codewhale/hooks.toml` using the same shape,
but only its `[[hooks]]` entries are merged — a project file cannot change
`enabled`, `default_timeout_secs`, or `working_dir`, which always come from your
own config. Because hooks are executable configuration, project hooks load
**only** after the workspace is trusted in user-owned config; session
`/trust on` alone does not enable them. Trusted project hooks are appended
after global hooks, so they run last and win `updatedInput` ties. A malformed
trusted project file logs a warning and Codewhale falls back to global hooks
only. Validation runs over the merged set, so a rejected project hook is
reported the same way a rejected global one is.

## The 11 events

| Event | Fires | Steering |
| --- | --- | --- |
| `session_start` | once, after the engine is up and before the first draw | observer |
| `session_end` | once, on graceful shutdown | observer |
| `message_submit` | before a submitted message reaches history or the model | **can replace or block the text** |
| `tool_call_before` | before each tool call executes | **can allow / deny / ask, rewrite input, add context** |
| `tool_call_after` | after each tool result settles, including completions the transcript does not redraw | observer |
| `mode_change` | on every applied Plan/Work/Operate transition (`Act` is a compatibility alias for Work) | observer |
| `on_error` | on transport, capacity, and auth errors, and on tool failures | observer |
| `turn_end` | after a turn completes and post-turn state is updated | observer |
| `subagent_spawn` | when a sub-agent starts | observer |
| `subagent_complete` | when a sub-agent completes, fails, or is cancelled | observer |
| `shell_env` | immediately before each `exec_shell` invocation | **contributes environment variables** |

### What "observer" means, exactly

Observer means Codewhale ignores the hook's **result**: stdout is discarded, a
non-zero exit is logged as a warning, and nothing about the turn, the tool
result, the sub-agent, or the error changes because of it.

Observer does **not** mean side-effect-free. An observer hook is an arbitrary
shell command running with your credentials. It can write files, push commits,
page an on-call rotation, or delete the workspace. The only thing it cannot do
is change what Codewhale itself does next.

The steering allowlist is exactly three events — `message_submit`,
`tool_call_before`, `shell_env` — and it is asserted by a test over every
variant, so a new event defaults to observer.

### Session identity

Every event in one TUI session carries the same `DEEPSEEK_SESSION_ID`. The id
is minted once at launch, in the form `sess_xxxxxxxx`, and it survives a
workspace switch and a trust decision that adds project hooks — both reload
the hook set without starting a new session. Engine-fired `tool_call_before`
reports the same id as the UI-fired events, so tool records correlate with the
session records around them.

`session_end` fires after the queued startup-default writes have been drained
and while the app is still live, so it observes the settled end state rather
than a half-torn-down one.

## Environment variables

Every hook receives the subset of these that applies to its event. The
`DEEPSEEK_` prefix is retained for compatibility with hooks written before the
rebrand.

| Variable | Set for | Notes |
| --- | --- | --- |
| `DEEPSEEK_SESSION_ID` | every event except `shell_env` | `sess_xxxxxxxx`, stable for the whole session |
| `DEEPSEEK_WORKSPACE` | every event except `shell_env` | absolute workspace path |
| `DEEPSEEK_MODEL` | every event except `shell_env` | active model id |
| `DEEPSEEK_MODE` | every event except `shell_env` | see the mode-spelling note below |
| `DEEPSEEK_TOTAL_TOKENS` | UI-fired events | session token total at fire time |
| `DEEPSEEK_MESSAGE` | `message_submit`, `subagent_*` | truncated at 5 000 bytes with a `...[truncated]` marker |
| `DEEPSEEK_ERROR` | `on_error` | error message, truncated at 5 000 bytes |
| `DEEPSEEK_PREVIOUS_MODE` | `mode_change` | mode label before the change |
| `DEEPSEEK_TOOL_NAME` | `tool_call_before`, `tool_call_after`, `shell_env`, `on_error` (tool failures) | |
| `DEEPSEEK_TOOL_CALL_ID` | `tool_call_before`, `tool_call_after`, `on_error` (tool failures) | engine call id; correlates before/after/error for one call |
| `DEEPSEEK_TOOL_ARGS` | `tool_call_before`, `shell_env` | tool input JSON preview, capped at 10 000 bytes |
| `DEEPSEEK_TOOL_RESULT` | `tool_call_after`, `on_error` (tool failures) | truncated at 10 000 bytes |
| `DEEPSEEK_TOOL_SUCCESS` | `tool_call_after`, `on_error` (tool failures) | `true` / `false` |
| `DEEPSEEK_TOOL_EXIT_CODE` | `tool_call_after` and `on_error` **when the tool reported one** | absent otherwise — never synthesized; 64-bit, so Windows crash codes such as `3221225477` survive |
| `DEEPSEEK_SESSION_COST` | when cost is supplied | USD, six decimal places |

**Mode-spelling note.** UI-fired events (`session_start`, `session_end`,
`message_submit`, `tool_call_after`, `mode_change`, `on_error`, `turn_end`,
`subagent_*`) set `DEEPSEEK_MODE` to the UI label — `ACT`, `PLAN`, `OPERATE`.
`tool_call_before` fires inside the engine and uses the engine's own mode
spelling (`Agent`, `Plan`, `Operate`). `mode` conditions compare
case-insensitively, so `{ type = "mode", mode = "plan" }` matches both, but a
hook that string-matches `$DEEPSEEK_MODE` exactly should accept both spellings.

**`shell_env` is the narrow one.** It receives only `DEEPSEEK_TOOL_NAME` and
`DEEPSEEK_TOOL_ARGS` — no session id, workspace, model, or mode. A
`{ type = "mode", … }` condition on a `shell_env` hook is therefore rejected at
load; scope those with `tool_name` or `tool_category` instead.

## Steering events

### `message_submit`

Receives JSON on stdin and may rewrite or block the submitted text.

```json
{
  "event": "message_submit",
  "text": "original user text",
  "text_bytes": 18,
  "text_original_bytes": 18,
  "text_truncated": false,
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "ACT",
  "model": "deepseek-chat",
  "total_tokens": 1234
}
```

The complete serialized stdin document is capped at 32 KiB. `text` is the
largest deterministic UTF-8 prefix that fits after JSON escaping and bounded
metadata are included. `text_original_bytes` records the producer's full byte
length, `text_bytes` records the retained prefix, and `text_truncated` states
whether they differ. This same boundary applies to immediate input, restored
queue entries, merged steers, and text produced by an earlier hook.

- exit `0` printing `{"text": "..."}` with a non-empty string replaces the text
- exit `0` with empty stdout, or JSON without `text`, leaves the text unchanged
- `{"text": ""}` or a replacement over 32 000 characters is invalid stdout,
  logged and ignored
- exit `2` blocks the submission before history or dispatch; a structured
  `reason` field supplies a bounded, redacted message shown in the TUI.
  Unstructured stdout/stderr/error output is never copied into the denial
- other non-zero exits follow `continue_on_error`: `true` warns and continues,
  `false` blocks the submission
- `background = true` makes the hook observer-only — it still receives this
  bounded payload on stdin, but it cannot transform or block

Multiple `message_submit` hooks run in config order and each sees the previous
hook's output.

### `tool_call_before`

Receives the tool context in environment variables and may print a JSON
decision on stdout with exit `0`:

```json
{
  "decision": "allow",
  "reason": "human-readable explanation, used for deny",
  "updatedInput": { "command": "ls -la" },
  "additionalContext": "text appended to the tool result for the model"
}
```

- `deny` blocks the tool; the model gets a permission-denied result carrying
  `reason`
- `ask` forces the interactive approval prompt in Ask and Auto-Review. Full
  Access does not open tool-approval prompts, so `ask` does not downgrade it
- `updatedInput` must be an object no larger than 32 KiB serialized and
  replaces the tool input; last hook wins
- `additionalContext` is appended to the tool result as `[hook context] ...`;
  multiple hooks concatenate
- `reason` and `additionalContext` are bounded and sanitized before use: each
  field is capped at 2 000 characters, the concatenated context for one tool
  call is capped at 8 000, control characters are stripped (so hook stdout
  cannot repaint the TUI or forge structure in the transcript), and a clipped
  value carries a `…[truncated]` marker. What a hook adds to the turn's context
  budget is therefore bounded no matter what it prints
- exit `2` is a legacy hard deny and wins regardless of stdout
- empty stdout, non-JSON stdout, and JSON without `decision` all mean allow
- precedence across matching hooks: no-verdict-with-`continue_on_error = false`
  > deny > ask > allow
- `background = true` hooks are submitted and never awaited, so they have no
  verdict and cannot steer; Codewhale logs a warning when one is configured
  for this event

**A gate that could not answer is not permission.** If a foreground
`tool_call_before` hook produces no verdict — it timed out, the process could
not be started, or a strict process exited non-zero without an explicit JSON
decision — and *that hook* is configured with
`continue_on_error = false`, the tool call is denied. Strictness is read off
the hook that actually ran, not off the event: a strict `write_file` gate whose
condition did not match an `exec_shell` call has no say in whether that call
proceeds, and a lenient hook's timeout never denies just because some other
strict hook exists in config. Every no-verdict outcome is logged either way.

The denial message names the hook and the reason and nothing else: the hook
name is truncated, the detail is truncated, control characters are stripped,
and a spawn failure is reported by error kind (`NotFound`,
`PermissionDenied`, …) rather than by echoing the command line or the resolved
interpreter path.

### `shell_env`

Runs synchronously before each `exec_shell` and its stdout is parsed as
`KEY=VALUE` lines. A leading `export ` is stripped, `#` comment lines and blank
lines are skipped, and a matching pair of surrounding single or double quotes is
removed from the value. Later hooks override earlier ones. Use it for ephemeral
credentials, per-skill `PATH` adjustments, or short-lived tokens.

`background` is ignored for this event: the hook always runs in the foreground
because its stdout is the contract.

An entry a shell cannot carry is dropped rather than allowed to break the tool
call: an empty name, a name containing whitespace, `=`, a control character, or
a NUL; a value containing a NUL; a value over 32 KiB; and anything past 256 KiB
of accumulated output from one hook. Each drop is logged by key name only. A
`shell_env` hook is an ordinary process whose stdout can contain anything —
"the hook printed something odd" must never become "the `exec_shell` call
aborted".

**Exactly what the shell command ends up with — local execution.** When
`exec_shell` runs the command locally (the default), it does not inherit
Codewhale's ambient environment. Its environment is built as:

1. a sanitized fixed allowlist of parent variables — `PATH`, `HOME`, `USER`,
   `LANG` and the other `LC_*`/locale entries, `TERM`, `SHELL`, `TMPDIR`,
   proxy variables, color/terminal entries such as `NO_COLOR`, `CARGO_HOME`/`RUSTUP_HOME`/`RUSTUP_TOOLCHAIN`, the Windows system and MSVC toolchain entries, and other platform keys (the full list lives in `crates/tui/src/child_env.rs`) — and nothing else. Variables
   outside that allowlist, including anything that looks like a secret, are
   dropped;
2. then the `KEY=VALUE` pairs your `shell_env` hooks produced, applied on top.
   These are explicit values you configured, so they win over the allowlist.

So a `shell_env` hook is the supported way to get a credential into one local
`exec_shell` invocation. Ambient secrets exported in the terminal that launched
Codewhale are **not** forwarded to a local `exec_shell` on their own.

**With an external sandbox backend configured, the allowlist above is not the
contract.** If `exec_shell` is routed to a configured sandbox/execution
backend, Codewhale does not construct the process environment at all: it hands
the command and your `shell_env` values to the backend as extra environment
variables, and the **backend owns its own base environment**. What is present
besides your values — an image's baked-in variables, the backend's own
injections, whatever a remote runner exports — is determined by that backend,
not by the list above. Do not assume the local allowlist applies there.

Disclosure, because it is the part that matters for a hook that emits
credentials: **`shell_env` values are transmitted to the configured backend.**
For a remote or containerized backend that means the values leave this machine
and are subject to that backend's logging, retention, and access controls.
Codewhale's own audit log still records key names only, but that says nothing
about what the backend does with the values. If a `shell_env` hook emits a
secret, scope it to a backend you trust with that secret — for example by
conditioning the hook, or by not configuring an external backend for sessions
where those hooks are active.

Resolved **key names — never values** — are written to `~/.codewhale/audit.log`
so a session can be reconciled afterwards. A hook that fails or times out
contributes no variables and does not abort the shell call.

```toml
[[hooks.hooks]]
name = "aws-creds"
event = "shell_env"
command = "aws-vault export my-profile --format=env"
condition = { type = "tool_category", category = "shell" }
```

## Structured observer payloads

`turn_end`, `subagent_spawn`, and `subagent_complete` receive JSON on stdin in
addition to the environment variables. Their stdout is ignored. Background
forms of these events receive the same payload on stdin.

The remaining observer events — `session_start`, `session_end`,
`tool_call_after`, `mode_change`, `on_error` — receive environment variables
only, with no stdin payload, in both foreground and background form.

### `turn_end`

Fires after post-turn state, usage totals, cost accounting, notifications,
receipts, and queue recovery have been updated, and before queued follow-up
dispatch — so the payload can report the queued count without a hook being able
to change what is sent next.

```json
{
  "event": "turn_end",
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "ACT",
  "created_at": "2026-07-12T10:30:00+00:00",
  "model_backed": true,
  "provider": "deepseek",
  "billing_surface": null,
  "model": "deepseek-chat",
  "turn_id": "turn_12345678",
  "status": "completed",
  "error": null,
  "duration_ms": 1834,
  "usage": {
    "input_tokens": 1200,
    "output_tokens": 180,
    "prompt_cache_hit_tokens": 900,
    "prompt_cache_miss_tokens": 300,
    "prompt_cache_write_tokens": 0,
    "reasoning_tokens": null,
    "reasoning_replay_tokens": null
  },
  "totals": {
    "session_tokens": 1380,
    "conversation_tokens": 1380,
    "input_tokens": 1200,
    "output_tokens": 180
  },
  "tool_count": 2,
  "queued_message_count": 1,
  "stop_hook_active": false
}
```

`created_at` anchors time-window pricing. `provider` and `model` identify the
effective route for model-backed turns. `billing_surface` is an optional,
non-secret classification of the endpoint that served the turn (recognized
StepFun routes emit `stepfun-payg` or `stepfun-plan`); the raw base URL is never
written to hook records. Shell-only, manual-compaction, and purge completions
have no matching `TurnStarted`, so they report `model_backed: false`, a `null`
provider, and a synthetic `lifecycle_<uuid>` turn id. `stop_hook_active` is
always `false` today; it reserves room for re-entry protection.

### `subagent_spawn` / `subagent_complete`

```json
{
  "event": "subagent_complete",
  "agent_id": "agent_1",
  "session_id": "sess_12345678",
  "workspace": "/path/to/workspace",
  "mode": "ACT",
  "model": "deepseek-chat",
  "total_tokens": 1234,
  "result_preview": "bounded preview of the result",
  "result_truncated": false,
  "status": "completed"
}
```

`subagent_spawn` carries `prompt_preview` / `prompt_truncated` instead, and no
`status`. Both payloads are bounded on purpose: previews are truncated rather
than shipping full prompts or results. These hooks are observer-only — failures
do not affect sub-agent scheduling, prompts, or results, and `continue_on_error`
has no effect because later matching hooks always run.

## Failure behavior

- A non-zero exit is logged at `warn` under the `hooks` tracing target with the
  hook name, event, exit code, duration, and a generic failure category. Raw
  stdout/stderr/error text is not persisted in the log receipt.
- For `execute`-path events, `continue_on_error = false` stops later hooks for
  that event; except on `tool_call_before` (above) it does not roll back the
  action that fired them.
- Structured observer events (`turn_end`, `subagent_*`) always continue to the
  next matching hook.
- Observer events use a bounded persistent dispatcher. Queue-full and
  dispatcher-unavailable submissions are not retried silently; the TUI keeps
  an event-specific error toast separate from the ordinary status line.
- A hook that exceeds its timeout has its whole process group killed and is
  then reaped, foreground or background — best-effort, with a bounded reap
  wait; see [Timeouts](#timeouts).

## Security notes

- Hooks are arbitrary shell commands from your own config; treat
  `~/.codewhale/config.toml` as executable.
- Project-supplied hooks require an explicit workspace trust decision in
  user-owned config.
- Hook commands inherit Codewhale's own environment. A local `exec_shell` does
  not — see [`shell_env`](#shell_env).
- `shell_env` audit records contain key names only. That covers Codewhale's own
  logging; with an external sandbox backend configured, the values themselves
  are transmitted to that backend and are then subject to its handling.
- With an external sandbox backend, the local parent-variable allowlist does
  not apply — the backend owns its base environment.
- Payload previews, tool arguments/results, error messages, captured stdout and
  stderr, replacement messages, and steering objects are bounded so hook input
  or output cannot become an unbounded copy of the transcript.
- Nothing Codewhale persists in a denial echoes the stdin payload, hook
  environment, raw stdout/stderr/error, command line, or a resolved filesystem
  path. `/hooks list` shows a sanitized, single-line command preview capped at
  60 characters; it is not a verbatim copy. Structured denial reasons are
  bounded and redact path-, argument-, command-, and secret-like tokens,
  including quoted or `key=value` forms and `Authorization: Bearer …`.
