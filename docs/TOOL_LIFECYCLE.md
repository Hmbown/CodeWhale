# Tool-Surface Lifecycle Policy (v0.8.53)

**Status:** Design doc / policy. No catalog code lands in this cycle — the code
work is **deferred**. This document is the umbrella policy for GitHub **#2681**,
with **#2682** and **#2683** as concrete instances of the planned diet. It
describes *what will be done* and the invariants any future diet PR must hold.

**Scope of related open work (do not contradict):**
- PR **#2684** — subagent role vocabulary, lifecycle signals, eval ergonomics.
  Legacy subagent-name cleanup + guardrail tests in this policy rebase on #2684.
- PR **#2685** — git-history active + RLM/field errors.

All file:line citations are against the verified tree at the current CodeWhale
checkout as of v0.8.52/0.8.53.

---

## 1. Purpose and the weaker-model problem

CodeWhale ships a large native tool surface. The first-turn *active* partition
of that surface is what every model sees before it has run a single
`tool_search_*` call. Today that active set contains several **near-duplicate
tools** that map to the *same* implementation under different names:

- `exec_wait` and `exec_shell_wait` are both `ShellWaitTool`
  (`crates/tui/src/tools/registry.rs:526,529`).
- `exec_interact` and `exec_shell_interact` are both `ShellInteractTool`
  (`registry.rs:527,530`).
- `tts` and `speech` are both `SpeechTool`
  (`registry.rs:787-792`, both deferred).
- `work_update`, `checklist_*`, and `todo_*` are the *same*
  `TodoWriteTool` surface, with only `work_update` visible to models.

For a strong model, redundant names are harmless noise. For **weaker / smaller
models** (the Arcee Trinity lane, `deepseek-v4-flash` child executors, and any
non-thinking executor), every additional near-duplicate in the visible set is a
real cost:

- It widens the choice space with options that do *nothing distinct*, increasing
  wrong-tool selection and oscillation between synonyms.
- It spends scarce first-turn catalog budget (Section 5) on zero-information
  entries.
- It dilutes the "one name = one thing" contract that lets a small model reason
  about the surface at all.

The lifecycle policy exists to **shrink and discipline the model-visible
surface** without ever breaking the ability to replay an old transcript that
referenced a now-retired name.

### Canonical work-tracking surface for v0.8.68

