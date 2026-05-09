# Release Checklist

A pre-tag checklist that the v0.8.21/v0.8.22 CHANGELOG gap proved we needed.
Step through this in order from a clean worktree on the release branch
(`work/vX.Y.Z-...`). Treat any unchecked box as a release blocker.

For deeper context on the underlying tools (preflight scripts, npm smoke,
publish-crates), see [`RELEASE_RUNBOOK.md`](RELEASE_RUNBOOK.md).

## 1. CHANGELOG entry exists for the version

- [ ] `CHANGELOG.md` has a `## [X.Y.Z] - YYYY-MM-DD` heading at the top
- [ ] The entry credits every external contributor whose commit lands in this
      version. Get the list with:
      ```
      git log vPREV..HEAD --no-merges --format="%h %an <%ae> %s" \
        | grep -v '<your-email@…>'
      ```
      For each contributor, link both their display name and (when known)
      `@github-handle`.
- [ ] The entry uses the Keep a Changelog headers — `Added`, `Changed`,
      `Fixed`, `Security`, `Removed`, `Deprecated`. Add `Known issues` only
      if there is something material the user must work around.
- [ ] The entry mentions all referenced issue/PR numbers as `#NNNN` so the
      auto-linker on GitHub picks them up.

## 2. Version pins are in sync

- [ ] `Cargo.toml` workspace `version` is bumped.
- [ ] All per-crate `crates/*/Cargo.toml` path-dependency `version = "..."`
      pins match the new workspace version.
- [ ] `npm/deepseek-tui/package.json` `version` AND `deepseekBinaryVersion`
      are both bumped.
- [ ] `Cargo.lock` is refreshed (`cargo update --workspace --offline`).
- [ ] `./scripts/release/check-versions.sh` reports
      `Version state OK: workspace=X.Y.Z, npm=X.Y.Z, lockfile in sync.`

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

- [ ] `cargo build --release --locked -p deepseek-tui-cli -p deepseek-tui`
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
- [ ] `npm view deepseek-tui@X.Y.Z version deepseekBinaryVersion --json`
      reports the new version on the npm registry.
- [ ] `crates.io` has the new version (or the `publish-crates.sh` job has
      pushed it).
- [ ] `ghcr.io/hmbown/deepseek-tui:vX.Y.Z` and `:latest` are updated.

## 8. Post-tag

- [ ] Edit the GitHub release notes to expand any CVE-style or attack
      details that were intentionally omitted from the PR title/body.
- [ ] Note any deferred items in the next release's tracking issue.
- [ ] Close any issues that this release fixed.

---

If a step fails, **fix the underlying cause** rather than skipping it. Pre-commit
hooks, signing, and CI are all here to catch real problems. `--no-verify`,
`--no-gpg-sign`, and force-pushing a release branch over reviewers should
remain hard-disabled by convention.
