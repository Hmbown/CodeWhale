# Surf-ace Verification — Does the Interstice Exist?

**Date:** 2026-07-27
**Scope:** Verifying the "Honest Part" of `Surf_So_What_Even_Is_This_v2.md` — does CodeWhale already have a deterministic, script-driven, receipt-producing, traffic-routing surface between Fleet/Workflow operations?

---

## The Claim (from v2)

> *"Between Fleet operations, there's a gap. Fleet runs a worker. The worker finishes. Fleet writes a receipt. Then what? The next worker spins up. But there's no point in between where I can deterministically say: stop. Verify. Decide."*

> *"I want a surface between Fleet operations where I can: verify, block, pause, run a script, redirect."*

> *"I haven't seen anyone else describe the surf-ace itself. The deliberate gap. The interstice. The point between steps where you can deterministically verify, block, pause, redirect — not as an emergency brake, but as a designed feature of the workflow."*

---

## What Exists: Workflow Gates

CodeWhale **does** have a gate system in the Workflow IR.

Source: `crates/workflow/src/gates.rs` (505 lines, committed to `main`)

### Gate Architecture

```rust
pub struct GateSpec {
    pub id: String,
    pub role: String,          // Role whose completion triggers this gate
    pub on: GateOn,            // RoleComplete or RoleStart
    pub gate: GateKind,        // Verify, Review, or Approve
    pub on_fail: GateOnFail,   // Retry, Block, or Escalate
    pub blocks_role: Option<String>,  // Downstream role blocked until gate passes
    pub max_retries: u32,
    pub artifact_kind: Option<String>,
    pub require_explicit_verdict: bool,
}
```

### Gate States

A gate can be: `Pending`, `Passed`, `Blocked { reason }`, `Retrying { attempt, reason }`, or `Escalated { reason }`.

### Gate Outcomes

A gate receives: `Pass`, `Fail { reason }`, or `HumanApprove { note }`.

### Canonical Pipeline (hardcoded in Rust)

```rust
pub fn stopship_gate_pipeline() -> Vec<GateSpec> {
    // scout-findings (Approve) → blocks implementer
    // reviewer-diff (Review)    → blocks verifier
    // verifier-suite (Verify)   → blocks release_lead
}
```

### Lane-Scoped Handoffs

Artifacts flow between roles: `HandoffArtifact { from_role, to_role, kind, payload }`. These are lane-scoped, not fleet-scoped.

---

## The Gap: Where the Surf-ace Would Go

### Gap 1: No deterministic script execution in gates

The `GateKind::Verify` says: "Compile/test/lint suite (verifier role; #4013)." But the verifier is a **Fleet role** — an LLM sub-agent. It is not a bash script. It does not run `cargo test` deterministically. It is an LLM that reads the implementer's output and decides whether it passes.

From the staged bugfix workflow (`wf_a2_staged_bugfix.workflow.js`):

```js
const verify = await task({
    type: "verifier",
    prompt: [
        "Read the implementer result and validate its reported path and diff summary.",
        "Confirm the intended one-line clarification was made only in the isolated worktree.",
        "Confirm the parent workspace remains unchanged.",
        "Do not implement further edits. Return PASS/FAIL with evidence.",
    ].join("\n"),
});
```

The verifier is an LLM. It reads text. It decides. It can hallucinate. It can rationalize. It can miss things. This is the **same problem** that issue #4032 identified: *"The LLM lies, writes temp scripts, rationalizes post-hoc."* A gate gated by an LLM is not a deterministic gate.

**What's missing:** A `GateKind::Script` or `GateKind::Check` that runs a user-supplied command (bash script, test suite, diff checker) and uses its exit code + stdout as the gate outcome. The infrastructure for blocking, retrying, escalating, and handoff artifacts already exists in the gate board. Only the evaluation source is missing.

### Gap 2: No user-supplied script execution in workflows

The Workflow VM has **no** filesystem, shell, network, env, imports, clock, or randomness (see `docs/AUTOMATIC_WORKFLOWS.md:72-77`). The supported host calls are: `task`, `parallel`, `pipeline`, `phase`, `log`, `budget`, `args`. There is no `exec` or `check` or `verify_script` host call.

The supported node wrappers in `docs/WORKFLOW_AUTHORING.md:83-85` are: `agent`, `branch`, `sequence`, `reduce`, `teacher_review`, `loop_until`, `cond`, and `expand`. There is no `gate` or `script` or `check` node exposed to JS authoring.

