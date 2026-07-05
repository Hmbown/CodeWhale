# How credit works here

CodeWhale follows a "land what's useful, credit the contributor" model. Not
every PR merges as-is — some are harvested into maintainer commits — but the
credit contract is the same either way: **if your work shipped, the record
says so, in a form GitHub's contribution graph can see.** This page documents
the machinery behind that promise.

The contributor-facing summary lives in
[CONTRIBUTING.md § How Your Contribution Lands](../CONTRIBUTING.md#how-your-contribution-lands);
the full per-PR record is [docs/CONTRIBUTORS.md](CONTRIBUTORS.md).

## The harvest-with-credit contract

When a PR is large, mixes scope, conflicts with `main`, or needs polish that's
faster to apply than to round-trip, a maintainer may **harvest** the useful
commits or hunks into a new commit instead of merging the PR directly. That is
not a rejection — it means the code landed. The contract:

- The harvested commit's body carries `Harvested from PR #N by @handle`. That
  line is the credit and the machine-readable signal.
- The [auto-close workflow](../.github/workflows/auto-close-harvested.yml)
  watches `main` for that line and closes PR #N with a templated thank-you
  linking the merged commit — so harvested PRs never rot open as
  `CONFLICTING`.
- Harvest commits are merged with rebase or a merge commit, **never squash**:
  a squash can rewrite the body, drop the `Harvested from PR` line, and
  silently lose both the credit and the auto-close.
- The next release's `CHANGELOG.md` entry and
  [docs/CONTRIBUTORS.md](CONTRIBUTORS.md) credit the contributor by handle.
  These two surfaces don't auto-populate from trailers and are refreshed by
  hand.
- A harvested PR is never closed with a bare "superseded." The closing comment
  names the handle, the exact commits where the work landed, and — when the PR
  contained more than what landed — a tracking issue for the remainder
  ([CONTRIBUTING.md](../CONTRIBUTING.md#path-2--harvest) has the template).

## Graph attribution: `.github/AUTHOR_MAP`

Prose credit is not enough — co-author credit should land in GitHub's
contributor graph. That only works when the `Co-authored-by:` trailer uses
GitHub's numeric noreply identity
(`id+login@users.noreply.github.com`), not a raw or machine-local email.

[`.github/AUTHOR_MAP`](../.github/AUTHOR_MAP) is the identity map: each line
maps an alias (a GitHub login, an old-style noreply address, or a raw email
seen in contributed commits) to the canonical
`Display Name <id+login@users.noreply.github.com>` identity. Maintainers use
it when writing trailers; when a contributor isn't mapped yet, the canonical
address comes from
`gh api users/<login> --jq '"\(.id)+\(.login)@users.noreply.github.com"'`.

## The CI gate: `check-coauthor-trailers.py`

[`scripts/check-coauthor-trailers.py`](../scripts/check-coauthor-trailers.py)
validates that harvested credit is GitHub-mappable. On new commits it checks
every `Co-authored-by:` trailer:

- The email must be a canonical numeric noreply address
  (`^[0-9]+\+login@users.noreply.github.com$`), so the credit actually
  registers in the contributor graph.
- Bot/tool trailers (Claude, codex, cursor, `noreply@anthropic.com`) are
  rejected — contributor trailers are for humans.
- The check is scoped to new commits; historical commits may carry raw or
  local emails and are left alone.

## Retroactive credit reconciliation

Credit is corrected backward, not just forward. When a review pass finds work
that shipped earlier without proper credit — a harvested hunk whose trailer
was dropped, a fix that landed before the AUTHOR_MAP entry existed — a
reconciliation pass adds the credit after the fact: a "Retroactive
reconciliation (shipped earlier, credited now)" entry in
[docs/CONTRIBUTORS.md](CONTRIBUTORS.md) naming the contributor and the
original PR/issue. The v0.8.62 band has an example of what that looks like.

## The stewardship rule

From the maintainer guidance ([AGENTS.md](../AGENTS.md#codewhale-stewardship),
[CLAUDE.md](../CLAUDE.md)): issue reports, repros, logs, reviews, and
verification comments are real project work, not queue noise. Every harvested
PR, issue report, or comment that materially shaped a fix gets credited —
authorship preserved where possible, mappable `Co-authored-by` trailers
otherwise, and visible credit in the changelog and release notes. Recurring
contributors stay credited in the public record even when the final patch had
to be narrowed, delayed, or folded into a maintainer commit.
