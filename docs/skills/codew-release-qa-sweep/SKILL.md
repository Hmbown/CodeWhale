---
name: codew-release-qa-sweep
description: "Use before claiming CodeWhale release work is done: run the full gate sweep and list the manual QA targets."
---

# CodeWhale Release QA Sweep

Run this before claiming any CodeWhale release work is "done." A green automated
gate sweep plus the three manual QA targets is the evidence bar. No sweep, no
"done" — report exactly what was run and the result of each step.

## When to use

- Before telling Hunter (or a PR thread) that release work is complete or
  merge-ready.
- After harvesting/landing PRs into the release branch, before the publish boundary.
- When verifying a release candidate on the **real** landing branch
  (e.g. `<release-branch>`), which is often local-only.

## Automated gate sweep

Run from the repo root, in order. Stop on the first failure and report it.

```bash
# 0. Confirm you are on the real release head, not a main-based assumption.
git branch --show-current          # expect e.g. <release-branch>
git status --short                 # working tree should be clean

# 1. Formatting + stray whitespace/conflict markers
cargo fmt --all --check
git diff --check

# 2. Library/protocol/cli/flow/state tests, locked
cargo test -p codewhale-config -p codewhale-protocol -p codewhale-cli \
  -p codewhale-workflow -p codewhale-state --locked

# 3. TUI test binaries, locked
cargo test -p codewhale-tui --bins --locked

# 4. TUI debug build, locked
cargo build -p codewhale-tui --locked

# 5. Release build for the shipped binaries, locked
cargo build --release --locked -p codewhale-cli -p codewhale-tui

# 6. Version-drift gate (workspace ↔ npm ↔ Cargo.lock ↔ changelog ↔ README)
./scripts/release/check-versions.sh

# 7. Binary smoke
./target/release/codewhale --version
```

If you are validating a PR for landing, also test mergeability against the
**actual** release head, never the main-based clean flag:

```bash
git merge-tree $(git merge-base <release-branch> <pr-head>) <release-branch> <pr-head>
```

A PR that is clean against `main` can still conflict with the release branch.

## Manual QA targets

Automated gates do not cover the live TUI. Exercise all three and record what you saw:

1. **Six-worker fanout liveness (#3216/#2211).** Spawn 6 sub-agents. Confirm
   typing, render, cancel, and the sidebar stay live throughout, and that **Esc
   cancels mid-fanout** (prompt interrupt, not a wedged ~24s burst or freeze).
   For the Windows Terminal retest path from #3289, start in plan mode, add
   follow-up input to the plan, press Esc, switch to yolo/accept flow, trigger
   at least two auto/Fleet worker spawns, and keep typing/cancel/mode-switch
   checks live for several minutes. Attach logs if the freeze reproduces.
2. **Multi-terminal route isolation (#3227).** Open multiple terminals on
   distinct provider/model routes. Confirm zero cross-terminal contamination and
   no provider+model mismatch — each terminal honors its own route.
3. **Queued steering + Ctrl+S (#3203).** Queue a steering message into a busy
   turn; confirm Ctrl+S sends the queued/draft message and queued-steering
   status reads clearly.

## Reporting format

Report a checklist: each command, pass/fail, and the salient output line
(test counts, the `--version` string, `check-versions.sh` verdict). For manual
QA, state what you actually observed per target, citing the issue number. If a
step was skipped or could not be run (e.g. no display for TUI QA), say so
explicitly — do not imply coverage you do not have.

## Red flags / don't

- Don't claim "done," "passing," or "merge-ready" without the evidence above.
  Assertions without command output are not acceptable.
- Don't trust the main-based mergeability flag for a release branch; use
  `git merge-tree` against the real head.
- Don't skip the manual TUI targets because the build is green — the freeze,
  route-mismatch, and steering regressions live in the runtime, not the gates.
- Don't tag, publish, create a GitHub Release, push artifacts, or merge/close
  any PR or issue without Hunter's explicit approval. A green sweep is readiness
  evidence, not permission.
- Never harvest/close from a PR title or label alone — review from code, tests,
  comments, and checks.
- When the sweep clears a harvested PR, preserve contributor credit: cherry-pick
  keeps the original author, otherwise add `Co-authored-by: Name <email>` and
  `Harvested-from: PR #N by @handle` so the auto-close-at-main workflow credits
  the contributor.
- Keep any contributor-facing comment positive and crediting; gates stay
  dry-run/advisory unless Hunter approves enforcement.
