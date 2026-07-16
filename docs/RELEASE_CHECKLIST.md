# Release Checklist

A pre-tag checklist that the v0.8.21/v0.8.22 CHANGELOG gap proved we needed.
Step through this in order from a clean worktree on the final release source.
Treat any unchecked box as a release blocker.

For deeper context on the underlying tools (preflight scripts, npm smoke,
publish-crates), see [`RELEASE_RUNBOOK.md`](RELEASE_RUNBOOK.md).
For larger milestone releases, add any version-specific acceptance matrix to
the release branch before tagging; use it for provider routes, feature gates,
GUI/runtime smoke, remote-workbench decisions, and credit hygiene that the
generic checklist does not enumerate.

## 0. Release source is frozen

- [ ] The live milestone and PR queue no longer contain work intended for this
      version:
      ```
      gh issue list --repo Hmbown/CodeWhale --milestone "vX.Y.Z" --state open
      gh pr list --repo Hmbown/CodeWhale --state open --limit 100
      ```
- [ ] Any remaining same-theme work is explicitly retargeted to a later
      version or called out as a known issue. Do not bump/tag while still
      planning to merge more same-version fixes.
- [ ] The release tag does not already point at an older source SHA, or the
      maintainer has deliberately chosen to publish exactly that older SHA:
      ```
      git ls-remote origin refs/heads/main refs/tags/vX.Y.Z
      gh release view vX.Y.Z --repo Hmbown/CodeWhale
      ./scripts/release/check-published.sh X.Y.Z
      ```
- [ ] If `vX.Y.Z` exists with no GitHub Release/packages and `main` has moved
      on, stop. Choose one of: publish the existing tag as-is, bump the later
      work to the next patch version, or explicitly approve deleting/recreating
      the unpublished tag. Do not silently move tags during PR cleanup.

## 1. CHANGELOG entry exists for the version

- [ ] `CHANGELOG.md` has a `## [X.Y.Z] - YYYY-MM-DD` heading at the top
- [ ] The entry credits every external contributor, harvested PR author,
      linked issue reporter, reproduction/log provider, reviewer, and
      verification helper whose work materially shaped this version. Get the
      commit list with:
      ```
      git log vPREV..HEAD --no-merges --format="%h %an <%ae> %s" \
        | grep -v '<your-email@…>'
      ```
      For each contributor, link both their display name and (when known)
      `@github-handle`. Then inspect linked issues and harvested PRs so
      reporters/helpers are not lost just because they did not author commits.
- [ ] The entry uses the Keep a Changelog headers — `Added`, `Changed`,
      `Fixed`, `Security`, `Removed`, `Deprecated`. Add `Known issues` only
      if there is something material the user must work around.
- [ ] The entry mentions all referenced issue/PR numbers as `#NNNN` so the
      auto-linker on GitHub picks them up.
- [ ] Run `scripts/sync-changelog.sh` to regenerate `crates/tui/CHANGELOG.md`
      (the recent-releases slice embedded in the binary for `/change`). Do
      not edit that file by hand, and do not copy the full root changelog
      into it — older entries live in `docs/CHANGELOG_ARCHIVE.md`.

## 2. Version pins are in sync

- [ ] Run `./scripts/release/prepare-release.sh X.Y.Z` — it bumps the
      workspace version, every per-crate dependency pin,
      `npm/codewhale/package.json` (`version` + `codewhaleBinaryVersion`),
      the README install-tag examples, refreshes `Cargo.lock`, regenerates
      `crates/tui/CHANGELOG.md` and `web/lib/facts.generated.ts`, and ends
      by running `check-versions.sh`. Write the CHANGELOG entry **before**
      running it.
- [ ] `npm/deepseek-tui/package.json` remains private/compatibility-only and
      is **not** bumped or published.
- [ ] `./scripts/release/check-versions.sh` reports
      `Version state OK: workspace=X.Y.Z, npm=X.Y.Z, lockfile in sync.`
- [ ] `./scripts/release/check-ohos-deps.sh` reports that the OpenHarmony
      target graph does not pull the unsupported `nix` 0.28/0.29,
      `portable-pty`, `starlark`, `arboard`, or `keyring` crates.

## 3. Preflight gates

Run, in order, from the repo root:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets --locked`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --all-features --locked`
      (Re-run any single failure in isolation with
      `cargo test -p PKG --bin BIN -- TEST_NAME` before declaring it a flake.
      Tests that mutate process-wide state — `HOME`, `cwd`, `RUST_LOG` —
      can race in parallel. Document confirmed flakes in `Known issues`.)
- [ ] `./scripts/release/publish-crates.sh dry-run`

## 4. npm wrapper smoke

- [ ] `cargo build --release --locked -p codewhale-cli -p codewhale-tui`
- [ ] `node scripts/release/npm-wrapper-smoke.js`
      (Set `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1` if you need to inspect the temp
      install afterwards.)

## 5. Branch and PR

- [ ] Branch is pushed: `git push -u origin work/vX.Y.Z-...`
- [ ] PR opened with `gh pr create --base main --title "chore(release): prepare vX.Y.Z"`
- [ ] The PR targets `main` and will be merged before any `vX.Y.Z` tag is
      pushed. Do not tag a release-only branch; GitHub will not process
      `Closes #N` keywords until those commits reach the default branch.
