# Sub-Agents

Sub-agents are the user-facing vocabulary for nested worker assignments: a
parent launches a focused role (`explore`, `review`, `implementer`, `verifier`,
...) through `agent` and gets back an `agent_id` plus transcript handle while
the worker runs.

Architecturally, sub-agents should not be a second execution substrate. The
durable primitive is the fleet-backed worker run described in
[`AGENT_RUNTIME.md`](AGENT_RUNTIME.md): retries, terminal status, receipts,
artifact refs, inspection, and restart behavior belong there. The
model-facing launcher is the single `agent` tool and detached work should
converge on the same lifecycle as Agent Fleet.

The current `agent` implementation delegates to the durable sub-agent runtime
while that cutover completes. It can still be useful for short in-session
delegation. Transient provider header/stream/time-out failures are retried with
backoff inside the child runtime before the worker is marked interrupted; if the
retry budget is exhausted, Codewhale preserves a checkpoint and returns a
continuation handle instead of leaving the parent to infer what happened. For
work that must survive process restarts, sleep, or remote execution, prefer
Fleet or a Workflow-backed fleet run.

Sub-agents inherit the parent's tool registry by default, but child agents are
leaf workers: they do not receive `agent` or nested lifecycle tools. `agent`
launches detached background work: cancelling the parent turn stops the parent
wait path, but it does not kill already-opened child runs.

This doc covers the role taxonomy and current compatibility controls. The active
orchestration surface is `agent`; see
`crates/tui/src/prompts/constitution.md` "Sub-Agent Strategy" and the in-line
tool description.

## Role taxonomy

The `type` field on `agent` selects a system-prompt posture for the child
(`agent_type` is accepted as a compatibility alias). Each role is a distinct
stance toward the work — not just a different label.

## Maintainer posture

Sub-agents help Codewhale move faster, but the parent agent still owns the
maintainer decision. Use children to gather evidence, review patches, and run
verification while keeping the community posture in
[`AGENT_ETHOS.md`](AGENT_ETHOS.md): issues are open intake, PR gates are
review-load controls, and harvested work needs clear contributor credit.

When a child reviews community work, the parent should still inspect the PR
diff, linked issues, tests, and CI before merging, harvesting, closing, or
deferring it. A sub-agent's result is a working set, not a substitute for
stewardship.

| Role          | Stance                                 | Writes? | Shell posture | Typical use                                  |
|---------------|----------------------------------------|---------|---------------|----------------------------------------------|
| `general`     | flexible; do whatever the parent says  | yes     | yes           | the default; multi-step tasks                |
| `explore`     | read-only; map the relevant code fast  | no      | read-only     | "find every call site of `Foo`"              |
| `plan`        | analyse and produce a strategy         | minimal | minimal       | "design the migration; don't execute"        |
| `review`      | read-and-grade with severity scores    | no      | read-only     | "audit this PR for bugs"                     |
| `implementer` | land a specific change with min edit   | yes     | yes           | "rewrite `bar.rs::Foo::bar` to do X"         |
| `verifier`    | run tests / validation, report outcome | no      | test-focused  | "run cargo test --workspace, report"         |
| `custom`      | explicit narrow tool allowlist         | depends | depends       | locked-down dispatch with hand-picked tools  |

Each role's full system prompt lives in
`crates/tui/src/tools/subagent/mod.rs` (search for
`*_AGENT_PROMPT`). The prompt prefix loads automatically when the
child agent boots; the parent's assignment prompt becomes the first
turn's user message.

## Context forking

`agent` starts fresh by default: the child gets its role prompt plus the
task you pass. Use `fork_context: true` when the child should continue from
the parent's current request prefix instead. In fork mode the runtime keeps the
parent prefill/prompt prefix byte-identical where available, appends a
structured state snapshot, then adds the sub-agent role instructions and task
at the tail. That preserves DeepSeek prefix-cache reuse while giving the child
the context needed for continuation, review, summarization, or compaction work.

Use fresh sessions for independent exploration. Use forked sessions when the
task depends on decisions, files, todos, or plan state already in the parent
transcript.

Forked state renders concrete Work progress through the canonical
`checklist_*` surface. The durable task/Fleet ledger owns lifecycle state;
checklist entries are the model-visible progress projection. Use `update_plan`
only for strategy metadata that helps a parent or later worker understand the
approach.

## Worktree isolation

For parallel edit lanes, launch the child with `worktree: true`. Codewhale
creates a fresh git worktree and branch for that child, runs the child from the
isolated checkout, and reports the resulting workspace/branch in the returned
session projection and worker record. By default the branch is
`codex/agent-<name>-<id>` and the checkout lives beside the parent repo under
`.codewhale-worktrees/`, so the parent checkout stays clean.

