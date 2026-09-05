# Fleet workers and sub-agent compatibility

> 阅读简体中文版：[zh_hans/SUBAGENTS.md](zh_hans/SUBAGENTS.md)

Fleet roles are the user-facing vocabulary for delegated work: a parent
launches a focused `general`, `explore`, `planner`, `reviewer`, `implement`,
`test`, or `advisor` through `agent` and gets back an `agent_id` plus transcript handle
while the worker runs. The internal runtime type is `FleetRole` (formerly
`SubAgentType`); the older role spellings (`worker`, `scout`, `plan`,
`review`, `builder`, `verifier`, `consultant`, `oracle`, …) remain accepted only as a persisted/deserialize
compatibility adapter during v0.9.x. New prompts and config should use fleet
names.

Architecturally, sub-agents should not be a second execution substrate. The
durable primitive is the fleet-backed worker run described in
[`AGENT_RUNTIME.md`](AGENT_RUNTIME.md): retries, terminal status, receipts,
artifact refs, inspection, and restart behavior belong there. The
model-facing launcher is the single `agent` tool and detached work should
converge on the same lifecycle as Agent fleet.

The current `agent` implementation delegates to the durable sub-agent runtime
while that cutover completes. It can still be useful for short in-session
delegation. Transient provider header/stream/time-out failures are retried with
backoff inside the child runtime before the worker is marked interrupted; if the
retry budget is exhausted, Codewhale preserves a checkpoint and returns a
continuation handle instead of leaving the parent to infer what happened. For
work that must survive process restarts, sleep, or remote execution, prefer
fleet or a Workflow-backed fleet run.

Sub-agents inherit the parent's tool registry by default, and that includes
`agent` itself: children are built with `with_full_agent_surface_options`
(`crates/tui/src/tools/subagent/mod.rs:12164`) so they can recurse. `agent` is
filtered out of a child's catalog only when the depth budget is spent —
`can_spawn_child = !runtime.would_exceed_depth()` (`mod.rs:12145`), enforced at
`mod.rs:12324` and `:12469`. With the default depth of 3
(`DEFAULT_SPAWN_DEPTH`, `crates/config/src/lib.rs:1671`) a child can spawn
grandchildren. The removed `agent_open`/`agent_eval`/`agent_close` lifecycle
tools are gone from every registry, parent and child alike.

`agent` launches detached background work: cancelling the parent turn stops the
parent wait path, but it does not kill already-opened child runs.

This doc covers the role taxonomy and current compatibility controls. The active
orchestration surface is `agent`; see the sub-agent guidance in
`crates/tui/src/prompts/text.rs` (`AGENT_MODE`) and the in-line
tool description.

## Role taxonomy

The `type` field on `agent` selects a fleet posture for the child
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

| Role          | Stance                                 | Writes? | Network? | Shell posture | Typical use                                  |
|---------------|----------------------------------------|---------|----------|---------------|----------------------------------------------|
| `general`     | flexible; do whatever the parent says  | yes     | yes      | yes           | the default; multi-step tasks                |
| `explore`     | read-only; map the relevant code fast  | no      | yes      | read-only (net + bounded verify) | "find every call site of `Foo`; check the PR with gh" |
| `planner`     | analyse and produce a strategy         | no      | yes      | read-only probes | "design the migration; don't execute"        |
| `reviewer`    | read-and-grade with severity scores    | no      | yes      | read-only (net + bounded verify) | "audit this PR for bugs"                     |
| `implement`   | land a specific change with min edit   | yes     | yes      | yes           | "rewrite `bar.rs::Foo::bar` to do X"         |
| `test`        | run tests / validation, report outcome | no      | yes      | bounded verification (no writes) | "verify the diff with the bounded test checks; report PASS/FAIL" |
| `advisor`     | short-lived, high-reasoning counsel     | no      | yes      | none          | "what are we missing in this design?"        |
| `custom`      | explicit narrow tool allowlist         | inherits | inherits | inherits     | hand-picked tools on the parent's posture    |

A role's default is what the role *intends*, and the parent's effective
posture is always the ceiling (a child never widens beyond its parent).
Read-only roles withhold **workspace writes** by intent; nothing else is
taken away by default — every role keeps network reads, and `custom`
inherits the parent's write/network/shell posture and is narrowed only by
its explicit tool list or the spawning call. The focused worker's header
states the effective posture (`scout · read-only · network · read-only
shell`) from the runtime's own permission snapshot.

