# Sessions and Persistence

CodeWhale saves your work continuously and locally. This page covers what a
session is, how to resume one, what survives a restart, and how the four
"more than one turn" mechanisms — sub-agents, the goal loop, Fleet, and
WhaleFlow — differ.

## What a session file contains

Each session is one JSON file at `$CODEWHALE_HOME/sessions/<session-id>.json`
(default `~/.codewhale/sessions/`), written atomically:

- the full conversation, **including tool calls and tool results** (kept
  in-session so provider prompt caches replay faithfully),
- metadata: title (derived from your first message), workspace path, model,
  mode, timestamps, token totals, a cost snapshot, and fork lineage,
- the system prompt, linked `@`/`/attach` context references, and an artifact
  registry for oversized tool outputs.

Persistence cadence: a **crash-recovery checkpoint**
(`sessions/checkpoints/latest.json`) is written before each turn starts and
during streaming; the full session file is written when the turn completes.
Queued messages and composer drafts are checkpointed to an offline queue and
restored when the *same* session resumes. Saving happens on a background
task — it never blocks the UI.

Retention: the newest 50 sessions are kept; `/sessions prune <days>` removes
older ones by age.

## Resuming

| Command | Behavior |
|:---|:---|
| `codewhale` | Starts fresh. An interrupted-turn checkpoint is preserved on disk but never silently attached. |
| `codewhale --continue` | Recovers the interrupted checkpoint if one exists, else resumes the latest session for this workspace. |
| `codewhale --resume <id>` | Resumes a specific session (id prefixes work). |
| `codewhale --fresh` | Starts fresh and ignores any crash-recovery checkpoint. |
| `codewhale resume [id] [--last]` | Resume via subcommand; `--last` skips the picker. |
| `codewhale sessions` / `codewhale fork [id]` | List saved sessions / fork one into a new lineage-tracked session. |
| `codewhale exec --resume <id> \| --continue` | Same continuity for headless runs. |
| `/sessions` (alias `/resume`), `/fork` | In-TUI session picker and forking. |

## What survives a restart

| State | Lives at | Notes |
|:---|:---|:---|
| Conversations | `~/.codewhale/sessions/` | Resume any time; newest 50 kept. |
| Workspace snapshots | `~/.codewhale/snapshots/<project>/<worktree>/` | Side-git safety net behind `/restore` and `/undo`; up to 50 per workspace, 7-day prune. See [RESTORE.md](RESTORE.md). |
| Fleet runs | `<workspace>/.codewhale/fleet.jsonl` + `.codewhale/fleet/` | Append-only ledger; survives manager exit, laptop sleep, restarts. `codewhale fleet resume <run-id>` replays it idempotently. |
| Sub-agent records | `<workspace>/.codewhale/state/subagents.v1.json` | Worker records (objective, lifecycle, artifacts) persist as an audit trail — but not as live work; see below. |
| Memory | `~/.codewhale/memory.md` (opt-in) | User-global, survives everything. See [MEMORY.md](MEMORY.md). |
| Session handoff | `<workspace>/.codewhale/handoff.md` | Written by `/relay`; injected into the next session's prompt. |
| Goal trophies | `~/.codewhale/trophies/` | A result card written when a `/goal` completes. |

[DIRECTORY_STRUCTURE.md](DIRECTORY_STRUCTURE.md) is the full map of both
`~/.codewhale/` and repo-local `.codewhale/`.

What does **not** survive, honestly:

- **In-flight sub-agents.** After a restart their persisted records are marked
  `Interrupted`; prior-session records are archived history, not resumable
  work. Restart-surviving work belongs in Fleet.
- **The `/goal` HUD state.** The active objective, budget, and continuation
  counters are in-memory and are not part of the session file (per-goal usage
  totals are recorded durably, but the loop itself does not resume itself
  after a restart).
- **A crash checkpoint you opt out of** — `--fresh` deliberately ignores it.
- **Pruned history** — sessions beyond the newest 50, snapshots older than 7
  days or beyond 50 per workspace.

## Fleet vs. WhaleFlow vs. goal loop vs. sub-agents

Four mechanisms keep an agent working past a single turn. Mental model:
**sub-agents parallelize a session, the goal loop extends one, WhaleFlow plans
a big job, Fleet makes it durable.**

| | Sub-agents | Goal loop | WhaleFlow | Fleet |
|:---|:---|:---|:---|:---|
| What | In-session child workers (`agent` tool: `explore`, `plan`, `review`, `implementer`, `verifier`, …) | `/goal <objective>` keeps re-dispatching turns until done | Workflow plan (DAG of phases) authored in JS/Starlark/IR, compiled to a typed spec | Durable multi-worker control plane; each worker is a headless `codewhale exec` |
| Started by | the model, mid-turn | you, `/goal` | agent-drafted or hand-authored source | `codewhale fleet run tasks.json`, or as WhaleFlow's executor |
| Stops when | its brief is done | model reports complete/blocked, you pause/clear, or a token/time budget runs out | plan completes (loops require `max_iterations`) | tasks complete; `fleet stop`, `interrupt`, `restart` per worker |
| Survives restart | no — records archive, live work is interrupted | no — usage totals persist, the loop doesn't | the plan doesn't execute itself — durability comes from Fleet | **yes** — ledger replay via `fleet resume <run-id>` |
| Reach for it when | parallel or context-heavy legwork inside one conversation | "keep going until this is actually done" within one session | a repeatable multi-step job worth planning explicitly | work that must survive sleep/restarts, run remotely, or leave an audited trail |

The division of labor between the last two is strict: WhaleFlow owns only the
*plan* (branch/sequence/loop/reduce decisions); Fleet owns *execution* —
slots, leases, heartbeats, logs, receipts, resume. Details:
[SUBAGENTS.md](SUBAGENTS.md), [FLEET.md](FLEET.md),
[WHALEFLOW_AUTHORING.md](WHALEFLOW_AUTHORING.md).
