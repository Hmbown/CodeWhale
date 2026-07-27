# Surf — Hand-off

**Date:** 2026-07-26
**Branch:** `wip/onboarding_suit` (PR #4762 — closed)
**Status:** Concluded. This document captures the outcome, the landscape, and the path forward.

---

## What Happened

PR #4762 was opened as a draft, reviewed by the audit trail (three documents), and closed by Hmbown with this guidance:

> *"closing this draft so we can fold the spirit of it into the existing grok tutorial docs rather than introduce 'Surf' as a new product term. The goal you're chasing (a simple, discoverable, deterministic testbed/onboarding story) is exactly right; we just want to keep the vocabulary minimal so people can find it easily. We'll capture the useful parts as a tutorial under the existing docs/onboarding surface. If you'd like to help shape that tutorial directly, a fresh PR against the tutorial docs (no new branded term) would be very welcome."*

**Translation:** The architecture is right. The name is not. Build it, but call it what it is — onboarding docs, tutorial, contributor setup — so new contributors find it instantly.

---

## What Was Learned

Over four documents and three days of code tracing, this chase produced:

| Document | What it captured |
|---|---|
| `Surf_Skill_Flow_Design.md` | The blueprint — state machine, 4 bash scripts, receipt format |
| `Surf_Audit_2026-07-24.md` | The reality check — `execute:` frontmatter gap, discovery path, SKILL.md drift, codebase traces |
| `Surf_PR4762_Response_Report.md` | The map — 7 existing CodeWhale structures Surf echoes (Fleet, worktrees, constitution, hooks, memory, #4032, #4042) |
| `Surf_Closing_Reflection.md` | The shape — composable verification primitive, three operating modes, pipeable JSON |

**The irreducible insight:** The pipeline is simple. Four bash scripts. `git pull --ff-only` → `cargo fmt --check` → `cargo clippy` → `cargo test --workspace` → print JSON receipt. The vision — "receipts running along the quiet stream" — is standard Unix composition. `surf ride --json | whatever`. The documentation weight came from proving this fits alongside Fleet, not from the tool itself being complicated.

---

## The Landscape Going Forward

### What already exists

`CONTRIBUTING.md` already describes the contributor loop: clone, build, `cargo fmt`, `cargo clippy`, `cargo test`. It's manual — a human reads prose and runs commands. The Surf proof-of-concept automates that loop into one deterministic script.

There is no `docs/onboarding/` directory yet. No tutorial docs for new contributors beyond `CONTRIBUTING.md`. Hmbown's "existing grok tutorial docs" is aspirational — it's a surface that *should* exist, and this is the invitation to help create it.

### The two-stream vision

Jay's closing comments from the PR sketch a layered model:

> *"I envision a 'fleet-stream' (the Fleet JSON/vocabulary) and also a 'surf-stream/layer' (Verification layer)."*

This maps cleanly to what the audit found:

- **Fleet-stream** — Durable workers, JSONL ledger, `fleet run` / `fleet inspect` / `fleet resume`. The heavy orchestration layer. Owned by CodeWhale core.
- **Surf-stream** — Deterministic shell scripts, JSON receipts, `surf status` / `surf ride` / `surf inspect`. The lightweight verification layer. Can run standalone or feed Fleet.

And the bridge between them:

> *"Surf using a deterministic shell-script that encapsulates Fleet to do a LLM verification/quality pass."*

Meaning: Surf's bash scripts can call `codewhale fleet run` with a task spec, capture the Fleet receipt, and present a human-readable summary. The LLM never touches the verification — Surf owns the deterministic wrapper, Fleet owns the execution, the LLM is optional flavor on top. This is exactly the "LLM optional" principle from the original design doc, applied at the Fleet integration layer.

### Where the name lives

Hmbown asked to drop "Surf" as a product term. Fair. But as Jay noted:

> *"The technical term will stick. For me, and probably for the project. Naming stuff helps compartmentalise."*

The name can live as internal vocabulary — a directory name, a script prefix, a commit-message tag — without being a user-facing product term. Internally: "the surf scripts." Externally: "the contributor verification tutorial." Same tool, two namespaces.

---

## Concrete Next Steps

### Immediate (this branch)

- [ ] Archive or delete `SKILL.md` (stale, references old onboarding-suite naming)
- [ ] Archive or delete `Skill_Flow_Design (old).md` (superseded by v2.0 design)
- [ ] Archive or delete `surf.md` and `surf-setup.md` (`execute:` frontmatter doesn't work)
- [ ] Keep the 4 bash scripts, the design doc, the audit, the response report, and the closing reflection
- [ ] The branch can stay as a reference; no need to delete it

### Short-term (new PR against tutorial docs)

- [ ] Check whether a `docs/onboarding/` or `docs/tutorial/` directory has been created since this hand-off was written
- [ ] Draft a contributor verification tutorial that covers the `clone → pull → fmt → clippy → test → receipt` loop
- [ ] Use the Surf bash scripts as reference implementation, but write the tutorial in plain prose a human follows manually (no new tool required)
- [ ] Link from `CONTRIBUTING.md` to the tutorial
- [ ] Do not introduce "Surf" as a branded term in user-facing docs

### Medium-term (tooling)

- [ ] Add `--json` flag to the bash scripts so they emit machine-readable receipts
- [ ] Replace `read -p` in `catch-wave.sh` with positional args
- [ ] Test the pipe: `surf ride --json | codewhale fleet run` (once Fleet supports stdin task specs, or via temp file)
- [ ] Consider contributing a `codewhale fleet run --stdin` feature if the pipe path proves useful

### Long-term (vision)

- [ ] A contributor runs one command and gets a receipt: "your environment is clean, main is at abc123, all 6384 tests pass, here's what changed since yesterday"
- [ ] That receipt feeds into Fleet for deeper verification, or into `jq` for scripting, or into a CI check — same JSON, same stream, different destinations
- [ ] The "quiet stream" flows between Surf, Fleet, Workflow, and CI without any tool needing to know about the others'

---

## The Shape (Final)

> **Surf is a receipt-producing, JSON-emitting, pipe-friendly verification surface that sits between a contributor's local checkout and the Fleet/CI infrastructure. It mirrors Fleet's verb vocabulary without depending on Fleet's internals. It produces structured evidence that any downstream tool — Fleet, `jq`, a Workflow, a CI runner, a human reading a terminal — can consume.**

That's the geometry. The proof of concept is four bash scripts doing `git pull --ff-only`, `cargo fmt`, `cargo clippy`, `cargo test`. The name is internal. The next step is a tutorial PR that captures the spirit in contributor-facing docs.

---

## Documents in This Branch

| File | Keep? | Reason |
|---|---|---|
| `Surf_Skill_Flow_Design.md` | ✅ Keep | Canonical design reference |
| `Surf_Audit_2026-07-24.md` | ✅ Keep | Codebase-grounded gap analysis |
| `Surf_PR4762_Response_Report.md` | ✅ Keep | Maps existing CodeWhale structures |
| `Surf_Closing_Reflection.md` | ✅ Keep | Closes the arc, defines the shape |
| `Surf_Handoff.md` | ✅ Keep | This document — hand-off / outcome |
| `surf.sh`, `check-wave.sh`, `catch-wave.sh`, `ride-wave.sh` | ✅ Keep | Working proof-of-concept scripts |
| `surf.md`, `surf-setup.md` | ❌ Archive | `execute:` frontmatter unsupported |
| `SKILL.md` | ❌ Archive | Stale, references old onboarding-suite naming |
| `Skill_Flow_Design (old).md` | ❌ Archive | Superseded by v2.0 design doc |

---

*Sources: PR #4762 conversation (closed by Hmbown 2026-07-26), issues #4227/#4032/#4042, `docs/FLEET.md`, `docs/SUBAGENTS.md`, `docs/CONFIGURATION.md`, `CONTRIBUTING.md`, `crates/tui/src/commands/user_registry.rs`, `crates/tui/src/skills/mod.rs`*

*Credit: written with CodeWhale (deepseek-v4-pro) assistant, guided by Jay, and shaped by multiple agents and assistants, human and otherwise.*

*All of it. Fine.* 🏄🐋
