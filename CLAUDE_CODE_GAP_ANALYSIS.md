# DeepSeek TUI vs. Claude Code (latest) — Gap Analysis & Modernization Plan

> Date: 2026-06-02 · Baseline: DeepSeek TUI v0.8.32 · Reference: Claude Code (latest, 2026)
> Scope: behavioral prompts, tools/capabilities, skills/subagents, lifecycle (hooks/memory).

## TL;DR

DeepSeek TUI is **not** a greenfield port — it is already a feature-rich Claude Code
analog built on the codex-rs architecture. The bulk of "Claude Code class" capability
is present: ~50 tools, 28 built-in slash-command modules plus user commands, a
`SKILL.md`-format skills system, an 8-event hook system, MCP client **and** server,
sub-agents with `fork_context`, plan/agent/yolo modes, approval policies, statusline,
checkpoints/undo, scheduling (`automation_*`), goal mode, share, and memory.

So "modernize to match latest Claude Code" is **not** about adding the big primitives —
they exist. It is about **closing the long tail**: a handful of lifecycle hooks, a more
structured memory model, named/file-defined agent types, richer output styles, and a set
of **behavioral-prompt** refinements that bring the agent's *defaults* in line with
current Claude Code norms (altitude, irreversible-action confirmation, faithful
reporting, schedule/loop discipline). The prompt work is the highest leverage-to-risk
ratio and should land first.

Legend — Effort: **S** (hours) · **M** (1–3 days) · **L** (week+). Risk reflects blast
radius in a ~210k-LOC production codebase.

---

## 1. Behavioral prompts & agent loop  ← highest leverage, lowest risk

`crates/tui/src/prompts/base.md` (217 lines) is mature. It already absorbed the
`PROMPT_ANALYSIS.md` recommendations (RLM patterns, Sub-Agent Strategy, Parallel-First
Heuristic, Verification Principle, Composition Pattern, bumped thinking budget). The
remaining gaps are *current* Claude Code behavioral norms not yet encoded:

| # | Claude Code norm | Present? | Gap | Effort/Risk |
|---|---|---|---|---|
| 1.1 | **Altitude** — solve at the right level of abstraction; don't over-fit to the literal ask or over-engineer | partial (decomposition only) | No explicit "right altitude / don't over-engineer / match the request's scope" guidance | S / low |
| 1.2 | **Confirm hard-to-reverse & outward-facing actions** (deletes, force-push, publishing to external services, sending messages) unless durably authorized | partial (`approvals/auto.md` mentions "pause before destructive") | Not a first-class principle in `base.md`; no "publishing is irreversible / approval doesn't transfer across contexts" framing | S / low |
| 1.3 | **Match surrounding code style** (comment density, naming, idiom) | no | Not stated; agent may impose its own style | S / low |
| 1.4 | **Look before you overwrite/delete** — inspect the target; if it contradicts how it was described, surface that instead of proceeding | no | Missing safety reflex | S / low |
| 1.5 | **Scheduling discipline** — when to *offer* `/automation` (`/schedule`/`/loop` analog) vs. just finish the work now | no | Scheduling tools exist but no guidance on *when* to reach for them; risk of over-offering | S / low |
| 1.6 | **Clickable references** — render `file:line` and PR/issue refs as links | partial (OSC-8 in TUI, #374) | Prompt doesn't instruct the model to *emit* `path:line` consistently | S / low |
| 1.7 | **Plan-mode etiquette** — investigate fully, then present a plan for approval before editing; don't ask "is the plan ready?" | partial (`modes/plan.md` is terse) | Could mirror Claude Code's ExitPlanMode discipline more explicitly | S / low |

**Recommendation:** a single surgical pass on `base.md` + `modes/*.md` + `approvals/*.md`.
All additive, prefix-cache-aware (append below volatile boundary), independently
testable via the existing snapshot tests (`crates/tui-core/tests/snapshot.rs`,
`crates/tui/src/prompts.rs`). **Do this first.**

---

## 2. Lifecycle: hooks

`crates/tui/src/hooks.rs` supports 8 events: `SessionStart`, `SessionEnd`,
`MessageSubmit`, `ToolCallBefore`, `ToolCallAfter`, `ModeChange`, `OnError`, `ShellEnv`.

Mapping to Claude Code's hook set:

| Claude Code hook | DeepSeek equivalent | Status |
|---|---|---|
| SessionStart / SessionEnd | same | ✅ |
| UserPromptSubmit | `MessageSubmit` | ✅ |
| PreToolUse / PostToolUse | `ToolCallBefore` / `ToolCallAfter` | ✅ |
| Notification | — | ❌ missing |
| Stop (agent finished a response) | — | ❌ missing |
| SubagentStop | — | ❌ missing (sub-agent completion is an internal sentinel, not a user hook) |
| PreCompact | — | ❌ missing |
| (n/a) | `ModeChange`, `OnError`, `ShellEnv` | ➕ DeepSeek extras (keep) |

**Gap:** no `PreCompact` (can't snapshot/guard before compaction), no `Stop`/`SubagentStop`
(can't trigger notifications, auto-format, or auto-test on turn completion), no
`Notification` event. The `Stop` hook in particular is the most-used Claude Code hook
(desktop notifications, auto-lint).

**Recommendation:** add `PreCompact`, `Stop`, `SubagentStop`, `Notification` variants to
`HookEvent` and wire dispatch sites. The enum + config + executor are already factored,
so this is mostly wiring. **Effort: M / Risk: medium** (touches the agent loop and
compaction path).

---

## 3. Lifecycle: memory

Today: `note` tool + `crates/tui/src/memory.rs` + `/memory` command + `tools/remember.rs`
+ `tools/recall_archive.rs`. Functional, but free-form.

Claude Code latest uses a **structured** file-based memory: one fact per file under a
`memory/` dir, each with frontmatter (`name`, `description`, `metadata.type ∈
{user,feedback,project,reference}`), a `MEMORY.md` index (one line per memory, loaded
each session), `[[wikilink]]` cross-references, a recall-relevance discipline, and a
`consolidate-memory` skill to dedupe/prune.

| Aspect | DeepSeek | Claude Code | Gap |
|---|---|---|---|
| Storage | free-form notes | one-fact-per-file + frontmatter | structure |
| Index | — | `MEMORY.md` loaded per session | no always-loaded index |
| Typing | — | user/feedback/project/reference | no taxonomy |
| Cross-links | — | `[[name]]` | none |
| Maintenance | — | `consolidate-memory` skill | none |
| Recall caveat | — | "memories reflect when written; verify before acting" | not encoded |

**Recommendation:** evolve `memory.rs` toward the structured schema + a session-loaded
index, and ship a `memory-consolidate` bundled skill. **Effort: M / Risk: low** (additive;
keep `note` as the write path).

---

## 4. Sub-agents & agent types

Today: `agent_spawn`/`agent_result`/`agent_wait`/… with `fork_context`,
`subagent_output_format.md`, worktree support already present in
`tools/subagent/mod.rs` and `snapshot/`. Strong.

Claude Code latest adds **named, file-defined agent types**: `.claude/agents/*.md` (and
bundled ones) with frontmatter (`name`, `description`, `tools`, `model`), surfaced to the
model as selectable `subagent_type`s with curated tool/model scoping, plus
`run_in_background` and "continue an existing agent via SendMessage."

| Aspect | DeepSeek | Claude Code | Gap |
|---|---|---|---|
| Spawn ad-hoc child | ✅ | ✅ | — |
| `fork_context` / prefix-cache reuse | ✅ | ✅ (worktree) | — |
| **Named reusable agent types** (file-defined, tool/model-scoped) | ❌ | ✅ | library of roles (Explore, Plan, …) |
| Background agents | partial (`task_shell`) | ✅ (`run_in_background` agents) | distinct background *agents* |
| Continue an existing agent | `resume_agent` / `agent_send_input` | ✅ SendMessage | ~parity |
| Worktree isolation | ✅ present | ✅ | verify it's exposed to model |

**Recommendation:** introduce `~/.deepseek/agents/*.md` (+ bundled `explore`, `plan`,
`code-review` agent defs) and let the model pick a `subagent_type`. **Effort: M / Risk:
low.**

---

## 5. Skills

Today: `SKILL.md` + frontmatter (ported from codex), `load_skill` tool, `/skills`,
bundled `skill-creator` + `v4-best-practices`. Format already matches Claude Code's.

**Gaps vs. Claude Code latest:**
- **Plugin packaging** — Claude Code bundles skills+commands+agents+hooks+MCP as
  installable *plugins* with a marketplace. DeepSeek has only minimal plugin refs.
  (**Effort: L / Risk: medium** — defer.)
- **Skill discovery surfacing** — verify skills are advertised to the model with
  descriptions for autonomous triggering (Claude Code injects the skill list each
  session). Appears present via the `## Skills` prompt section; confirm.
- **Bundled library breadth** — Claude Code ships docx/pdf/pptx/xlsx and workflow skills.
  DeepSeek ships 2. Optional content expansion. (**S–M, low risk.**)

---

## 6. Output styles vs. personalities

Today: `prompts/personalities/{calm,playful}.md` — *tone* overlays.

Claude Code "output styles" are broader — they can re-shape the agent's whole operating
posture (e.g. explanatory/teaching modes, terse-vs-verbose, role changes), not just tone.

**Gap:** only two, tone-scoped. **Recommendation:** generalize "personalities" into
"output styles" with a couple of behavior-shaping presets (e.g. `concise`, `explanatory`)
and user-definable styles under `~/.deepseek/styles/*.md`. **Effort: S–M / Risk: low.**

---

## 7. Smaller UX/tool gaps

| Item | Claude Code | DeepSeek | Note |
|---|---|---|---|
| Structured multi-choice question | `AskUserQuestion` (2–4 options, multi-select) | `request_user_input` (free-form) | add structured option UI — **S** |
| Out-of-scope task flagging | spawn-task chip | — | nice-to-have — **S** |
| Session chapters / ToC | mark-chapter + floating ToC | cycle dividers (#395) | partial; could add explicit chapters — **S** |
| Code-review effort levels | `/code-review low…ultra` | `/review` + `review` tool | add effort tiers — **S** |
| Deferred-tool search | ToolSearch | `tool_search_tool_regex/bm25` | ✅ parity |
| Background bash | `run_in_background` | `task_shell_start` | ✅ parity |
| Checkpoints/rewind | rewind | `/undo`, `revert_turn`, snapshots | ✅ parity |
| Web search/fetch | WebSearch/WebFetch | `web_search`/`fetch_url`/`web.run` | ✅ parity |

---

## Prioritized roadmap

**Wave 1 — Behavioral prompt alignment (S, low risk) — DO FIRST.**
Section 1: altitude, irreversible/outward-facing confirmation, match-surrounding-style,
look-before-overwrite, scheduling discipline, clickable refs, plan-mode etiquette.
Verifiable via existing prompt snapshot tests. One reviewable commit.

**Wave 2 — Lifecycle hooks (M, medium).** Add `Stop`, `SubagentStop`, `PreCompact`,
`Notification` to `HookEvent` + dispatch. Biggest single capability gain (auto-test/
auto-format/notify on completion).

**Wave 3 — Structured memory + consolidate skill (M, low).** Schema + `MEMORY.md` index +
`memory-consolidate` skill.

**Wave 4 — Named agent types (M, low).** `~/.deepseek/agents/*.md` + bundled roles.

**Wave 5 — Output styles generalization (S–M, low).** Behavior-shaping presets +
user-defined styles.

**Wave 6 — Smaller UX (S each).** Structured AskUserQuestion, review effort tiers,
explicit chapters, out-of-scope flagging.

**Deferred — Plugin/marketplace system (L).** Largest, lowest marginal value given skills/
commands/agents already work individually.

---

## What NOT to change

- The codex-derived crate architecture — sound and well-factored.
- DeepSeek-specific prompt content (V4 characteristics, prefix-cache economics, RLM,
  language-mirroring rules) — correct and tailored; not Claude Code's to dictate.
- DeepSeek-only hook extras (`ModeChange`, `OnError`, `ShellEnv`) — keep.
- Terminal-output formatting guidance — terminal constraints are real.
