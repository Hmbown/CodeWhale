# Issue drafts — #5123-class tool-surface sweep

Drafted 2026-08-03 from FINISH-0.9.4.md Appendix A entry #28 (the audit sweep
of every model-facing tool schema under `crates/tui/src/tools/` plus
registry/MCP/turn-loop error paths). **Not filed** — filing is the owner's
call. Same disease class as #5123: silent clamps/defaults that contradict
declared intent, contradictions that don't fail fast, errors that don't name
the fix.

Checked and honestly fine (do NOT file): Bash/shell tool (all contradictions
fail fast with corrective messages), workflow, tasks, github, rlm,
agents/list|message|followup|interrupt|wait, handle_read, MCP tool-name
collision handling (fails closed), arg_repair (byte-level only),
ToolError→model reporting.

---

## Draft 1 (release-blocker-adjacent): Fuzzy hallucinated-tool-name resolution executes an arbitrary sibling tool

**Title:** tool-surface: unknown tool names execute a random sibling via ≥3-char prefix match — must never execute a guess

**Body:**

When the model hallucinates a tool name, `tools/registry.rs:340-348` resolves
it by a ≥3-char prefix match over a `HashMap` (iteration order random per
process) and the rewritten call is dispatched at
`core/engine/turn_loop.rs:3960-3978` with log-only notice — the model is never
told its call was rewritten.

Consequences: `"agents"` can map to `agents/interrupt`; `"terminal"` to
`terminal/reset`. The runtime executes a mutating tool the model never asked
for, and the success/failure it observes belongs to a different action than
it intended — silent intent violation of the worst kind.

**Fix:** exact/normalized match only (case/alias/prefix-strip normalization is
fine); otherwise fail with `unknown tool "X", did you mean: …` (top
suggestions by edit distance) and **never execute a guess**. The "did you
mean" list is corrective text back to the model, not a dispatch.

**Anchors:** `crates/tui/src/tools/registry.rs:340-348`,
`crates/tui/src/core/engine/turn_loop.rs:3960-3978`.

---

## Draft 2 (release-blocker-adjacent): MCP `isError: true` payloads wrapped in `ToolResult::success`

**Title:** tool-surface: MCP tool errors reported to the model as success

**Body:**

MCP servers signal tool failure with `isError: true` on the result content.
`tools/registry.rs:1227-1235` wraps the payload in `ToolResult::success`
regardless; `mcp.rs:2289-2304` has no `is_error` handling anywhere in the
call path. The model sees "success" with an error message body and proceeds
on false premises — e.g. continues a multi-step plan assuming a write or
query worked when the server rejected it.

**Fix:** map `isError: true` → `ToolResult::error`, preserving the text
payload verbatim so the model still sees the server's message.

**Anchors:** `crates/tui/src/tools/registry.rs:1227-1235`,
`crates/tui/src/mcp.rs:2289-2304`.

---

## Draft 3: `web.run` returns empty success when no op key matches

**Title:** tool-surface: web.run with an unmatched op returns empty success instead of failing fast

**Body:**

A model passing the natural `{"query": …}` (no recognized op key) gets
`{"warnings":[]}` as a **success** — an empty result that reads as "nothing
found" rather than "you called me wrong". Silent no-op on the model-facing
search surface.

**Fix:** when no op key matches, return `ToolResult::error` naming the
accepted op keys and the received keys.

**Anchors:** `crates/tui/src/tools/web_run.rs:360-443,681,1176-1180`.

---

## Draft 4: `automation` update coerces unknown status to Active

**Title:** tool-surface: automation update silently coerces unknown status to Active (opposite of pause intent)

**Body:**

`tools/automation.rs:362-365` maps any unrecognized `status` value to
**Active** — a mutating, run-scheduling state. A model trying to pause an
automation with e.g. `"paused"` (wrong spelling of the accepted enum) gets
the automation *activated* instead, with a success receipt. Worst possible
direction for a silent clamp.

