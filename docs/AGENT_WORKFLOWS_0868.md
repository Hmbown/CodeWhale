# CodeWhale v0.8.68 — Agent Workflow Playbook

> **Status (2026-07-15): Historical execution playbook superseded by v0.9.0.**
> v0.8.68 was never tagged or publicly published,
> and the waves/checkmarks below are not proof that current acceptance criteria
> or release gates pass. Use the current
> [v0.9.0 release-candidate ledger](releases/v0.9.0-release-candidate.md) for
> release truth. The settled product vocabulary remains useful and should not
> be re-derived: **Fleet = who · Workflow = what order · Lane = running
> instance · Runtime = where/how**. The architecture/dogfood issues linked
> below remain GitHub-tracked work until their acceptance evidence is reviewed.
> As of 2026-07-15, milestone #53 is closed historical. Do not use this
> playbook or the `v0868_*` workflows to assign new issues. Current agent-task
> intake is version-neutral, and maintainers assign the active milestone
> explicitly (the first post-v0.9.0 roadmap milestone is `v0.9.1`).

This document tells autonomous agents how to systematically complete the v0.8.68
release. It pairs with:

- **Milestone:** `v0.8.68` (GitHub milestone #53)
- **Architecture tracker:** issue [#4175](https://github.com/Hmbown/CodeWhale/issues/4175) (Fleet / Workflow / Lane / Runtime)
- **Triage packet:** issue [#4092](https://github.com/Hmbown/CodeWhale/issues/4092)
- **Master checklist:** `CODEWHALE_0_8_68.md` (harness workspace) or tracker in repo root after merge
- **Workflow files:** `workflows/v0868_*.workflow.js`

### Architecture phases (post-stopship product work)

| Phase | Issue | Scope |
|-------|-------|-------|
| 1 | [#4176](https://github.com/Hmbown/CodeWhale/issues/4176) | Lane CLI + Runtime (tmux, worktrees, logs) |
| 2 | [#4177](https://github.com/Hmbown/CodeWhale/issues/4177) | Workflow steps → Fleet roles |
| 3 | [#4179](https://github.com/Hmbown/CodeWhale/issues/4179) | Gates and handoffs between roles |
| Dogfood | [#4178](https://github.com/Hmbown/CodeWhale/issues/4178) | Stopship as fleet-backed lane |

Vocabulary: **Fleet** = who · **Workflow** = what order · **Lane** = running instance · **Runtime** = where/how (tmux, VM, CI).

## Source of truth

- **Implementation base:** `main` — all v0.8.68 fix branches start here. PR #4099
  merged the quick-win cutover; do not use `work/v0.9.0-cutover` or
  `.cw-worktrees/v0867-pr4047`.
- **`codex/0868-next`:** stale reference only. Cherry-pick from it only when a
  specific issue needs a specific commit — never treat it as the active dev branch.
- **Playbook/workflow definitions:** merged in [PR #4163](https://github.com/Hmbown/CodeWhale/pull/4163) on `main`; implementation PRs branch from `main`.

## Defer policy (v0.8.69 / architecture refactors)

Defer v0.8.69 refactors and broad feature lanes unless they **directly unblock**
a stopship issue (#4090, #4093, #4094).

| Category | When | Notes |
|----------|------|-------|
| Stopship (#4090, #4093, #4094) | **Now** | Wave 1 — release-blocking |
| Dogfood regressions (#3986, #3990) | After stopship | Same lane, lower priority |
| Catalog lane (Wave 2) | After stopship green | #4109, #4114–#4119, #4139–#4141, #4184–#4188 |
| Workflow UI lane (Wave 3) | After stopship green | #4038, #4110, #4120–#4135 |
| TUI copy lane (Wave 4) | After stopship green | #4112, #4142–#4148 |
| v0.8.69 refactors / 0.9.0 architecture | **Deferred** | Unless required to fix #4090/#4093/#4094 |

Issues labeled `v0.8.69` still in milestone `v0.8.68` should be reclassified to
DEFER (0.9.0) during sweep unless tied to a stopship fix.

## Quick start

```bash
# 1. Use a clean disposable checkout for acceptance. Do not create a fix branch.
cd CodeWhale
git fetch origin
git status -sb

# 2. Board truth
gh issue list -R Hmbown/CodeWhale --milestone "v0.8.68" --state open --limit 200
gh pr list -R Hmbown/CodeWhale --state open --limit 50 \
  --json number,title,isDraft,mergeable,milestone

# 3. Read the triage packet (do not skip)
gh issue view 4092 -R Hmbown/CodeWhale

# 4. Run verification gate before and after changes
cargo fmt --all --check
cargo clippy --workspace --all-features --locked -D warnings \
  -A clippy::uninlined_format_args -A clippy::too_many_arguments \
  -A clippy::unnecessary_map_or -A clippy::collapsible_if -A clippy::assertions_on_constants
cargo test --workspace --locked
cargo build --release -p codewhale-tui
```

## Execution order (waves)

The implementation waves below are historical. The active Wave 1 task is the
read-only orchestration acceptance run; do not use it to reopen or recreate fix
branches for the already-landed #4090/#4093/#4094 work.

| Wave | Workflow file | Theme | GitHub issues | Status |
|------|---------------|-------|---------------|--------|
| 0 | `v0868_issue_sweep.workflow.js` | Triage + release plan | all milestone | On demand |
| 1 | `v0868_stopship_lane.workflow.js` | Read-only orchestration acceptance | #4175, #4177, #4178, #4179 | **Active** |
| 2 | `v0868_catalog_lane.workflow.js` | Model catalog + Models.dev live catalog | #4109, #4114–#4119, #4139–#4141, #4184–#4188 | Deferred |
| 3 | `v0868_workflow_ui_lane.workflow.js` | Workflow orchestration UI | #4038, #4110, #4120–#4135 | Deferred |
| 4 | `v0868_tui_copy_lane.workflow.js` | Transcript/copy polish | #4112, #4142–#4148 | Deferred |
| 5 | `v0868_release_gate.workflow.js` | Final verification + handoff | milestone closeout | After Waves 1–4 |

### Models.dev live catalog chain (Wave 2)

Execute sequentially after stopship is green:

**#4184 → #4185 → #4186 → #4187 → #4188**

| Issue | Scope |
|-------|-------|
| [#4184](https://github.com/Hmbown/CodeWhale/issues/4184) | Models.dev as source of truth for provider/model metadata |
| [#4185](https://github.com/Hmbown/CodeWhale/issues/4185) | Accept current live Models.dev schema in catalog parser |
| [#4186](https://github.com/Hmbown/CodeWhale/issues/4186) | Normalize Models.dev provider IDs onto CodeWhale provider kinds |
| [#4187](https://github.com/Hmbown/CodeWhale/issues/4187) | Fetch and cache live Models.dev catalog into ProviderLake |
| [#4188](https://github.com/Hmbown/CodeWhale/issues/4188) | Demote curated bundled model data after live catalog lands |

Parent tracker: [#4109](https://github.com/Hmbown/CodeWhale/issues/4109).

## How to launch a workflow

### Fleet-backed acceptance lane (dogfood #4178)

Named fleet file: [`fleets/v0868-stopship.toml`](../fleets/v0868-stopship.toml)
(roles: `scout`, `implementer`, `reviewer`, `verifier`, `release_lead`).
Workflow: `workflows/v0868_stopship_lane.workflow.js` (steps bind fleet
`role` — not raw provider/model identity).

**Target shape** (Phase 1 Lane CLI #4176 + Phase 2 role resolution #4177):

```bash
# Launch from a disposable checkout. The fixture is host-enforced read-only and
# does not create fix branches or edit the workspace.
codewhale workflow run stopship \
  --issue 4178 \
  --fleet v0868-stopship \
  --runtime tmux \
  --token-budget 360000 \
  --goal "Verify v0.8.68 role resolution, gates, and terminal receipts without editing the workspace."

codewhale lane list
codewhale lane status <lane-id>          # reconciles a finished tmux process
codewhale lane attach <lane-id>          # or: codewhale lane attach <lane-id> --print
codewhale lane logs <lane-id>
codewhale lane stop <lane-id>
```

`workflow run` validates the checked-in Workflow source and named Fleet roster,
creates the Lane record, and invokes the existing Workflow tool directly via
the selected Runtime backend. It does not spend an operator-model turn asking a
model to choose the tool. The Workflow driver resolves each `task({ role })`
through the named fleet before spawning sub-agents and emits live
`workflow_event` NDJSON receipts for run, phase, task, gate, and terminal state.
Inline writes each receipt to the Lane journal as it arrives. The tmux backend
launches a hidden Rust supervisor that frames arbitrary or binary child output
as valid NDJSON, removes its private 0600 JSON environment sidecar before the
child starts, and atomically publishes a separate bounded exit receipt.
`lane status` and `lane list` reconcile that receipt to `completed` or `failed`,
and fail a Lane closed when its tmux session vanished without a receipt. Start,
stop, and reconciliation transitions share a per-Lane lock; a failed or
unverifiable tmux kill leaves the Lane active and never triggers worktree
cleanup. Every tmux Lane also persists an explicit registry-owned server socket
and uses it for start, attach, liveness, and kill operations, so later changes
to `TMUX_TMPDIR` cannot redirect lifecycle commands to the wrong server.
Missing tmux and the not-yet-implemented VM/CI backends fail terminally instead
of reporting a fictional Running Lane.

The public `workflow run` command is the explicit approval of the Workflow
plan envelope and records `approved_explicit_cli_command` in the durable plan
receipt. That approval does not silently grant child shell or full-disk
authority. This acceptance fixture declares every role `read_only`; the host
therefore narrows every child to file listing, reading, and search even when
the named Fleet maps `implementer` to the built-in `builder` profile. Runtime
secrets are bridged outside persisted Lane argv, and an isolated worktree run
resolves its workspace from the Runtime-owned worktree cwd rather than the
original checkout.

The ordered chain explicitly exercises `scout`, `implementer`, `reviewer`,
`verifier`, and `release_lead`. Workflow-owned gates promote each successful
role output as a lane-scoped handoff and block the next role if the upstream
child fails or returns an exact first-line rejection verdict. Each fixture role
must begin its response with standalone `APPROVE` or `BLOCK`; the host maps that
first non-empty line to the gate outcome, while verdict words later in prose do
not control admission. Every fixture gate sets `require_explicit_verdict`, so a
missing or malformed first-line verdict blocks the next role instead of passing
on child completion alone. The host-emitted `gate_updated` receipt remains
authoritative. Prompts require grep-first, bounded source reads. Live DeepSeek
Flash and GLM-5-Turbo acceptance measured 17,457 and 17,550 tokens respectively
before the first useful tool turn completed, so the old 16k/12k/12k/12k/8k
caps could not admit even one turn. The fixture now budgets 24k per intended
model turn, with paired step/token caps of 4/96k, 3/72k, 3/72k, 3/72k, and
2/48k from scout through release lead. The 360k shared cap above matches that
bounded aggregate; a lower caller-supplied shared cap can still stop the chain
before every role runs. These are structural guardrails with measured
headroom, not proof that a live provider run will complete.
Acceptance needs all of these in `lane logs`: role-resolved `task_started`
events, passed or blocked `gate_updated` events, `run_completed`, and the
terminal Lane receipt.

Validate fleet role resolution without launching agents:

```bash
# Pure unit path (CI-safe)
cargo test -p codewhale-workflow --lib named_fleet
```

### Direct tool path

From a disposable checkout, the inline runtime provides a faster non-tmux check:

```bash
# Inline acceptance lane (deterministic host dispatch)
codewhale workflow run stopship \
  --fleet v0868-stopship \
  --runtime inline \
  --issue 4178 \
  --goal "Verify v0.8.68 orchestration receipts without editing the workspace."
```

The acceptance Workflow uses every Fleet role in sequence, including the
`implementer` role, while keeping each task host-enforced read-only. The
`workflow run` command approves only that checked-in plan envelope; its
children inherit the configured durable-task permission and sandbox posture. Interactive
`/workflow` runs retain their normal approval surfaces, but they are not a
substitute for the named-Fleet CLI acceptance path. The final `release_lead`
child owns synthesis so every spawned task remains Fleet-bound; do not append
an unroled reducer to this fixture. Keep `#4175`, `#4177`, `#4178`, and `#4179`
open until the live Lane log contains complete role, gate, and terminal
receipts.

## Per-issue implementation (single issue)

For one `agent-ready` issue:

1. `gh issue view <N> -R Hmbown/CodeWhale`
2. Confirm issue is in milestone `v0.8.68` and has label `v0.8.68`
3. Run `workflows/v0868_issue_implement.workflow.js` with the issue number in the goal
4. Or use headless: `codewhale exec --auto` with the issue body as prompt
5. Open PR referencing `Fixes #<N>`; do not close issues until merged

Label hygiene for agent execution:

```bash
gh issue edit <N> --add-label agent-in-progress --remove-label agent-ready
# after PR merged:
gh issue close <N> --comment "Fixed in PR #<PR>"
```

## PR harvest lane (parallel to waves)

Review community PRs without squashing authorship. Order from #4092:

| PR | Issue | Notes |
|----|-------|-------|
| #4088 | #4026 | Mergeable; terminal selection highlight |
| #4087 | #4082 | Draft refactor; finish review |
| #4084 | #4065 | Fleet alias cleanup |
| #3761 | #3757 | Conflicting; cherry-pick if needed |
| #3969 | #3965 | Conflicting; align with #4065 first |

## Skills to load

Copy or reference these maintainer skills from `docs/skills/`:

- `gh-compile-issues` — classify done/quick-fix/design/defer with evidence
- `codew-release-qa-sweep` — release gate commands
- `gh-find-prs` — locate related PRs before implementing

## Agent constraints

- **Do not** push to `main`, tag, release, or close issues without explicit approval
- **Do not** force-push or amend pushed commits
- **Do** cite `path:line` evidence for every "done" claim
- **Do** run the verification gate after each wave
- **Do** update issue #4092 with handoff notes when switching agents

## Milestone status (2026-07-07)

- **Source of truth:** `main` (PR #4099 merged — quick-win cutover landed)
- Milestone `v0.8.68` (#53): ~70 open / ~105 total
- Labels: `v0.8.68` synced with milestone membership
- Release blockers: #4093, #4094
- Top dogfood regression: #4090 (Ctrl+C re-prompt)
- **Deferred:** v0.8.69 refactors and Waves 2–4 until stopship green
- **Stale reference only:** `codex/0868-next` — cherry-pick per-commit when needed
