# Workflow Authoring

> **Ordinary multi-agent work does not require this file.** In Operate, send
> normal messages; Codewhale can work directly or prefer background workers
> when parallelism, isolation, or duration makes delegation useful. Use Workflow
> when ordered phases, gates, shared budgets, replay, or deterministic fan-in
> matter; Act/Agent may also use optional soft-auto launch. See
> [Automatic Workflows](AUTOMATIC_WORKFLOWS.md).

Workflow has one runtime boundary: authored source lowers to typed
Rust `WorkflowSpec`, Rust validates the IR, and the scheduler/headless worker
runtime executes leaves. Authoring languages do not get hidden authority to own
files, shell, network, providers, cancellation, or TUI state.

Compatibility launch paths on the `workflow` tool:

| Input | When to use |
|-------|-------------|
| `plan` | Structured goal / phases / children (preferred agent path) |
| `script` | Short inline JS the model owns |
| `source_path` | Checked-in `.workflow.js` / `.workflow.ts` in the workspace |

For a guided walkthrough from fleet task specs to Workflow authoring and
monitoring, see [fleet + Workflow Tutorial](FLEET_WORKFLOW_TUTORIAL.md).


## Access model

The Workflow script is a **coordinator only**. It has no filesystem or shell of
its own. Real work happens in sub-agents the script launches.

| Layer | What it can access |
|-------|--------------------|
| Workflow script (JS VM) | Script variables, branching/loops, `task()` / `parallel()` / `pipeline()`, `phase` / `log`, `budget` / `args`. **No** direct FS, shell, network, env, imports, clock, or randomness. |
| Workflow-spawned sub-agents | Normal tool surface (read/search/edit/write, shell, web, MCP) subject to role posture, allowlists, and parent policy. File edits for write-capable roles auto-accept under Workflow; shell / web / MCP still require parent auto-approve or fail closed. |
| Parent session | Working directory, configured tools/MCP, permission mode, sandbox/network rules. |

### Scale

- Up to **16 concurrent** live agents in one run (additional spawns wait for a slot).
- Up to **1_000 agents per run** (VM lifetime spawn cap).
- Soft auto-launch still uses a lower child soft-cap (`auto_start_child_limit`).

See the Workflow JS sandbox tests for the fail-closed host surface inventory.

## Language Choice

| Surface | Strength | Tradeoff | v0.8.60 stance |
|---|---|---|---|
| YAML / JSON IR | Simple, reviewable, no runtime | Verbose for generated workflows | Keep as interchange/debug format |
| JavaScript | Familiar object syntax and easy agent generation | Unsafe if executed as a general runtime | First-class authoring through declarative compile-only subset |
| TypeScript | Best editor/types story for workflow SDK | Needs stripping/typechecking if full TS is supported | Same compile-only subset for now; richer SDK later |

The default high-capability path is TypeScript/JavaScript authoring, but only as
a compile step. The compiler accepts a JSON-compatible object inside
`workflow({...})` from `.workflow.js` or `.workflow.ts`, lowers it to
`WorkflowSpec`, and runs the Rust validation gate. (Starlark authoring was a
bootstrap reference and has been removed; Workflow authoring is JS-only.)

## Contract

Accepted source shape:

```js
export default workflow({
  "id": "issue-audit-js",
  "goal": "Audit an issue fix with parallel agents",
  "nodes": [
    {
      "branch": {
        "id": "parallel-audit",
        "children": [
          { "agent": { "id": "code-audit", "prompt": "Review code", "agent_type": "review" } },
          { "agent": { "id": "test-audit", "prompt": "Review tests", "agent_type": "verifier" } }
        ]
      }
    },
    { "reduce": { "id": "summary", "inputs": ["code-audit", "test-audit"], "prompt": "Summarize" } }
  ]
});
```

Supported node wrappers: `agent`, `branch`, `sequence`, `reduce`,
`teacher_review`, `loop_until`, `cond`, and `expand`. Raw `WorkflowNode` JSON IR
with `kind` / `spec` also remains valid.

An `agent` node may declare `"profile": "reviewer"` to run as a named fleet
roster profile. The name is trimmed and lowercased at compile time and must be
a single token (no whitespace, quotes, or `=`); the saved roster is resolved at
dispatch time, and explicit fields on the agent override profile defaults.

The runtime `task()` surface also accepts `cwd` for an existing repository-
relative working directory. This is required when a workflow is launched from
a multi-repository workspace and the child needs shell or file access. `cwd`
is validated by the host, does not grant mutation authority, and should be
paired with `worktree: true` when the child needs an isolated checkout.

The compiler rejects effectful constructs such as `import`, `require`, `fetch`,
`process`, `Deno`, `Bun`, `child_process`, file reads/writes, `eval`, `async`,
and `await`. This is intentionally stricter than JavaScript: workflow source is
a familiar declaration format, not a second execution runtime.

## Verification

- `cargo test -p codewhale-workflow --locked javascript`

Current example: `workflows/issue_audit.workflow.js`.

## Agent-Written fleet Workflows

The primary product flow is not "ask the user to write a script." The main
agent should decide when a task deserves workflow orchestration, draft the
Workflow source, show the plan for the current permission mode, and then let
the runtime compile and monitor it.

Workflow owns the plan: phases, branches, loops, reducers, and intermediate
results. fleet owns the durable roster, member identity, semantic role, and
saved provider/model pins or inheritance. Runtime owns tool posture, launch
concurrency, leases, heartbeats, logs, receipts, and resume/stop/restart
controls. In other words, a workflow can select fleet members and monitor their
Runtime runs, but it must not become a second executor with its own shell or
filesystem authority.

Workflow-to-Runtime launch validation applies a conservative default shape
before any Workflow IR is lowered to selected workers:

- up to 1,000 total worker agents per Workflow run;
- up to 16 live worker agents at once; larger populations queue (block) on the
  host's per-run concurrency gate until a live slot frees, then select through
  fleet and execute through Runtime;
- Workflow IR structural nesting no deeper than 5;
- Runtime child delegation defaults to 3 levels and has an opt-in hard ceiling
  of 8; that execution budget is independent of Workflow IR shape;
- loops require `max_iterations`;
- dynamic `expand` nodes require `max_children` and a template.

Those limits distinguish population from instantaneous launch concurrency. A
valid 1,000-agent Workflow can still drain through a smaller Runtime worker
pool. Model selection stays per member: a DeepSeek preset can suggest
`deepseek-v4-pro` for the orchestrator and `deepseek-v4-flash` for nearby
workers, but users and agents may override any slot when the task calls for it.

## Experimental search is a Workflow option

Experimental search generalizes the existing best-of-N recipe without adding a
new product mode, scheduler, or sub-agent API. A provider-neutral
`WorkflowSearchSpec` freezes the objective, baseline, model request and resolved
version, public evidence, evaluator hash, hard gates, scoring rule, budgets,
write scope, rounds, and review-only integration policy before admission.

The current JS starter supports structured generation and read-only review with
`strategy: "search"`. Runtime-owned command gates, hidden evaluation, benchmark
scoring, and clean-baseline replay are an explicit host seam still to wire; a
candidate's self-verdict must never be promoted into evaluator truth. See
[Workflow Experimental Search](WORKFLOW_EXPERIMENTAL_SEARCH.md).
