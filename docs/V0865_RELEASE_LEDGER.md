# v0.8.65 Release Ledger

Updated: 2026-06-23 10:54 America/Los_Angeles.

This ledger tracks completion by concrete outcome: merged, replaced by a clean PR, absorbed with evidence, closed after implementation evidence, or blocked only by Hunter approval.

## Current Baseline

| Item | Track | Status | Completion evidence required | Next action |
| --- | --- | --- | --- | --- |
| PR #3468 WeCom `activeTurnId` fix | B/H | Merged into `main` | PR #3468 merged after green CI | Done |
| PR #3472 retired sub-agent refs | B | Merged into `main` | PR #3472 merged after green CI | Done |
| PR #3476 digest archive route | B | Merged into `main` | PR #3476 merged after green CI | Done |
| PR #3473 fact-drift CI gate | B | Replaced | Conflicting PR #3473 closed with evidence; clean replacement PR #3481 merged (closes #3415) | Done |
| PR #3481 fact-drift CI gate replacement | B | Merged into `main` | PR #3481 squash-merged at `38f915208` after green CI | Done |
| PR #3479 YOLO git tag probe fix | B/H | Merged into `main` | PR #3479 merged after green CI | Done |
| Issue #3477 install script | C | Replaced by PR #3482 (merged) | PR #3482 closes #3477; `sh -n`, web lint/build, fake-release smoke completed locally | Done |
| PR #3482 install script | C | Merged into `main` | PR #3482 squash-merged at `61cc1cb81` after green CI | Done |
| Release ledger | A | Merged into `main` (PR #3483) | PR #3483 squash-merged at `f61182952`; this refresh on branch `codex/v0.8.65-ledger-update` | Keep updated as lanes land |

## Worktree Layout

The outer `/Users/hunter/Desktop/Harnesses/CodeWhale` directory is a harness folder, not a Git repository. Keep unrelated repos there, but do not use them for CodeWhale release work.

CodeWhale worktrees currently in use:

| Worktree | Branch | Purpose |
| --- | --- | --- |
| `CodeWhale` | `milestone/v0.8.65-provider-model-routing` | Dirty provider-routing stabilization work; do not reset |
| `CodeWhale-install-script` | `codex/install-script-website` | PR #3482 merged; worktree kept intact (do not clean without Hunter approval) |
| `CodeWhale-yolo-approval` | `codex/v0.8.65-yolo-git-readonly-approval` | PR #3479 merged; local worktree kept intact |
| `CodeWhale-pr3473-fix` | `codex/finish-pr-3473` | PR #3481 merged; worktree kept intact (do not clean without Hunter approval) |
| `CodeWhale-v0865-release-ledger` | `codex/v0.8.65-release-ledger` | PR #3483 merged; worktree kept intact (do not clean without Hunter approval) |
| `CodeWhale-v0865-ledger-update` | `codex/v0.8.65-ledger-update` | This ledger refresh after Phase 1 |

Unrelated repos under the same harness folder include `codew`, `codewhale-bench`, `codewhale-bench-v0862-final`, and `cw-deepswe`.

## Merge Order

1. Phase 1 queue complete: #3481, #3482, and ledger #3483 merged into `main`.
2. Stabilize provider route resolution from clean worktrees before dependent provider UI/pricing/context work (Track D).
3. Land Fleet substrate/loadout/persona work before Fleet parity proof and final docs.
4. Run full release verification before any version bump, tag, artifact publish, or GitHub Release.

## Blockers

| Blocker | Track | Owner action |
| --- | --- | --- |
| Version bump/tag/release | A | Blocked on Hunter approval only |

## Next: Track D provider/model routing

Stabilize provider/model route resolution from clean sibling worktrees off `origin/main` rather than growing the dirty `milestone/v0.8.65-provider-model-routing` branch. Target the known failing test `commands::groups::core::core::tests::two_sessions_keep_independent_provider_model_routes` and split foundational work into focused worktrees (route resolution, model catalog, wire protocol, fallback chain).

```bash
git worktree add ../CodeWhale-v0865-route-resolution -b codex/v0.8.65-route-resolution origin/main
```
