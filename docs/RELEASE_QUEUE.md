# Release queue and PR integration

Procedure for triaging a crowded release queue. Consult this when you are
actually working the queue; it is not a rule you break by not having memorized
it.

This guidance is for **live** work — open PRs and branches close enough to
`main` to reconcile honestly. For work that has drifted far enough that the
merge is an excavation, see "Intent is the artifact" in `AGENTS.md`: capture the
intent as an issue, drop the branch, rebuild from current `main`. A useful rule
of thumb — if the conflicts are in the files the branch most wanted to change,
you are reconstructing intent anyway; do it in the editor, not the merge tool.

## Working the queue

Order of attack: release blockers, recently approved PRs, clean PRs with small
scope, blocked PRs with obvious fixes, dirty PRs that can be harvested safely,
then larger architecture issues.

Start from the current GitHub release milestone named in `docs/ops/CURRENT.md`
and refresh state before acting:

```sh
gh issue list --repo Hmbown/CodeWhale --milestone "<current milestone>" --state open
```

Older per-version triage docs under `docs/` are historical reference only.

## Scratch integration branches

- Use scratch integration branches to expose conflicts, missing tests,
  duplicate work, and hidden coupling quickly. Name them like
  `scratch/vX.Y.Z-pr-train-YYYYMMDD` and create them from the real landing
  branch.
- Treat scratch branches as evidence, not as the artifact to ship. Land work by
  harvesting the safe resolved hunks or commits back into the release branch in
  narrow, reviewable commits — keep tags, releases, and fast-forwards off the
  scratch train.
- A PR that is clean against `main` can still conflict with a release branch.
  Test against the actual release head before calling it merge-ready.

## Merging and harvesting

- Prefer direct GitHub merge only when the PR is clean against the real landing
  branch, has acceptable checks, and does not cross trust-boundary surfaces.
- For already approved PRs, start with a scratch merge against the release
  branch, then decide between direct merge, cherry-pick with conflict
  resolution, or credited harvest. Maintainer approval is a priority signal,
  not permission to skip review or tests.
- Review PRs from code, tests, linked issues, comments, and check results — let
  those, rather than the title or labels alone, drive every merge, close,
  harvest, or defer decision.
- Close or update issues and PRs only after verifying the landed commit on the
  relevant branch. If the release branch already contains equivalent behavior,
  leave a clear note linking the commit and describing any remaining delta.

## Credit (CI-enforced)

- When harvesting, preserve or add machine-readable credit: keep the original
  author where possible, add `Co-authored-by` using `.github/AUTHOR_MAP` or the
  GitHub numeric noreply identity, and include `Harvested from PR #N by @handle`
  in the commit body so the auto-close workflow can close the PR with credit
  after it reaches `main`.
- Merge a PR whose commit carries that line with **rebase or a merge commit** so
  the body survives intact — a squash can rewrite it, drop the
  `Harvested from PR` line, and silently lose both the machine-readable credit
  and the auto-close.
- Keep `Co-authored-by` trailers to human contributors.
  `scripts/check-coauthor-trailers.py` rejects bot/tool ones (Claude, codex,
  cursor, `noreply@anthropic.com`) on harvest commits.
- Refresh the manual credit surfaces that do not auto-populate from trailers:
  `docs/CONTRIBUTORS.md` and `CHANGELOG.md`.
