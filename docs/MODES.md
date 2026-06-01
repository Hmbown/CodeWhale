# Modes and Approvals

codewhale has two related concepts:

- **TUI mode**: what kind of visible interaction you're in (Plan/Agent/YOLO).
- **Approval mode**: how aggressively the UI asks before executing tools.

Model selection is separate. `--model auto` and `/model auto` route each turn to
a concrete model and thinking level; they are not TUI modes and are not part of
the `Tab` cycle.

Each user turn includes a small `<turn_meta>` block with the current local date
and the concrete model sent to the provider. When `--model auto` is active, the
same block also records that the model was auto-routed.

## TUI Modes

Press `Tab` to complete composer menus, queue a draft as a next-turn follow-up
while a turn is running, or cycle through the visible modes when the composer is
otherwise idle: **Plan → Agent → YOLO → Plan**.
Press `Shift+Tab` to cycle reasoning effort.
Run `/mode` to open the mode picker, or switch directly with `/mode agent`,
`/mode plan`, `/mode yolo`, `/mode 1`, `/mode 2`, or `/mode 3`.

- **Plan**: design-first prompting. Read-only investigation tools stay available; shell and patch execution stay off. Use this when you want to think out loud and produce a plan to hand to a human (yourself later, or a reviewer).
- **Agent**: multi-step tool use. Shell execution (`exec_shell`, `task_shell_start`, `task_shell_wait`) requires `allow_shell = true` in config; approval prompts gate each call. File writes are allowed without a prompt.
- **YOLO**: enables shell + trust mode and auto-approves all tools. Use only in trusted repos.

All action-capable modes have access to persistent RLM sessions through `rlm_open`, `rlm_eval`, `rlm_configure`, and `rlm_close`. Inside an RLM Python REPL, `sub_query_batch` fans out 1-16 cheap parallel child calls pinned to `deepseek-v4-flash`. The model reaches for it when work is too large or repetitive for the parent transcript.

The fast `deepseek-v4-flash` / thinking-off path is called Fin in the product
language. Fin is a seam for routing, summaries, cheap child calls, and
coordination work; it does not change approval behavior.

`/goal` sets a session objective with an optional token budget and keeps that
objective visible as Work context. It does not change the active TUI mode,
approval mode, or model route. This remains distinct from `--model auto`, which
only controls model and thinking selection.

## Compatibility Notes

- Older settings files with `default_mode = "normal"` still load as `agent`; saving rewrites the normalized value.

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
- `auto`: auto-approves all tools (similar to YOLO approval behavior, but without forcing YOLO mode).
- `never`: blocks any tool that isn't considered safe/read-only.

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

YOLO mode enables trust mode automatically.

## MCP Behavior

MCP tools are exposed as `mcp_<server>_<tool>` and use the same approval flow as built-in tools. Read-only MCP helpers may auto-run in suggestive approval modes; MCP tools with possible side effects require approval.

See `MCP.md`.

## Related CLI Flags

Run `codewhale --help` for the canonical list. Common flags:

- `-p, --prompt <TEXT>`: one-shot prompt mode (prints and exits)
- `codewhale exec --auto --output-format stream-json <PROMPT>`: run the tool-backed non-interactive agent and emit one JSON object per line for harnesses and backend wrappers
- `codewhale exec --resume <ID|PREFIX> <PROMPT>` / `--session-id <ID|PREFIX>`: continue a saved session non-interactively
- `codewhale exec --continue <PROMPT>`: continue the most recent saved session for this workspace non-interactively
- `codewhale swebench run --instance-id <ID> --issue-file <PATH>`: run the tool-backed agent on one SWE-bench task and write/update a prediction JSONL row
- `codewhale fork <ID|PREFIX>` / `codewhale fork --last`: copy a saved session into a new sibling session; forked sessions retain additive parent-session metadata and show that lineage in session listings
- `--model <MODEL>`: when using the `codewhale` facade, forward a DeepSeek model override to the TUI
- `--workspace <DIR>`: workspace root for file tools
- `--yolo`: start in YOLO mode
- `-r, --resume <ID|PREFIX|latest>`: resume a saved session
- `-c, --continue`: resume the most recent session in this workspace
- `--max-subagents <N>`: clamp to `1..=20`
- `--mouse-capture` / `--no-mouse-capture`: opt in or out of internal mouse scrolling, transcript selection, right-click context actions, and transcript scrollbar dragging. Mouse capture is enabled by default on non-Windows terminals and on Windows Terminal/ConEmu/Cmder so drag selection copies only transcript text, removes visual wrap-column line breaks from paragraphs, and stays scoped to the transcript pane; hold Shift while dragging or use `--no-mouse-capture` for raw terminal selection. It defaults off on legacy Windows console (CMD without `WT_SESSION` / `ConEmuPID`) and inside JetBrains JediTerm — PyCharm/IDEA/CLion/etc. — where the terminal advertises mouse support but forwards SGR mouse events as raw text (#878, #898). Use `--mouse-capture` to opt in anywhere it's defaulted off. Raw terminal selection may cross the right sidebar and include visual wraps because the terminal, not the TUI, owns the selection.
- `--profile <NAME>`: select config profile
- `--config <PATH>`: config file path
- `-v, --verbose`: verbose logging

## Branching and Rollback

DeepSeek-TUI has three related but intentionally separate recovery paths:

- `codewhale fork <ID>` creates a new saved session from an existing saved
  conversation and records the source session id. This is the safe way to
  explore a different answer path without overwriting the original session.
- Esc-Esc backtrack rewinds the live transcript to a previous user prompt and
  restores that prompt into the composer for editing.
- `/restore` and the `revert_turn` tool restore workspace files from side-git
  snapshots. They do not rewrite conversation history.

A Pi-style in-file tree browser is a larger UI/data-model project. v0.8.40
ships the bounded fork/backtrack primitives and explicit lineage metadata.
