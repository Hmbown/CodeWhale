---
name: gh-find-prs
description: "Survey open Codewhale PRs and triage each for mergeability and disposition against the real landing branch."
---

# gh-find-prs

Survey the open PR queue and assign each PR a disposition — backed by code, tests, and checks, never by title — testing real mergeability against the actual release branch (often local-only, e.g. `<release-branch>`), not the main-based GitHub flag.

## When to use

- A maintainer asks "what's in the PR queue?", "what can we land?", or "triage open PRs".
- Before a release cut, to sweep community contributions into the active release branch.
- Whenever you need a per-PR DIRECT-MERGE / HARVEST / DEFER / CLOSE-WITH-NOTE call with credit attached.

This is read-and-recommend. You do NOT merge, close, tag, or publish. You surface evidence and a proposed disposition; the maintainer approves.

## Workflow

1. **Inventory the queue.** One call, structured:
   ```
   gh pr list --repo Hmbown/CodeWhale --state open \
     --json number,title,author,headRefName,baseRefName,isDraft,mergeStateStatus,statusCheckRollup
   ```
   Note `mergeStateStatus` (CLEAN / BLOCKED / DIRTY / UNKNOWN) but treat it as a hint only — it is computed against `main`, and the real landing target is usually a different branch.

2. **Identify the real landing branch.** The release head is frequently local-only:
   ```
   git branch --list 'codex/v0.8*' 'codex/v0.9*'
   git log --oneline -1 <release-branch>
   ```
   Use that ref, not `main`, for every mergeability test below.

3. **Read each candidate from code, not title.** For every non-trivial PR:
   ```
   gh pr view <N> --repo Hmbown/CodeWhale \
     --json files,additions,deletions,statusCheckRollup,body,comments
   gh pr diff <N> --repo Hmbown/CodeWhale
   ```
   Read the diff. A "fix(exec): ..." can be a no-op or a regression; a "chore" can be the real fix. Judge the change, the tests it adds, and any review comments.

4. **Decode check failures — distinguish trivial from real.** In `statusCheckRollup`, find each `conclusion: FAILURE` and read its job. Codewhale's CI jobs are `Lint`, `Test (ubuntu-latest|macos-latest|windows-latest)`, `Version drift`, `gate` (Contribution gate), `npm wrapper smoke`, `Mobile runtime smoke`, `Documentation`, `GitGuardian Security Checks`.
   - A `Lint` failure that is only `cargo fmt` drift is trivial — harvestable, fix on landing with `cargo fmt --all`.
   - A failing `Test (...)` or `clippy` under Lint is real — read the log before trusting it.
   - `Version drift` failing on a community PR is expected (they bumped, or didn't); not a blocker for harvest.
   ```
   cargo fmt --all -- --check && cargo clippy --workspace --all-targets
   ```

5. **Test-merge against the real release head.** The `mergeStateStatus` flag lies for local branches. Probe the actual merge:
   ```
   git merge-tree --write-tree --messages <release-branch> origin/pr/<N>   # if PR ref is fetched
   git merge-tree --write-tree --messages <release-branch> <pr-head-sha>
   ```
   Exit 0 and no `CONFLICT` lines → clean against the release branch (DIRECT-MERGE candidate even when GitHub shows BLOCKED/DIRTY). Conflicts printed → HARVEST or DEFER. This is read-only; it writes objects to the object store, not to any branch or working tree.

6. **Assign a disposition with required credit.** Per PR, recommend exactly one:
   - **DIRECT-MERGE** — diff is sound, checks are green or trivially-fixable, `merge-tree` is clean against the release head. Land via cherry-pick to preserve the original author automatically.
   - **HARVEST** — the change is good but conflicts, needs fmt/rebase, or is entangled with the release work. Reimplement on the release branch and credit with trailers (cherry-pick is not preserving authorship here):
     ```
     Co-authored-by: Name <email>
     Harvested-from: PR #<N> by @handle
     ```
     The `Harvested-from:` trailer lets the auto-close-at-main workflow close the PR with credit once the change reaches main.
   - **DEFER** — sound but blocked by an open question, missing tests, or a release freeze. Leave a positive, specific comment; do not close.
   - **CLOSE-WITH-NOTE** — superseded, duplicated, or out of scope. Propose the close to the maintainer with a crediting, appreciative note; never close it yourself.

7. **Report, don't act.** Output a compact table: PR | author | landing-branch verdict | check summary | disposition | credit line. Stop there for maintainer approval.

## Red flags / don't

- **Don't judge by title.** "fix(...)" / "feat(...)" / emoji-prefixed test PRs prove nothing. Open the diff every time.
- **Don't trust `mergeStateStatus` for the real target.** CLEAN/BLOCKED/DIRTY are vs `main`; always confirm with `git merge-tree <release> <pr-head>`.
- **Don't conflate trivial and real check failures.** A fmt-only `Lint` red is harvestable; a failing `Test (...)` is not — read the log.
- **Don't drop credit.** Every harvest carries `Co-authored-by:` + `Harvested-from:`; every cherry-pick keeps the original author. No silent reimplementation.
- **Don't merge, close, retarget, tag, publish, or release.** Recommend; the maintainer decides.
- **Don't post negative or nitpicking comments.** GitHub-facing comments are positive and crediting; keep critique in your internal report to the maintainer.
- **Don't modify the working tree or any branch.** `git merge-tree --write-tree` is the only "write" allowed — it touches the object store only.
