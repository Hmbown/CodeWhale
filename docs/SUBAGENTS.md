# Sub-Agents

Sub-agents are persistent background instances of the agent loop. The parent
opens one with a focused task, gets back an `agent_id` and session name
immediately, and continues working while the sub-agent runs to completion.
Sub-agents inherit the parent's tool registry by default and run with
`CancellationToken::child_token()`, so cancelling the parent cancels every
descendant.

This doc covers the role taxonomy. The active orchestration surface is
`agent_open`, `agent_eval`, and `agent_close`; see `prompts/base.md`
"Sub-Agent Strategy" and the in-line tool descriptions.

## Role taxonomy

The `type` field on `agent_open` selects a system-prompt posture for the child
(`agent_type` is accepted as a compatibility alias). Each role is a distinct
stance toward the work — not just a different label.

| Role          | Stance                                 | Writes? | Runs shell? | Typical use                                  |
|---------------|----------------------------------------|---------|-------------|----------------------------------------------|
| `general`     | flexible; do whatever the parent says  | yes     | yes         | the default; multi-step tasks                |
| `explore`     | read-only; map the relevant code fast  | no      | yes (read)  | "find every call site of `Foo`"              |
| `plan`        | analyse and produce a strategy         | minimal | minimal     | "design the migration; don't execute"        |
| `review`      | read-and-grade with severity scores    | no      | no          | "audit this PR for bugs"                     |
| `implementer` | land a specific change with min edit   | yes     | yes         | "rewrite `bar.rs::Foo::bar` to do X"         |
| `verifier`    | run tests / validation, report outcome | no      | yes (test)  | "run cargo test --workspace, report"         |
| `custom`      | explicit narrow tool allowlist         | depends | depends     | locked-down dispatch with hand-picked tools  |

Each role's full system prompt lives in
`crates/tui/src/tools/subagent/mod.rs` (search for
`*_AGENT_PROMPT`). The prompt prefix loads automatically when the
child agent boots; the parent's assignment prompt becomes the first
turn's user message.

## Context forking

`agent_open` starts fresh by default: the child gets its role prompt plus the
task you pass. Use `fork_context: true` when the child should continue from
the parent's current request prefix instead. In fork mode the runtime keeps the
parent prefill/prompt prefix byte-identical where available, appends a
structured state snapshot, then adds the sub-agent role instructions and task
at the tail. That preserves DeepSeek prefix-cache reuse while giving the child
the context needed for continuation, review, summarization, or compaction work.

Use fresh sessions for independent exploration. Use forked sessions when the
task depends on decisions, files, todos, or plan state already in the parent
transcript.

### When to pick which role

- **`general`** — when the task is "do this whole thing", not "go
  look", "design", or "verify". This is the right default; reach for
  a more specific role only when the posture matters.
- **`explore`** — when the parent needs evidence before deciding what
  to do next. Explorers are cheap and fast; open 2–3 in parallel
  for independent regions.
  They should orient first: confirm the project root, read relevant
  `AGENTS.md`/`README.md` guidance in unfamiliar trees, search only the
  likely scope, and return `path:line-range` evidence instead of a narrative
  tour. The role name to use is `explore` or `explorer`.
- **`plan`** — when the parent has an objective but no executable
  decomposition. Planners write artifacts (`update_plan` rows,
  `checklist_write` entries) but don't carry them out.
- **`review`** — when there's already a change and the parent wants
  it graded. Reviewers don't patch — they describe the fix in the
  finding so the parent can dispatch an Implementer if the verdict
  is "fix it".
- **`implementer`** — when the change is already specified and just
  needs to land. Implementers stay tightly scoped: minimum edit, no
  drive-by refactoring, run a quick verification before handing back.
- **`verifier`** — when the parent needs an authoritative pass/fail
  on the test suite or other validation. Verifiers don't fix
  failures; they capture the failing assertion + stack and put fix
  candidates under RISKS.
