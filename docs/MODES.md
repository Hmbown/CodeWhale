# Modes and Permission Postures

codewhale has three related concepts:

- **TUI mode**: what kind of visible interaction you're in (Plan/Act/Operate).
- **Permission posture**: how aggressively the UI asks before executing tools.
- **Workflow overlay**: optional long-running orchestration that can
  run on top of any TUI mode when a task needs many coordinated workers.

Model selection is separate. `--model auto` and `/model auto` route each turn to
a concrete model and thinking level; they are not TUI modes and are not part of
the `Tab` cycle.

Workflow is also separate from the mode itself. It is the visible ordered
orchestration layer for repeatable workflows and Fleet workers. High fan-out
routes through durable Fleet-backed workers instead of prompt-only sub-agent
fanout. The active mode
still controls permissions; Workflow controls whether a large task is planned
into a resumable workflow with its own progress view.

## TUI Modes

Press `Tab` to complete composer menus or cycle through the visible modes
when the composer is empty: **Plan → Act → Operate → Plan**. `Tab` never sends
or queues composer text; use `Enter` to send or queue it.
Press `Shift+Tab` to cycle permission posture (Ask → Auto-Review → Full Access).
Press `Ctrl+T` to cycle reasoning effort.
Run `/mode` to open the mode picker, or switch directly with `/mode act`,
`/mode plan`, or `/mode operate`.

- **Plan**: design-first prompting. Read-only investigation tools stay available; shell and patch execution stay off. Use this when you want to think out loud and produce a plan to hand to a human (yourself later, or a reviewer).
- **Act** (Agent): multi-step tool use. In interactive TUI sessions, the canonical `Bash` tool is available by default and approval prompts gate each call. Set top-level `allow_shell = false` to hide it for a workspace/profile. The canonical `File`, `Git`, and `Run` action tools cover structured workspace work.
- **Operate**: multitask conductor posture. Send ordinary messages and use the same direct tools, shell configuration, sandbox, permission posture, ask-rules, and repository protections as Act. The parent session is the **operator**: dispatching background workers is the **default** way real multi-step or independent work happens (no special multitask command). Handle small or tightly coupled tasks in the parent; for everything else, set a goal when work spans streams, start background `agent` workers early, treat queued follow-ups as new tasks, and keep the parent free for steers and synthesis. **Dispatch is not completion** — every write-capable child must return verification evidence (verifier child, `run_verifiers`, or structured PASS/FAIL with real commands). Prefer direct workers for independent streams; use Workflow when order, phases, gates, shared budgets, or deterministic fan-in matter (starter recipes under `workflows/operate_*.workflow.js`: staged-fix, read-audit, parallel-scout, best-of-n). Best-of-N (skill + starter workflow) runs N worktree implementers then a reviewer; apply the winner only after PASS.

**Act** is accepted as an alias for Agent mode. Saved settings still normalize to `agent` for backward compatibility.

### Tool availability by mode

| Tool family | Plan | Act | Operate |
|:---|:---:|:---:|:---:|
| Read-only file, search, and diagnostic tools | yes | yes | yes |
| File write and patch tools | no | yes | yes; same active posture and protections as Act |
| `Bash` (`run`, `wait`, `interact`, `cancel`) | no | approval-gated by default, hidden when `allow_shell = false` | same as Act; delegation is preferred when parallelism or isolation helps |
| Paid or external-service tools | follows permission posture | follows permission posture | follows permission posture |
| Access outside the workspace root | explicit trusted paths only | only through trusted paths or trust mode | same trusted-path/trust policy as Act; Fleet profiles never widen it |

Operate changes scheduling emphasis, not authority. It neither adds a
mode-specific tool denial nor bypasses the active approval, sandbox, shell,
ask-rule, repository-law, or managed-policy boundary. Plan remains the
mode-specific read-only boundary for shell and write-capable tools.

### Operate loop (one screen)

