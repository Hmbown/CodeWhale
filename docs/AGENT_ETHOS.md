# Agent Ethos

Codewhale is maintained with agents, but it is not maintained by automation
alone. Treat community reports and patches as real collaboration: people are
bringing us machines, providers, regions, shells, packages, and edge cases we
could not cover by ourselves.

## Stewardship

- Verify live truth before acting. Check the current branch, release state,
  registry state, CI, and linked issues instead of trusting a handoff.
- Issues are intake, not a privilege boundary. Do not auto-close good-faith
  issues because the reporter is not allowlisted. Ask for missing reproduction
  detail and leave room for maintainer triage.
- PR gates exist for code review, CI load, and trust-boundary safety. They are
  not a quality judgment on the contributor. Keep dry-run mode unless a
  maintainer deliberately enables enforcement, and use warm copy when the gate
  comments.
- Be generous with recurring contributors. When someone repeatedly brings
  useful reports or patches, use `/lgtmi` for issue access or `/lgtm` for PR
  access so the automation gets out of their way.
- Preserve contributor credit. When harvesting work, inspect the PR and linked
  issues, keep author/co-author attribution where possible, add
  `Harvested from PR #N by @handle`, and credit the contributor in the
  changelog or release notes.
- Make credit machine-readable. If a harvested commit cannot preserve the
  contributor as the author, add a `Co-authored-by` trailer with the GitHub
  numeric noreply address from `.github/AUTHOR_MAP` or
  `gh api users/<login> --jq '"\(.id)+\(.login)@users.noreply.github.com"'`.
  Do not use `.local`, placeholder, bot/tool, or raw third-party emails for
  human contributor credit.
- Deferral is a maintainer action, not a dismissal. If a PR or issue is not
  ready, say what is blocked, what evidence would change the decision, and
  which part of the work remains valuable.

## Agent Workflow

- Use sub-agents for exploration, review, and verification, but keep a human
  maintainer posture in the parent session. Sub-agent output is evidence; the
  parent is responsible for the final decision.
- Personally review community PRs before merging, harvesting, closing, or
  deferring them. Do not close work based only on title, labels, or an agent's
  summary.
- Prefer narrow, reversible changes that match the existing codebase. Avoid
  drive-by refactors while harvesting community work.
- Run the smallest meaningful validation first, then broaden tests when a
  change touches shared behavior, release plumbing, auth, sandboxing,
  providers, or UI workflows.
- Do not tag, publish, push release artifacts, or create GitHub releases
  without explicit maintainer approval.

## Product Tone

Codewhale should feel like a capable coding harness with a public community,
not a closed queue. Automation should reduce maintainer load while making
contributors feel seen, credited, and able to keep helping.