Optional fields:

- `worktree_branch`: exact branch to create.
- `worktree_base`: git ref to branch from; defaults to `HEAD`.
- `worktree_path`: exact checkout path. Relative paths stay under the default
  sibling `.codewhale-worktrees/` root.

Do not combine `cwd` with `worktree`; `cwd` remains the manual escape hatch for
an already-created directory inside the parent workspace.

## Delegation briefs

The parent should pass a compact brief instead of a loose paragraph. The current
model-facing `agent` tool still accepts a single `prompt` string, so put these
fields in that string:

```
QUESTION:
SCOPE:
ALREADY_KNOWN:
EFFORT: quick | medium | thorough
STOP_CONDITION:
OUTPUT: VERDICT, EVIDENCE, GAPS, NEXT
```

`explore` briefs default to quick, read-only investigation. About 3-5 tool calls
is enough for quick exploration: orient, search, read the decisive lines, and
return. Do not repeat `ALREADY_KNOWN` work unless evidence contradicts it. Review
and verifier briefs can spend more calls, but should stop after decisive
evidence. Implementer and repair-style briefs should use checkpoints before
scope expansion or after repeated failures rather than a tiny call cap.

Good delegation prompt examples:

```text
QUESTION: Does PR #3124 introduce release-risk behavior around provider routing?
SCOPE: PR #3124 diff, linked issue, provider routing tests, docs/PROVIDERS.md.
ALREADY_KNOWN: Branch is hunter/0.8.62-glm-subagents; workspace version stays 0.8.61.
EFFORT: medium
STOP_CONDITION: Return once you have either one BLOCKER/MAJOR issue or enough evidence for no MAJOR+ issues.
OUTPUT: VERDICT, EVIDENCE with file:line refs or PR refs, GAPS, NEXT.
```

```text
QUESTION: Where is the child-agent prompt assembled?
SCOPE: crates/tui/src/prompts*, crates/tui/src/tools/subagent/*.
ALREADY_KNOWN: The model-facing launcher is only `agent`; do not look for removed lifecycle tools.
EFFORT: quick
STOP_CONDITION: Stop after identifying the prompt source files and the function that wraps assignment text.
OUTPUT: VERDICT, EVIDENCE, GAPS, NEXT.
```

```text
QUESTION: Is the focused prompt/subagent test filter valid, and what fails if not?
SCOPE: cargo test -p codewhale-tui --bin codewhale-tui --locked prompt; subagent filter if needed.
ALREADY_KNOWN: Do not fix failures; capture exact command, exit code, and first relevant assertion.
EFFORT: medium
STOP_CONDITION: Stop after one clean PASS or one reproducible failing assertion with command evidence.
OUTPUT: VERDICT, EVIDENCE, GAPS, NEXT.
```

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
  on legacy/internal sub-agent records; the model-facing `agent` tool keeps the
  public schema intentionally small.

### Aliases

The model can spell each role multiple ways:

| Canonical     | Aliases                                                          |
|---------------|------------------------------------------------------------------|
| `general`     | `worker`, `default`, `general-purpose`                           |
| `explore`     | `explorer`, `exploration`                                        |
| `plan`        | `planning`, `planner`, `awaiter`                                 |
| `review`      | `reviewer`, `code-review`, `code_review`                         |
| `implementer` | `implement`, `implementation`, `builder`                         |
| `verifier`    | `verify`, `verification`, `validator`, `tester`                  |
| `custom`      | (none; explicit `allowed_tools` array required)                  |

All matching is case-insensitive. Unknown values produce a typed
error listing the accepted set, so the model can self-correct on
the next turn.

## Concurrency cap

Up to **64** sub-agents run concurrently by default (`DEFAULT_MAX_SUBAGENTS`),
configurable via `[subagents].max_concurrent` in `~/.codewhale/config.toml` up to
the hard ceiling of **128** (`MAX_SUBAGENTS`). The session admits a bounded
queue of up to **200** running plus queued sub-agents by default, so a turn can
request broad fan-out and let the manager drain it without creating an
unbounded population.

By default every admitted child may start immediately — there is no artificial
throttle. If you want gentler fan-out, lower `[subagents].launch_concurrency`
(how many direct children start at once); children beyond that limit **queue**
for a launch slot rather than bursting. `launch_concurrency` defaults to the
resolved `max_subagents` cap. (The pre-v0.8.61 `interactive_max_launch` key is
still accepted as a deprecated alias; the new key wins when both are set.)

