# Claude Repository Guidance

Read `AGENTS.md` first. This file exists as a compatibility instruction source
for Claude-based agents working in this repository.

## Stewardship Defaults

- Treat community PRs and issues as maintainer evidence. Inspect code, tests,
  linked issues, comments, and CI before merging, harvesting, closing, or
  deferring work.
- Do not tag, publish, create a GitHub Release, or push release artifacts
  without Hunter's explicit approval.
- Keep CodeWhale branding while preserving first-class DeepSeek model/provider
  support and legacy migration care.
- Preserve contributor credit for harvested work with authorship,
  `Co-authored-by`, `Harvested from PR #N by @handle`, and changelog/release
  notes where applicable. Use canonical GitHub-noreply identities from
  `.github/AUTHOR_MAP`; never add bot/tool `Co-authored-by` trailers (Claude,
  codex, cursor) — the `check-coauthor-trailers.py` CI gate rejects them.

## Scratch Integration Branches

- For release queues, create disposable local branches from the real landing
  branch, for example `scratch/vX.Y.Z-pr-train-YYYYMMDD`.
- Use the scratch branch to merge or cherry-pick candidate PR heads in batches
  and learn which conflicts, tests, and overlaps are real.
- Do not ship the scratch branch itself. It may contain noisy merge commits,
  partial conflict resolutions, and unrelated PR interactions.
- After the scratch experiment, move only the safe result back to the release
  branch as narrow commits or direct merges. Keep each final commit explainable
  and testable.
- A PR that is clean against `main` is not necessarily clean against a release
  branch. Test mergeability against the branch that will actually receive the
  work.
- For already approved PRs, treat approval as a strong priority signal. Still
  inspect diffs, comments, check results, and release-branch conflicts before
  landing.

## Current Release Work

- The active branch for this release lane is `codex/v0.8.63-integration`
  (also at `origin/codex/v0.8.63-integration`). This repo lives on multiple
  devices, so do not hard-code a checkout path; work in whichever local
  checkout you have and confirm with `git branch --show-current` before
  editing. Never commit directly to `main`.
- The workspace version is `0.8.63`. Do not tag, publish, create a GitHub
  Release, push release artifacts, or merge to `main` without Hunter's
  explicit approval.
- Base release triage on the GitHub `v0.8.63` milestone
  (`gh issue list --repo Hmbown/CodeWhale --milestone "v0.8.63" --state open`)
  unless Hunter gives a newer branch/milestone.
- Work the queue in this order: release blockers, recently approved PRs, clean
  PRs with small scope, blocked PRs with obvious fixes, dirty PRs that can be
  harvested safely, then larger architecture issues.
- Prefer batching PR conflict discovery on scratch branches, then harvesting
  reviewed, credited, tested slices back into the release branch.
- Before claiming an issue is done, verify whether the branch already contains
  equivalent work. If it does, prepare the GitHub note/closure path instead of
  reimplementing it.
- See `AGENTS.md` → "Where to work right now" for build/test commands, known
  suite papercuts, and the removed-machinery guardrails (agent-only surface,
  no lifecycle/coherence systems).