**What's missing:** A workflow node or host call that runs a deterministic script and uses its result (exit code, stdout JSON) as a gate outcome or routing decision.

### Gap 3: No traffic routing based on verification results

Workflow nodes can branch (`branch`, `cond`) and loop (`loop_until`), but these are compile-time structures — the branching logic is written into the workflow definition. There is no runtime mechanism to say: *"the verifier script returned 'degraded_function_calling' → spin up the debugging agent instead of the promoter."*

The `cond` node is a static branch. The `reduce` node aggregates results into a summary. Neither one dynamically routes traffic based on a receipt's contents.

**What's missing:** A `route` or `dispatch` node that reads a receipt (from a script or from a gate outcome) and selects the next worker dynamically.

### Gap 4: The `require_explicit_verdict` field defaults to false

In the canonical stopship pipeline, all three gates have `require_explicit_verdict: false`. When enabled (per the docs in `gates.rs:75-79`), it requires *"a standalone first-line PASS/APPROVE/BLOCK/FAIL verdict from a successfully completed role."* But this verdict still comes from an LLM role, not from a deterministic script. The field is about format, not about source.

**What's missing:** A field or mechanism that says *"this gate's outcome comes from a deterministic script, not from an LLM role."*

---

## What This Means

### The Gate Infrastructure Exists

CodeWhale already has:
- ✅ Gate specs (block, retry, escalate)
- ✅ Gate state tracking (pending, passed, blocked, retrying, escalated)
- ✅ Role blocking (downstream role can't start until gate passes)
- ✅ Handoff artifacts (structured data flowing between roles)
- ✅ Lane-scoped gate boards (persisted to JSON)

### The Verification Source Does Not

CodeWhale does **not** have:
- ❌ A gate that runs a deterministic script instead of an LLM role
- ❌ A workflow host call that executes a user-supplied command
- ❌ A mechanism to route traffic based on script output
- ❌ A gate outcome that comes from a bash exit code rather than an LLM verdict

### The Surf-ace Fits in an Existing Seam

The gate infrastructure (`GateSpec`, `LaneGateBoard`, `GateOnFail::Block/Retry/Escalate`) provides the structural scaffolding for exactly what Jay wants: block, retry, escalate, handoff artifacts. What's missing is the **evaluation source** — a way to say "this gate is evaluated by running a script, not by asking an LLM."

A `GateKind::Script` that takes a command, runs it, and maps exit-code-0 to `GateOutcome::Pass` and non-zero to `GateOutcome::Fail { reason: stdout }` would slot directly into the existing gate board without changing any of the blocking/retry/escalation/handoff logic.

This would give Jay's surf-ace five verbs:

| Verb | Existing Mechanism | Missing Piece |
|---|---|---|
| **Verify** | Gate evaluation (`GateOutcome::Pass/Fail`) | Script as evaluation source |
| **Block** | `GateOnFail::Block` + `blocks_role` | Already exists |
| **Pause** | `GateOnFail::Escalate` (surfaces to human) | Already exists |
| **Run a script** | N/A | Needs `GateKind::Script` or Workflow `exec` host call |
| **Redirect** | `HandoffArtifact` + `cond` (static) | Dynamic routing based on receipt contents |

---

## The Honest Verdict

Jay's claim in v2 — *"there's no point in between where I can deterministically say: stop. Verify. Decide."* — is **substantially correct.**

The scaffolding for "stop" (block/retry/escalate) and "decide" (handoff artifacts, role blocking) already exists in the gate system. The "verify" part exists only through LLM sub-agents — which is the same non-deterministic path that #4032 identified as unreliable. A deterministic script-execution gate does not exist.

This is not a criticism of the gate system. The gate infrastructure is well-designed and the right foundation. It's just missing one evaluation source: a script that produces a Pass/Fail outcome without going through an LLM.

**The gap is real. It is specific. It is addressable.** Adding `GateKind::Script` (or an equivalent mechanism) would close it without redesigning the gate board, the lane system, or the Workflow IR.

---

*Sources: `crates/workflow/src/gates.rs` (full file, 505 lines), `docs/WORKFLOW_AUTHORING.md`, `docs/AUTOMATIC_WORKFLOWS.md`, `docs/examples/dogfood-automatic/wf_a2_staged_bugfix.workflow.js`, `docs/examples/dogfood-automatic/wf_a3_partial_failure_synthesis.workflow.js`*