High-fanout Workflows can tune that bounded population with `[subagents]
max_admitted` (aliases: `max_total`, `admission_limit`). That total ceiling
counts both **running** and **queued** agents, while `launch_concurrency` keeps
instantaneous execution bounded. Completed / failed / cancelled records persist
for inspection but don't occupy an admission slot. Agents that lost their
`task_handle` (e.g. across a process restart) also don't count against the cap.

Provider profiles let one config stay aggressive for direct API routes while
keeping subscription or aggregator routes gentle. Every key under
`[subagents.providers.<provider>]` inherits from `[subagents]` when omitted.
Provider keys accept canonical names such as `deepseek`, `zai`, `openrouter`,
and aliases such as `glm` for Z.ai:

```toml
[subagents]
# Global fallback for providers without a profile.
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200
max_depth = 6
token_budget = 100000

[subagents.providers.deepseek]
# Direct API key with room to fan out.
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200

[subagents.providers.glm]
# Z.ai / GLM subscription-style route: keep pressure tight.
max_concurrent = 4
launch_concurrency = 3
max_admitted = 12
max_depth = 2
api_timeout_secs = 180
heartbeat_timeout_secs = 240

[subagents.providers.openrouter]
max_concurrent = 5
launch_concurrency = 3
max_admitted = 20

[subagents.providers.anthropic]
max_concurrent = 3
launch_concurrency = 2
max_admitted = 12
```

Use `/config subagents status` to see both the global values and the active
provider's resolved fanout, depth, and timeout profile.

## Token budget governor

Set `[subagents].token_budget` to give each root `agent` run an aggregate
token ceiling shared by that child and all of its descendants. A child can also
start a new scoped budget with the model-facing `agent` tool's
`token_budget` field (the `max_tokens` alias is accepted for Workflow-shaped
callers). When no budget is configured or supplied, behavior is unchanged.

Provider-reported input and output tokens are folded into the worker record as
each child model call completes. The persisted `usage` object shows the
worker's own totals plus aggregate `budget_spent_tokens` and
`budget_remaining_tokens` for the shared scope. Once the shared scope is
exhausted, further descendant spawns are rejected with an actionable message
instead of opening more agents into a spent pool.

## Per-role models (#3018)

Children can run on a different model than the parent. Two config surfaces
feed the same override map (`[subagents.models]` keys win on conflict, keys
are case-insensitive):

```toml
[subagents]
default_model  = "deepseek-v4-flash"   # fallback for every role
worker_model   = "deepseek-v4-pro"     # worker / general
explorer_model = "deepseek-v4-flash"   # explorer / explore
awaiter_model  = "deepseek-v4-flash"   # awaiter / plan
review_model   = "deepseek-v4-pro"     # review
custom_model   = "deepseek-v4-pro"     # custom

[subagents.models]
# Free-form role → model map; any role alias accepted by agent works.
implementation = "deepseek-v4-pro"
```

Model ids may be **any model the active provider accepts** — validation is
provider-aware and happens at spawn time, not load time. On the official
DeepSeek API only DeepSeek ids are accepted; every other provider passes the
id through to the provider API, which is the authority. A non-DeepSeek
example:

```toml
provider = "moonshot"
model = "kimi-k2.7-code"

[subagents]
worker_model = "kimi-k2.6"
```

Model ids are validated the same way when applied to a child route; an invalid
id on the official DeepSeek API fails the spawn with the accepted-id list
instead of an opaque provider 400.

With `/model auto`, sub-agent routing is provider-aware too: providers with a
known big/cheap pair (DeepSeek, and the hosted DeepSeek routes on NVIDIA NIM,
OpenRouter, Novita, SiliconFlow, SGLang, vLLM) route between that pair;
providers without a known cheap tier (e.g. Ollama, Moonshot) skip the
network router and keep children on the session model.

## Per-profile provider routes (#3965)

`[subagents.models]` changes the child model within the active provider. To pin
a child to a different provider, use a Fleet/AgentProfile and pass it to the
model-facing `agent` tool with `profile`. The profile's explicit `provider` +
`model` fields win over the parent session route; omitting `provider` preserves
the existing inherit behavior.

Example: keep the parent session on DeepSeek, but run a formatter child on a
local LM Studio OpenAI-compatible endpoint:

```toml
# ~/.codewhale/config.toml or workspace config
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_DEEPSEEK_KEY"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
api_key = "lm-studio"
model = "qwen-2.5-7b"
```

```toml
# .codewhale/agents/local-formatter.toml
id = "local-formatter"
role_hint = "formatter"
provider = "lm-studio"
model = "qwen-2.5-7b"
reasoning_effort = "off"

[instructions]
text = "Use small, local edits. Keep formatting changes mechanical."
```