**Delegation moves work, never authority** (the containment answer for
#5426). A read-only role delegating to a write-capable role (scout →
builder) is a supported escape hatch for *work capacity* — the child brings
its own model, route, and step budget — but the child's authority is clamped
against the delegating parent's live posture, not the operator's: a scout's
builder child lands read-only with raw shell and mutating tools denied, and
canonical `Bash` is denied to it too (only bounded-inspection roles keep the
classified read-only shell). Delegating to obtain shell is therefore
mechanically useless — the scout's own bounded shell (`git -C … log`,
`find … | head`, `npm view …`, classifier-gated) is the only shell path a
read-only parent has. Read-only is transitive through any delegation chain:
the clamp (`ChildAuthority::clamp` in `fleet/exact.rs`) intersects every
field with the narrower side, the deny-list union means a descendant can
never drop an ancestor's restriction, and `inherit_disallowed_tools: false`
cannot drop a posture denial (`is_posture_denial`). This is pinned by
`a_read_only_parents_delegation_never_widens_authority` in
`crates/tui/src/fleet/exact.rs` tests.

The session's **permission posture** applies inside every child exactly as
it applies to the parent turn: under Auto-Review the same deterministic
floor and one-shot model guardian decide a worker's held calls (never a
prompt; an unavailable guardian denies, fail closed); under Ask a held call
the role cannot delegate is raised as an approval prompt in the parent's
UI and the worker waits visibly (`waiting for user`), or is denied with the
reason on hosts that cannot prompt; Full Access still fails closed on the
non-bypassable safety floor. Each decision nobody was prompted for is a
one-line note in that worker's transcript (visible when it is focused) and
an audit-log record. See `docs/MODES.md`.

Each role's full system prompt lives in
`crates/tui/src/tools/subagent/mod.rs` (search for
`*_AGENT_INTRO`). The prompt prefix loads automatically when the
child agent boots; the parent's assignment prompt becomes the first
turn's user message.

## Context forking

`agent` starts fresh by default: the child gets its role prompt plus the
task you pass. Use `fork_context: true` when the child should continue from
the parent's current request prefix instead. (`fork_context` is not in the
advertised v0.9.9 schema — it stays parse-accepted for compat callers, and
auto-forking for read-only roles continues unchanged.) In fork mode the runtime keeps the
parent prefill/prompt prefix byte-identical where available, appends a
structured state snapshot, then adds the sub-agent role instructions and task
at the tail. That preserves DeepSeek prefix-cache reuse while giving the child
the context needed for continuation, review, summarization, or compaction work.

Use fresh sessions for independent exploration. Use forked sessions when the
task depends on decisions, files, todos, or plan state already in the parent
transcript.

Forked state shows the parent's To-do snapshot — the sole Work surface, written
by `todo_write`. The child's `<codewhale:fork_state>` block carries the bounded
body rendered by `crates/tui/src/todo_snapshot.rs`, so a fork continues from the
parent's real progress position rather than a paraphrase. That To-do section is
resolved when the spawn happens, so a `todo_write` earlier in the same parent
turn is included.

