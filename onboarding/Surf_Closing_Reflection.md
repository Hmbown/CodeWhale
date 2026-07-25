# Surf — Closing Reflection

**Date:** 2026-07-25
**Branch:** `wip/onboarding_suit` (PR #4762)
**Status:** Draft — architectural direction document, not a feature spec

---

## Where We Are

Three documents now sit in this branch:

| Document | Role |
|---|---|
| `Surf_Skill_Flow_Design.md` | The blueprint — state machine, scripts, entry points |
| `Surf_Audit_2026-07-24.md` | The reality check — what works, what doesn't, what the codebase actually supports |
| `Surf_PR4762_Response_Report.md` | The map — existing CodeWhale structures that Surf echoes or could align with |

Together they trace an arc: design → audit → contextualization. This document closes that arc.

---

## The Reflection

Jay's closing thought from the PR conversation:

> *"I'm not sure what exactly I would need per se. But the myriad of possibilities are where my head is at. I'm thinking all the best from a myriad of worlds, so to speak. Mirroring fleet structure (not using it directly for now) for example. Both full integrations as well as maximum separation. I like being able to pipe surf into fleet into surf back into fleet. Whatever the eventual content/features of surf will be. This would be the vision: be able to exactly pick the right tool at the right moment, with receipts running along the quiet stream."*

This is not indecision. It's a clear architectural instinct: **Surf should be a composable primitive, not a monolithic suite.**

The right tool at the right moment means Surf doesn't need to be *the* testbed manager, or *the* Fleet task, or *the* CLI toolkit. It needs to be a shape that fits into all of those positions — pipeable, inspectable, receipt-producing — so the user can compose it into whatever workflow the moment calls for.

---

## Cross-Check: Is This Vision Feasible?

Three claims to verify against the actual codebase:

### Claim 1: "Mirroring fleet structure (not using it directly for now)"

**Verdict: Feasible and well-precedented.**

Fleet's CLI surface is a set of verbs on a noun: `fleet status`, `fleet run`, `fleet inspect`, `fleet artifacts`, `fleet resume`. Surf can adopt the same pattern with different nouns: `surf status`, `surf ride`, `surf inspect`, `surf catch`.

This is not duplication — it's a shared vocabulary. The verbs mean the same thing in both tools (status = "what's the current state", inspect = "show me the last result", resume = "pick up where I left off"). A user who knows Fleet already knows Surf. And Surf's output can be piped to Fleet because they speak the same structural language (JSON receipts, status codes, commit hashes).

The key design constraint: **Surf must not depend on Fleet internals.** Mirroring the shape is fine. Importing Fleet's Rust types or requiring a Fleet ledger to exist is not. Surf's bash scripts should produce the same JSON shapes that Fleet expects, not call Fleet's APIs. This keeps the door open for both "use Surf standalone" and "pipe Surf into Fleet."

### Claim 2: "Piping surf into fleet into surf back into fleet"

**Verdict: Structurally compatible with one minor CLI gap.**

Here's the pipeline vision, annotated with what's real today:

```bash
# Step 1: Surf inspects the testbed, outputs JSON
surf status --json
# → {"state": "testbed", "dirty": false, "branch": "my-feature", "commit": "abc123"}
# ✅ Surf's bash scripts can emit this. Fleet's status can read it.

# Step 2: Surf's output feeds a Fleet task spec
surf status --json | jq '{tasks: [{id: "verify", worker: {role: "builder"}, workspace: {required_files: ["Cargo.toml"]}}]}' > task.json
codewhale fleet run task.json
# ⚠️ Fleet's `run` takes a file path, not stdin. `surf status --json | fleet run -` isn't
#    supported today. But writing to a temp file is trivial, and stdin support is a
#    minor CLI change — not a structural barrier.

# Step 3: Fleet produces a receipt, Surf inspects it
codewhale fleet inspect <worker-id> --json | surf digest -
# → "🌊 abc123: All checks passed. 0 failures."
# ✅ Fleet outputs JSON with commit/status/test_counts. Surf's bash scripts can parse it.

# Step 4: Surf catches a new wave based on Fleet's result
surf digest - < receipt.json | jq '.commit' | xargs surf catch
# ✅ surf catch <repo-url> <branch> already accepts positional args (after the planned fix).
```

The only real blocker is Fleet's `run` not accepting stdin — and that's a feature request, not a design flaw. Everything else is JSON in, JSON out, across both tools.

### Claim 3: "Receipts running along the quiet stream"

**Verdict: The receipt shapes are naturally compatible.**

| Receipt field | Surf (`latest_receipt.json`) | Fleet (`fleet inspect --json`) |
|---|---|---|
| `timestamp` | ISO 8601 | Present in worker record |
| `branch` | From `.surf-config` | From task spec or worktree |
| `commit` | `git rev-parse --short HEAD` | Present in worker events |
| `status` | `"success"` / `"failure"` | `succeeded` / `failed` / `interrupted` |
| `message` | Human-readable summary | Worker completion payload |

The fields don't need to match exactly — they need to be mappable. A `jq` one-liner can translate Surf's receipt into a Fleet task spec, and another `jq` one-liner can extract a digest from Fleet's worker record into Surf's format. The "quiet stream" is JSON flowing between tools without either one needing to know about the other's internals.

---

## The Two Poles (and Why Both Matter)

Jay described "both full integrations as well as maximum separation." These are not contradictions — they're two valid operating modes for a composable tool:

### Maximum Separation (Mode A: Standalone)

Surf as a bash toolkit. No CodeWhale dependency. No TUI integration. No `execute:` frontmatter needed.

```bash
surf status          # human-readable
surf status --json   # machine-readable
surf ride            # pull + verify
surf ride --json     # pull + verify, output receipt
surf inspect         # read latest receipt
surf catch <url> <branch>  # clone + init
```

This mode works **today** — the scripts already do this, they just need `--json` flags and the `read -p` fix in `catch-wave.sh`. No Rust changes. No TUI changes. No frontmatter gap to solve.

### Full Integration (Mode B: Inside CodeWhale)

Surf as a Fleet-aware companion. Commands dispatch through the TUI. Receipts land in the Fleet ledger.

```bash
/surf                 # TUI command → runs surf.sh
/surf setup           # TUI command → runs catch-wave.sh
$surf --summary       # skill → LLM reads receipt, adds context
```

This mode requires the `execute:` frontmatter gap to be solved (or Surf to become a native built-in command). It's the heavier lift but gives the slickest UX.

### The Bridge (Mode C: Piped)

The middle ground. Surf runs standalone but produces/consumes Fleet-compatible JSON.

```bash
surf status --json | codewhale fleet run -    # Surf → Fleet
codewhale fleet inspect <id> --json | surf digest -  # Fleet → Surf
```

This is where the vision lives. Neither tool needs to know about the other. They just need to speak the same JSON dialect.

---

## What This Means for the PR

The PR (#4762) is correctly positioned as a draft. It is not ready to merge. But it is ready to **converse**. The three documents in this branch give reviewers a complete picture:

1. Here's the design (what I want to build)
2. Here's the audit (what actually works today)
3. Here's the response (what existing structures can help)

The PR doesn't need to resolve every open question before merging. It needs to land the design doc and the audit so the conversation has a stable reference point. The bash scripts can stay as scaffolding — they demonstrate the shape without claiming completeness.

**Recommended PR scope for merge-readiness:**
- Keep `Surf_Skill_Flow_Design.md` (the blueprint)
- Keep `Surf_Audit_2026-07-24.md` (the reality check)
- Keep `Surf_PR4762_Response_Report.md` (the map)
- Keep this document (the closing reflection)
- Keep the bash scripts as working prototypes
- **Drop or archive** `SKILL.md` (stale) and `Skill_Flow_Design (old).md` (superseded)
- **Drop** `surf.md` and `surf-setup.md` (the `execute:` frontmatter doesn't work — they're dead scaffolding until that gap is resolved)
- Mark the PR as ready for review with a note: "Design sketch and prototype scripts. Not functional from inside CodeWhale yet. Seeking architectural feedback."

---

## The Shape of the Thing

After three documents and two days of tracing code, the shape is clear:

> **Surf is a receipt-producing, JSON-emitting, pipe-friendly verification surface that sits between a contributor's local checkout and the Fleet/CI infrastructure. It mirrors Fleet's verb vocabulary without depending on Fleet's internals. It produces structured evidence that any downstream tool — Fleet, `jq`, a Workflow, a CI runner, a human reading a terminal — can consume.**

That's not a testbed manager. That's not an onboarding suite. That's not a skill. It's a **composable verification primitive**. The right tool at the right moment, with receipts running along the quiet stream.

---

## What's Left to Decide

| Decision | Status | Blocked by |
|---|---|---|
| Standalone CLI first, or TUI integration first? | Open | Nothing — both paths are clear |
| Mirror Fleet's JSON receipt schema exactly, or define Surf's own? | Open | Need to compare Fleet's worker receipt format in detail |
| Keep the bash scripts or rewrite in Rust as a `codewhale surf` subcommand? | Open | Depends on integration depth decision |
| Does Surf's `.surf-config` stay, or does it move to `.codewhale/surf.json`? | Open | Namespace question — `.codewhale/` is the project-local convention |
| Archive or delete the stale SKILL.md and old design doc? | Deferred | Should be done before merging the PR |

---

## Closing Note

Jay:

> *"Maybe the geometry is begging the question. Not: 'What should I build next?' But: 'What is the shape of the thing that I'm building toward?'"*

The shape, as best I can trace it from the code and the conversation, is this: **a tool that doesn't care whether it's being used by a human, a Fleet worker, a `jq` pipeline, or another tool.** It takes input, produces a receipt, and gets out of the way. That's the geometry. Everything else — the TUI commands, the skill, the state machine, the wave metaphor — is a surface for that geometry.

---

*Cross-checked against: `docs/FLEET.md`, `docs/FLEET_WORKFLOW_TUTORIAL.md`, `docs/SUBAGENTS.md`, `crates/tui/src/commands/user_registry.rs`, `crates/tui/src/skills/mod.rs`, PR #4762 conversation, issues #4227/#4032/#4042*

*Credit: written with CodeWhale (deepseek-v4-pro) assistant, guided by Jay, and shaped by multiple agents and assistants, human and otherwise.*

*All of it. Fine.* 🏄🐋