```text
User message
  → small / chat / one-file?  → parent does it (Act-equivalent tools)
  → real / multi-stream work? → goal (if needed) → dispatch background workers
       → each write child: implement → VERDICT PASS/FAIL with evidence
       → ordered / gated fan-in? → Workflow (operate_* starters)
       → high-stakes ambiguous? → best-of-n (N worktrees + reviewer; apply on PASS)
  → parent synthesizes receipts; stays free for the next ask
```

Lifecycle claims stay exact: dispatched ≠ settled ≠ verified.

If a shell tool is missing from the model-visible catalog in Act or Operate, check
for an explicit `allow_shell = false` in the active config/profile or runtime
session. Durable tasks and automation keep conservative omitted-field defaults;
they only receive shell access when their task settings explicitly grant it.
`allow_shell = true` controls shell availability only; direct multiline `Bash`
`run` commands remain blocked by shell safety validation. For heredocs,
embedded scripts, or long manual flows, use single-line commands, write a
script/file first, or use `Bash` with its background, `wait`, and `interact`
actions.
Full Access turns shell access on together with trust mode and auto-approval.

Action-capable modes can discover the deferred `rlm` family through
`tool_search`; its `open`, `eval`, `configure`, and `close` actions own persistent
RLM sessions. The legacy split `rlm_*` spellings remain replay-only aliases.
Inside an RLM Python REPL, `sub_query_batch` fans out 1-16 cheap parallel child
calls pinned to `deepseek-v4-flash`.

The fast `deepseek-v4-flash` / thinking-off path is called Fin in the product
language. Fin is a seam for routing, summaries, cheap child calls, and
coordination work; it does not change approval behavior.

`/goal` sets a session objective with an optional token budget and keeps active
objectives visible as Work context. `/goal pause` stops goal continuation without
changing the objective, `/goal resume` resumes and sends the objective back into
the turn, `/goal complete` marks it done, `/goal blocked` marks it blocked, and
`/goal clear` removes it. Goal state does not change the active TUI mode,
permission posture, or model route. This remains distinct from `--model auto`, which
only controls model and thinking selection.

Workflow builds on the same separation: a goal can ask the agent to keep
working, while Workflow supplies the repeatable workflow/progress surface for
large fanout. In the UI, a Workflow run should be shown as an overlay on the
main screen, not as another mode beside Plan, Act, and Operate.

App-server clients can persist a thread-scoped goal with `thread/goal/set`, read
it with `thread/goal/get`, and clear it with `thread/goal/clear`. That persisted
record carries `active`, `paused`, `blocked`, `usage_limited`, `budget_limited`,
or `complete` status plus token/time accounting fields for clients that need
thread resume semantics.

## Mode Persistence

Choosing a mode interactively also sets the mode a fresh session starts in.
Tab/Shift+Tab cycling, the `Alt+A` / `Alt+P` / `Alt+Y` shortcuts, the hotbar's
Plan/Act/Operate actions, and `/mode` all write `default_mode` to
`~/.codewhale/settings.toml`, so switching to Operate survives a restart. The
write happens off the event loop; if it fails, the TUI says so in a warning
toast rather than reverting silently on the next launch.

Mode, thinking level, and the model picker share one serialized writer, so the
selection you made last is the one on disk — a burst of Tab presses cannot end
up persisting whichever write happened to finish last — and a mode write never
rolls back an unrelated key such as `default_model`.

Two paths deliberately do **not** rewrite the startup default: restoring a saved
session (which re-installs the mode that session was in) and a mode change
refused because a turn is in flight. The legacy `yolo` entry point installs Act
plus bypass approvals, and `agent` is what it persists — `yolo` is a permission
alias, never a startup mode.

Re-selecting the mode you are already in is not a no-op. After a restored
session the live mode and `default_mode` routinely disagree, so choosing the
live mode again is how you make it durable; Codewhale confirms with a
"saved as startup default" receipt rather than reporting "already in that mode".

While a turn is running, every change to the live route is refused — mode,
model, thinking level, and provider — no matter which surface you use. That
now includes the slash surfaces (`/mode`, `/model`, `/set <key> <value>`,
`/config <key> <value>`, `/config preset`), which are reachable mid-turn. Press
Esc to interrupt first. The restart-only `default_mode` key is exempt, because
it does not touch the running turn.

