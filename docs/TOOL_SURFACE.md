# Tool surface

Why these specific tools, in this groupings, and how each one is meant to be
chosen over the available shell equivalent. Companion to `crates/tui/src/prompts/agent.txt`.

## Design stance

- **Dedicated tools over `exec_shell` whenever the dedicated tool returns
  structured output.** Bash escaping is error-prone and platform behavior
  varies (GNU vs BSD `grep`, `rg` is not always installed). Structured
  output also frees the model from re-parsing free-form text.
- **`exec_shell` for everything else.** Build, test, format, lint, ad-hoc
  commands, anything platform-specific. We don't try to wrap the long tail.
- **Drop tools that don't beat their shell equivalent.** Two-tool aliases
  for the same backing operation are a model trap — the LLM will alternate
  between them and the cache hit rate suffers.

## Current surface (v0.8.35)

### File operations

| Tool | Niche |
|---|---|
| `read_file` | Read a UTF-8 file. PDFs auto-extracted via `pdftotext` (poppler) when available; `pages: "1-5"` slices large docs. |
| `list_dir` | Structured, gitignore-aware listing. Preferred over `exec_shell("ls")`. |
| `write_file` | Create or overwrite a file. |
| `edit_file` | Search-and-replace inside a single file. Cheaper than a full rewrite. |
| `apply_patch` | Apply a unified diff. The right tool for multi-hunk edits. |
| `retrieve_tool_result` | Read summaries or slices of prior large tool outputs spilled to `~/.deepseek/tool_outputs/`; use `summary`, `head`, `tail`, `lines`, or `query` instead of replaying the whole result. |
| `handle_read` | Read bounded projections from `var_handle` payloads held by live tool environments. This is the foundation for RLM sessions, sub-agent transcripts, and other large symbolic payloads. |

### Search

| Tool | Niche |
|---|---|
| `grep_files` | Regex search file contents within the workspace; structured matches + context lines. Pure-Rust (`regex` crate), no `rg`/`grep` shell-out. |
| `file_search` | Fuzzy-match filenames (not contents). Use when you know roughly the name. |
| `web_search` | DuckDuckGo (with Bing fallback); ranked snippets + `ref_id` for citation. |
| `fetch_url` | Direct HTTP GET on a known URL. Faster than `web_search` when the link is already known. HTML stripped to text by default. |

### Shell

| Tool | Niche |
|---|---|
| `exec_shell` | Run a shell command. Foreground runs are cancellable, but use them only for bounded commands; timeout kills the process and returns a background-rerun hint. |
| `exec_shell_wait` | Poll a background task for incremental output. Canceling the turn stops waiting without killing the task. |
| `exec_shell_interact` | Send stdin to a running background task and read incremental output. |
| `exec_shell_cancel` | Cancel one running background shell task by id, or all running background shell tasks when explicitly requested. |
| `task_shell_start` | Start a long-running command in the background and return immediately. Preferred over foreground shell for diagnostics, tests, searches, and servers that may run for minutes. |
| `task_shell_wait` | Poll a background command. If `gate` is supplied after completion, record structured gate evidence on the active durable task. |

When a foreground shell command times out, the process is not continued
silently. The tool result tells the model to rerun long work with
`task_shell_start` or `exec_shell` with `background = true`, then poll with
`task_shell_wait` or `exec_shell_wait`.

Interactive shell jobs are also visible through `/jobs`. The TUI job center is
fed by the same shell manager as `exec_shell`/`task_shell_start`, and shows the
command, cwd, elapsed time, status, output tail, process-local shell id, and
linked durable task id when available. `/jobs show`, `/jobs poll`, `/jobs wait`,
`/jobs stdin`, and `/jobs cancel` provide inspect, polling, stdin, and cancel
controls for live jobs. Jobs are process-local; after restart, live process
state is not reattached, and any remembered detached entries must be marked
stale rather than presented as live processes.

### MCP manager and palette discovery

MCP server configuration is surfaced in the TUI through `/mcp` and the
`mcp_config_path` row in `/config`. `/mcp` shows the resolved config path,
server enabled/disabled state, transport, command or URL, timeouts, connection
errors, and discovered tools/resources/prompts. It supports narrow manager
actions for init, add, enable, disable, remove, validate, and reload/reconnect.
Config edits are written immediately, but the model-visible MCP tool pool is
restart-required after edits.

The command palette includes MCP entries grouped by server. Disabled and failed
servers stay visible, and discovered tools/prompts use the runtime names shown
to the model, such as `mcp_<server>_<tool>`.

### Git / diagnostics / testing

| Tool | Niche |
|---|---|
| `git_status` | Inspect repo status without running shell. |
| `git_diff` | Inspect working-tree or staged diffs. |
| `diagnostics` | Workspace, git, sandbox, and toolchain info in one call. |
| `run_tests` | `cargo test` with optional args. |