- [ ] PR body includes:
  - one-paragraph summary of the release theme
  - a punch list of the new commits since the last release
  - explicit call-out of any **Security** items so reviewers see them
  - the contributor thank-you list
  - the `Known issues` block from the CHANGELOG, if any
- [ ] PR title is **neutral** — do not put CVE-style language or specific
      attack details in the title. Save those for the GitHub release notes
      after the tag is pushed.

## 5b. Branch hygiene (post-merge)

After the release/integration merge lands, make it obvious where the release
tip lives and clean up stale branches **safely**. A working checkout left on a
scratch/renovate branch (even when `HEAD` already matches the tag) creates
release anxiety: contributors cannot tell whether their work merged.

- [ ] Run the dry-run report first (read-only, deletes nothing):

      ```sh
      ./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z
      ```

      It prints: the current checkout branch, the local + remote release tips,
      and the main ref; the branches that are **safe to delete** (tip already
      contained in the configured main ref or the release branch); and a
      **keep / needs review** list naming each branch, its unique commit count,
      the author(s), and the keep reason. The summary line reports how many are
      safe-deletes, how many were kept for contributor work, and how many need a
      human decision. A diverged local/remote release tip exits non-zero. Use
      `--remote upstream` when the canonical release refs live on `upstream`
      instead of `origin`.
- [ ] If the working checkout is parked on a stale branch, switch to the
      release branch and fast-forward it:

      ```sh
      git switch codex/vX.Y.Z
      git fetch origin && git merge --ff-only origin/codex/vX.Y.Z   # if behind
      ```
- [ ] Only after reviewing the dry-run, delete the **safe** branches. Local
      first; add `--prune-remote` to also delete remote safe-deletes:

      ```sh
      ./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z --prune --yes
      ```

      The script **never** auto-deletes a branch with unique commits from a
      contributor other than Hunter unless that work is already merged. Those
      land in the keep/review list with author and reason; review, merge,
      harvest with credit, or explicitly preserve them before removing the
      branch. When in doubt, leave the branch and record the decision.

## 6. CI green and review

- [ ] All required CI jobs are green. The `versions` job should mirror the
      preflight `check-versions.sh` and is your last line of defense.
- [ ] PR has been reviewed.

## 7. Tag and release (after review)

- [ ] Release PR is merged into `main`, then local `main` is fast-forwarded:
      `git switch main && git fetch origin main && git merge --ff-only origin/main`
- [ ] The release source is reachable from `main`:
      `./scripts/release/ensure-release-on-main.sh HEAD`
- [ ] Create `vX.Y.Z` from the final `main` SHA using the **Create release tag**
      workflow, or create and push a signed local tag:
      `git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z`
- [ ] The `release.yml` workflow has built and uploaded artifacts to the
      GitHub release for this tag.
- [ ] The public GitHub Release assets are proven to match the tag commit
      before publishing Cargo or npm:
      ```
      ./scripts/release/verify-release-assets.sh X.Y.Z
      ```
      This checks the local tag, remote tag, successful Release workflow SHA,
      full binary/archive/installer asset set, and both checksum manifests. If
      it fails, rerun or repair the GitHub Release workflow before touching any
      registry.
- [ ] The live GitHub Release body has its own `## Contributors` or
      `## Credits` section; do not rely on "see CHANGELOG" alone. Verify with:
      ```
      gh release view vX.Y.Z --repo Hmbown/CodeWhale --json body \
        --jq '.body | test("## (Contributors|Credits)")'
      ```
- [ ] `npm view codewhale@X.Y.Z version codewhaleBinaryVersion --json`
      reports the new version on the npm registry.
- [ ] `npm view deepseek-tui deprecated` is non-empty. The legacy npm package
      is deprecated and must not receive an `X.Y.Z` publish.
- [ ] Distribution channels are canonical-first: the website install page
      (codewhale.net/install) shows Codewhale-native commands first (`npm install -g
      codewhale`, `curl .../install.sh | sh`); Homebrew is labeled as legacy
      compatibility; the shell installer uses codewhale-native names as documented
      in `docs/REBRAND.md#homebrew`.
- [ ] `crates.io` has the new version (or the `publish-crates.sh` job has
      pushed it).
- [ ] `ghcr.io/hmbown/codewhale:vX.Y.Z` and `:latest` are updated.
- [ ] The final registry verification passes:
      ```
      ./scripts/release/check-published.sh X.Y.Z
      ```

## 8. Post-tag

- [ ] Edit the GitHub release notes to expand any CVE-style or attack
      details that were intentionally omitted from the PR title/body.
- [ ] Re-run the GitHub Release body check after any release-workflow rerun;
      workflows can overwrite notes and accidentally remove contributor credit.
- [ ] Note any deferred items in the next release's tracking issue.
- [ ] Close any issues that this release fixed.

---

If a step fails, **fix the underlying cause** rather than skipping it. Pre-commit
hooks, signing, and CI are all here to catch real problems. `--no-verify`,
`--no-gpg-sign`, and force-pushing a release branch over reviewers should
remain hard-disabled by convention.