- **`custom`** — only when the parent needs to constrain the tool
  set explicitly. Pass the allowlist via the `allowed_tools` field
  on `agent_open`.

### Aliases

The model can spell each role multiple ways:

| Canonical     | Aliases                                                          |
|---------------|------------------------------------------------------------------|
| `general`     | `worker`, `default`, `general-purpose`                           |
| `explore`     | `explorer`, `exploration`                                        |
| `plan`        | `planning`, `awaiter`                                            |
| `review`      | `reviewer`, `code-review`                                        |
| `implementer` | `implement`, `implementation`, `builder`                         |
| `verifier`    | `verify`, `verification`, `validator`, `tester`                  |
| `custom`      | (none; explicit `allowed_tools` array required)                  |

All matching is case-insensitive. Unknown values produce a typed
error listing the accepted set, so the model can self-correct on
the next turn.

## Concurrency cap

The dispatcher caps concurrent sub-agents at 10 by default
(configurable via `[subagents].max_concurrent` in `~/.deepseek/config.toml`,
hard ceiling 20). When the parent hits the cap, `agent_open` returns
an error with the cap value; the parent should use `agent_eval` to wait for
completion or `agent_close` to free a slot before retrying.

The cap counts only **running** agents — completed / failed /
cancelled records persist for inspection but don't occupy a slot.
Agents that lost their `task_handle` (e.g. across a process
restart) also don't count against the cap.

## Lifecycle

Each opened session produces a record that progresses through:

```
Pending → Running → (Completed | Failed(reason) | Cancelled | Interrupted(reason))
```

`Interrupted` fires when the manager detects a `Running` agent
whose task handle is gone — typically after a process restart that
loaded the agent from `~/.deepseek/subagents.v1.json`. The parent
can open a replacement session with the same assignment or treat it as a
terminal state.

### Session boundaries (#405)

Each `SubAgentManager` instance assigns itself a fresh
`session_boot_id` on construction. Every new session stamps the agent
with that id; the persisted state file carries it across restarts.

`agent_eval` and the sidebar/status projections focus on current-session
agents by default. Prior-session agents that are not still running are treated
as archived records so the model does not mistake stale work for live work.

Records that loaded from a pre-#405 persisted state file (no
`session_boot_id` field) classify as prior-session because the
manager can't match them to the current boot.

## Output contract

Every sub-agent produces a final result string with five sections,
in order:

```
SUMMARY:    one paragraph; what you did and what happened
CHANGES:    files modified, with one-line descriptions; "None." if read-only
EVIDENCE:   path:line-range citations and key findings; one bullet each
RISKS:      what could go wrong / what the parent should double-check
BLOCKERS:   what stopped you; "None." if you finished cleanly
```

The exact format lives in `crates/tui/src/prompts/subagent_output_format.md`.
The parent reads `EVIDENCE` as a working set for the next turn, so
explorers and reviewers should be precise here.

## Memory and the `remember` tool (#489)

Sub-agents inherit the parent's memory file when memory is enabled
(`[memory] enabled = true` or `DEEPSEEK_MEMORY=on`). They can
append durable notes via the `remember` tool — handy for an
explorer that discovers a project convention worth carrying across
sessions, or a verifier that learns "this test is flaky".

Memory writes are scoped to the user's own `memory.md` file; they
don't go through the standard write-approval flow.

## Implementation notes

- Source: `crates/tui/src/tools/subagent/mod.rs` (about 3500 LOC).
- Persisted state: `~/.deepseek/subagents.v1.json`. Schema version
  `1` (forward-compatible — new optional fields use
  `#[serde(default)]`).
- The `is_running` check ignores agents whose `task_handle` is
  `None`; this avoids counting persisted-but-detached records
  toward the concurrency cap (#509).
- `SharedSubAgentManager` is `Arc<RwLock<...>>` — read paths use
  read locks so `/agents` and the sidebar projection don't block
  the main loop during multi-agent fan-out (#510).
