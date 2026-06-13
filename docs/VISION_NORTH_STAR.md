# CodeWhale North Star (0.9.0+)

> **STATUS: DIRECTION, NOT COMMITTED WORK.**
> Everything in this document is the maintainer's intended *direction* for
> CodeWhale 0.9.0 and beyond. **None of it is committed 0.8.53 work.** The
> 0.8.53 cycle ships **design docs only** for these areas — no tool-catalog code
> lands this cycle except the small, already-scoped subagent/git/RLM fixes in
> PR #2684 and PR #2685. Treat every "rough shape" below as a sketch to be
> refined, not an API contract. Where this doc names tools that do not exist yet
> (`codebase_search`, `read_file` as a canonical alias, `agent_run`, etc.) those
> are **aspirational names** that will *map onto today's tools*; see each
> section.

## Why this document exists

The vision is at risk of being lost between point releases. CodeWhale is
accumulating capability (subagents, RLM, skills, workflows, an enormous tool
catalog) faster than it is accumulating *shape*. This is the north star that the
incremental 0.8.x stabilization work is steering toward, written down once so it
survives the next dozen PRs.

### The one principle

**The harness handles memory, search, routing, state, and guardrails so a
weaker model can just *think*.** Every design decision below is in service of
moving cognitive load *out* of the model and *into* the harness. A
`deepseek-v4-flash`-class model should not have to remember ~80 tool names, hold
the codebase index in its head, track which layer of memory a fact lives in, or
re-derive a recovery path after a malformed tool call. The harness does that.
The model decides *what it wants*; the harness figures out *how*.

---

## Ground-truth anchor (today's reality)

So the direction is honest about where it starts:

- **Active first-turn tool set** is `DEFAULT_ACTIVE_NATIVE_TOOLS`
  (`crates/tui/src/core/engine/tool_catalog.rs:37-64`) — 26 tools. Everything
  else is **deferred** and hydrates via `tool_search_tool_regex` /
  `tool_search_tool_bm25` (`tool_catalog.rs:26-35`).
- **Catalog-head byte-stability is a hard invariant** for DeepSeek's KV
  prefix cache (`tool_catalog.rs:169-196`). The active first-turn tool block
  must stay byte-identical run-to-run; any change to it is a **one-time,
  deterministic edit**, never a per-turn or per-mode mutation.
- **Arcee** narrows the first turn to 8 read-only tools
  (`ARCEE_FIRST_TURN_NATIVE_TOOLS`, `tool_catalog.rs:106-115`) as a Cloudflare
  WAF workaround — proof the active partition is already provider-shaped.
- **Subagent tools that are model-visible:** only `agent_open`, `agent_eval`,
  `tool_agent`, `agent_close` (`crates/tui/src/tools/registry.rs:1017-1029`).
  All legacy names (`agent_spawn`, `spawn_agent`, `agent_result`, `agent_wait`,
  `agent_send_input`, `agent_assign`, `agent_list`, `agent_cancel`,
  `resume_agent`, `delegate_to_agent`, …) are `#[allow(dead_code)]` structs in
  `crates/tui/src/tools/subagent/mod.rs`, never instantiated outside tests →
  **already not model-visible**. The live internal `send_input` / `cancel` /
  `resume` methods on `SubAgentManager` (`mod.rs:1495,1521,1605`) back
  `agent_eval` / `agent_close` and **stay**.
- **`tool_agent` is "Fin"** — the experimental fast-lane executor: DeepSeek V4
  Flash with thinking forced off (`mod.rs:5233`, `TOOL_AGENT_INTRO`;
  `DEFAULT_CHILD_MODEL = "deepseek-v4-flash"`, `rlm.rs:26`).
- **Known duplicates today:** `exec_wait ≡ exec_shell_wait`,
  `exec_interact ≡ exec_shell_interact` (same structs, all four in the active
  set), `tts ≡ speech` (both deferred). `todo_*` are deferred twins of
  `checklist_*` (same `TodoWriteTool`, `::new` vs `::checklist`,
  `todo.rs:187,194`). The router already unifies `exec_wait`/`exec_shell_wait`
  (`crates/tui/src/tui/tool_routing.rs:1139-1140`).

