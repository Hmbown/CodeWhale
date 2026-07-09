# Test-Time Compute (TTC) in CodeWhale — design

Status: **approved direction** (maintainer greenlit). Synthesized from three independent reviews — the verify-tool implementation contributor, GLM 5.2, and an internal analysis — which all converged. This doc is the spec; implementation lands post-stopship (v0.8.69) and is split so nothing here blocks the v0.8.68 release.

## What TTC means here

Spend *more inference at decision time* for a better answer, on the **agent's own judgment** — not an always-on tax. Two capabilities:

- **(A) An agent-invoked `verify` / critic pass** — the model chooses to adversarially review its own recent work before claiming done, to catch "green-but-wrong" (motivating case: a fix passed 16/16 CI but only covered a CLI path, not the interactive TUI; a critic pass caught it, deterministic CI did not).
- **(B) Reasoning-effort escalation for sub-agents** — un-cap the hard `Low` clamp so a Fleet role can think at the tier its job needs.

## Core principle: one `CriticEngine`, three triggers

Factor a single `CriticEngine` that owns: the **target-context snapshot** (recent tool calls + the claimed-done state + a diff/evidence gather), the **prompt-template family** (adversarial "refute it"), the **reasoning effort** (Max), the **tools-disabled** flag, and the **structured verdict schema** (`verdict: pass|fail|uncertain`, findings `[{severity, issue, evidence, suggested_fix}]`, `unresolved_risk`).

Then three *distinct* entry points share that engine but **keep their own invocation contracts** — do NOT merge the triggers:

| Trigger | Contract | Issue |
|---|---|---|
| `verify` **tool** | sync, model-chosen, default-on | #4196 (MVP in PR #4199) |
| advisor **watcher** | async, rate-limited, off-by-default | #3982 |
| verification **gates** | post-turn, deterministic (compile/test/lint/review) | #4013 |

> Unify the **engine**, never the **trigger**. Merging sync/model-chosen + async/throttled + post-turn/deterministic produces a Frankenstein. Sharing the engine keeps drift at zero while each entry point keeps its character.

## (A) The `verify` tool

