# Modes and Approvals

codewhale has two related axes:

- **TUI mode** (Tab): the visible interaction posture — **Act / Plan / Operate** only.
- **Permission posture** (Shift+Tab): how aggressively the UI asks before executing tools — Ask → Auto-Review → Full Access.
- **Workflow overlay**: optional long-running orchestration that can run on top of any TUI mode when a task needs coordinated workers.

Model selection is separate. `--model auto` and `/model auto` route each turn to
a concrete model and thinking level; they are not TUI modes and are not part of
the `Tab` cycle. Press `Ctrl+T` to cycle **reasoning effort** (not mode).

Workflow is also separate from the `Tab` mode cycle. It is the visible
continuous-work layer for repeatable workflows and Fleet workers. High fan-out
routes through durable Fleet-backed workers instead of prompt-only sub-agent
fanout. The active mode still controls permissions; Workflow controls whether a
large task is planned into a resumable workflow with its own progress view.

## TUI Modes

Press `Tab` to complete composer menus, queue a draft as a next-turn follow-up
while a turn is running, or cycle through the visible modes when the composer is
otherwise idle: **Plan → Act → Operate → Plan**.

Press `Shift+Tab` to cycle permission posture (Ask → Auto-Review → Full Access).

Run `/mode` to open the mode picker, or switch directly with `/mode act`,
`/mode plan`, `/mode operate`, `/mode 1`, `/mode 2`, or `/mode 3`.

| Mode | Number | Role |
|------|--------|------|
| **Act** | `1` | Solo tool work in the current session — edits and shell under the active permission posture |
| **Plan** | `2` | Read-only design and research — investigation tools stay available; shell and patch execution stay off |
| **Operate** | `3` | Fleet + workflow conductor posture — prefer rostered workers and `/workflow` over solo tool spam |

**Act** is also accepted as the historical **Agent** name. Saved settings still
normalize to `agent` for backward compatibility; the UI label is **Act**.

### What each mode means

- **Plan**: design-first prompting. Read-only investigation tools stay available;
  shell and patch execution stay off. Use this when you want to think out loud
  and produce a plan to hand to a human (yourself later, or a reviewer).
- **Act**: multi-step tool use in the current session. In interactive TUI
  sessions, shell tools (`exec_shell`, `task_shell_start`, `task_shell_wait`) are
  available by default and approval prompts gate each call according to
  permission posture. Set top-level `allow_shell = false` to hide shell tools for
  a workspace/profile. File writes are allowed without a prompt when permissions
  allow.
- **Operate**: conductor posture — prefer Fleet roster + `/workflow` orchestration
  over solo inline tool chains; delegate by default. Parallel launch floors are
  higher so background fan-out is not throttled to a single slot.

### Permissions are not a mode

`Shift+Tab` cycles **Ask → Auto-Review → Full Access**. That axis is independent
of Act / Plan / Operate. Full Access means tools can run without approval prompts
while still sitting in **Act** (or another visible mode).

Legacy inputs such as `/mode yolo` or numeric `4` remain supported as an
**invisible permission shim** (Act + Full Access). They are not shown in the
mode picker, Tab cycle, footer mode chip, or docs as a taught mode. Prefer
`Shift+Tab` to Full Access in trusted repos.

Unreleased **Multitask** never shipped as a public mode. Leftover config values
`multitask` / `multi` / `5` load as **Operate** and are not offered in the picker.

### Tool availability by mode

| Tool family | Plan | Act | Operate |
|:---|:---:|:---:|:---:|
| Read-only file, search, and diagnostic tools | yes | yes | yes |
| File write and patch tools | no | yes | yes |
| Shell tools (`exec_shell`, `task_shell_start`, waits, interact, cancel) | no | approval-gated by default, hidden when `allow_shell = false` | approval-gated by default (delegation preferred) |
| Paid or external-service tools | approval-gated | approval-gated | approval-gated |
| Access outside the workspace root | no | only with trust mode | only with trust mode |

If a shell tool is missing from the model-visible catalog in Act mode, check
for an explicit `allow_shell = false` in the active config/profile or runtime
session. Durable tasks and automation keep conservative omitted-field defaults;
they only receive shell access when their task settings explicitly grant it.
`allow_shell = true` controls shell availability only; direct multiline
`exec_shell` commands remain blocked by shell safety validation. For heredocs,
embedded scripts, or long manual flows, use single-line commands, write a
script/file first, or run through `task_shell_start`/background shell.

All action-capable modes have access to persistent RLM sessions through `rlm_open`,
`rlm_eval`, `rlm_configure`, and `rlm_close`. Inside an RLM Python REPL,
`sub_query_batch` fans out 1-16 cheap parallel child calls pinned to
`deepseek-v4-flash`. The model reaches for it when work is too large or
repetitive for the parent transcript.

The fast `deepseek-v4-flash` / thinking-off path is called Fin in the product
language. Fin is a seam for routing, summaries, cheap child calls, and
coordination work; it does not change approval behavior.

`/goal` sets a session objective with an optional token budget and keeps active
objectives visible as Work context. `/goal pause` stops goal continuation without
changing the objective, `/goal resume` resumes and sends the objective back into
the turn, `/goal complete` marks it done, `/goal blocked` marks it blocked, and
`/goal clear` removes it. Goal state does not change the active TUI mode,
permission posture, or model route. This remains distinct from `--model auto`,
which only controls model and thinking selection.