**The list is shown once, at that spawn, and never re-sent.** No sub-agent
request re-states a To-do list, and neither does a parent request. Each agent
keeps its own private list (#4810); what it knows about that list comes from the
tool results its own `todo_write` calls returned, which are ordinary messages in
its own transcript. A worker therefore cannot read or write a parent's or a
sibling's list, and a forked child cannot mutate the snapshot it was handed or
keep reading later parent changes.

That same private list is what the child's in-transcript card shows. A
delegate card renders a bounded projection of **its own** agent's To-do — the
settled/total count, the in-progress item always included, up to three rows, and an
explicit `… +N more` when the bound elides the rest — built by
`card_todo_projection` from the same snapshot, priority order, and sanitizer the
model-facing body uses. A card only ever consumes an envelope whose `agent_id`
matches it, so a parent's list never appears under a child and no sibling's list
appears under another. An agent that has stated no work shows no To-do rows at
all rather than a placeholder task, and a terminal card keeps the last snapshot
its agent actually published. Fanout cards stay a dot grid and do not show child
To-do: with many workers behind one card there is no truthful place to hang a
single list. A child To-do appears only when the runtime already represents
that child as its own delegate card.

The durable Runtime ledger (projected through fleet task status) still owns
lifecycle state. `update_plan` is no
longer reachable by a model: `model_visible()` returns `false`
(`crates/tui/src/tools/plan.rs:408-413`), so it is filtered out of the API tool
list and never appears to a child. It survives only to replay older transcripts.
Strategy that used to go there now goes in the response body, and lifecycle
state goes in `todo_write`.

## Worktree isolation

For parallel edit lanes, launch the child with `worktree: true`. Codewhale
creates a fresh git worktree and branch for that child, runs the child from the
isolated checkout, and reports the resulting workspace/branch in the returned
session projection and worker record. By default the branch is
`codex/agent-<name>-<id>` and the checkout lives beside the parent repo under
`.codewhale-worktrees/`, so the parent checkout stays clean.

Isolation is not write authority. A prompt-only worker starts read-only.
A writer also declares `write_authority: "workspace_write"` or
`"worktree_write"` and at least one normalized repo-relative `write_roots`,
`exact_files`, or `coordination_contracts` value. Active overlapping shared
claims fail before mutation; a real isolated worktree may proceed in parallel.

Optional fields:

- `worktree_branch`: exact branch to create.
- `worktree_base`: git ref to branch from; defaults to `HEAD`.
- `worktree_path`: exact checkout path. Relative paths stay under the default
  sibling `.codewhale-worktrees/` root.

Do not combine `cwd` with `worktree`; `cwd` remains the manual escape hatch for
an already-created directory inside the parent workspace.

## Delegation briefs

The parent should pass a compact brief instead of a loose paragraph. Use the
structured `dependencies` and `acceptance` arrays for bounded prerequisite facts
and observable checks; keep the focused objective in `prompt`. Do not copy raw
parent reasoning or an unbounded transcript.

```
QUESTION:
SCOPE:
ALREADY_KNOWN:
EFFORT: quick | medium | thorough
STOP_CONDITION:
OUTPUT: VERDICT, EVIDENCE, GAPS, NEXT
```

`scout` briefs default to quick, read-only investigation (no writes, but
network reach and the bounded verification surface are available for real
scouting). About 3-5 tool calls
is enough for quick exploration: orient, search, read the decisive lines, and
return. Do not repeat `ALREADY_KNOWN` work unless evidence contradicts it. Review
and verifier briefs can spend more calls, but should stop after decisive
evidence. Builder and repair-style briefs should use checkpoints before
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
  to do next. Scouts are cheap and fast; open 2–3 in parallel
  for independent regions.
  They should orient first: confirm the project root, read relevant
  `AGENTS.md`/`README.md` guidance in unfamiliar trees, search only the
  likely scope, and return `path:line-range` evidence instead of a narrative
  tour. The role name to use is `explore`.
- **`planner`** — when the parent has an objective but no executable
  decomposition. Planners write artifacts (`todo_write` items,
  strategy in the response body) but don't carry them out.
- **`reviewer`** — when there's already a change and the parent wants
  it graded. Reviewers don't patch — they describe the fix in the
  finding so the parent can dispatch a builder if the verdict
  is "fix it".
- **`implement`** — when the change is already specified and just
  needs to land. Builders stay tightly scoped: minimum edit, no
  drive-by refactoring, run a quick verification before handing back.
- **`test`** — when the parent needs an authoritative pass/fail
  on the test suite or other validation. Verifiers don't fix
  failures; they capture the failing assertion + stack and put fix
  candidates under RISKS. The verifier posture never writes, and shell
  is clamped to the bounded built-in verification surface: the write
  ceiling is read-only and unbounded shell forms are refused (#5186).
- **`advisor`** — when the operator wants a high-leverage second opinion
  before cheaper execution continues. Consultants read enough to ground a
  recommendation, but cannot write or run shell commands. `oracle` and
  `advisor` remain accepted only when loading older requests or persisted
  records; new prompts, receipts, and UI use `advisor`.
- **`custom`** — only when the parent needs to constrain the tool
  set explicitly. Pass the allowlist via the `allowed_tools` field
  on legacy/internal sub-agent records; the model-facing `agent` tool keeps the
  public schema intentionally small.

### Aliases

The model can spell each role multiple ways:

| Canonical     | Aliases                                                          |
|---------------|------------------------------------------------------------------|
| `general`     | `worker`, `default`, `general-purpose`, `general_purpose`         |
| `explore`     | `scout`, `explorer`, `exploration`                               |
| `planner`     | `plan`, `planning`, `awaiter`                                    |
| `reviewer`    | `review`, `code-review`, `code_review`                           |
| `implement`   | `builder`, `implementer`, `implementation`                       |
| `test`        | `verifier`, `verify`, `verification`, `validator`, `tester`       |
| `advisor`     | `consultant`, `oracle` (compatibility input only)                 |
| `custom`      | (none; explicit `allowed_tools` array required)                  |

All matching is case-insensitive. Unknown values produce a typed
error listing the accepted set, so the model can self-correct on
the next turn.

## Concurrency cap

Up to **64** sub-agents run concurrently by default (`DEFAULT_MAX_SUBAGENTS`),
configurable via `[subagents].max_concurrent` in `~/.codewhale/config.toml` up to
the hard ceiling of **128** (`MAX_SUBAGENTS`). The session admits a bounded
queue of up to **1024** running plus queued sub-agents by default
(`MAX_SUBAGENT_ADMISSION`, `crates/tui/src/config/subagent_limits.rs:21`), so a turn can
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
# Operator-selected Runtime delegation depth. The default is 3; this explicit
# value opts in above the default but remains below the hard ceiling of 8.
max_depth = 6
# Omitted or zero model-step budget is unbounded. Set a positive value only
# when an operator deliberately wants a per-child cap.
default_max_steps = 0
default_wall_time_secs = 1800
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

## Advertised agent-tool fields (v0.9.9)

The model-facing `agent` tool schema advertises exactly **12 fields**
(#5324, #5123):

`action`, `prompt`, `type`, `profile`, `name`, `agent_id`, `message`,
`until`, `detached`, `worktree`, `write_roots`, `resume_from`

plus the action-discriminated `dependentSchemas` tree (`start` requires
`prompt`; `message`/`followup` require a target and `message`; `peek`/
`interrupt`/`cancel` require a target). The schema change is part of the
pinned prompt prefix, so upgrading re-fills the provider KV prefix once per
session (docs/CACHE.md; accepted at the v0.9.9 boundary).

**Parse-accepted but unadvertised (compat).** The following inputs were
removed from the advertised schema but remain accepted for saved transcripts,
ACP/MCP clients, fleet execution data, and internal/operator compatibility.
They are never advertised to the model; Runtime still validates, clamps, and
intersects them with live policy:

- budgets: `max_steps`, `wall_time_secs` (see
  [Child budgets](#child-budgets-steps-wall-time) for their defaults)
- delegation compatibility: `max_depth`, `maxDepth`, or `max_spawn_depth`;
  values are restricted to 0 through the Runtime hard ceiling of 8. New
  model-authored calls inherit the operator's `[subagents] max_depth` instead.
- routing: `model`, `model_strength`, `thinking` (a `profile` pins route and
  thinking tier; without one the child inherits the operator model)
- workspace/isolation: `workspace_policy`, `write_authority`, `fork_context`,
  `cwd`, `worktree_path`, `worktree_branch`, `worktree_base`
- spawn contract: `deliberate`, `dependencies`, `acceptance`,
  `expected_artifact`, `exact_files`, `coordination_contracts`
- lifecycle extras: `timeout_secs` (wait), `reason` (interrupt),
  `include_archived` (status), and `token_budget`

Authority never widens: the #5426/#5435 containment clamps are unchanged —
`write_authority` moves to roles/profiles, and delegation can only narrow
inherited authority.

## Child budgets (steps, wall time)

Per-child run budgets are no longer per-call schema fields (#5324). They come
from, in order:

1. an explicit parse-accepted `max_steps` / `wall_time_secs` on the call
   (replay compat),
2. the operator defaults `[subagents] default_max_steps` and
   `[subagents] default_wall_time_secs`,
3. fleet role defaults: **unbounded model turns** for every role
   (`WorkerRuntimeProfile::default_max_steps` returns zero), plus a **1800 s**
   wall-clock default.

Omitted or zero `max_steps` remains unbounded even when an operator default is
configured; positive step values clamp to the 2000-turn hard ceiling.
Wall-time values clamp to 1..=86400 s.

## Token budget governor

Set `[subagents].token_budget` to give each root `agent` run an aggregate
token ceiling shared by that child and all of its descendants. When no budget
is configured, behavior is unchanged.

`token_budget` is **not** a field on the model-facing `agent` schema, and its
absence is deliberate — `crates/tui/src/tools/subagent/tests.rs:4260-4263`
asserts it, on the grounds that "ad-hoc children should inherit the generous
runtime budget; exposing an optional cap invites accidental micromanagement."
The parser still accepts the key (plus the `tokenBudget`/`max_tokens` aliases,
`mod.rs:10620`) so Workflow-shaped callers that construct the call themselves
can scope a budget, but the model is never told the field exists. Configure it
through `[subagents].token_budget` instead. Since v0.9.9 it is one entry in
the wider parse-accepted-but-unadvertised compat list above.

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
worker_model   = "deepseek-v4-pro"     # worker
scout_model    = "deepseek-v4-flash"   # scout
planner_model  = "deepseek-v4-flash"   # planner
reviewer_model = "deepseek-v4-pro"     # reviewer
custom_model   = "deepseek-v4-pro"     # custom

[subagents.models]
# Free-form role → model map; any role alias accepted by agent works.
builder = "deepseek-v4-pro"
```

The v0.9.x convenience keys `explorer_model`, `awaiter_model`, and
`review_model` remain accepted as deprecated aliases so existing config files
do not break.

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
a child to a different provider, use a fleet/AgentProfile and pass it to the
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
children build a client for `lm-studio`; fleet workers forward
`--provider lm-studio` to `codewhale exec`, which resolves the same
`[providers.lm-studio]` table. Unknown or unconfigured provider ids fail the
spawn rather than silently falling back to the parent provider.

## Per-step API timeout (#1806, #1808)

Each sub-agent step wraps its DeepSeek `create_message` call in a
per-step timeout so a single stuck request can't pin the parent's
completion wakeup channel indefinitely. The default is `600` seconds.
A timed-out attempt is retried with exponential backoff (up to 5
retries) before the step interrupts with a preserved checkpoint.
Long-thinking children that legitimately exceed that, for example
heavy plan or review work behind `agent`, can extend the timeout in
`~/.codewhale/config.toml`:

```toml
[subagents]
api_timeout_secs = 900  # 15 minutes; clamped to 1..=3600
```

Values are clamped to `1..=3600`. `0` and `unset` keep the `600`
second default.

## Stale-agent heartbeat (#2614)

Running agents also track manager-visible progress. If a child stops emitting
progress for the heartbeat window, the manager auto-cancels it, releases its
sub-agent slot, and keeps the cancelled record inspectable through the returned
transcript handle and persisted worker record. The default is 5 minutes
(resolved to at least 30 seconds above `api_timeout_secs`, so 630 seconds
with the 600-second default API timeout):

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

Work-bar/status projections focus on current-session agents by default.
Prior-session agents that are not still running are treated as archived records
so the model does not mistake stale work for live work. This is a
*prior-session* rule only: agents that finished in the CURRENT session keep
their work-bar rows for the rest of the session (quiet completion), and their
details still open from those rows.

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

Non-scout sub-agents end with five Markdown headings, in this order:

```
### SUMMARY    one paragraph; what you did and what happened
### EVIDENCE   path:line-range citations and key findings; one bullet each
### CHANGES    files modified, with one-line descriptions; "None." if read-only
### RISKS      what could go wrong / what the parent should double-check
### BLOCKERS   what stopped you; "None." if you finished cleanly
```

They are `### HEADING` lines, not `HEADING:` labels, and `EVIDENCE` comes
before `CHANGES`. That five-heading contract is `SUBAGENT_OUTPUT_FORMAT` in
`crates/tui/src/prompts/text.rs`. `prompt_documents_structured_subagent_briefs`
in `crates/tui/src/prompts.rs` asserts every heading against it.

Scouts are the carve-out (#5189 F5): they end with `### SUMMARY` and
`### EVIDENCE` only (`SUBAGENT_SCOUT_OUTPUT_FORMAT` in
`crates/tui/src/prompts/text.rs`). `FleetRole::system_prompt` in
`crates/tui/src/tools/subagent/mod.rs` injects the scout contract for
`FleetRole::Scout` and the five-heading contract for every other role. A
subagent test pins that scouts contain `## Output contract (scout)` and do
not contain `### BLOCKERS`.

The parent reads `EVIDENCE` as a working set for the next turn, so
scouts and reviewers should be precise here.

## Memory and the `remember` tool (#489)

Sub-agents share the parent's native memory store when memory is enabled
(`[memory] enabled = true` or `DEEPSEEK_MEMORY=on`). They can
append durable notes via the `remember` tool — handy for a
scout that discovers a project convention worth carrying across
sessions, or a verifier that learns "this test is flaky".

`remember` takes a `scope` of `global` or `workspace`
(`crates/tui/src/tools/remember.rs:79-108`) and writes through
`NativeMemoryStore` to `~/.codewhale/memory/global/MEMORY.md` or
`~/.codewhale/memory/workspace/<id>/MEMORY.md`. Writes do not go through the
standard write-approval flow. The legacy single-file `memory.md` path was
removed in v0.9.4 (remember.rs:165); see `docs/MEMORY.md` for the full layout.

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
  read locks so `/agents` and the workbar projection don't block
  the main loop during multi-agent fan-out (#510).
