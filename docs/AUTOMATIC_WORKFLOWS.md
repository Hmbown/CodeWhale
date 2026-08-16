# Automatic Workflows

You do **not** need to write a `.workflow.js` file to coordinate agents. In
Operate, ordinary messages can use direct tools or background workers; workers
are preferred for independent, parallel, background, or long-running work.
Workflow is reserved for ordered phases, gates, shared budgets, replay, or
deterministic fan-in. Act/Agent can still use the optional soft-auto policy
described below.

Related docs:

- [Workflow Authoring](WORKFLOW_AUTHORING.md) — checked-in scripts and IR
- [Fleet + Workflow Tutorial](FLEET_WORKFLOW_TUTORIAL.md) — manual Fleet paths
- [Configuration](CONFIGURATION.md) — `[workflow]` knobs
- [Sandbox](SANDBOX.md) — what the Workflow VM cannot do

## Soft-auto in Act/Agent

1. **You ask naturally** — “audit every crate for unsafe,” “scout then implement,”
   “compare these two providers in parallel.”
2. **Codewhale decides in Act/Agent** — broad, independent, or staged work can
   trigger Workflow; one-file edits, simple commands, and pure Q&A do not.
3. **It tells you first** — e.g. “This looks set up for a Workflow — three scouts
   then one verifier.”
4. **Optional setup** — if one or two facts would change the plan (read-only vs
   writes, scope, child count), it opens the **`request_user_input`** modal
   (structured multiple choice, not a long free-form interview).
5. **Launch** — structured `plan` JSON (goal / phases / children) or a short
   inline script. Parallel branches use `parallel()` partial-success semantics.

In Operate, those same asks prefer one or more direct background workers when
the split improves throughput, isolation, or context focus. Small or tightly
coupled work can stay in the parent under the active tool and approval policy.
You can always type `/workflow` to request orchestration explicitly.

## Read-only auto-start vs write approval

`[workflow]` config (see `config.example.toml`):

| Knob | Default | Meaning |
|------|---------|---------|
| `automatic` | `true` | Soft-auto orchestration is enabled |
| `auto_start_read_only` | `true` | Read-only plans may start without a write-approval card |
| `require_approval_for_writes` | `true` | Gates the plan-approval card for writes / elevated starts |
| `auto_start_child_limit` | `16` | Soft cap on automatic child count |
| `max_children` / `max_concurrent` / `max_depth` | `1000` / `16` / `2` | Hard ceilings |
| `default_token_budget` | `120000` | Shared admission hint; not an exact mid-stream cutoff |
| `persist_completed_activity` | `true` | Keep completed panel/history activity |

Elevated work (writes, shell beyond read-only, network, secrets, worktrees, high
budget) surfaces an approval card with goal, child summary, capability flags,
and budget before launch (#4126) when `require_approval_for_writes` is on.
That flag only gates the card. Session-level auto-approve (YOLO / Full Access /
`bypass`) still skips it, the same as other ordinary `Required` tools.
Writes inside a running VM `task()` step are the VM runtime contract
(sandbox, `writeAuthority`, parent tool policy) — this flag does not re-ask
for each child write.

Worktree isolation and write ownership are separate. A write-capable `task()`
declares `writeAuthority: "workspace_write"` or `"worktree_write"` plus at
least one repo-relative `writeRoots`, `exactFiles`, or
`coordinationContracts` value. `worktree: true` selects isolation but does not
silently grant mutation authority. A prompt-only general task is read-only.
`dependencies` and `acceptance` carry bounded child-specific prerequisites and
observable completion checks; they are not a parent-transcript copy.

When a workflow runs from a workspace containing multiple repositories, a
child that needs shell or file access must set `cwd` to the repository-relative
directory it should use. The host validates that the directory exists inside
the parent workspace before dispatch. Use `worktree: true` for isolated writes;
`cwd` selects an existing checkout and does not grant write authority or
isolation by itself.

## Controlling a run

`/workflow status [run_id]`, `/workflow cancel [run_id]`, and `/workflow
settings` are answered by Codewhale itself from the run journal and the live
run state — they never spend a model turn, so a status check is free and a
cancel lands even while the model is busy. `/workflow cancel` with no id stops
the only running workflow. `/workflow run <path/to/x.workflow.js>` launches a
checked-in workflow as-is; `/workflow <objective>` and bare `/workflow` ask
the model to author and run one. The `[workflow]` table above is read from
your `config.toml` for every launch decision (auto-start, write-approval
card, child limits); `/workflow settings` prints the effective values with
what each one does. Reloading `config.toml` refreshes that table for both
settings and the workflow tool.

## What you see while it runs

- **Workflow panel** — phases, children, status, budget
- **Compact history card** — one calm row that expands for detail
- **One artifact per delegated unit** — no duplicate “delegate + tool card”
- **Typed child identity** — labels/roles; no “unknown child” in the default UI

Cancel stops the run and child agents. Completed activity can persist across the
session (and across restarts when configured).

## Sandbox guarantees

The Workflow JS VM has **no** filesystem, shell, network, env, imports, clock, or
randomness. Allowed host calls: `task`, `parallel`, `pipeline`, `phase`, `log`,
`budget`, `args`. Real work happens in sub-agents / Fleet under normal tool and
approval policy. See [Sandbox](SANDBOX.md).

## Synthesis and compatibility

- Prefer `responseSchema` on children that must return structured fields.
- Ordinary failed parallel slots become `null` (partial success); filter them
  before synthesizing one operator-facing summary. A `responseSchema` mismatch
  is a contract failure and intentionally fails the run instead of being
  silently converted to `null`.
- Workflow token budgets govern admission and aggregate accounting. Once
  exhausted they reject later or descendant spawns, but children already
  running in parallel can reconcile aggregate usage above the hint because
  providers report usage only at response boundaries.
- Compatibility paths remain: `script`, `source_path` (checked-in
  `.workflow.js` / `.workflow.ts`), and structured `plan`.

## When automatic stays off

Automatic Workflow is suppressed for:

- One-file edits and tiny one-step asks  
- Simple commands / factual questions  
- Highly interactive design conversations  
- Risky writes without a clear decomposition  
- Estimated children above `auto_start_child_limit` (ask or shrink first)

In those cases Codewhale uses direct tools or a single `agent` instead.

## Example scenarios (#4131)

Checked-in example workflows cover four automatic-Workflow scenarios:

1. Read-only repo audit  
2. Staged bug fix with worktree implementer + verifier  
3. Partial failure and synthesis  
4. Cancellation mid-run  

Fixtures: [`docs/examples/dogfood-automatic/`](examples/dogfood-automatic/).
Panel regression tests use the `dogfood_` prefix in
`crates/tui/src/tui/widgets/workflow_panel.rs`.
