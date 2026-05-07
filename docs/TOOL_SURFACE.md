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

## Current surface (v0.7.5)

### File operations

| Tool | Niche |
|---|---|
| `read_file` | Read a UTF-8 file. PDFs auto-extracted via `pdftotext` (poppler) when available; `pages: "1-5"` slices large docs. |
| `list_dir` | Structured, gitignore-aware listing. Preferred over `exec_shell("ls")`. |
| `write_file` | Create or overwrite a file. |
| `edit_file` | Search-and-replace inside a single file. Cheaper than a full rewrite. |
| `apply_patch` | Apply a unified diff. The right tool for multi-hunk edits. |
| `retrieve_tool_result` | Read summaries or slices of prior large tool outputs spilled to `~/.deepseek/tool_outputs/`; use `summary`, `head`, `tail`, `lines`, or `query` instead of replaying the whole result. |

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

`agent_spawn` plus the supporting tools (`agent_result` / `wait` / `send_input` /
`agent_assign` / `agent_cancel` / `resume_agent` / `agent_list`).
See `agent.txt` for the delegation protocol and
[`SUBAGENTS.md`](SUBAGENTS.md) for the role taxonomy
(`general` / `explore` / `plan` / `review` / `implementer` /
`verifier` / `custom`).

`agent_spawn` defaults to a fresh child conversation. Pass
`fork_context: true` for continuation-style work that should inherit the
parent's system prompt and message prefix for DeepSeek prefix-cache reuse.
The deprecated `delegate_to_agent` compatibility wrapper routes through
`agent_spawn` and defaults `fork_context` to true.

### Parallel fan-out: cost-class caps

Two tools offer parallel fan-out with different concurrency limits that
reflect very different cost classes:

| Tool | What each child does | Wall-clock | Token cost | Cap |
|---|---|---|---|---|
| `agent_spawn` | Full sub-agent loop (planning, tool calls, multi-turn streaming, can spawn children) | minutes | thousands of tokens | 10 in flight by default (`[subagents].max_concurrent`, hard ceiling 20) |
| `rlm` helper `llm_query_batched` | One-shot non-streaming Chat Completions calls pinned to `deepseek-v4-flash` | seconds | ~hundreds of tokens | 16 per call |

The caps appear in each tool's description and error messages so the model
(and the user) can choose the right tool for the job. If one sub-agent is
enough but you need parallel lookups, prefer `rlm` with `llm_query_batched`; if each task needs
its own tool-carrying agent loop, use `agent_spawn` (and cancel completed
ones to free slots).

## Recently consolidated (v0.5.1)

Removed from the prompt as duplicates of equivalent tools (the underlying
dispatchers still resolve them, so existing sessions don't break — they just
no longer pollute the model's tool list):

- `spawn_agent` → use `agent_spawn`.
- `close_agent` → use `agent_cancel`.
- `assign_agent` → use `agent_assign`.

## Deprecation schedule (v0.6.2 → v0.8.0)

The alias tools below still execute successfully but now attach a
`_deprecation` block to every result they return. Models should migrate to
the canonical name before v0.8.0, when the aliases will be removed.

| Deprecated alias | Canonical name | Warning since | Removal |
|---|---|---|---|
| `spawn_agent` | `agent_spawn` | v0.6.2 | v0.8.0 |
| `delegate_to_agent` | `agent_spawn` | v0.6.2 | v0.8.0 |
| `close_agent` | `agent_cancel` | v0.6.2 | v0.8.0 |
| `send_input` | `agent_send_input` | v0.6.2 | v0.8.0 |

The `_deprecation` block shape:

```json
{
  "_deprecation": {
    "this_tool": "spawn_agent",
    "use_instead": "agent_spawn",
    "removed_in": "0.8.0",
    "message": "Tool 'spawn_agent' is deprecated; switch to 'agent_spawn' before v0.8.0."
  }
}
```

This block is merged into the tool result's `metadata` object alongside any
other metadata keys (e.g. `status`, `timed_out`) so it does not displace
existing metadata.  A one-line deprecation warning is also emitted to the
audit log at `tracing::warn` level every time an alias is invoked.

## Why we don't ship a single `bash` tool

Single-`bash` agents (Claude Code's design) are powerful but hand the model
all the foot-guns of shell scripting: quoting, platform divergence,
side-effects from misread cwd, `cd` not persisting between calls, etc. Our
file tools are also significantly cheaper to render in the transcript
(structured JSON-shaped output collapses better than `ls -la` walls of text).

The model can always fall back to `exec_shell` when something is missing.
The dedicated tools just take the common 80% off the shell escape-hatch.
