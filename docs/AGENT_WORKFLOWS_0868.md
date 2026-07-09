# CodeWhale v0.8.68 — Agent Workflow Playbook

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
# 1. Sync and verify branch (implementation always from main)
cd CodeWhale
git fetch origin
git checkout main && git pull origin main
git checkout -b codex/v0868-fix-<issue>   # e.g. codex/v0868-fix-4090
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

Work top-to-bottom. **Do not start Waves 2–4 or v0.8.69 refactors until stopship
is green** (#4090, #4093, #4094 closed or verified fixed on `main`).

| Wave | Workflow file | Theme | GitHub issues | Status |
|------|---------------|-------|---------------|--------|
| 0 | `v0868_issue_sweep.workflow.js` | Triage + release plan | all milestone | On demand |
| 1 | `v0868_stopship_lane.workflow.js` | Release blockers + dogfood regressions | #4090, #4093, #4094, #3986, #3990 | **Active** |
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

Branch from `main` before starting implementation agents:

```bash
git checkout main && git pull origin main
git checkout -b codex/v0868-stopship-<issue>
```

### Fleet-backed stopship lane (dogfood #4178)

Named fleet file: [`fleets/v0868-stopship.toml`](../fleets/v0868-stopship.toml)
(roles: `scout`, `implementer`, `reviewer`, `verifier`, `release_lead`).
Workflow: `workflows/v0868_stopship_lane.workflow.js` (steps bind
`profile` / fleet roles — not raw provider/model identity).

**Target shape** (requires Phase 1 Lane CLI #4176 + Phase 2 role resolution #4177):

```bash
# Create a durable tmux-backed lane bound to stopship + fleet
codewhale lane start \
  --workflow stopship \
  --fleet v0868-stopship \
  --runtime tmux \
  --issue 4090 \
  -- codewhale exec --auto --output-format stream-json \
    "Run workflows/v0868_stopship_lane.workflow.js with fleet v0868-stopship. Fix #4090, #4093, #4094. Branch from main."

codewhale lane list
codewhale lane attach <lane-id>          # or: codewhale lane attach <lane-id> --print
codewhale lane logs <lane-id>
codewhale lane stop <lane-id>
```

Validate fleet role resolution without launching agents:

```bash
# Pure unit path (CI-safe)
cargo test -p codewhale-workflow --lib named_fleet
```

### Interim paths (until `workflow run --fleet` lands)

From CodeWhale TUI or headless exec:

```bash
# Headless stopship lane (preferred for CI/VM agents today)
codewhale exec --auto --output-format stream-json \
  "Run workflows/v0868_stopship_lane.workflow.js on branch codex/v0868-stopship. Fix #4090, #4093, #4094. Branch from main. Use fleet profiles scout/builder/reviewer/verifier from fleets/v0868-stopship.toml."

# Per-issue headless (single stopship issue)
codewhale exec --auto --output-format stream-json \
  "Run workflows/v0868_issue_implement.workflow.js for issue #4090. Branch from main."

# TUI explicit path
/workflow start workflows/v0868_stopship_lane.workflow.js
```

Workflows use read-only scouts first, then implementation agents in sequence.
Write agents require approval in default modes; use `--auto` for headless VM runs.
Do **not** close #4090/#4093/#4094 until human-verified on `main`.

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
