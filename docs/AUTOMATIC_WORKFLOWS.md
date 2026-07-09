# Automatic Workflows

You do **not** need to say “workflow” or write a `.workflow.js` file for ordinary
multi-agent work. CodeWhale decides when orchestration helps, tells you the
shape, may ask a short setup question, then launches a Workflow.

Related docs:

- [Workflow Authoring](WORKFLOW_AUTHORING.md) — checked-in scripts and IR
- [Fleet + Workflow Tutorial](FLEET_WORKFLOW_TUTORIAL.md) — manual Fleet paths
- [Configuration](CONFIGURATION.md) — `[workflow]` knobs
- [Sandbox](SANDBOX.md) — what the Workflow VM cannot do

## Soft-auto (default product path)

1. **You ask naturally** — “audit every crate for unsafe,” “scout then implement,”
   “compare these two providers in parallel.”
2. **CodeWhale decides** — broad, independent, or staged work triggers Workflow;
   one-file edits, simple commands, and pure Q&A do not.
3. **It tells you first** — e.g. “This looks set up for a Workflow — three scouts
   then one verifier.”
4. **Optional setup** — if one or two facts would change the plan (read-only vs
   writes, scope, child count), it opens the **`request_user_input`** modal
   (structured multiple choice, not a long free-form interview).
5. **Launch** — structured `plan` JSON (goal / phases / children) or a short
   inline script. Parallel branches use `parallel()` partial-success semantics.

You can still type `/workflow` to force “orchestrate the current work.”

## Read-only auto-start vs write approval

`[workflow]` config (see `config.example.toml`):

| Knob | Default | Meaning |
|------|---------|---------|
| `automatic` | `true` | Soft-auto orchestration is enabled |
| `auto_start_read_only` | `true` | Read-only plans may start without a write-approval card |
| `require_approval_for_writes` | `true` | Writes / elevated plans need explicit approval |
| `auto_start_child_limit` | `8` | Soft cap on automatic child count |
| `max_children` / `max_depth` | `64` / `2` | Hard ceilings |
| `default_token_budget` | `120000` | Shared budget hint |
| `persist_completed_activity` | `true` | Keep completed panel/history activity |

Elevated work (writes, shell beyond read-only, network, secrets, worktrees, high
budget) should surface an approval card with goal, child summary, capability
flags, and budget before launch (#4126).

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
- Failed parallel slots become `null` (partial success); filter them before
  synthesizing one operator-facing summary.
- Compatibility paths remain: `script`, `source_path` (checked-in
  `.workflow.js` / `.workflow.ts`), and structured `plan`.

## When automatic stays off

Automatic Workflow is suppressed for:

- One-file edits and tiny one-step asks  
- Simple commands / factual questions  
- Highly interactive design conversations  
- Risky writes without a clear decomposition  
- Estimated children above `auto_start_child_limit` (ask or shrink first)

In those cases CodeWhale uses direct tools or a single `agent` instead.
