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
| `require_approval_for_writes` | `true` | Writes / elevated plans need explicit approval |
| `auto_start_child_limit` | `8` | Soft cap on automatic child count |
| `max_children` / `max_depth` | `64` / `2` | Hard ceilings |
| `default_token_budget` | `120000` | Shared admission hint; not an exact mid-stream cutoff |
| `persist_completed_activity` | `true` | Keep completed panel/history activity |

Elevated work (writes, shell beyond read-only, network, secrets, worktrees, high
budget) should surface an approval card with goal, child summary, capability
flags, and budget before launch (#4126).

Worktree isolation and write ownership are separate. A write-capable `task()`
declares `writeAuthority: "workspace_write"` or `"worktree_write"` plus at
least one repo-relative `writeRoots`, `exactFiles`, or
`coordinationContracts` value. `worktree: true` selects isolation but does not
silently grant mutation authority. A prompt-only general task is read-only.
`dependencies` and `acceptance` carry bounded child-specific prerequisites and
observable completion checks; they are not a parent-transcript copy.

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