Codewhale writes `settings.toml` under a lock that spans processes, and replaces
the file atomically, so a second Codewhale instance on the same home directory
cannot lose your selection or read a half-written file. At exit, queued writes
are flushed before the terminal is restored; anything that failed is printed on
the way out instead of disappearing with the alternate screen.

## Compatibility Notes

- Older settings files with `default_mode = "normal"` still load as `agent`; saving rewrites the normalized value.

## Escape Key Behavior

`Esc` is a cancel stack, not a mode switch.

- Close slash menus or transient UI first.
- Cancel the active request if a turn is running.
- Discard a queued draft if the composer is empty.
- Clear the current input if text is present.
- Otherwise it is a no-op.

## Permission Posture

Permission posture controls tool approval and whether a turn may pause for a
missing user decision. It is one layer of the full
[authorization order](AUTHORIZATION_ORDER.md), not a bypass for tool admission,
repository law, or sandbox enforcement. Cycle it with `Shift+Tab`, or edit it
at runtime:

```text
/config
# edit the approval_mode row to: suggest | auto | never
```

Legacy note: `/set approval_mode ...` was retired in favor of `/config`.

- `suggest` (**Ask**, default): tool approvals may interrupt, and Codewhale asks
  when an unresolved user choice materially changes authority, cost, scope, or
  outcome.
- `auto` (**Auto-Review**): the fully autonomous posture. It never opens a user
  question; the model resolves ambiguity from context, chooses a safe reversible
  interpretation, or reports that it cannot proceed safely. Tool safety holds
  remain separate from user questions.
- `bypass` (**Full Access**): ordinary tool calls do not show approval prompts,
  while deliberate user questions remain available. Non-bypassable safety,
  repository-law, and managed-policy holds fail closed as hard blocks instead
  of contradicting Full Access with an approval modal.
- `never`: blocks any tool that is not considered safe/read-only; deliberate
  user questions remain available.

The effective posture and its question discipline are projected into every
turn from the same runtime authority that gates tools. A mode/posture change is
therefore visible to the next turn. Untrusted runtime-generated input is
narrowed before metadata is built and cannot invent approval authority. An
explicit Full Access sub-agent handoff preserves the parent's standing posture
so ordinary child work does not begin prompting again.

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

Full Access enables trust mode automatically.

## MCP Behavior

MCP tools are exposed as `mcp_<server>_<tool>` and use the same approval flow as
built-in tools. Read-only MCP helpers may auto-run in Ask and Auto-Review when
policy permits; MCP tools with possible side effects require approval. Full
Access does not bypass hard policy holds.

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
- `-r, --resume <ID|PREFIX|latest>`: resume a saved session
- `-c, --continue`: resume the most recent session in this workspace
- `--max-subagents <N>`: clamp to `1..=128`
- `--mouse-capture` / `--no-mouse-capture`: opt in or out of internal mouse scrolling, transcript selection, right-click context actions, and transcript scrollbar dragging. Mouse capture is enabled by default on non-Windows terminals and on Windows Terminal/ConEmu/Cmder so drag selection copies only transcript text, removes visual wrap-column line breaks from paragraphs, and stays scoped to the transcript pane; hold Shift while dragging or use `--no-mouse-capture` for raw terminal selection. It defaults off on legacy Windows console (CMD without `WT_SESSION` / `ConEmuPID`) and inside JetBrains JediTerm — PyCharm/IDEA/CLion/etc. — where the terminal advertises mouse support but forwards SGR mouse events as raw text (#878, #898). Use `--mouse-capture` to opt in anywhere it's defaulted off. Raw terminal selection may cross the right sidebar and include visual wraps because the terminal, not the TUI, owns the selection.
- `--profile <NAME>`: select config profile
- `--config <PATH>`: config file path
- `-v, --verbose`: verbose logging

## Branching and Rollback

Codewhale has three related but intentionally separate recovery paths:

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