The model-visible progress surface is a single tool: `work_update` (#4132).
Agents and Fleet workers use it for concrete To-do / Work progress under the
active runtime thread or durable task.

`task_*` and the Fleet/Workflow ledger remain the durable lifecycle owners.
Checklist metadata is the model-visible projection of progress:
`task_updates.checklist` carries the current items, completion percentage, and
in-progress item. `update_plan` is optional Strategy metadata/context/route for
complex initiatives; it must not duplicate To-do items or become a parallel
progress store.

The legacy `checklist_*` and older `todo_*` names are hidden compatibility
aliases. They remain registered and dispatchable against the same To-do state
so old transcripts replay without data loss, but they are not advertised to the
model catalog.

---

## 2. The five lifecycle states

Every native tool name occupies exactly one lifecycle state.

| State | Meaning | Visible on first turn? | In `tool_search_*`? | Executes if called? | When used |
|---|---|---|---|---|---|
| **active** | Canonical, in the first-turn catalog head | **Yes** | n/a (already active) | Yes | The tool a model should reach for by default |
| **deferred** | Registered + discoverable, hydrated on demand | No | **Yes** | Yes | Real, useful tools that don't earn a first-turn slot |
| **hidden-compatibility** | Registered + dispatchable, but removed from active **and** from search | No | **No** | **Yes — identical behavior, silent** | Old synonym kept only so old transcripts replay; no model should newly discover it |
| **deprecated** | Like hidden-compat, but execution **appends a replacement notice to result metadata** | No | **No** | **Yes — works, plus a "use X instead" notice** | A retired name we actively steer callers off of, still safe to replay |
| **removed** | Not registered at all | No | No | **No — hard error** | Only after `planned_removal_version`, once replay support is formally dropped |

### hidden-compatibility vs deprecated — be precise

Both states are **invisible** (not active, not in tool search) and both remain
**dispatchable** (calling them still works). The *only* difference is the
caller-facing signal:

- **hidden-compatibility:** completely silent. The tool behaves byte-for-byte
  like its canonical twin. We use this when there is *no behavioral or naming
  lesson to teach* — the name was a pure alias and we simply don't want models
  re-learning it. (Example: `exec_wait` is literally `exec_shell_wait`.)
- **deprecated:** behaves identically *and succeeds*, but the tool result's
  **metadata** carries an appended notice like
  `"deprecated: use <replacement> instead"`. The notice goes **only in the
  result metadata returned for that call** — never in the cached tool catalog
  prefix (see Section 8). We use this when there is a canonical replacement we
  want the caller (and any human reading the transcript) nudged toward.

Neither state ever changes the *behavior* of the call. Replay always works.

---

## 3. Representation in code

The lifecycle is represented as **const name-sets plus an alias/manifest table**
in `crates/tui/src/core/engine/tool_catalog.rs`, alongside the existing
`DEFAULT_ACTIVE_NATIVE_TOOLS` (`tool_catalog.rs:37-64`) and
`ARCEE_FIRST_TURN_NATIVE_TOOLS` (`tool_catalog.rs:106-115`).

### 3a. Name-sets and the manifest (sketch)

```rust
// crates/tui/src/core/engine/tool_catalog.rs  (planned)

/// Tools removed from the active set AND from tool-search, but still
/// registered and dispatchable with byte-identical behavior. Silent.
pub(super) const HIDDEN_COMPATIBILITY_TOOLS: &[&str] = &[
    "exec_wait",          // == exec_shell_wait  (ShellWaitTool)
    "exec_interact",      // == exec_shell_interact (ShellInteractTool)
    "tts",                // == speech (SpeechTool)
    "checklist_write",    // == work_update (TodoWriteTool)
    "checklist_add",      // == work_update single-item add
    "checklist_update",   // == work_update single-item update
    "checklist_list",     // == work_update list
    "todo_write",         // == work_update
    "todo_add",           // == work_update single-item add
    "todo_update",        // == work_update single-item update
    "todo_list",          // == work_update list
];

/// Deprecated aliases: invisible + dispatchable, with a replacement notice
/// appended to RESULT METADATA only (never the cached prefix).
pub(super) struct DeprecatedAlias {
    pub name: &'static str,
    pub replacement: &'static str,
    pub note: &'static str,
}

pub(super) const DEPRECATED_ALIASES: &[DeprecatedAlias] = &[
    // Empty in the #4132 work-surface cutover: checklist_* and todo_* are
    // silent hidden-compatibility aliases of work_update for transcript replay.
];

#[inline]
pub(super) fn is_hidden_or_deprecated(name: &str) -> bool {
    HIDDEN_COMPATIBILITY_TOOLS.contains(&name)
        || DEPRECATED_ALIASES.iter().any(|d| d.name == name)
}
```

### 3b. The two filter points

1. **Catalog / tool-search exclusion (tool_catalog.rs).**
   Deferral is decided by `should_default_defer_tool` (`tool_catalog.rs:66-82`),
   and the active set is the head built by `build_model_tool_catalog`
   (`tool_catalog.rs:178-196`). Hidden-compat and deprecated tools must be
   forced *out of the active head* and *out of the tool-search-discoverable
   pool*. Concretely, the deferral predicate gains a short-circuit so these
   names are never active, and the tool-search index builder skips any name for
   which `is_hidden_or_deprecated(name)` is true. Arcee's narrowed first-turn
   path (`apply_provider_tool_policy`, `tool_catalog.rs:134-149`) already
   excludes them by construction since they aren't in
   `ARCEE_FIRST_TURN_NATIVE_TOOLS`.

2. **Result-notice append (tool_routing.rs).**
   Dispatch already routes by tool name in
   `crates/tui/src/tui/tool_routing.rs` (e.g. the wait/interact unification at
   `tool_routing.rs:1139-1140`). After a successful dispatch, if the called name
   is in `DEPRECATED_ALIASES`, the router appends the matching `note` to the
   **result metadata only**. Hidden-compat names append nothing.

### 3c. Why name-sets, not a per-`ToolSpec` enum field

A per-`ToolSpec` `lifecycle: Lifecycle` field was rejected for three reasons:

- **Prefix-cache safety.** The tool catalog array is part of DeepSeek's
  immutable KV prefix (`tool_catalog.rs:169-177`). A per-spec field invites
  serializing lifecycle state *into* each tool's schema, which is exactly the
  kind of head mutation that forces a full re-prefill. Name-sets live entirely
  in the catalog-build logic and never touch the emitted tool JSON.
- **Single source of truth + diffability.** The diet for a release is one small,
  reviewable edit to two or three const arrays in one file, instead of scattered
  field flips across many tool modules.
- **Registration stays orthogonal.** Tools remain registered exactly as today
  (e.g. `with_shell_tools`, `registry.rs:523-531`). Lifecycle is a *catalog
  policy* layered on top of registration, not a property baked into the tool.

---

## 4. Deprecation manifest (the #2681 acceptance-criteria table)

This is the authoritative manifest. Columns are the #2681 AC columns. No entry
is "removed" in 0.8.53; replay is supported for everything listed.

| Alias | Replacement (canonical) | Lifecycle state | first_deprecated_version | planned_removal_version | replay_supported |
|---|---|---|---|---|---|
| `exec_wait` | `exec_shell_wait` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `exec_interact` | `exec_shell_interact` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `tts` | `speech` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `checklist_write` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_add` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_update` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_list` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_write` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_add` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_update` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_list` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |

**Legacy subagent names — removed, no manifest entry needed.**
The model-visible subagent surface is only `agent`. The old lifecycle names and
the experimental tool-agent lane were removed rather than kept as hidden
compatibility tools.

`planned_removal_version` is intentionally `TBD`: a name only moves to **removed**
once we formally drop replay for transcripts old enough to contain it, which is a
separate, deliberate decision per name.

---

## 5. Active-catalog budget (per mode, per provider)

The active set is the first-turn cost. Do not duplicate the exact
`DEFAULT_ACTIVE_NATIVE_TOOLS` count here: adjacent PRs in the v0.8.53 batch may
add or remove active tools, and the source of truth is always
`tool_catalog.rs`. This document defines the diet policy and invariants, not a
second catalog snapshot.

### Per provider

| Provider | First-turn active source | Budget policy |
|---|---|---|
| Default (DeepSeek et al.) | `DEFAULT_ACTIVE_NATIVE_TOOLS` | Remove duplicate aliases from the active head when their canonical twins stay active; any net growth needs an explicit budget decision. |
| Arcee (Trinity) | `ARCEE_FIRST_TURN_NATIVE_TOOLS` | Provider-specific read-only WAF workaround; unchanged by the default diet unless explicitly reviewed. |

The default diet removes `exec_wait` and `exec_interact` from the active head
(they become hidden-compat; their canonical twins `exec_shell_wait` /
`exec_shell_interact` stay). `tts` and `todo_*` are *already not* in the active
set, so they do not change the active budget in this diet. The net effect of
this specific diet is to remove two duplicate active aliases from whatever
default active head is current after the surrounding v0.8.53 PR batch.

### Per mode (Plan / Agent / YOLO)

The native active head is the **same set across modes** by design — mode does not
add or remove native tools from `DEFAULT_ACTIVE_NATIVE_TOOLS`
(`should_default_defer_tool` ignores `_mode` for native tools,
`tool_catalog.rs:66-68`). Mode affects **MCP** deferral instead:
`apply_mcp_tool_deferral` keeps MCP tools deferred unless `mode == Yolo`
(`tool_catalog.rs:162-167`).

| Mode | Native active budget | MCP tools active? |
|---|---|---|
| Plan | same native head | No (deferred) |
| Agent | same native head | No (deferred) |
| YOLO | same native head | Yes (a known, intentional widening) |

**Budget rule:** the native active head must stay byte-identical across Plan ↔
Agent ↔ YOLO (Section 8). Any growth of the head requires retiring something
else or an explicit budget bump in this doc.

---

## 6. The canonical-surface rule

> **Every model-visible (active or deferred-discoverable) tool must have one
> clear niche. If a tool is superseded, it gets a named replacement and moves to
> hidden-compatibility or deprecated — it does not stay visible.**

### Canonical vs compatibility summary for the confusing clusters

| Cluster | Canonical (keep visible) | Compatibility / retired | Notes |
|---|---|---|---|
| **Shell wait** | `exec_shell_wait` | `exec_wait` → hidden-compat | Same `ShellWaitTool` (`registry.rs:526,529`); router already unifies (`tool_routing.rs:1139`) |
| **Shell interact** | `exec_shell_interact` | `exec_interact` → hidden-compat | Same `ShellInteractTool` (`registry.rs:527,530`) |
| **Work progress / checklist / todo** | `work_update` | `checklist_write/add/update/list`, `todo_write/add/update/list` → hidden-compat | Same `TodoWriteTool`; compatibility names replay old transcripts only |
| **Speech / tts** | `speech` | `tts` → hidden-compat | Same `SpeechTool` (`registry.rs:787-792`) |
| **Subagent lifecycle** | `agent` | old lifecycle names and tool-agent lane removed | Single async launcher; child agents are leaf workers |
| **Edit family** | `apply_patch`, `edit_file`, `write_file`, `fim_edit` | none — **all distinct niches** | NOT touched (per #2681 non-goals); doc-only canonical guidance |
| **Search family** | `grep_files` (content), `file_search` (filename), `project_map` (structure) | none — **distinct niches** | NOT touched; no FTS5/BM25/semantic index exists today |

**Non-goals (explicitly NOT diet targets in this cycle, per #2681):**
`apply_patch` / `edit_file` / `write_file` / `fim_edit`;
`grep_files` / `file_search` / `project_map`;
`fetch_url` / `web.run` / `web_search`;
`task_shell_*`; `handle_read` / `retrieve_tool_result`. These have distinct
niches and receive **canonical guidance only** — no lifecycle change.

The RLM surface (`rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close` /
`rlm_session_objects`, `crates/tui/src/tools/rlm.rs`) is likewise out of scope;
`handle_read` retrieves var handles, and `finalize` / `FINAL` is an in-kernel
Python function, **not a tool** — so there is nothing to retire there.

---

## 7. Subagent cutover decision: one visible launcher

The old lifecycle trio and tool-agent lane are removed, not hidden compatibility
tools.

**Decision: expose only `agent`.**

- `agent` starts one focused background child and returns the agent id plus
  transcript handle.
- Child results arrive as completion events. The parent should keep working
  instead of polling a lifecycle tool.
- Child tool catalogs exclude subagent lifecycle tools, so children are leaf
  workers and cannot recursively summon more agents.
- Detailed inspection goes through `handle_read` on the returned transcript
  handle.

This is a lifecycle simplification, not a provider gate.

---

## 8. Prefix-cache safety + replay guarantee

### Prefix-cache rules every diet PR MUST follow

The tools array is part of DeepSeek's immutable KV prefix. The catalog-head
byte-stability invariant (`tool_catalog.rs:169-196`) is binding:

1. **Never mutate the active head non-deterministically.** The first-turn active
   block must be **byte-identical run-to-run** and across Plan ↔ Agent ↔ YOLO.
2. **A diet is a one-time deterministic edit.** Removing a name from
   `DEFAULT_ACTIVE_NATIVE_TOOLS` shifts the head exactly once; after that it must
   be stable. Land such edits as their own focused change.
3. **Notices live in result metadata, never the prefix.** Deprecated replacement
   notes are appended at dispatch time in `tool_routing.rs` to the *call result*
   only. **Nothing** about hidden/deprecated state may be serialized into a tool
   schema, description, or the catalog array.
4. **Preserve ordering and partitioning.** `build_model_tool_catalog` sorts each
   partition by name and keeps built-ins as a contiguous prefix ahead of MCP
   tools (`tool_catalog.rs:186-194`). Diet edits must not break this.
5. **Hidden/deprecated tools are excluded *before* the head is built**, so their
   removal is the only head change — they do not appear in the prefix at all.

### Old-transcript replay guarantee

> For every name in the deprecation manifest with `replay_supported = Yes`, the
> tool stays **registered and dispatchable with identical behavior**. Replaying
> an old transcript that calls `exec_wait`, `exec_interact`, `tts`, or any
> `todo_*` produces the same result it always did. Deprecated names additionally
> attach a result-metadata notice; hidden-compat names are silent. A name is only
> ever made non-dispatchable (**removed**) after a deliberate, per-name decision
> to drop replay support at `planned_removal_version`.

---

## 9. Required tests

Any diet PR (and the umbrella #2681 work) must add/keep:

1. **Duplicate-active-alias guard.** A test asserting that no name in
   `HIDDEN_COMPATIBILITY_TOOLS` or `DEPRECATED_ALIASES` appears in
   `DEFAULT_ACTIVE_NATIVE_TOOLS` or `ARCEE_FIRST_TURN_NATIVE_TOOLS`, and that no
   two active entries resolve to the same underlying tool implementation.

2. **Tool-search exclusion test.** Assert that hidden-compat and deprecated names
   are absent from the tool-search-discoverable pool while remaining present in
   the registry (dispatchable).

3. **Replay / dispatch tests.** For each manifest name, calling it still
   executes and returns the same result as its canonical twin. Deprecated names
   additionally assert the replacement note is present **in result metadata** and
   absent from the catalog/prefix. Hidden-compat names assert **no** added
   notice.

4. **Golden active-block byte test.** A snapshot test pinning the byte
   serialization of the first-turn active tool block, asserting it is identical
   across Plan / Agent / YOLO (native head) and stable run-to-run — enforcing the
   `tool_catalog.rs:169-196` invariant. The golden updates **only** as a
   reviewed, deliberate one-time edit when the diet lands.

5. **Subagent guardrail test.** Assert only `agent` is registered as a
   model-visible subagent tool and that hidden/legacy names from
   `subagent/mod.rs` are not advertised.

6. **Leaf-worker test.** Assert subagent tool catalogs exclude `agent` and
   retired legacy lifecycle names.
