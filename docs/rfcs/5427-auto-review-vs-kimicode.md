# Auto-review vs kimicode — keep the deterministic-first hybrid (#5427)

**Status:** recommendation for the call. Implementation only after Hunter
decides, per the issue.

**Date:** 2026-08-16. **Grounding:** `crates/tui/src/tui/auto_review.rs`,
`crates/tui/src/tui/gate_receipts.rs`, `crates/tui/src/core/engine/turn_loop.rs`,
`crates/tui/src/core/engine/reviewer.rs`, `crates/tui/src/tui/approval/policy.rs`,
`crates/tui/src/fleet/exact.rs`, kimicode `packages/agent-core-v2`, and the
`codewhale-ops` grokbuild notes.

## Recommendation in one paragraph

**Keep the hybrid we already ship — deterministic engine first, one-shot model
guardian only for the fallback hold band, fail-closed — and do not move toward
kimicode's posture or a model-first gate.** The evidence below shows that the
premise behind the comparison is weaker than it looked: kimicode does **not**
run a model-in-the-loop tool gate (its gate is a deterministic policy chain
plus interactive human approval), and Codewhale has **already** shipped the
hybrid since v0.9.8. The real gaps are not enforcement gaps; they are
(a) zero instrumentation on the deterministic path, (b) no aggregated guardian
latency, and (c) the grokbuild "steal order" item — durable plan artifact and
commentable review UI — which is a review *surface*, not a review *model*.

## What we actually are today

The module doc of `auto_review.rs` says it plainly: "Deterministic auto-review
policy evaluation." The pipeline, in order:

1. **Hard blocks never reach a model.** Configured block rules and the
   built-in safety floor (`safety_gate()`) are decided before anything else.
2. **Deterministic fallback** classifies by `(category, risk, action_kind)`:
   publish-like → hold; destructive background/headless → hold; Auto + write +
   unbounded targets → ask; configured allow rules → allow; then the
   benign/destructive table.
3. **Guardian tier (v0.9.8).** Only a deterministic *fallback hold* — an
   `AskUser` outcome that Auto posture would otherwise convert into a bare
   denial — is eligible for one reviewer call. Reviewer failure is a denial
   (fail closed). No secondary advisory path, no remembered reviewer state
   (`auto_review.rs:448-500`). Budgets: `REVIEWER_TIMEOUT = 90s`,
   `MAX_REVIEW_CONTEXT_BYTES = 64KiB`, `max_tokens = 384`
   (`core/engine/reviewer.rs:34-39`).
4. **Receipts name the layer that decided.** `AutoReviewReceiptGuardian*`
   vs `AutoReviewReceiptDeterministic*` vs the fail-closed
   `AutoReviewReceiptGuardianUnavailable` wording (`locales/en.json:1448-1452`).
   The audit event carries `gate: "deterministic" | "guardian"` plus decision,
   risk, reason (`turn_loop.rs:2768-2830, 432-533`). The "model guardian"
   wording means exactly the risk-classifier tier, not an always-on LLM pass.

So "keep deterministic / hybrid / model-first" is not a three-way fork from
zero: **hybrid already won the 0.9.8 call**, and this issue is about confirming
it with evidence rather than re-deciding it blind.

## What kimicode actually does (correcting the premise)

Reading `kimicode/kimi-code` (`packages/agent-core-v2/src/agent/permissionGate`,
`permissionPolicy/policies/`, `features/plan/`):

- **There is no model-in-the-loop tool-action gate.** `AgentPermissionGate`
  runs a deterministic permission-policy chain on every tool execution; `ask`
  defers to an interactive `toolApproval` round-trip with a human. Every
  decision is telemetried as `permission_policy_decision` with
  `policy_name, tool_name, permission_mode, decision, reason`.
- The only model-driven "reviewer" is a **user/workspace-defined agent
  profile** (e.g. `reviewer.md` / `code-reviewer`) — a model sub-agent doing
  review as a *task*, the same shape as Codewhale's reviewer Fleet role.
- Its review surfaces are interactive plan/goal approvals; auto-approved plan
  results explicitly warn "auto-approved without user review" in the
  transcript (`exitPlanModeTool.ts:176`). No prompt-injection defense code and
  no gate-latency budget exists on that path.

In other words: kimicode's reviewer is *ask-mode UX plus a reviewer role*,
both of which Codewhale already has (`Shift+Tab` Ask posture; the fleet
reviewer role with read-only authority clamps in `fleet/exact.rs`). Nothing in
kimicode's code argues for moving a model into the Codewhale gate.

## What the ops notes already decided