Then call `agent(profile: "local-formatter", prompt: "...")`. In-process
children build a client for `lm-studio`; Fleet workers forward
`--provider lm-studio` to `codewhale exec`, which resolves the same
`[providers.lm-studio]` table. Unknown or unconfigured provider ids fail the
spawn rather than silently falling back to the parent provider.

## Per-step API timeout (#1806, #1808)

Each sub-agent step wraps its DeepSeek `create_message` call in a
per-step timeout so a single stuck request can't pin the parent's
completion wakeup channel indefinitely. The default is `120` seconds,
which matches the legacy hardcoded value. Long-thinking children that
legitimately exceed that, for example heavy plan or review work behind
`agent`, can extend the timeout in `~/.codewhale/config.toml`:

```toml
[subagents]
api_timeout_secs = 900  # 15 minutes; clamped to 1..=1800
```

Values are clamped to `1..=1800`. `0` and `unset` keep the legacy
`120` second default, so existing installs see no behavior change.

## Stale-agent heartbeat (#2614)

Running agents also track manager-visible progress. If a child stops emitting
progress for the heartbeat window, the manager auto-cancels it, releases its
sub-agent slot, and keeps the cancelled record inspectable through the returned
transcript handle and persisted worker record. The default is 5 minutes:

```toml
[subagents]
heartbeat_timeout_secs = 300  # clamped to 30..=3600
```

The effective heartbeat is kept at least 30 seconds above
`api_timeout_secs`, so a configured long model request is not cancelled before
its own request timeout can fire.

## Lifecycle

Each opened session produces a record that progresses through:

```
Pending → Running → (Completed | Failed(reason) | Cancelled | Interrupted(reason))
```

`Interrupted` fires when the manager detects a `Running` agent whose task
handle is gone — typically after a process restart that loaded the workspace's
persisted state from `.codewhale/state/subagents.v1.json`. The parent can open a
replacement session with the same assignment or treat it as a terminal state.

### Session boundaries (#405)

Each `SubAgentManager` instance assigns itself a fresh `session_boot_id` on
construction. Every new session stamps the agent with that id; the workspace
state file records it for restart recovery.

Sidebar/status projections focus on current-session agents by default.
Prior-session agents that are not still running are treated as archived records
so the model does not mistake stale work for live work.

Records that loaded from a pre-#405 persisted state file (no
`session_boot_id` field) classify as prior-session because the
manager can't match them to the current boot.

## Run receipts, follow-up, and takeover

Each compatibility sub-agent has a persisted worker record in
`.codewhale/state/subagents.v1.json`. The record is the current run-ledger
slice for sub-agent lanes until those lanes are backed directly by the fleet
ledger: it stores `run_id`, objective, role/model,
workspace/branch, lifecycle events, artifact refs, follow-up target, takeover
target, usage provenance, and verification provenance.

`agent` returns a session projection with these fields at the top level and
inside `worker_record`. The normal parent contract is not polling: keep working
and consume the completion event when the child finishes. If audit detail is
needed, inspect the returned `transcript_handle` with `handle_read`.

Legacy follow-up delivery is retained only for old transcripts and internal
recovery. If a message was delivered, the worker record stores a bounded preview
and timestamp. New model-facing flows should open a replacement `agent` when a
child's assignment no longer fits.

Artifacts are symbolic refs. Use `handle_read` on the returned
`transcript_handle` for transcript details, and treat `result_summary` as a
child self-report unless `verification.status` points to a separate gate or
receipt. `usage.status` is `unknown` until provider usage is reported; then it
switches to `reported`, or `budget_exhausted` when a configured shared token
budget has no remaining tokens.

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

- Source: `crates/tui/src/tools/subagent/mod.rs`.
- Persisted state: `<workspace>/.codewhale/state/subagents.v1.json`. Schema
  version `1` (forward-compatible — new optional fields use
  `#[serde(default)]`).
- Worker records are pruned by time: completed / failed / cancelled /
  interrupted records are evicted after the same retention window used for
  finished agents (default 1h, `COMPLETED_AGENT_RETENTION`). Running /
  starting / waiting records are preserved. The hard cap of 256 records
  remains as a safety bound (#4217).
- `SubAgentRuntime::background_runtime()` starts from `child_runtime()` but
  replaces the turn-scoped child token with a fresh cancellation token, so
  parent turn cancellation does not stop detached background sessions.
- The `is_running` check ignores agents whose `task_handle` is
  `None`; this avoids counting persisted-but-detached records
  toward the concurrency cap (#509).
- `SharedSubAgentManager` is `Arc<RwLock<...>>` — read paths use
  read locks so `/agents` and the sidebar projection don't block
  the main loop during multi-agent fan-out (#510).