**Why a tool, not a critic sub-agent:** a `verify` tool is *structurally isomorphic to the existing `review` tool* — same `ToolSpec` trait, same `ToolRegistryBuilder` path, same `Feature` gate, same `MessageRequest` reasoning normalization. It inherits every existing guarantee for almost no new surface. A critic *sub-agent* would be a **second runtime with a second policy surface** (spawn-depth, allowlist, sub-agent tier resolution) — the textbook bolted-on smell. (A sub-agent critic that autonomously explores may return as an opt-in follow-up *behind the same tool contract* once #4193's spawn work has settled — but it is NOT the default.)

**Interface:** registered via `ToolRegistryBuilder::with_verify(critic)`, gated by a `Feature` flag. Input: `claim` (required) + optional `requirement`, `scope` (`diff|staged|none`), `base`, `files[]`, `focus`. It snapshots evidence deterministically (diff by scope — **including uncommitted working-tree changes when a base is given**, per PR #4199 fix — plus named files), builds ONE `MessageRequest` at `ReasoningEffort::Max` with **tools disabled**, and returns the structured verdict as a tool result.

**Where it plugs in:** the standard tool loop. No new control plane. The model invokes it like `read`/`edit`/`review`.

**How the model decides (and abuse is bounded):**
- **Constitution rule** (harness-enforced, not prose): verify before claiming done when debugging, on multi-file changes, security-sensitive edits, or changes touching *divergent surfaces* (CLI vs TUI, sync vs async). The green-CI-but-wrong case is the canonical trigger.
- **Engine-level rate limit / `TtcBudget`**: per-turn (one verify) and per-session budget, held in the **engine**, not in `MessageRequest`.
- **Feedback loop**: the verdict returns to the model; several consecutive clean verdicts should, via the Constitution, discourage further calls that session. The model sees its own hit rate and self-corrects.

**Verdict semantics** (PR #4199, already hardened): any finding at **medium severity or above** forces `unresolved_risk = true` and downgrades an `upheld` verdict to `uncertain`; only `low` nits are exempt. Advisory by default; a Constitution rule may make a `fail`/`unresolved_risk` verdict something the agent must address before claiming done (soft-block in the harness, not hard-coded in the tool).

**Recursion / cost bounding:**
1. *By construction*: tools disabled inside the critic call ⇒ no further tool calls.
2. *By registry*: `verify` is **structurally refused** when building a `SubAgentRuntime` allowlist — via a `Feature::CriticProducerOnly` (or equivalent) the `ToolRegistryBuilder` checks. The spawn-depth guard is only the *backup* line, not the primary.
3. *By budget*: the per-session `TtcBudget` consulted by the engine.

## (B) Sub-agent reasoning — replace the clamp with a floor

The bug in `auto_reasoning.rs` was never "Low is wrong" — it's that **Low is a *ceiling*** for sub-agents. Fix: **Low stays the default *floor*; remove the ceiling.**

**Tier resolution order:** `Profile (#4137) → explicit task override → session default → Low`.
- `SubAgentRuntime.reasoning_effort` continues to be forwarded verbatim.
- `Auto` inside a sub-agent resolves through a **Fleet-role-aware resolver** (a `review`-role profile pins High, a `search`-role pins Low, a `planner` pins Max) — NOT the global keyword resolver.
- The `agent` tool's `reasoning_effort` becomes "inherit from the Fleet profile unless explicitly overridden at spawn."

**Do not change the default floor from Low** — sub-agent traffic is majority search/lookup, and raising the floor silently raises cost on every existing fleet. Non-surprising > clever. This composes with #4137 (profile carries the tier alongside provider/model) rather than competing with it.

## Anti-patterns (what would read as bolted-on *in CodeWhale specifically*)

1. A **second critic implementation** — if `verify`/`review`/#3982/#4013 each roll their own prompt+call+parse, four paths diverge on the first bug. The single `CriticEngine` is the whole game.
2. A **non-tool control plane for reasoning escalation** — CodeWhale's model contract is tool-shaped; a side-channel breaks symmetry and bleeds into every provider adapter. The `verify` call *is* the escalation (Max internally). One vocabulary.
3. **Recursion policy in Constitution prose** — enforce it in the registry builder; the depth guard is secondary.
4. **Cost accounting leaking into `MessageRequest`** — budget belongs in the engine + a session `TtcBudget`. Don't make every tool cost-aware.
5. **`auto_reasoning.rs` becoming TTC-aware** — Auto resolves *effort per turn*; the model decides verify. Keep them separate or you get non-determinism the user can't reason about.
6. **Gate-ordering ambiguity between #4013 and `verify`** — different lifecycle points (mid-turn/model-chosen vs post-turn/deterministic). Document in the Constitution so contributors don't merge them.
7. **Conflating the watcher's contract with verify's** — #3982 is async/throttled/off; `verify` is sync/chosen/on. Share the engine; never the trigger.

## Issue map & sequencing

- **#4196** — `verify` tool. MVP in **PR #4199** (direct-critic, Max, tools-disabled, recursion-guarded, verdict-hardened). Refactor to sit on the extracted `CriticEngine` before merge. *Config-disjoint (`crates/tui/src/tools/`).*
- **CriticEngine extraction** — new; refactor `review`'s call/parse into the shared engine, then have `verify` consume it. Prereq for wiring #3982/#4013 to it later.
- **#4137** — Fleet profile carries a `reasoning` tier; drives (B). *Touches `crates/config` — sequence with the config work + after #4136 (canonical AgentProfile) and #4193 (landed).*
- **(B) resolver** — `auto_reasoning.rs` clamp→floor + Fleet-role-aware Auto resolution.
- **#3982 / #4013** — rebase onto the `CriticEngine` as additional triggers (later).

All of the above is **v0.8.69, behind the v0.8.68 stopship** (which is green).
