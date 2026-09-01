---
name: gh-credit-harvest
description: "Harvest one community PR into a release branch with authorship and credit preserved, verified green, and a warm thank-you."
---

# gh-credit-harvest

Harvest exactly one community PR into the real landing branch with full
authorship and machine-readable credit, verified green, then thank the
contributor. A PR is evidence: judge it from code, tests, comments, and checks,
never the title. Do not merge, close, tag, or publish without Hunter's approval —
this skill lands a credited commit and posts thanks; the workflow closes the PR.

## When to use

- You have approval to land ONE specific community PR into a release branch.
- The PR is not yet on the landing branch (if it is, close-with-credit instead — see `gh-close-issues`).
- The landing branch may be local-only (e.g. `<release-branch>`); a main-based "mergeable" flag does not prove it lands cleanly.

## Workflow

1. Find the real landing branch (the one Hunter named, not always `main`) and fetch the PR head:
   ```bash
   git switch <release-branch>
   git fetch origin pull/<N>/head
   git log -1 --format='%H %an <%ae>' FETCH_HEAD   # author to preserve
   ```
2. Review from evidence, not the title. Read the diff, tests, linked issue, comments, and CI:
   ```bash
   gh pr view <N> --repo Hmbown/CodeWhale --json title,author,files,statusCheckRollup
   gh pr diff <N> --repo Hmbown/CodeWhale
   ```
3. Test mergeability against the REAL landing branch (local-only branches lie via the main flag):
   ```bash
   git merge-tree $(git merge-base HEAD FETCH_HEAD) HEAD FETCH_HEAD   # empty/clean = no conflict
   ```
4. Land it, preferring cherry-pick — it preserves the original author automatically:
   ```bash
   git cherry-pick <sha>            # one or more commits from FETCH_HEAD
   ```
5. If it conflicts, spans noise, or needs squashing, re-apply the narrow slice and commit with explicit author + credit trailers. Resolve `--author` and the co-author from `.github/AUTHOR_MAP` (fall back to numeric noreply):
   ```bash
   gh api users/<handle> --jq '"\(.id)+\(.login)@users.noreply.github.com"'
   git commit --author="Name <ID+handle@users.noreply.github.com>" -m "fix(scope): what changed (#<N>)" \
     -m "Harvested from PR #<N> by @<handle>" \
     -m "Co-authored-by: Name <ID+handle@users.noreply.github.com>"
   ```
   The `Harvested from PR #<N> by @<handle>` line in the body is what `.github/workflows/auto-close-harvested.yml` matches to auto-close with credit once the commit reaches `main`.
6. Format and run the focused tests for the touched crate — only land green:
   ```bash
   cargo fmt --all
   cargo test -p <crate>            # the crate(s) the PR touched, not the whole workspace
   git log -1 --format='%an <%ae>%n%(trailers:key=Co-authored-by)'  # credit the human, from .github/AUTHOR_MAP
   ```
7. Post a brief, warm, specific thank-you on the PR — name what the change fixed, no drama. Leave the PR open; the workflow closes it with credit when the commit lands on `main`:
   ```bash
   gh pr comment <N> --repo Hmbown/CodeWhale \
     --body "Thank you @<handle> — clean fix for <the specific bug>. Harvested into the v0.8.61 lane with your authorship preserved; it'll auto-close with credit once it reaches main."
   ```

Grounded example: PR #3221 by @hongchen1993 (honour `DEEPSEEK_BASE_URL`/`DEEPSEEK_MODEL` in exec) cherry-picks cleanly, so its author is preserved with no manual trailers; a focused `cargo test -p` on the touched crate is enough to land it green.

## Red flags / don't

- Don't judge or land from the title or labels alone — read code, tests, comments, and checks.
- Don't trust the GitHub main-based mergeable flag for a local-only release branch; prove it with `git merge-tree`.
- Don't squash away the original author. Cherry-pick when you can; only fall back to `--author` + trailers when you must.
- Don't invent co-author emails. Use `.github/AUTHOR_MAP`, then numeric noreply; never raw third-party, `.local`, placeholder, or bot emails.
- Don't omit the `Harvested from PR #<N> by @<handle>` body line — without it the PR won't auto-close with credit.
- Don't land red, harvest more than one PR per commit, or batch unrelated changes into the harvest.
- Don't merge, close, tag, publish, or push release artifacts without Hunter's approval. Keep the comment positive and crediting.
- Already on the landing branch? Don't re-harvest — close-with-credit via `gh-close-issues`.