- grokbuild steal order (`codewhale-ops/grokbuild.md:284-296`): "Plan artifact
  + commentable review — **keep CW's gate**; add durable plan + review UI."
- Explicit "Do not take" (`notes/grokbuild-feel-clickable-bottom-2026-08-14.md:240`):
  transcript-fed auto classifier. A model-fed gate is a prior decision
  *against*, not an open question.
- The adjacent "steal" that survives scrutiny is deterministic:
  "Typed bash findings force Auto-Review — map `SafetyAnalysis` → closed enum;
  nonempty cannot heuristic-Allow; high/critical never auto-run" — more
  deterministic signal, not more model.

## Evidence: sampled gate receipts

Real receipts from live fleet sessions exist under
`~/.codewhale/auto-review-decisions/` (62 JSONL session files; 96 recorded
decisions sampled 2026-08-16). Outcome mix:

- `ask_user` (deterministic_fallback): 43 — the band that *would* be
  model-eligible under Auto posture
- `allow`: 28
- `hold_for_review` (built-in safety gate): 25
- Risk mix: 68 destructive / 28 benign. Category mix: 49 shell / 28 safe /
  12 mcp_action / 7 unknown.

One receipt (redacted input, schema_version 1) shows the honesty contract in
the wild: `decision: ask_user, decision_source: deterministic_fallback, reason:
"destructive action requires explicit review", reviewer_kind:
deterministic_policy, bypassed_by: full_access, enforcement: no_change,
receipt_status: recorded, planning_enforcement: auto_approve`. The local slice
predates the 0.9.8 guardian tier; guardian-outcome receipts are exercised by
in-tree PTY flows (`agent_focus_pty.rs:549-557`, `qa_pty.rs:3295-3312`) and
unit fixtures (`gate_receipts.rs`), so both layers' receipt wording is
tested, not just claimed.

## Latency appendix

| Layer | Measured cost | Evidence |
| --- | --- | --- |
| Deterministic engine | in-process table lookups; no instrumentation exists — estimate µs-scale, **measure before asserting** | no timing code in `auto_review.rs` or `approval/policy.rs` |
| Guardian (model) | one provider round-trip per held action, hard-capped at 90s; per-event `duration_ms` in `TurnUsage` (`turn_loop.rs:450-464`) | `reviewer.rs` budgets; no aggregate stats yet |
| Receipt/audit write | append-only, amortized | `audit_event` schema |

Two honest gaps to close before the appendix can carry real numbers:
instrument the deterministic path (one timer, one counter) and aggregate
guardian `duration_ms` + first-token time into session metrics. Until then,
any latency claim about the deterministic gate is an estimate, and this doc
will not present it as measurement.

## Injection-risk appendix

- **Deterministic path: zero model surface.** No prompt is built, so no
  prompt-injection risk exists for allow/block/ask outcomes. This is the
  strongest property of the current design and the main reason not to widen
  the model tier.
- **Guardian path: bounded and fail-closed.** The guardian policy treats the
  input as untrusted evidence, forbids intent inference, denies exfiltration /
  credentials / security-weakening / destructive patterns, and requires exactly
  one JSON object `{risk_level, decision, reason}`
  (`DEFAULT_GUARDIAN_POLICY`, ported "in spirit" from Codex's guardian
  taxonomy). Context is capped at 64KiB, output at 384 tokens, wall time at
  90s, and there is no remembered state across calls — a poisoned review
  cannot persist and cannot veto a hard block. Failure = denial.
- **Widening the model tier would grow the surface without growing the
  guarantee.** Every additional model-reviewed band adds a prompt that embeds
  attacker-influenced text (tool names, args, workspace content) and a
  nondeterministic outcome that the receipt must honestly attribute. The
  current design confines that to the narrow band where Auto posture would
  otherwise silently deny — the only place the trade is worth making.

## What to do after the call (candidate follow-ups)

1. **Keep**: deterministic-first hybrid as shipped. No model-first gate, no
   transcript-fed classifier.
2. **Instrument**: deterministic gate latency counter + guardian
   `duration_ms` aggregation into session metrics (makes the appendix real).
3. **Harden deterministically, not with a model**: adopt the grokbuild
   "typed bash findings → closed enum" mapping so novel shell segments force
   Auto-Review via `SafetyAnalysis`, high/critical never auto-run.
4. **Surface**: durable plan artifact + commentable review UI (grokbuild
   steal #3) — the actual review gap is UX, and it does not need a model.
5. **Receipts**: keep naming the deciding layer; add aggregate receipt stats
   (per-verdict counts) to `/doctor` or session summary when cheap.

None of 2–5 changes posture; each is independently shippable.
