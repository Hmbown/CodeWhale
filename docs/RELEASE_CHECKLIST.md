# Release Checklist

A pre-tag checklist that the v0.8.21/v0.8.22 CHANGELOG gap proved we needed.
Step through this in order from a clean worktree on the release branch
(`work/vX.Y.Z-...`). Treat any unchecked box as a release blocker.

For deeper context on the underlying tools (preflight scripts, npm smoke,
publish-crates), see [`RELEASE_RUNBOOK.md`](RELEASE_RUNBOOK.md).
For v0.9.0, also complete the dedicated
[`V0_9_0_RELEASE_ACCEPTANCE.md`](V0_9_0_RELEASE_ACCEPTANCE.md) matrix before
tagging; it covers provider routes, WhaleFlow feature gates, GUI/runtime smoke,
remote workbench decisions, and credit hygiene that the generic checklist does
not enumerate.

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
- [ ] PR body includes:
  - one-paragraph summary of the release theme
  - a punch list of the new commits since the last release
  - explicit call-out of any **Security** items so reviewers see them
  - the contributor thank-you list
  - the `Known issues` block from the CHANGELOG, if any
- [ ] PR title is **neutral** — do not put CVE-style language or specific
      attack details in the title. Save those for the GitHub release notes
      after the tag is pushed.

## 6. CI green and review

- [ ] All required CI jobs are green. The `versions` job should mirror the
      preflight `check-versions.sh` and is your last line of defense.
- [ ] PR has been reviewed.

## 7. Tag and release (after review)

- [ ] `git tag -s vX.Y.Z -m "vX.Y.Z"`
- [ ] `git push origin vX.Y.Z`
- [ ] The `release.yml` workflow has built and uploaded artifacts to the
      GitHub release for this tag.
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
- [ ] `crates.io` has the new version (or the `publish-crates.sh` job has
      pushed it).
- [ ] `ghcr.io/hmbown/codewhale:vX.Y.Z` and `:latest` are updated.

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
