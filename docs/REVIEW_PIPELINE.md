\# CodeWhale Review Pipeline



Welcome to CodeWhale! We receive a high volume of community PRs. To ensure a smooth and fast review process, please review our pipeline expectations below. 



\## 1. CI Gates (Pre-Review Checklist)

Before a maintainer reviews your PR, it must pass our continuous integration (CI) checks. 



\*\*Required Checks (Must Pass):\*\*

Please run these locally before pushing your code to avoid CI failures:

\* \*\*Format:\*\* `cargo fmt --all -- --check`

\* \*\*Linting:\*\* `cargo clippy --workspace --all-targets --all-features`

\* \*\*Tests:\*\* `cargo test --workspace --all-features --locked`



\*\*Informational Checks:\*\*

Checks from \*\*Greptile\*\* and \*\*GitGuardian\*\* are informational. If they flag something, review it, but they do not strictly block a review on their own unless a secret is leaked.



\## 2. Common Failure Modes \& Local Fixes

If CI fails, it is usually one of these three reasons:

\* \*\*Version Drift (`Cargo.lock` out of date):\*\* Run `cargo update` or `cargo build` locally to update the lockfile and commit the changes.

\* \*\*Lint Failures:\*\* Check the clippy warnings from the command above and fix the specific lines flagged.

\* \*\*Windows Test Flakiness:\*\* Occasionally, tests may time out on Windows runners. If you are confident your code didn't break it, leave a comment asking a maintainer to re-trigger the CI.



\## 3. PR Etiquette

To help us review your code quickly, please adhere to the following:

\* \*\*One Concern Per PR:\*\* Keep diffs highly focused. Do not mix refactoring with new feature additions.

\* \*\*Link the Issue:\*\* Always include `Closes #N` (replace N with the issue number) in your PR description so GitHub automatically links them.

\* \*\*Rebase:\*\* Always rebase your branch onto the latest `main` branch before requesting a review.



\## 4. The Review Workflow

Once CI is green, your PR enters the review queue.

\* \*\*Who reviews:\*\* Core maintainers will review the PR. 

\* \*\*`autonomous-ready` Label:\*\* If a maintainer applies this label, it means the PR is approved in concept and is queued for our automated integration system.

\* \*\*The Nightly Loop:\*\* We run extensive integration loops overnight. If your PR is approved, it may wait for this nightly loop before final merging to ensure system stability.



\## 5. Post-Merge Actions

After your code is merged, the following automated actions occur:

\* `CHANGELOG.md` is updated.

\* `npm` wrappers are synced.

\* Binary rebuilds are triggered for all platforms.

\* Website and documentation are synced with your new changes.



Thank you for contributing to CodeWhale!