This is the surface the north star refactors *toward simplicity*.

---

## 1. Intent Router

**What it is.** A thin layer where the model declares an **intent** —
*search / inspect / edit / test / delegate / ask-user / run-shell /
run-workflow* — and the harness maps that intent to the correct low-level tool
and arguments. The model picks from a tiny, stable verb vocabulary instead of
recalling ~80 concrete tool names and their schemas.

**Why it helps weaker models.** Tool-name recall is one of the largest sources
of wasted turns for small models: choosing a deferred tool (double-invoke),
choosing a deprecated alias, or hallucinating a name. A fixed intent vocabulary
collapses that decision space to ~10 verbs. The model spends its budget on
*reasoning about the task*, not on *remembering the API*.

**Rough shape.** A small **canonical visible set** — aspirational names that
route onto today's tools:

| Intent verb (aspirational) | Routes onto today |
|---|---|
| `codebase_search` | concept-level retrieval over the hybrid index (§2); today: `grep_files` + `file_search` + `project_map` |
| `read_file` | `read_file` (already canonical) |
| `apply_patch` | `apply_patch` (canonical; `edit_file`/`write_file`/`fim_edit` remain as distinct lower-level tools) |
| `run_tests` | `run_tests` / `run_verifiers` |
| `git_status` | `git_status` |
| `git_diff` | `git_diff` |
| `work_update` | `update_plan` / `checklist_write` |
| `ask_user` | `request_user_input` |
| `shell_run` | `exec_shell` (canonical; `exec_wait`/`exec_interact` hidden — §10) |
| `agent_run` | `agent_open` / `tool_agent` (gated, §3) / `agent_eval` / `agent_close` |
| `workflow_run` | WhaleFlow runner (§4) |

The router is the *only* place the catalog's full complexity is allowed to live.
It is also where **tool repair** (§7) hooks in: a mis-stated intent or a
deferred/deprecated name is rewritten to the canonical route.

**Dependencies.** The small canonical surface (§3), the lifecycle alias table
(§3 / `docs/TOOL_LIFECYCLE.md`), and the hybrid index for `codebase_search`
(§2). Must respect the **catalog-head byte-stability invariant**: the visible
verb set is itself a one-time deterministic edit, not a dynamic per-turn list.

---

## 2. Default Hybrid Codebase Intelligence

**What it is.** An always-on, local-first codebase index that ships with the
harness — not an opt-in tool the model has to remember to build. It fuses:

- plain **text** search,
- **symbol** index (definitions/references),
- **import / call graph**,
- **FTS5 + BM25** lexical ranking (rusqlite is already a dependency —
  `Cargo.toml`),
- **sparse** retrieval,
- optional **dense** (embedding) retrieval,
- **PR / commit / issue history** as a first-class retrieval source,
- a **codemap** (structural overview, the successor to today's deferred
  `project_map`).