### Task management and durable work

| Tool | Niche |
|---|---|
| `update_plan` | Structured checklist for complex multi-step work. |
| `task_create` | Create/enqueue a durable background task through `TaskManager`. This is the real executable work object for long-running agent work. |
| `task_list` | List durable tasks with status and linked runtime ids. |
| `task_read` | Read durable task detail: thread/turn linkage, timeline, checklist, gates, artifacts, PR attempts, GitHub events. |
| `task_cancel` | Cancel a queued or running durable task. Approval-required. |
| `checklist_write` | Granular progress under the active thread/task. Checklist state is subordinate to the durable task. |
| `checklist_add` / `checklist_update` / `checklist_list` | Single-item checklist operations. |
| `todo_write` / `todo_add` / `todo_update` / `todo_list` | Compatibility aliases for the checklist tools. Existing sessions keep working, but new prompts should use `checklist_*`. |
| `note` | One-off important fact for later. |

### Verification gates and artifacts

| Tool | Niche |
|---|---|
| `task_gate_run` | Run an approved verification command and attach structured evidence to the active durable task: command, cwd, exit code, duration, classification, summary, and log artifact. |

Large logs and command outputs should be artifacts with compact summaries in the transcript. `task_gate_run` handles this automatically for active durable tasks.

### GitHub context and guarded writes

| Tool | Niche |
|---|---|
| `github_issue_context` | Read-only issue context via `gh issue view`; large bodies become task artifacts when possible. |
| `github_pr_context` | Read-only PR context via `gh pr view`; optional diff capture via `gh pr diff --patch`; large bodies/diffs become task artifacts when possible. |
| `github_comment` | Approval-required issue/PR comment with structured evidence. |
| `github_close_issue` | Approval-required issue closure. Requires non-empty acceptance criteria and evidence; refuses dirty worktrees unless explicitly allowed. Never close an issue merely because an agent is stopping. |

### PR attempts

| Tool | Niche |
|---|---|
| `pr_attempt_record` | Capture the current git diff as attempt metadata plus a patch artifact on a durable task. |
| `pr_attempt_list` | List attempts recorded on a task. |
| `pr_attempt_read` | Inspect one recorded attempt and its artifact reference. |
| `pr_attempt_preflight` | Run `git apply --check` against an attempt patch. No worktree mutation. |

### Automations

| Tool | Niche |
|---|---|
| `automation_create` | Create a scheduled automation. Approval-required. |
| `automation_list` / `automation_read` | Inspect durable automations and recent runs. |
| `automation_update` | Update prompt, schedule, cwds, or status. Approval-required. |
| `automation_pause` / `automation_resume` / `automation_delete` | Lifecycle controls. Approval-required. |
| `automation_run` | Run an automation now; the run enqueues a normal durable task. Approval-required. |

### Sub-agents

v0.8.33 began moving large tool outputs toward symbolic handles: tools return
small `var_handle` objects, and `handle_read` retrieves bounded slices, counts,
or JSON projections from the backing environment. This keeps the parent
transcript small while preserving a recovery path to the full payload.

The active model-facing sub-agent surface is persistent and intentionally small:

| Tool | Niche |
|---|---|
| `agent_open` | Open a named sub-agent session for independent work. Returns a session projection immediately so the parent can keep coordinating. |
| `agent_eval` | Send follow-up input, block for completion, or fetch the current projection/transcript handle for an existing session. |
| `agent_close` | Cancel or release a sub-agent session by name or id. |

See `agent.txt` for the delegation protocol and
[`SUBAGENTS.md`](SUBAGENTS.md) for the role taxonomy
(`general` / `explore` / `plan` / `review` / `implementer` /
`verifier` / `custom`).

`agent_open` defaults to a fresh child conversation. Pass
`fork_context: true` for continuation-style work or multi-perspective reviews
that should inherit the parent's context. In fork mode, the runtime preserves
the parent prefill/prompt prefix byte-identically where available so DeepSeek's
prefix cache can be reused, then appends the child role instructions and task.

### Recursive LM sessions

RLM is now persistent as well:

| Tool | Niche |
|---|---|
| `rlm_open` | Open a named Python REPL over a file, inline content, or URL. |
| `rlm_eval` | Run bounded Python against that session, using deterministic code and in-REPL semantic helpers such as `sub_query_batch`. |
| `rlm_configure` | Adjust output feedback, child-query timeout/depth, and session-sharing settings. |
| `rlm_close` | Shut down the Python runtime and return final session stats. |

Large RLM outputs should come back as `var_handle`s. Use `handle_read` for
bounded text slices, line ranges, counts, or JSONPath projections instead of
replaying the full value into the parent transcript.

