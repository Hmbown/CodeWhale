# Repository Agent Guidance

## Start With Live Truth

This repo moves through release and integration lanes quickly. Do not rely on a
hard-coded branch, milestone, or version in this file. Before editing, establish
the current lane from live state:

```sh
git status --short --branch
git branch --show-current
git fetch origin main --prune --no-tags
gh issue list --repo Hmbown/CodeWhale --state open --limit 100 --json number,title,labels,milestone,updatedAt,url
gh pr list --repo Hmbown/CodeWhale --state open --limit 100 --json number,title,headRefName,baseRefName,isDraft,url
```

Use the user's current goal, live GitHub milestones, open PRs, and the current
branch as the source of truth. If local notes or old handoffs disagree with live
state, trust live state and mention the mismatch in your handoff.

## Branch And Release Safety

- Never commit directly to `main`.
- Work on the active integration branch or create a focused branch such as
  `issue/<number>-short-slug` from the correct live base.
- Keep each branch scoped to one issue or one reviewable concern unless issues
  are genuinely inseparable.
- Do not bump versions, tag, publish, create GitHub Releases, push release
  artifacts, or merge to `main` without Hunter's explicit approval.
- Preserve unrelated dirty or untracked files. Do not revert work you did not
  make.

## Working A GitHub Issue

1. Refresh live issue and PR state.
2. Check whether an open PR already covers the issue.
3. Inspect the issue body, linked PRs, comments, code, docs, and tests before
   deciding what to change.
4. Implement the smallest coherent slice that moves the issue toward done.
5. Format, run targeted tests, commit, push, and open a draft PR.
6. In the PR body include goal, changes, verification commands/results, risks,
   and the linked issue.

If the issue is already fixed, verify it from current code or CI before
commenting or closing. If blocked, leave a precise comment with the blocker,
attempted work, branch or commit if any, and next action.

## Verification Defaults

Run `cargo fmt` before pushing Rust changes. Then run the targeted tests for the
area you touched, for example:

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked <filter>
cargo test -p codewhale-config --locked <filter>
cargo test -p codewhale-protocol --locked <filter>
```

Use broader gates when the change crosses crate boundaries:

```sh
cargo test --workspace
cargo build --release -p codewhale-cli -p codewhale-tui
```

Known local-suite papercuts should be verified before blaming a new change.
Historically, config command tests can be affected by non-hermetic user config,
and some verifier background tests have been flaky under full-suite parallelism
while passing in isolation.

## Architecture And Product Guardrails

- Keep CodeWhale branding while preserving first-class DeepSeek model and
  provider support.
- Do not reintroduce removed model-facing sub-agent tool names. The current
  model-facing sub-agent surface is `agent`.
- Avoid speculative runtime systems such as capacity/coherence tags, lifecycle
  tools, or prompt/tag injection unless the current issue explicitly calls for a
  reviewed design.
- Prefer provider/model/Fleet changes that separate provider facts, model facts,
  offerings, route resolution, and runtime readiness.
- Treat provider docs and hosted model catalogs as time-sensitive. When current
  provider behavior matters, check the actual provider docs or API and add tests
  or drift checks where practical.

## Stewardship

- Treat community reports and PRs as maintainer evidence. Review code, tests,
  linked issues, comments, and check results before merging, harvesting,
  closing, or deferring.
- Preserve contributor credit for harvested work with authorship when possible,
  `Co-authored-by` trailers where appropriate, and clear PR/issue references.
- Keep gates helpful and dry-run unless Hunter approves enforcement.
- Keep public wording neutral for local hardening and internal reliability work.
