# The CodeWhale Agent Runtime — one durable substrate, familiar launchers

This document explains how sub-agents, the headless `exec` path, and Agent Fleet
relate. It exists because these had drifted into *two* parallel "worker"
systems, and the fix is to make the **fleet-backed worker run** the durable
primitive. "Sub-agent" remains useful product vocabulary for a nested role, but
it must not imply a separate execution substrate with weaker lifecycle
semantics. It also answers the open direction question in #2972 ("how much
Claude Code convergence is right?").

## The core idea

There is exactly **one** thing that runs detached agent work: a **headless agent
runtime** wrapped in a durable worker lifecycle. It is a model loop with the
full (policy-gated) tool surface that can, in turn, delegate child work through
the same lifecycle. Everything else is just a different way to *launch* that one
runtime, or a different way to *observe* it.

```
                         ┌───────────────────────────────┐
                         │     headless agent runtime     │
                         │  (full tools + can sub-spawn)  │
                         └───────────────────────────────┘
                            ▲             ▲             ▲
            launches │              │              │ launches
                     │              │              │
        ┌────────────┴───┐  ┌───────┴────────┐  ┌──┴───────────────────┐
        │   TUI turn     │  │ `codewhale     │  │   Agent Fleet         │
        │  (interactive, │  │   exec`        │  │  (durable: ledger,    │
        │   in-process)  │  │  (headless CLI,│  │   scheduler, SSH,     │
        │                │  │   anyone/any-  │  │   alerts) — launches   │
        │                │  │   time)        │  │   `codewhale exec`     │
        └────────────────┘  └────────────────┘  │   per worker          │
                                                 └───────────────────────┘
```

- A **sub-agent** is the user-facing name for a *nested assignment* with a role
  (`explore`, `review`, `implementer`, `verifier`, ...). It should be backed by
  the same worker run lifecycle as fleet. `agent` is the model-facing launcher,
  not a second runtime.
- **`codewhale exec`** is the headless front door: usable by anyone at any time
  (CI, scripts, another agent), full tools, emits a `stream-json` event stream,
  and can spawn sub-agents. It is *the* runtime with a CLI on it.
- A **fleet worker** *is* a `codewhale exec` run that the fleet launches and
  tracks durably — locally as a subprocess, or remotely as
  `ssh host … codewhale exec …`. The fleet does not re-implement execution; it
  adds **orchestration** (durable ledger, scheduling/leasing/retry, host
  transport, alert escalation) *over* the one runtime.

So "fleet vs sub-agent" is not two categories. It is **the same headless run**:
Fleet is the durable control plane, while sub-agent is the role/UX vocabulary
for a nested worker.

## The cutover rule

If a detached `agent` child can fail on a one-off provider timeout with no
retry while an equivalent fleet worker would retry and preserve ledger evidence,
then the cutover is incomplete. Treat that as a CodeWhale runtime gap, not as
normal "sub-agent behavior".

The compatibility `agent` runtime now retries transient provider header,
stream, and timeout failures with backoff before marking a worker interrupted;
when retries are exhausted it preserves a checkpoint and returns a continuation
handle. The remaining convergence work is to keep that lifecycle durable across
process restarts, remote execution, and full fleet-ledger scheduling.

The target rule is:

- durable or long-running work goes through the fleet worker lifecycle;
- `agent` should enqueue
  or observe a fleet-backed worker run instead of owning an independent
  lifecycle;
- in-process children are allowed only as a small compatibility/latency
  optimization, and they must expose the same terminal states, retry semantics,
  receipts, and inspection handles as the fleet path.

In product language it is fine to say "open a sub-agent". In architecture
language that means "start a nested fleet worker with this role".

## Why this shape (and why it fixes the lag)

The motivating problem: spawning many in-process sub-agents made the TUI lag,
because each child cloned a heavy runtime and rebuilt the whole tool registry,
*and* the TUI rendered a full card/transcript per child.

Surveying Claude Code, Codex, and Kimi, the thing that keeps an orchestrator
light at high fanout is **not** a process boundary — all three run sub-agents
in-process. It is **isolation + a compact event stream**:

- a child's transcript **never** flows back into the parent — the parent gets a
  result summary and a small lifecycle event stream;
- the UI renders **counts** (`2 running / 3 done`), not a child session per
  worker;
- each worker's tool surface is built directly from a **role/capability
  profile**, not "build everything then filter".

"Headless" therefore means *the execution is not shaped like the UI* — it does
**not** mean fewer abilities. A headless worker keeps the full toolset and can
spawn sub-agents.

When the work also needs to be **durable** (survive the TUI closing, a laptop
sleeping) or **remote** (SSH), the fleet runs the worker out-of-process as
`codewhale exec`. The heavy construction then lives in another process entirely,
so the orchestrator stays smooth regardless of fanout, and the run survives
restarts — the day-scale autonomy goal of #3154.

## One recursion axis

A worker runs at `spawn_depth = 0` and may spawn children while
`spawn_depth + 1 ≤ max_spawn_depth`, so a budget of `N` affords `N` nested
delegation levels. Sub-agents and fleet workers share **one** axis, sourced from
`codewhale_config`:

- `DEFAULT_SPAWN_DEPTH = 3` — the default budget for both standalone sub-agents
  and fleet workers (so they cannot drift into "two moving targets");
- `MAX_SPAWN_DEPTH_CEILING = 8` — the opt-in cap that every configured value
  (fleet `max_spawn_depth`, `agent`'s `max_depth`) clamps to.

The root worker always runs even at budget 0; the budget gates *child*
delegation. The default affords at least three nested levels.

## Event vocabulary

The fleet ledger persists the worker's own event stream rather than a separate,
simulated taxonomy. `codewhale exec --output-format stream-json` emits
`{"type": "content" | "tool_use" | "tool_result" | "workflow_event" |
"metadata" | "done" | "error"}` lines, which map onto the fleet ledger's
`FleetWorkerEventPayload` (`RunningTool`, `WorkflowEvent`, `Running`,
`Completed`, `Failed`, …). `workflow_event` carries the typed
run/phase/task/gate receipt while a Workflow is in flight and is retained as a
typed `WorkflowEvent` in the Fleet ledger; the enclosing worker still owns the
terminal `done` or `error`. One vocabulary, two surfaces.

## Convergence with Claude Code (#2972)

CodeWhale should converge with Claude Code on **shape**, not on branding:

- **Adopt**: a headless runtime with a real CLI/SDK front door; sub-agents as
  isolated runs that return summaries (not transcripts); a compact, event-driven
  fanout projection; capability/role tool profiles; the skills ecosystem
  (#2743); structured run receipts.
- **Keep distinct**: CodeWhale branding and first-class DeepSeek/GLM/MiniMax/
  multi-provider support; the local-first **Agent Fleet** (durable, SSH-capable
  orchestration) as CodeWhale's own layer above the shared runtime; Workflow as
  the orchestration overlay.
- **Do not** fork execution semantics per surface. The TUI, `agent`,
  `exec`, the Runtime API, and the fleet must all drive the *same* runtime and
  observe the *same* event stream — divergence there is what produced the "two
  moving targets" this document exists to prevent.

The litmus test for any new agent surface: *does it launch and observe the one
runtime, or does it invent a second one?* Only the former is allowed.

## What remains after v0.9.0

Refreshed 2026-07-15 from a full audit of the older 0.9-era documents. Those
plans are evidence, not a second source of truth. v0.9.0 consolidates the
underwater shell, message-first Operate, permission postures, the wired
Workflow engine and durable run journal, Lane CLI/runtime, setup with
`operate_ready`, constitution rebalance, and ProviderLake/Models.dev. The
remaining work belongs to later releases:

1. **Rebrand completion** — the only hard-dated obligations: remove the
   `deepseek`/`deepseek-tui` binary shims and shim release assets; finish the
   Homebrew `codewhale` formula rollout (`docs/REBRAND.md`).
2. **Operate as a value stream** — a control-board surface over the underwater
   shell (WIP, queue age, bottleneck); model-visible Work state (#3983); phase
   ledger (#4039); Workrooms Phase 2 (#3209/#3210) as the inbox substrate;
   receipt reconciliation.
3. **Flow control** — real WIP limits and visible queues (#4015, #4016),
   reconciled with the shipped 16-concurrent/1k-run access model (#4292).
4. **Fleet/Workflow convergence residuals** — live tmux/verifier-gate dogfood
   closing #4175/#4177/#4178/#4179; Fleet consuming canonical AgentProfiles;
   Conductor/topology (#4010, #4012) as stretch.
5. **TTC_DESIGN implementation** — approved and now unblocked after v0.9.0.
6. **HarnessProfile completion** — the status/UX display lane
   (`docs/rfcs/HARNESS_PROFILE_CUTLINE.md`).
7. **File decomposition, re-scoped** — `ui.rs` (~13.6k lines) and `main.rs`
   (~12.1k) are the current offenders (`docs/rfcs/FILE_DECOMPOSITION_0_9_0.md`).

Explicitly deferred by their own documents: external workflow memory (boundary
only), automatic harness evolution, hosted workrooms, `constitution_modules`
(needs sign-off), permission profiles (#3211, needs design), and plan-ceiling
probing (needs a product decision).

## Public launch contract for an external harness (#4641)

An external evaluation harness (for example a future Verifiers v1 built-in
harness) embeds CodeWhale by launching the public `codewhale exec` front door
against an interception endpoint it owns. CodeWhale owns only its **launch
contract**; the harness owns interception, traces, model-call timing, token
accounting, retries, rollout limits, and runtime orchestration. Do not add a
harness runtime, trace parser, or receipt schema to CodeWhale.

A reproducible headless launch uses only existing generic surfaces:

- an explicit temporary config that names the route and the credential
  **environment variable**, never the secret itself:

  ```toml
  provider = "openai"

  [providers.openai]
  base_url = ""            # the harness fills in its interception endpoint
  model = ""               # the harness fills in the target model
  api_key_env = "VF_CODEWHALE_API_KEY"
  ```

- `CODEWHALE_HOME` set to a fresh per-run directory;
- `CODEWHALE_SECRET_BACKEND=file`;
- `CODEWHALE_MCP_CONFIG` pointing to a generated per-run MCP JSON file that
  contains only the task servers the harness supplies
  (`{"mcpServers":{"task-tools":{"url":""}}}`; the `mcpServers` alias and
  URL-based Streamable HTTP / SSE transports already exist);
- `CODEWHALE_MEMORY=false` and `CODEWHALE_TELEMETRY=false`;
- `CODEWHALE_ALLOW_INSECURE_HTTP=1` **only** when the harness supplies a
  trusted `http://` interception endpoint (container/tunnel endpoints are not
  always loopback);
- `--append-system-prompt` and `--disallowed-tools` when the caller supplies
  them.

The interception secret stays in the child environment (resolved through the
route's `api_key_env`); it is never written into argv, the route config, logs,
the `stream-json` stream, or any generated file.

The exact argument order is:

```sh
codewhale \
  --config .vf-codewhale/config.toml \
  --workspace . \
  --no-project-config \
  --skip-onboarding \
  exec \
  --auto \
  --sandbox danger-full-access \
  --output-format stream-json \
  -- "<task prompt>"
```

`--no-project-config` must appear **before** the subcommand (like
`--skip-onboarding`). The public dispatcher parses it and forwards it ahead of
the TUI subcommand; `Exec` then skips the workspace-specific
`[workspace]`/`[projects]` user-config overlay so the config surface depends
only on the explicit `--config`. `crates/tui/tests/verifiers_harness_contract.rs`
is the provider-free acceptance lock for this contract.

### Future upstream checklist (out of scope here — do not run)

Actually adding CodeWhale as a built-in harness lives in the external Verifiers
repository, **after** a public, immutable CodeWhale `v0.9.1` GitHub Release and
checksum manifest exist. That upstream change is expected to be limited to a new
`verifiers/v1/harnesses/codewhale/` package plus its test-matrix and docs
registration, with `CodewhaleHarnessConfig` pinning `0.9.1`, `setup()`
downloading and verifying the released archive, and `launch()` writing the
temporary route/MCP files above and calling `runtime.run_program(...)`.

Holdouts, explicitly **not** performed by this contract work: tagging,
publishing, or creating a CodeWhale release; opening or submitting the upstream
Verifiers PR; running its credentialed E2E matrix; or claiming
runtime/architecture support before the exact released archive has run in that
upstream runtime.