Inside `rlm_eval`, the loaded source is available as `_context`; `content` is
also bound as a convenience alias because agents naturally reach for it during
Python analysis. The shorter `context` and `ctx` names are intentionally not
bound so user variables can use them without colliding with the bootstrap.

Child-call timeouts are session policy: use `rlm_configure` with
`sub_query_timeout_secs` before running a large fan-out. The helpers
`sub_query`, `sub_query_batch`, `sub_query_map`, and `sub_rlm` accept a
`timeout_secs` keyword for compatibility with common agent guesses, but the
effective timeout remains configured at the RLM session level.

`finalize(value, confidence=...)` preserves JSON-serializable values. Strings
become text handles; dicts, lists, numbers, booleans, and null become JSON
handles that `handle_read` can project with JSONPath.

### Session relay

`/relay [focus]` asks the current agent to write `.deepseek/handoff.md` as a
compact `# Session relay` artifact for the next thread. The filename remains
for compatibility with existing prompt loading and older sessions; the visible
mental model is relay / 接力.

Aliases: `/batonpass`, `/接力`.

Use it before a long break, compaction, or moving work to a fresh session. The
relay should preserve the goal, current Work checklist item, changed files,
decisions, verification state, and one concrete next action.

### Parallel fan-out: cost-class caps

Two tools offer parallel fan-out with different concurrency limits that
reflect very different cost classes:

| Tool | What each child does | Wall-clock | Token cost | Cap |
|---|---|---|---|---|
| `agent_open` | Full sub-agent loop (planning, tool calls, multi-turn streaming, can open children) | minutes | thousands of tokens | 10 in flight by default (`[subagents].max_concurrent`, hard ceiling 20) |
| `rlm_eval` helper `sub_query_batch` | One-shot non-streaming Chat Completions calls pinned to `deepseek-v4-flash` inside a live RLM session | seconds | ~hundreds of tokens | 16 per call |

The caps appear in each tool's description and error messages so the model
(and the user) can choose the right tool for the job. If one sub-agent is
enough but you need parallel semantic lookups over the same loaded context,
prefer `rlm_eval` with `sub_query_batch`; if each task needs its own
tool-carrying agent loop, use `agent_open` and close completed sessions to free
slots.

## Removed legacy aliases and surfaces

v0.8.33 removed the old model-facing sub-agent fan-out surface from active
prompting and tool catalogs. Do not use these names in new active guidance:
`agent_spawn`, `agent_wait`, `agent_result`, `agent_send_input`,
`agent_assign`, `agent_resume`, `agent_list`, `spawn_agent`,
`delegate_to_agent`, `send_input`, and `close_agent`.

The old one-shot `rlm` model-facing tool is also replaced by persistent
`rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close` sessions.

Historical compatibility results may include a `_deprecation` block shaped
like this:

```json
{
  "_deprecation": {
    "this_tool": "spawn_agent",
    "use_instead": "agent_open",
    "removed_in": "0.8.33",
    "message": "Tool 'spawn_agent' is deprecated; switch to 'agent_open'."
  }
}
```

This is a legacy/compatibility note, not the active recommended surface.

## Release smoke: verify the live names

When validating a release, verify the model-visible registry names directly.
Do not grep random handler function names; handler names are allowed to drift
while the registry contract stays stable.

Version smoke:

```bash
deepseek --version
deepseek-tui --version
```

Tool-surface smoke:

```bash
rg -n '"handle_read"|"rlm_open"|"rlm_eval"|"rlm_configure"|"rlm_close"|"agent_open"|"agent_eval"|"agent_close"' crates/tui/src
rg -n 'handle_read|rlm_open|rlm_eval|rlm_configure|rlm_close|agent_open|agent_eval|agent_close' docs crates/tui/src/prompts crates/tui/src/tools
```

The canonical v0.8.35 live names are:

- `handle_read`
- `rlm_open`, `rlm_eval`, `rlm_configure`, `rlm_close`
- `agent_open`, `agent_eval`, `agent_close`

The registry should not actively advertise the legacy one-shot names
`agent_spawn`, `agent_wait`, `agent_result`, or the old foreground `rlm` tool
outside legacy/removal notes. Historical changelog entries and compatibility
code may still mention them.

## Why we don't ship a single `bash` tool

Single-`bash` agents (Claude Code's design) are powerful but hand the model
all the foot-guns of shell scripting: quoting, platform divergence,
side-effects from misread cwd, `cd` not persisting between calls, etc. Our
file tools are also significantly cheaper to render in the transcript
(structured JSON-shaped output collapses better than `ls -la` walls of text).

The model can always fall back to `exec_shell` when something is missing.
The dedicated tools just take the common 80% off the shell escape-hatch.
