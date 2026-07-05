# Rollback and Restore

CodeWhale snapshots your working tree automatically so a bad turn is cheap to
undo. This page explains what gets snapshotted, when, and how to roll back
with `/restore` and `/undo`.

## How snapshots work

Every workspace gets a **side git repository** stored under
`~/.codewhale/snapshots/<project_hash>/<worktree_hash>/` (legacy installs fall
back to `~/.deepseek/snapshots/`). Snapshots are commits in that side repo:

- **Your own `.git` is never touched.** Every snapshot operation pins both
  `--git-dir` (the side repo) and `--work-tree` (your workspace), so the side
  repo and your real repository stay fully independent. Workspaces that are
  not git repositories get snapshots too.
- Snapshots capture **working-tree files only** — not your repo's git history,
  branches, or staged state.
- Snapshotting is a **safety net, not a correctness gate**: if git is missing
  or the disk is full, the turn proceeds and a warning is logged.

Snapshots are taken at three points, labeled so you can recognize them:

| Label | Taken |
|:---|:---|
| `pre-turn:<seq>` | before each turn starts (the label embeds your prompt) |
| `tool:<call_id>` | before each file-modifying tool call inside a turn |
| `post-turn:<seq>` | after the turn finishes |

Retention: each workspace keeps up to 50 snapshots; entries older than 7 days
are pruned at session start.

## `/restore` — revert files to a snapshot

```text
/restore            # list the 20 most recent snapshots (newest first)
/restore list 50    # list more, up to 100
/restore 3          # restore the 3rd-most-recent snapshot (1 = newest)
```

Each row shows `#N  <UTC time>  <short sha>  <label>`. Pick the last point
that was good and run `/restore <N>`.

`/restore` reverts **workspace files only** — the success message is explicit:
*"Workspace files have been reverted; conversation history is unchanged."*
Outside YOLO, `/restore <N>` refuses until you opt in with `/trust on` or
`/mode yolo`, because restoring can rewrite files across the workspace.

## `/undo` and `revert_turn`

- `/undo` first attempts a surgical, snapshot-based undo of the most recent
  file changes; when no snapshots are available it falls back to unwinding the
  last conversation exchange instead.
- `revert_turn` is the agent-callable equivalent: if you type "undo your last
  edit", the model can call it with a turn offset (up to 50 turns back). It is
  approval-gated and, like `/restore`, touches working-tree files only.

## Choosing the right tool

| Situation | Use |
|:---|:---|
| The code broke, but the conversation is useful | `/restore <N>` — files roll back, context stays |
| The prompt was wrong, but the edits are good | `/undo` after a turn with no file changes, or just correct course in the next message — files are untouched |
| Start over from a known good turn | `/restore` to the `pre-turn:` snapshot of that turn, then continue the conversation from there |
| Undo just the last edit mid-flow | ask the agent ("revert your last edit") — it calls `revert_turn` |

## Interaction with normal git

Snapshots complement git; they do not replace it. Commits you make remain the
durable history — `/restore` simply rewrites working-tree files, so `git diff`
afterwards shows the restore like any other local change. Files that exist now
but were absent in the target snapshot are removed as part of the restore.

The Runtime API exposes snapshot *listing* for GUI clients; restore/undo
mutation endpoints over HTTP are intentionally deferred (see
[RUNTIME_API.md](RUNTIME_API.md)).

Next: [workflows/rollback-with-restore.md](workflows/rollback-with-restore.md)
is the short copy-paste version of this page.