Workflow builds on the same separation: a goal can ask the agent to keep
working, while Workflow supplies the repeatable workflow/progress surface for
large fanout. In the UI, a Workflow run should be shown as an overlay on the
main screen, not as a fourth mode next to Act, Plan, and Operate.

App-server clients can persist a thread-scoped goal with `thread/goal/set`, read
it with `thread/goal/get`, and clear it with `thread/goal/clear`. That persisted
record carries `active`, `paused`, `blocked`, `usage_limited`, `budget_limited`,
or `complete` status plus token/time accounting fields for clients that need
thread resume semantics.

## Compatibility Notes

- Older settings files with `default_mode = "normal"` still load as `agent`; saving rewrites the normalized value.
- `default_mode = "yolo"` / CLI `--yolo` / `/mode yolo` / `4` map to Act with Full Access permissions (not a taught mode).
- `default_mode = "multitask"` / `multi` / `5` load as Operate (never a visible mode).
- Internal label `agent` is the durable setting key for **Act**.

## Escape Key Behavior

`Esc` is a cancel stack, not a mode switch.

- Close slash menus or transient UI first.
- Cancel the active request if a turn is running.
- Discard a queued draft if the composer is empty.
- Clear the current input if text is present.
- Otherwise it is a no-op.

## Approval Mode

You can override approval behavior at runtime:

```text
/config
# edit the approval_mode row to: suggest | auto | never
```

Legacy note: `/set approval_mode ...` was retired in favor of `/config`.

- `suggest` (default): uses the per-mode rules above.
- `auto`: auto-approves all tools (similar to Full Access permission behavior, without changing TUI mode).
- `never`: blocks any tool that isn't considered safe/read-only.

Interactive permission cycling via **Shift+Tab** is the preferred user path:
Ask → Auto-Review → Full Access.

## Small-Screen Status Behavior

When terminal height is constrained, the status area compacts first so header/chat/composer/footer remain visible:

- Loading and queued status rows are budgeted by available height.
- Queued previews collapse to compact summaries when full previews do not fit.
- `/queue` workflows remain available; compact status only affects rendering density.

## Workspace Boundary and Trust Mode

By default, file tools are restricted to the `--workspace` directory. Enable trust mode to allow file access outside the workspace:

```text
/trust
```

Full Access permission posture enables trust mode as part of its bypass policy.

## MCP Behavior

MCP tools are exposed as `mcp_<server>_<tool>` and use the same approval flow as built-in tools. Read-only MCP helpers may auto-run in suggestive approval modes; MCP tools with possible side effects require approval.

See `MCP.md`.

## Related CLI Flags

Run `codewhale --help` for the canonical list. Common flags:

- `-p, --prompt <TEXT>`: one-shot prompt mode (prints and exits)
- `codewhale exec --auto --output-format stream-json <PROMPT>`: run the tool-backed non-interactive agent and emit one JSON object per line for harnesses and backend wrappers
- `codewhale exec --resume <ID|PREFIX> <PROMPT>` / `--session-id <ID|PREFIX>`: continue a saved session non-interactively
- `codewhale exec --continue <PROMPT>`: continue the most recent saved session for this workspace non-interactively
- `codewhale fork <ID|PREFIX>` / `codewhale fork --last`: copy a saved session into a new sibling session; forked sessions retain additive parent-session metadata and show that lineage in session listings
- `--model <MODEL>`: when using the `codewhale` facade, forward a DeepSeek model override to the TUI
- `--workspace <DIR>`: workspace root for file tools
- `--yolo`: start with Act + Full Access (silent permission shim; not a taught mode)
- `-r, --resume <ID|PREFIX|latest>`: resume a saved session
- `-c, --continue`: resume the most recent session in this workspace
- `--max-subagents <N>`: clamp to `1..=128`
- `--mouse-capture` / `--no-mouse-capture`: opt in or out of internal mouse scrolling, transcript selection, right-click context actions, and transcript scrollbar dragging. Mouse capture is enabled by default on non-Windows terminals and on Windows Terminal/ConEmu/Cmder so drag selection copies only transcript text, removes visual wrap-column line breaks from paragraphs, and stays scoped to the transcript pane; hold Shift while dragging or use `--no-mouse-capture` for raw terminal selection. It defaults off on legacy Windows console (CMD without `WT_SESSION` / `ConEmuPID`) and inside JetBrains JediTerm — PyCharm/IDEA/CLion/etc. — where the terminal advertises mouse support but forwards SGR mouse events as raw text (#878, #898). Use `--mouse-capture` to opt in anywhere it's defaulted off. Raw terminal selection may cross the right sidebar and include visual wraps because the terminal, not the TUI, owns the selection.
- `--profile <NAME>`: select config profile
- `--config <PATH>`: config file path
- `-v, --verbose`: verbose logging

## Branching and Rollback

CodeWhale has three related but intentionally separate recovery paths:

- `codewhale fork <ID>` creates a new saved session from an existing saved
  conversation and records the source session id. This is the safe way to
  explore a different answer path without overwriting the original session.
- Esc-Esc backtrack rewinds the live transcript to a previous user prompt and
  restores that prompt into the composer for editing.
- `/restore` and the `revert_turn` tool restore workspace files from side-git
  snapshots. `/restore list [N]` lists more snapshot options before choosing a
  rollback point. They do not rewrite conversation history.

A Pi-style in-file tree browser is a larger UI/data-model project. v0.8.40
ships the bounded fork/backtrack primitives and explicit lineage metadata.