**Fix:** unknown status → error naming the accepted enum values; never
default a mutating field.

**Anchor:** `crates/tui/src/tools/automation.rs:362-365`.

---

## Draft 5: `work_update` coerces unknown todo status to pending

**Title:** tool-surface: work_update silently coerces unknown status to pending on the canonical progress surface

**Body:**

`tools/todo.rs:326` (synonym table at 40-48) maps unknown statuses —
`complete`, `blocked`, `in-progress` — to `pending`. On the canonical
progress surface, a model reporting a task finished sees it recorded as not
started, and the success receipt hides the rewrite.

**Fix:** extend the synonym table for the obvious near-misses
(`complete`→completed, `in-progress`→in_progress, `blocked`) and error on
anything else, naming the accepted values.

**Anchor:** `crates/tui/src/tools/todo.rs:326,40-48`.

---

## Draft 6: `agents/coordinate` declares ReadOnly + auto-approval while mutating the coordination ledger

**Title:** tool-surface: agents/coordinate capability metadata lies — ReadOnly/auto-approved but mutates the ledger

**Body:**

`tools/subagent/coord.rs:2526-2562` declares `ReadOnly` capability and
auto-approval while the tool mutates the coordination ledger and expands
write claims. Approval/policy layers that trust declared capabilities let a
mutating call through unreviewed. Additionally 18 of 19 schema fields have
no description, so the model is flying blind on a coordination surface.

**Fix:** declare the real capability (WritesFiles/state mutation) and
approval requirement; add field descriptions.

**Anchor:** `crates/tui/src/tools/subagent/coord.rs:2526-2562`.

---

## Draft 7: Retired `exec_shell` still taught by live tool descriptions; unknown-tool error misdiagnoses it

**Title:** tool-surface: exec_shell breadcrumbs point the model at a retired tool name

**Body:**

`exec_shell` was renamed to `Bash`, but live tool descriptions still teach
the old name (`tools/file.rs:100,553,668,1285`, `tools/tasks.rs:925`,
`tools/verifier.rs:307`), and when the model then calls `exec_shell` the
unknown-tool error misdiagnoses it as an `allow_shell` permission issue
(`core/engine/tool_catalog.rs:797-824`) instead of naming the rename.

**Fix:** sweep descriptions to `Bash`; add an explicit "exec_shell was
renamed to Bash" diagnostic in the unknown-tool path.

**Anchors:** `crates/tui/src/tools/file.rs:100,553,668,1285`,
`crates/tui/src/tools/tasks.rs:925`, `crates/tui/src/tools/verifier.rs:307`,
`crates/tui/src/core/engine/tool_catalog.rs:797-824`.

---

## Draft 8: `update_goal` ships an `objective` knob documented as ignored

**Title:** tool-surface: update_goal accepts objective, returns success, changes nothing

**Body:**

`tools/goal.rs:835-838,853-923` accepts an `objective` parameter documented
as ignored: success receipt, no behavior. The model believes it re-scoped
the active goal; the runtime did nothing.

**Fix:** either implement objective mutation or remove the parameter and
error when it is supplied ("objective is immutable after create_goal; use
complete + create").

**Anchor:** `crates/tui/src/tools/goal.rs:835-838,853-923`.

---

## Draft 9 (documentation, not code): provider schema sanitizers demote root oneOf/anyOf to prose

**Title:** tool-surface: root oneOf/anyOf demoted to prose by schema sanitizers — document the gap

**Body:**

`tools/schema_sanitize.rs:88-98` demotes root-level `oneOf`/`anyOf` to prose
for providers that reject them. Runtime enforcement in apply_patch currently
covers the practical gap, so the call is: **document the gap rather than
fix**. This draft exists so the demotion is a recorded decision, not
folklore.

**Anchor:** `crates/tui/src/tools/schema_sanitize.rs:88-98`.
