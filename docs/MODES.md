# Modes and Approvals

DeepSeek TUI has two related concepts:

- **TUI mode**: what kind of visible interaction you're in (Plan/Agent/YOLO).
- **Approval mode**: how aggressively the UI asks before executing tools.

## TUI Modes

Press `Tab` to complete composer menus, queue a draft as a next-turn follow-up
while a turn is running, or cycle through the visible modes when the composer is
otherwise idle: **Plan → Agent → YOLO → Plan**.
Press `Shift+Tab` to cycle reasoning effort.
Run `/mode` to open the mode picker, or switch directly with `/mode agent`,
`/mode plan`, `/mode yolo`, `/mode 1`, `/mode 2`, or `/mode 3`.

- **Plan**: design-first prompting. Read-only investigation tools stay available; shell and patch execution stay off. Use this when you want to think out loud and produce a plan to hand to a human (yourself later, or a reviewer).
- **Agent**: multi-step tool use. Approvals for shell and paid tools (file writes are allowed without a prompt).
- **YOLO**: enables shell + trust mode and auto-approves all tools. Use only in trusted repos.

All three modes have access to persistent RLM sessions through `rlm_open`, `rlm_eval`, `rlm_configure`, and `rlm_close`. Inside an RLM Python REPL, `sub_query_batch` fans out 1-16 cheap parallel child calls pinned to `deepseek-v4-flash`. The model reaches for it when work is too large or repetitive for the parent transcript.

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

Run `deepseek --help` for the canonical list. Common flags:

- `-p, --prompt <TEXT>`: one-shot prompt mode (prints and exits)
- `deepseek exec --output-format stream-json <PROMPT>`: emit one JSON object per line for harnesses and backend wrappers
- `deepseek exec --resume <ID|PREFIX> <PROMPT>` / `--session-id <ID|PREFIX>`: continue a saved session non-interactively
- `deepseek exec --continue <PROMPT>`: continue the most recent saved session for this workspace non-interactively
- `--model <MODEL>`: when using the `deepseek` facade, forward a DeepSeek model override to the TUI
- `--workspace <DIR>`: workspace root for file tools
- `--yolo`: start in YOLO mode
- `-r, --resume <ID|PREFIX|latest>`: resume a saved session
- `-c, --continue`: resume the most recent session in this workspace
- `--max-subagents <N>`: clamp to `1..=20`
- `--mouse-capture` / `--no-mouse-capture`: opt in or out of internal mouse scrolling, transcript selection, right-click context actions, and transcript scrollbar dragging. Mouse capture is enabled by default on non-Windows terminals and on Windows Terminal/ConEmu/Cmder so drag selection copies only transcript text and stays scoped to the transcript pane; hold Shift while dragging or use `--no-mouse-capture` for raw terminal selection. It defaults off on legacy Windows console (CMD without `WT_SESSION` / `ConEmuPID`) and inside JetBrains JediTerm — PyCharm/IDEA/CLion/etc. — where the terminal advertises mouse support but forwards SGR mouse events as raw text (#878, #898). Use `--mouse-capture` to opt in anywhere it's defaulted off. Raw terminal selection may cross the right sidebar because the terminal, not the TUI, owns the selection.
- `--profile <NAME>`: select config profile
- `--config <PATH>`: config file path
- `-v, --verbose`: verbose logging
