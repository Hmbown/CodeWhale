# Surf PR #4762 — Response Report

**Date:** 2026-07-24
**Based on:** PR #4762 conversation, linked issues (#4227, #4032, #4042), current milestone state, and repo docs
**Scope:** Responding to Jay's open questions with repo-grounded answers

---

## What Jay Asked

From the PR conversation, Jay raised several explicit and implicit questions:

1. *"Am I reinventing the same wheel, in slightly different packaging, over and over again?"*
2. *"Do I need to be more aware of structures that are already there? Concepts I can mirror, align with, or incorporate — rather than building alongside them?"*
3. *"Which execution model makes sense?"* (from the audit report's resolution paths)
4. *"What is the shape of the thing that I'm building toward?"*
5. *"Is this a new tool, or the same tool, being surfed from a different angle?"*
6. The CLI pipeline idea: `surf status | jq`, `surf ride --json`, `surf inspect --receipt` — "a toolkit, not just a script"

---

## Existing Structures That Surf Echoes (or Could Align With)

Jay's instinct that "very similar structures already exist, possibly on multiple levels" is correct. Here's what's in the codebase today that resonates with Surf's shape:

### 1. Agent Fleet — the durable worker runtime

`docs/FLEET.md` describes a local-first control plane for durable multi-worker runs with:

- `codewhale fleet init` — initialize a ledger
- `codewhale fleet run tasks.json --max-workers 4` — run workers
- `codewhale fleet status` — inspect state
- `codewhale fleet inspect/logs/artifacts <worker-id>` — drill into results
- `codewhale fleet interrupt/restart/resume <id>` — lifecycle management
- State stored in `.codewhale/fleet.jsonl` (JSONL ledger) and `.codewhale/fleet/` (logs)
- Workers are headless `codewhale exec` runs — not a separate engine

**Resonance with Surf:** The "launch something → check status → inspect receipt" pattern is exactly Fleet's CLI surface. Surf's orchestrator (`surf.sh` → check → ride → receipt) is a simpler, single-worker version of Fleet's multi-worker orchestration. The `.surf-config` file is structurally parallel to Fleet's task spec JSON.

**Question this raises:** Should Surf be a Fleet profile rather than a standalone suite? A Fleet task spec that says "clone this repo, run fmt/clippy/test, write a receipt" does what `ride-wave.sh` does, but with Fleet's ledger, retry, and resume baked in.

### 2. Git worktree isolation (sub-agents and Fleet workers)

`docs/SUBAGENTS.md:95-106` describes worktree isolation:

> For parallel edit lanes, launch the child with `worktree: true`. Codewhale creates a fresh git worktree and branch for that child, runs the child from the isolated checkout, and reports the resulting workspace/branch… By default the branch is `codex/agent-<name>-<id>` and the checkout lives beside the parent repo under `.codewhale-worktrees/`, so the parent checkout stays clean.

**Resonance with Surf:** Surf's entire value proposition is "isolated testbed that stays clean." That's literally what `worktree: true` on a Fleet worker or sub-agent does — except CodeWhale already has the mechanism, the naming convention (`codex/agent-<name>-<id>`), and the lifecycle management. Surf's `catch-wave.sh` (clone → `.surf-config`) is a manual version of Fleet's automatic worktree creation.

**Question this raises:** Should `surf setup` just be `codewhale fleet run` with a task that creates a worktree? Does Surf need its own `.surf-config` marker when CodeWhale already has `.codewhale/fleet.jsonl`?

### 3. Repo-local constitution (`.codewhale/constitution.json`)

`docs/CONFIGURATION.md:32-33`:

> **Repo-local constitution** — optional project policy in `.codewhale/constitution.json`… Compiled into write holds that even Full Access can't skip.

**Resonance with Surf:** Surf's `.surf-config` (a repo-URL-and-branch marker) sits in the same namespace as `.codewhale/constitution.json`. The constitution mechanism is how CodeWhale enforces repo policy statically — exactly the kind of "write hold" that prevents a testbed from being accidentally mutated. If Surf wants to protect a testbed, a `.codewhale/constitution.json` that gates writes is the existing mechanism, not a custom `.surf-config` marker.

### 4. Hooks (pre/post-turn shell scripts)

`docs/CONFIGURATION.md` covers hooks — user-configured shell commands that run before or after turns. The config format:

```toml
[hooks]
pre_turn = "cargo fmt --check"
post_turn = "scripts/receipt.sh"
```

**Resonance with Surf:** Surf's `ride-wave.sh` is essentially a manually-triggered pre-commit hook: fmt → clippy → test. CodeWhale hooks automate this per-turn. The difference is that hooks are reactive (run on every turn) while Surf is proactive (user decides when to ride). But the verification pipeline is identical.

### 5. User memory (`~/.codewhale/memory.md`)

`docs/MEMORY.md` describes persistent notes injected into the system prompt:

> It's the place to put preferences and conventions that should survive across sessions — "I prefer pytest over unittest", "always run `cargo fmt` before committing"

**Resonance with Surf:** Jay's original issue #4227 said "I'm spending more time catching up than building." Memory is how CodeWhale already handles "persistent contributor context." If the real problem is "I forget what changed since yesterday," memory + a Fleet worker that generates a digest might be lighter than a full testbed suite.

### 6. Issue #4032 — Constitutional crisis (now closed, milestone v0.9.1)

This is the issue that started this whole thread. Jay wanted CodeWhale to follow his SKILL.md instructions (including "no ad-hoc scripts") deterministically. The resolution was structural enforcement through `--disallowed-tools` and the gate chain (#4042), not prompt-based persuasion.

**Resonance with Surf:** This is the core insight of Surf's design: "deterministic by default, LLM optional." The lesson from #4032 is that structural mechanisms (tool restrictions, gate chains, sandboxes) work; prompt instructions don't guarantee compliance. Surf's bash scripts are the structural mechanism — they can't go off-script because they're not LLM-driven. This validates Surf's deterministic-core approach.

### 7. Issue #4042 — Tool sandboxing (closed, milestone v0.9.0)

This was the structural enforcement counterpart to #4032. It validated that `--disallowed-tools` flows across sessions, sub-agents, and Fleet workers. The key finding: "the mechanism already exists."

**Resonance with Surf:** If Surf needs to run in a restricted environment (e.g., a testbed that shouldn't write to the parent repo), the `--disallowed-tools` + worktree isolation is the existing CodeWhale answer. Surf's `.surf-config` marker and Surf's clean/dirty checks are doing the same thing at the bash level that CodeWhale already does at the Rust/tool-gate level.

---

## Answering Jay's Questions

### "Am I reinventing the same wheel?"

**Partially, yes — but the wheel is worth reinventing if it's lighter.**

Surf's bash scripts (`surf.sh`, `check-wave.sh`, `ride-wave.sh`) are doing what Fleet + worktrees + hooks do, but with zero dependencies, zero Rust, and zero LLM. That's not wasted effort — it's a different point on the complexity spectrum.

However, the CLI pipeline idea Jay mentioned is where the duplication becomes visible:

```bash
surf status | jq '.status'
surf ride --json | jq '.digest'
```

This is almost verbatim what Fleet already has:

```bash
codewhale fleet status
codewhale fleet inspect <worker-id> --json | jq '.digest'
codewhale fleet artifacts <worker-id>
```

If Surf's CLI pipeline is the goal, Fleet already has the JSON output, the ledger, the retry, the resume. Building Surf as a Fleet profile or a Workflow task gets you all of that for free.

**The synthesis:** The bash scripts are fine as plumbing. The question is whether the user-facing surface should be `/surf` (a new command namespace) or a Fleet task spec that says "here's my testbed: clone, verify, receipt." The latter integrates with existing infrastructure; the former creates a parallel surface.

### "Do I need to be more aware of structures that are already there?"

**Yes. Here's a map:**

| Surf Concept | Existing CodeWhale Structure | Location |
|---|---|---|
| `.surf-config` marker | `.codewhale/constitution.json`, `.codewhale/fleet.jsonl` | `docs/CONFIGURATION.md`, `docs/FLEET.md` |
| Isolated testbed | `worktree: true` on Fleet worker / sub-agent | `docs/SUBAGENTS.md:95-106` |
| `ride-wave.sh` (fmt/clippy/test) | Hooks (`[hooks] pre_turn`) | `docs/CONFIGURATION.md` |
| `receipts/latest_receipt.json` | Fleet ledger + worker artifacts | `docs/FLEET.md` |
| State machine (empty/testbed/dirty/unknown) | Fleet worker status (pending/running/succeeded/failed/interrupted) | `docs/FLEET.md` |
| Deterministic execution (no LLM) | `--disallowed-tools` + gate chain (#4042) | `docs/SANDBOX.md` |
| Fork-aware config | Fleet task `repo` + `branch` fields | `docs/FLEET.md` |
| Digest from CHANGELOG | Workflow post-task aggregation | `docs/WORKFLOW_AUTHORING.md` |

**The most under-explored alignment:** Fleet worktree isolation. If you give Fleet a task spec that says "clone this fork, branch this feature, run these verification commands, write a receipt," you get:

- Worktree isolation (no dirty-tree risk)
- Ledgered audit trail (survives restarts)
- `codewhale fleet resume` (pick up where you left off)
- Existing CLI surface (no new `/surf` command needed)
- Structured JSON output (already pipeable to `jq`)

### "Which execution model makes sense?"

Based on what exists today, the answer depends on what you're optimizing for:

**If you want something that works today (zero code changes):**
- Ship Surf as a standalone bash toolkit (CLI pipelines). `surf status`, `surf ride --json`, `surf inspect`. No TUI integration needed. Just `alias surf=./scripts/surf.sh`.

**If you want TUI integration with existing infrastructure:**
- Make Surf a **Fleet task spec** + a **Workflow**. Users do `codewhale fleet run surf-testbed.json`. The Workflow handles the state machine, the Fleet handles the ledger/retry. No new commands needed.

**If you want the full `/surf` command experience:**
- The `execute:` frontmatter gap is real (see audit). You'd need to add it to `user_registry.rs`. This is the heaviest lift but gives the slickest UX.

**If you want the `$surf` skill experience:**
- Skills can't execute scripts (see audit). You'd be instructing the LLM to run them, which contradicts the deterministic principle. This path doesn't align with your design goals.

**Recommendation from the evidence:** Start with the Fleet/Workflow path. It gives you the most features (ledger, retry, resume, worktree isolation, pipeable JSON) for the least new code. The bash scripts become the verification commands inside a Fleet task spec. If that proves too heavy for daily use, the standalone CLI pipeline is the lighter fallback.

### "What is the shape of the thing I'm building toward?"

Based on the trajectory from #4032 → #4227 → PR #4762, the shape is:

> **A contributor-local verification surface that sits between "I just cloned the repo" and "I'm ready to submit a PR."** It should be deterministically reproducible, fork-aware, and produce structured evidence that the contributor's environment is in a known-good state.

This is a real need. It bridges the gap between CI (which runs on GitHub's machines) and local development (where "works on my machine" is the norm). The question isn't whether to build it — it's whether to build it *inside* CodeWhale (as a command/skill) or *alongside* CodeWhale (as a Fleet profile or standalone CLI).

The CLI pipeline vision (`surf status | jq`, `surf ride --json`) suggests "alongside" is the natural shape. The tool doesn't need to be inside the TUI to be useful — it needs to be composable.

### "Is this a new tool, or the same tool, surfed from a different angle?"

**It's both.** Every iteration has clarified the shape:

- v0 (original #4032 SKILL.md): "Constitution-enforcing skill with deterministic scripts"
- v1 (#4227 onboarding issue): "Sync/build/verify/digest for contributors"
- v2 (old `Skill_Flow_Design.md`): "Onboarding Suite with 5-step flow"
- v3 (current `Surf_Skill_Flow_Design.md`): "Surf — deterministic testbed management"

Each version strips away assumptions and gets closer to the irreducible core: **isolated checkout → pull → verify → receipt.** The remaining question is whether that core belongs in bash scripts, in a Fleet task spec, or as a CLI pipeline. The answer might be "all three, at different levels of the stack."

---

## The CLI Pipeline Vision (with real evidence)

Jay's comment about CLI pipelines is the most promising direction in the PR conversation:

> *"The more I think about the CLI integration side of this, the more excited I get."*

This aligns with how CodeWhale's own Fleet CLI works today:

```bash
# Fleet already does this pattern:
codewhale fleet status | jq '.workers[].status'
codewhale fleet inspect <id> --json | jq '.receipt'
codewhale fleet artifacts <id> | grep "test_results"

# Surf could follow the same pattern:
surf status --json | jq '.state'
surf ride --json | jq '.tests.passed'
surf inspect --receipt receipts/latest_receipt.json | jq '.commit'
```

The Fleet CLI proves this model works in the CodeWhale ecosystem. Surf's bash scripts could adopt the same `--json` flag convention and become a lightweight companion tool — no TUI changes needed.

---

## What the Audit Revealed (in context of the PR discussion)

The audit (`Surf_Audit_2026-07-24.md`) identified the `execute:` frontmatter gap as the root blocker. In the context of Jay's broader questions:

- **The gap is real** — but it may be a signal that the TUI command path is the wrong integration point
- **The scripts are sound** — they work, they're well-structured, they do what the design says
- **The discovery issue** (`onboarding/` vs `.codewhale/` root) is a packaging question, not an architecture question
- **The SKILL.md drift** confirms Jay's instinct: the design has been iterating faster than the implementation, and some artifacts are stale

The audit validates that the current PR is exactly what Jay said it is: "a design sketch and work-in-progress." It is not ready to merge. But it is coherent enough to ground a conversation about direction.

---

## Suggested Next Steps (informed by repo evidence)

1. **Experiment with Fleet worktree isolation.** Create a Fleet task spec that does what `catch-wave.sh` + `ride-wave.sh` do: clone (or worktree), verify, receipt. Compare the ergonomics to the bash-script approach. The Fleet path gives you ledger, retry, and resume for free.

2. **Prototype the CLI pipeline surface.** Write `surf status`, `surf ride --json`, `surf inspect --receipt` as standalone bash scripts with `--json` output. Test pipeability with `jq`. This doesn't require any CodeWhale changes.

3. **Archive the stale SKILL.md** and either update it or delete it. The `Skill_Flow_Design (old).md` should also be removed — it's confusing to have both in the tree.

4. **Decide on ownership of the PR.** If this is staying as a design sketch, mark it as such in the PR description and link to this report. If it's evolving toward a mergeable artifact, pick one of the execution models (Fleet profile, CLI pipeline, or `execute:` feature) and narrow the PR to just that.

5. **Close the loop with #4227.** The original onboarding issue is in milestone v0.9.2 and still open. If Surf is the answer to that issue, link the PR. If Surf is a separate thing, say so explicitly so the issue can be triaged independently.

---

*Sources: PR #4762 conversation, issues #4227/#4032/#4042, `docs/FLEET.md`, `docs/SUBAGENTS.md`, `docs/CONFIGURATION.md`, `docs/MEMORY.md`, `docs/architecture/command-dispatch.md`, `crates/tui/src/commands/user_registry.rs`, `crates/tui/src/commands/groups/skills/skills.rs`, `crates/tui/src/skills/mod.rs`*