**Why it helps weaker models.** Today the model must orchestrate `grep_files`
(content), `file_search` (filename), and `project_map` (structure) by hand,
reconcile their outputs, and re-run them as it narrows. There is **no FTS5/BM25
or semantic index today** — every search is a cold walk (`file_search` uses the
`ignore` crate's `WalkBuilder` for vendor exclusion, `file_search.rs:~210`). A
weaker model burns turns stitching partial results. A single `codebase_search`
intent backed by a hybrid index returns ranked, concept-level hits in one call,
so the model reasons about *answers*, not *query mechanics*.

**Rough shape.** A background indexer maintains a SQLite store (FTS5 + symbol +
graph tables), refreshed on file change and on git events. `codebase_search`
(§1) queries it; the codemap is regenerated incrementally. Vendor exclusion
reuses the existing `ignore`/`WalkBuilder` path.

**Dependencies.** rusqlite/FTS5; the Intent Router (§1) for the
`codebase_search` verb; the trace store (§6/§8) for history retrieval. **Full
design lives in `docs/rfcs/CODEBASE_SEARCH_DESIGN.md`** (to be written this cycle).

---

## 3. Small Canonical Tool Surface

**What it is.** A deliberately tiny set of always-visible canonical tools;
**everything else is hidden, deferred, or skill-scoped**. The catalog grows
behind the scenes but the *visible* surface stays small and stable.

**Why it helps weaker models.** Fewer choices, no aliases competing for the same
job, no deferred double-invokes for common operations. The model sees the verbs
it needs and nothing else.

**Rough shape — tool lifecycle states.** Five states, represented as **const
name-sets plus an alias table in `tool_catalog.rs`** (NOT a per-`ToolSpec`
field, to preserve the byte-stable head):

1. **active** — in the first-turn catalog head.
2. **deferred** — registered, hydrated via tool-search.
3. **hidden-compatibility** — registered + dispatchable, **dropped from both
   active and search**, identical behavior, **no notice**. (For exact
   duplicates that should simply disappear from discovery.)
4. **deprecated** — registered + dispatchable, **dropped from search**, appends
   a *replacement notice to RESULT METADATA only* — **never** to the cached
   prefix.
5. **removed** — final state; no longer registered.

**Invariant:** deprecated and hidden-compatibility tools **stay registered and
dispatchable forever** so old transcripts always replay deterministically.

**Planned diet (documented this cycle, not yet coded):**

- `exec_wait`, `exec_interact`, `tts` → **hidden-compatibility** (exact
  duplicates of `exec_shell_wait`, `exec_shell_interact`, `speech`).
- `todo_*` (`todo_write/add/update/list`) → **deprecated → checklist_*** (drop
  from tool-search, keep registered, add result-metadata notice).
- Legacy subagent names → already hidden; remaining work is **cleanup +
  guardrail tests**, rebased on PR #2684.

**Explicitly NOT touched** (distinct niches, per #2681 non-goals) — doc-only
canonical guidance, no diet: `apply_patch` / `edit_file` / `write_file` /
`fim_edit`; `grep_files` / `file_search` / `project_map`; `fetch_url` /
`web.run` / `web_search`; `task_shell_*`; `handle_read` /
`retrieve_tool_result`.

**`tool_agent` gating decision.** `tool_agent` ("Fin") **stays** as a canonical
subagent tool, but is **gated to DeepSeek-V4 models only**. It is the fast,
non-thinking executor lane built on `deepseek-v4-flash`; offering it to other
providers/models is meaningless (the lane *is* a specific model) and would just
add a name to recall. The gate is provider/model-conditional in the same spirit
as the Arcee first-turn narrowing.

**Dependencies.** The alias table backs the Intent Router (§1) and Tool Repair
(§7). **Full spec in `docs/TOOL_LIFECYCLE.md`** (to be written this cycle).

---

## 4. WhaleFlow / Workflow Mode

**What it is.** A typed, multi-agent **workflow runner**. A workflow is a graph
of typed nodes — **branches, leaves, reviewers, verifiers, test-runners,
PR-creators**, with **trace-replay** and a **progress-monitor**. Authors write
workflows in **Starlark or YAML**, which compile to a **typed Rust IR**; the
**Rust executor** runs the IR. "Like Claude's workflow mode, but safer" — the
safety comes from the typed IR and Rust execution boundary rather than free-form
model-driven orchestration.

**Why it helps weaker models.** Long-running, multi-step work (implement →
review → verify → test → open PR) is exactly where weaker models drift, lose
state, or skip verification. Encoding the *process* as a typed graph means the
model only has to be competent at each *leaf*, while the harness guarantees the
sequencing, the verification gates, and the evidence trail.

**Rough shape.** Starlark/YAML → typed IR → Rust executor. Nodes map to
subagent lanes (`agent_open` / `tool_agent` / `agent_eval` / `agent_close`,
`registry.rs:1017-1029`). Reviewer/verifier/test-runner nodes are first-class
node *types*, not ad-hoc prompts. Every run emits a trace (→ §8). Surfaced via
`/workflow` (alias `/whaleflow`) and the `workflow_run` intent (§1).

**Dependencies.** Subagent runtime; the evaluation loop (§8) for traces;
Skills & Rules (§5) so a skill can *define* a workflow; the command taxonomy
(§9).

---

## 5. Skills & Rules as First-Class Runtime

**What it is.** Skills and rules become real runtime objects, not just prompt
text. Skills gain **activation modes**:

- **always-on** — injected every turn,
- **glob** — activated when matching files are in scope,
- **model-decision** — offered to the model to opt into,
- **manual** — only via explicit `$<skill-name>` invocation (§9).

Skills can **restrict the tool surface**, **define workflows** (§4), and
**inject repo context**.

**Why it helps weaker models.** A skill scoped to a task can shrink the tool
surface to exactly what that task needs and pre-load the relevant rules and
context — so the model operates inside a curated, smaller world instead of the
full catalog.

**Rough shape (vs. today).** Today: skills are discovered
(`crates/tui/src/tools/skills/mod.rs`, `discover_in_workspace ~421`; struct
parses name/description `~382-388`), enable-state is tracked
(`skill_state.rs`, `SkillStateStore::is_enabled ~73`), and there's an
inline-mention popup (`slash_menu.rs ~86`). **But:** no parser activates inline
`$` mentions on submit (submit path: `ui.rs build_queued_message ~4721`), there
is **no activation-mode concept**, and **skills cannot restrict tools**. The
direction adds (a) a submit-time `$<skill-name>` activation parser, (b) the
four activation modes in skill metadata, and (c) a tool-restriction field
enforced by the registry/router.

**Dependencies.** Tool lifecycle/alias table (§3) for restriction; Intent Router
(§1); WhaleFlow (§4); command taxonomy (§9). **Full design in
`docs/SKILL_INVOCATION_DESIGN.md`** (to be written this cycle).

---

## 6. Context Memory Stack

**What it is.** Memory modeled as **explicit, layered, inspectable** stores
rather than one undifferentiated blob. Each layer is **visible, inspectable,
clearable, and scoped**:

1. **User memory** — small user prefs/facts (surfaced via `/memory`, §9).
2. **Repo rules** — checked-in guidance (`/rules`).
3. **Codemap-wiki** — derived structural/semantic knowledge of the repo (§2).
4. **Trace store** — recorded workflow/turn evidence (§8).
5. **ARMH–RLM memo** — the RLM kernel's in-session working memory
   (`rlm_open`/`rlm_eval`/`rlm_configure`/`rlm_close`/`rlm_session_objects`,
   `crates/tui/src/tools/rlm.rs`; `handle_read` retrieves var handles;
   `finalize`/`FINAL` is an *in-kernel Python function*, not a tool).
6. **Cached-main overlay** — promoted lessons from the cached main branch
   (`/overlay`, §9).
7. **External memory (Aleph)** — large local data via the `aleph` skill;
   see `docs/rfcs/WHALEFLOW_EXTERNAL_MEMORY.md` for the v0.9.0 cutline that keeps
   this optional, explicit, inspectable, and out of the default path.

**Why it helps weaker models.** The model never has to *guess* where a fact
should live or *re-derive* context it already established. Each layer has a
clear scope and a clear command to inspect/clear it, so stale context is
visible and removable rather than silently poisoning the prefix.

**Rough shape.** A `/context` dashboard (§9) renders all active layers and their
sizes; `/memory` manages the small user layer; `/overlay` manages promoted
lessons. The RLM layer already exists and is plumbed through `rlm.rs`.

**Dependencies.** Command taxonomy (§9); codebase intelligence (§2); evaluation
loop (§8) for promotion into the overlay.

---

## 7. Tool Repair & Autoload

**What it is.** When the model emits a wrong, deferred, deprecated, or
environment-blocked tool call, the harness **repairs** it instead of returning a
bare error — and **autoloads** what's needed.

**Why it helps weaker models.** Recovery from a malformed call is precisely
where weak models loop or give up. Turning every failure into an actionable,
schema-bearing correction keeps the model on-task.

**Rough shape — representative repairs:**

- **Wrong/legacy name** → *"you meant `agent_eval`; here's the schema"* (autoload
  the deferred tool's schema in the same turn).
- **Mode mismatch** → *"shell is unavailable in Plan mode — ask the user or
  switch modes"*.
- **Missing dependency** → *"this tool needs Node; Node is missing"*
  (dependency probe via `ExternalTool`, already imported in `tool_catalog.rs`).
- **Deprecated alias** → silently **routed to the canonical** tool, with the
  replacement notice in **result metadata only** (§3) — never the cached prefix.

**Dependencies.** The alias table + lifecycle states (§3); the Intent Router
(§1); dependency detection (`ExternalTool`). Builds on PR #2685's actionable
RLM/field errors and PR #2684's lifecycle signals — **must not contradict
either**.

---

## 8. Evaluation Loop

**What it is.** Every workflow run **leaves evidence**: the tests it ran, the
diffs it produced, the failures it hit, the searches it issued, the claims it
verified, and the PR outcome. A **teacher/student replay** turns *good* traces
into reusable **rules, skills, tests, and cached guidance**.

**Why it helps weaker models.** The system gets better at *this repo* over time
without the model getting smarter. Verified good traces become rules/skills the
weaker model can lean on next time, and become the source of the cached-main
overlay (§6).

**Rough shape.** Workflow nodes (§4) emit structured evidence into the trace
store (§6). A replay/distillation pass (teacher reviews student trace) promotes
high-value traces into: repo rules (`/rules`), skills (§5), regression tests,
and overlay guidance (`/overlay`). Verified-claim tracking ties into the
adversarial-verification posture already used elsewhere.

**Dependencies.** WhaleFlow (§4) for trace emission; trace store + overlay (§6);
Skills & Rules (§5) as promotion targets.

---

## 9. Command-Surface Taxonomy

**What it is.** One name = **one thing**. The command surface is split so each
prefix has a single, memorable responsibility:

| Surface | Responsibility |
|---|---|
| `/memory` | **Small** user prefs/facts only |
| `/context` | **Dashboard** of all active memory layers (§6) |
| `/rules` | Repo guidance |
| `.codewhale/constitution.json` | Repo constitution: checked-in **local law** |
| `/workflow` (`/whaleflow`) | Long-running multi-agent runs (§4) |
| `/overlay` | Promoted cached-main lessons (§6/§8) |
| `$<skill-name>` | Skill invocation — **the token *is* the skill name** |
| `codebase_search` | Concept-level code retrieval (§2) |

The repo constitution is not another memory bucket. It is the local-law layer in
a layered authority model:

```
base myth / global Constitution
  -> repo constitution (.codewhale/constitution.json)
  -> task packet
  -> runtime policy
```

At conflict time, the **current user request for the task remains above the repo
constitution**; the repo constitution supplies durable defaults and local law
only when the active task packet and runtime policy leave room. Runtime policy is
the compiled enforcement surface for the run, not a separate place for the model
to invent new rules.

**Why it helps weaker models (and users).** No overloaded command does five
jobs; the model/user never has to disambiguate *which* `/memory` behavior they
meant. `$systematic-debugging` self-documents what it invokes.

**`/memory` subcommand sketch:**

```
/memory add "<fact>"        # store a small pref/fact
/memory edit                # edit stored facts
/memory search <query>      # find a stored fact
/memory clear               # clear user memory
/memory doctor              # health check; detects legacy ~/.deepseek path
/memory promote <fact>      # (later) promote a fact to a higher layer
```

`doctor` specifically detects the **legacy `~/.deepseek`** path and guides
migration.

**`$<skill-name>` invocation examples:**

```
$systematic-debugging       # local skill
$github:gh-fix-ci           # namespaced skill
```

The submit-time parser (to be added; submit path `ui.rs ~4721`) recognizes the
`$` token and activates the named skill (§5).

**`/context` layers dashboard (example render):**

```
/context
  user-memory      ▸ 7 facts                 (12 KB)   [clear]
  repo-constitution ▸ .codewhale/constitution.json (4 KB) [view]
  repo-rules       ▸ CLAUDE.md, AGENTS.md     (8 KB)   [view]
  codemap-wiki     ▸ 412 symbols indexed     (auto)    [rebuild]
  trace-store      ▸ 3 recent workflow runs  (—)       [open]
  rlm-memo         ▸ 0 active sessions        (—)       [—]
  cached-overlay   ▸ 5 promoted lessons       (3 KB)   [view]
  aleph-external   ▸ not attached             (—)       [attach]
```

**Dependencies.** Memory stack (§6); skills (§5); codebase intelligence (§2);
workflow runner (§4).

---

## 10. Deferred-Not-Done 0.8.53 Diet Items

Recorded here so they are **not silently dropped** — these were considered for
the 0.8.53 diet and deliberately **deferred** (design-only or out of scope this
cycle):

- **File-mutation overload** — `apply_patch` / `edit_file` / `write_file` /
  `fim_edit` overlap in purpose. Per #2681 non-goals these stay distinct;
  canonical *guidance* (prefer `apply_patch`) is doc-only, no consolidation
  this cycle.
- **`task_shell_*` ↔ `exec_*` redundancy** — `task_shell_start` /
  `task_shell_wait` overlap conceptually with the `exec_*` family. Left intact
  this cycle (distinct niche per #2681); revisit under §1/§3.
- **`handle_read` / `retrieve_tool_result`** — result-handle plumbing kept as-is
  (doc-only canonical guidance); folds naturally into the memory stack (§6) and
  intent routing (§1) later.
- **Search-cluster consolidation** — `grep_files` / `file_search` /
  `project_map` remain three tools this cycle; consolidation is the *job of the
  hybrid index* (§2) under `codebase_search`, not a catalog edit in 0.8.53.

---

## Phased Roadmap

### 0.8.53 — design + small fixes only
- **Code:** only the already-scoped, narrow fixes — PR #2684 (subagent role
  vocab, lifecycle signals, eval ergonomics) and PR #2685 (read-only git history
  active + actionable RLM/field errors). Subagent legacy-name cleanup +
  guardrail tests rebased on #2684.
- **Docs:** this north star, plus `docs/TOOL_LIFECYCLE.md`,
  `docs/rfcs/CODEBASE_SEARCH_DESIGN.md`, `docs/SKILL_INVOCATION_DESIGN.md`.
- **No tool-catalog code:** the diet (§3), the Intent Router (§1), and the
  hybrid index (§2) are **documented, not coded** this cycle.

### 0.9.0 — first structural moves
- Implement the **tool lifecycle** const name-sets + alias table in
  `tool_catalog.rs` (§3) as a one-time deterministic head edit.
- Land the **planned diet**: `exec_wait`/`exec_interact`/`tts` →
  hidden-compatibility; `todo_*` → deprecated→`checklist_*` (result-metadata
  notice only).
- Gate **`tool_agent`** to DeepSeek-V4 models only (§3).
- First version of the **default hybrid codebase index** (FTS5/BM25 + symbol +
  codemap) behind `codebase_search` (§2).
- First **Intent Router** verbs mapping onto today's tools (§1).
- **Tool Repair** for deferred/deprecated/mode/dependency cases (§7).

### Later (post-0.9.0)
- **WhaleFlow** typed-IR workflow runner (§4) and the **evaluation loop** /
  teacher-student replay (§8).
- **Skills activation modes** + tool restriction + `$<skill-name>` submit-time
  activation (§5).
- Full **Context Memory Stack** with `/context` dashboard, `/overlay`
  promotion, and Aleph external memory (§6).
- Dense/semantic retrieval and PR/commit/issue history in the index (§2).
- Search-cluster consolidation and the remaining §10 deferred items.

---

## North-star one-liner

> **The harness handles memory, search, routing, state, and guardrails — so a
> weaker model can just think.**
