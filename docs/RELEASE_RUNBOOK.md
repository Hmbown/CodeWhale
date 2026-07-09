# CodeWhale Release Runbook

This runbook is the source of truth for shipping Rust crates, GitHub release assets,
and the `codewhale` npm wrapper.

Current packaging note:
- `codewhale-tui` is the live runtime crate shipped to users today.
- `codewhale-app-server` is a supporting library crate. The shipped entrypoint
  is `codewhale app-server`; do not add or publish a standalone app-server binary.

## Canonical Publish Targets

- End-user crates:
  - `codewhale-tui`
  - `codewhale-cli`
- Supporting crates published from this workspace:
  - `codewhale-secrets`
  - `codewhale-config`
  - `codewhale-protocol`
  - `codewhale-state`
  - `codewhale-agent`
  - `codewhale-execpolicy`
  - `codewhale-hooks`
  - `codewhale-mcp`
  - `codewhale-tools`
  - `codewhale-core`
  - `codewhale-app-server`
  - `codewhale-workflow`

## Version Coordination

- Rust crates inherit the shared workspace version from [Cargo.toml](../Cargo.toml).
- Internal path dependency versions should match the shared workspace version; stale older pins are release blockers once the workspace version moves.
- The npm wrapper version lives in [npm/codewhale/package.json](../npm/codewhale/package.json).
- `codewhaleBinaryVersion` controls which GitHub release binaries the npm wrapper downloads.
- Packaging-only npm releases are allowed:
  - bump the npm package version
  - leave `codewhaleBinaryVersion` pinned to the previously released Rust binaries
  - rerun `npm pack` smoke checks before `npm publish`

## Release Source Timing

Freeze the source before creating a public `vX.Y.Z` tag. The version bump is
not the release; it is the last source-prep commit before the tag. Do not keep
merging same-version feature/fix PRs after `vX.Y.Z` exists and assume the
release workflow will pick them up. It will not: the tag is the release anchor.

Before tagging, verify the live queue and existing anchors:

```bash
gh issue list --repo Hmbown/CodeWhale --milestone "vX.Y.Z" --state open
gh pr list --repo Hmbown/CodeWhale --state open --limit 100
git ls-remote origin refs/heads/main refs/tags/vX.Y.Z
gh release view vX.Y.Z --repo Hmbown/CodeWhale
./scripts/release/check-published.sh X.Y.Z
```

If a same-version tag already exists but there is no GitHub Release and nothing
is published, stop and choose deliberately:

- publish exactly the tagged SHA, leaving later commits for the next patch;
- bump the later work to the next patch version and tag that later SHA; or
- with explicit maintainer approval only, delete/recreate the unpublished tag
  after confirming no package, GitHub Release, mirror, or installer consumer has
  treated it as public.

Do not delete, move, or recreate a release tag implicitly as part of ordinary
PR merge or milestone cleanup work.

## Preflight

Run these from the repository root before cutting a tag:

```bash
./scripts/release/check-versions.sh   # version drift between workspace, npm, lockfile
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo publish --dry-run --locked --allow-dirty -p codewhale-tui
./scripts/release/publish-crates.sh dry-run
```

`check-versions.sh` also runs in CI on every push/PR (the `versions` job in
`.github/workflows/ci.yml`), so drift between `Cargo.toml`, the per-crate
manifests, `npm/codewhale/package.json`, and `Cargo.lock` is caught before
release time rather than at it.

The source-controlled CNB pipeline mirrors the heavy Linux version/fmt/check/
clippy/test/npm-smoke gates for `fix/*`, `rebrand/*`, `work/v*`, and `main`.
GitHub Actions keeps the cheap drift/fmt statuses plus macOS and Windows
coverage, while CNB carries the Linux work.

`publish-crates.sh dry-run` performs a full `cargo publish --dry-run` for crates
without unpublished workspace dependencies and a packaging preflight for dependent
workspace crates. That avoids false negatives from crates.io not yet containing the
new workspace version while still validating package contents before publish.

For npm wrapper verification, build the two shipped binaries and run the
cross-platform smoke harness. This packs the npm wrapper, installs it into a
clean temporary project, serves local release assets over HTTP, and checks both
the dispatcher-to-TUI path (`codewhale doctor --help`) and the direct TUI
entrypoint (`codewhale-tui --help`).

```bash
cargo build --release --locked -p codewhale-cli -p codewhale-tui
node scripts/release/npm-wrapper-smoke.js
```

Set `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1` to keep the temporary pack/install
directory for inspection.

To exercise `npm run release:check` locally as well, regenerate the local asset
directory with a full asset matrix fixture before starting the server:

```bash
DEEPSEEK_TUI_PREPARE_ALL_ASSETS=1 node scripts/release/prepare-local-release-assets.js
cd npm/codewhale
DEEPSEEK_TUI_VERSION=X.Y.Z DEEPSEEK_TUI_RELEASE_BASE_URL=http://127.0.0.1:8123/ npm run release:check
```

Set `DEEPSEEK_TUI_VERSION` to the npm package version you are verifying for that local run.

The CNB workflow runs the Linux tarball install + delegated-entrypoint smoke
test; GitHub Actions keeps macOS and Windows smoke coverage.

After publishing, prove the release is visible in both registries:

```bash
./scripts/release/check-published.sh X.Y.Z
```

Do not mark a Rust release complete until that command sees `codewhale@X.Y.Z`
on npm and every `codewhale-*` crate at `X.Y.Z` on crates.io. For a rare
npm packaging-only release, run with `--allow-npm-binary-mismatch` and keep the
release notes explicit that no new Rust binary version shipped.

## Post-Merge Branch Hygiene

After a release or scratch integration branch lands, run the branch hygiene
helper before pruning anything:

```bash
./scripts/release/branch-hygiene.sh --release-branch codex/vX.Y.Z
```

The default mode is a dry run. It reports the current checkout branch, main ref,
local and remote release tips, safe local or remote branch deletes, branches
kept for contributor work, and branches that still need a human decision. Review
that report before running `--prune --yes`, and add `--prune-remote` only when
you have confirmed the remote branches are safe to delete.

Use `--remote upstream` when you are working from a fork and the canonical
release refs live on the upstream remote instead of `origin`.

Verify the helper itself after changing it:

```bash
bash scripts/release/branch-hygiene.test.sh
bash scripts/release/ensure-release-on-main.test.sh
```

Those scripts are pinned to LF line endings so the same command works from a
Windows checkout under Bash.

## Rust Crates Release

Crate publishing to crates.io is **manual** — there is no automated
`crates-publish` GitHub workflow. Operators run the helpers in
`scripts/release/` from a developer workstation that has `cargo login`
configured.

Release commits must land on `main` before any `vX.Y.Z` tag is pushed. Do not
tag a release-only branch. Open the release PR against `main`, let required
review and CI finish, merge it, then explicitly tag the final source commit
that is reachable from `main`. This is what lets GitHub process `Closes #N`
lines automatically and show the release PR as merged. The tag release workflow runs
`scripts/release/ensure-release-on-main.sh` for tag pushes and manual dispatches,
and fails branch-only release sources before assets are published.

1. Write the CHANGELOG entry, then run
   `./scripts/release/prepare-release.sh X.Y.Z` — it bumps every
   version-bearing file (workspace + crate pins + npm wrapper + README
   install tags), refreshes the lockfile and generated files, and runs
   `check-versions.sh`.
2. Run `./scripts/release/publish-crates.sh dry-run` locally; it must be clean.
3. Merge the release PR into `main` before tagging. After the same-version
   queue is frozen and `main` is at the intended source SHA, create `vX.Y.Z`
   from `main` with the manual **Create release tag** workflow or with a signed
   local tag push from a developer machine. See the npm wrapper release section
   below for the `RELEASE_TAG_PAT` / manual release dispatch caveat.
4. Publish crates in this order with `./scripts/release/publish-crates.sh publish`:
   - `codewhale-mcp`
   - `codewhale-protocol`
   - `codewhale-release`
   - `codewhale-secrets`
   - `codewhale-state`
   - `codewhale-workflow`
   - `codewhale-execpolicy`
   - `codewhale-hooks`
   - `codewhale-tools`
   - `codewhale-config`
   - `codewhale-agent`
   - `codewhale-tui`
   - `codewhale-core`
   - `codewhale-app-server`
   - `codewhale-cli`
5. Wait for each published crate version to appear on crates.io before publishing dependents.

The publish helper is idempotent for reruns: already-published crate versions are skipped.

## GitHub Release Assets

`.github/workflows/release.yml` builds these binaries:

- `codewhale-*` CLI binaries for Linux x64/arm64, macOS x64/arm64, and Windows x64
- `codewhale-tui-*` TUI binaries for the same target matrix
- `codew-*` shortcut binaries for the same target matrix
- `codewhale.bat` for the Windows npm launcher

The release job also uploads `codewhale-artifacts-sha256.txt`. The npm installer and
release verification script both depend on that checksum manifest. The
authoritative npm-facing asset list lives in
`npm/codewhale/scripts/artifacts.js`.

Before any Cargo or npm publish, prove that the public GitHub Release assets
belong to the tag commit you are publishing:

```bash
./scripts/release/verify-release-assets.sh X.Y.Z
```

That gate compares the local and remote `vX.Y.Z` tag SHAs, confirms a
successful `Release` workflow run used that SHA, then runs the npm wrapper's
release check against the public GitHub asset URLs. The npm check fails if the
release is missing an npm-facing asset, the checksum manifest omits a required
binary, or the assets predate the matching release workflow run. If the command
fails, rerun or repair `release.yml`; do not publish Cargo or npm against stale
assets.

## npm Wrapper Release

**The npm publish step is manual.** `release.yml` no longer runs `npm publish`
because the npm account requires 2FA OTP on every publish, and an automation
token that bypasses 2FA has not been provisioned. The GitHub Release flow
remains fully automated; only the npm wrapper publish requires a developer
on a workstation with `npm login` and an authenticator app.

### Steps

1. Set the npm package version in [npm/codewhale/package.json](../npm/codewhale/package.json) to match the workspace `Cargo.toml`. CI's version-drift guard will catch mismatches before tag.
2. Set `codewhaleBinaryVersion` to the GitHub release tag that should supply binaries.
3. Push the version bump to `main`. After the release source is frozen, create
   the matching `vX.Y.Z` tag from `main`; `release.yml` then builds the binary
   matrix and drafts the GitHub Release.
4. **Wait for the GitHub Release to finalize** with the full npm-facing binary
   matrix plus `codewhale-artifacts-sha256.txt`. The npm `prepublishOnly` hook
   (`scripts/verify-release-assets.js`) requires every asset to be present.
5. Run the public asset freshness gate from the repo root:

```bash
./scripts/release/verify-release-assets.sh X.Y.Z
```

For a rare packaging-only npm release where the npm package version intentionally
points at older Rust binaries, add `--allow-npm-binary-mismatch` and keep the
release notes explicit that no new binary version shipped.

6. From a developer machine, confirm npm auth and publish the wrapper manually:

```bash
npm whoami
cd npm/codewhale
npm publish --access public
# (you will be prompted for the npm OTP from your authenticator)
npm view codewhale@X.Y.Z version codewhaleBinaryVersion --json
cd ../..
./scripts/release/check-published.sh X.Y.Z
```

If `npm whoami` or `npm publish` reports `E401`, `ENEEDAUTH`, or an OTP/login
failure, do not edit package contents. Run:

```bash
npm login
npm whoami
cd npm/codewhale
npm publish --access public
```

Rerun the same `npm publish --access public` command after completing the login
or OTP prompt. The package's `prepublishOnly` hook reruns the release-asset
gate before each publish attempt, so an auth failure cannot accidentally skip
asset verification on retry.

Do not publish `npm/deepseek-tui`; it is deprecated compatibility metadata only.

### Why not automated?

- `release.yml`'s old `publish-npm` job used `secrets.NPM_TOKEN`, but npm's 2FA-by-default policy means a publish token must be either an automation token with "Bypass 2FA for token authentication" enabled OR an account-level 2FA-disabled state. We don't have either configured.
- The standalone `publish-npm.yml` and `crates-publish.yml` workflows have been removed; no inert automation plumbing remains. A future move to npm Trusted Publishing (OIDC) would re-introduce a dedicated workflow at that point.

### If you fix the token later

To re-enable automated publish: provision an npm automation token with "Bypass 2FA for token authentication" enabled (or set up npm Trusted Publishing via OIDC), store the corresponding secret on the repo, and re-add a `publish-npm` job to `release.yml` (or a dedicated workflow) along with reverting this section's "manual" framing.

## CNB Cool mirror

Every push to `main`, `fix/*`, `rebrand/*`, `work/v*`, and every `v*` tag is mirrored to
`cnb.cool/codewhale.net/codewhale` via the `Sync to CNB` workflow
so users behind GitHub-blocking networks can fetch the source and so CNB can
run the heavy Linux CI lane. After a release tag, **verify the mirror caught
it** before declaring the release shipped:

```bash
git ls-remote https://cnb.cool/codewhale.net/codewhale.git refs/tags/vX.Y.Z
```

If the workflow failed for the release tag, the manual fallback is
documented in [docs/CNB_MIRROR.md](CNB_MIRROR.md) (one-time `git
remote add cnb …`, then `git push cnb vX.Y.Z`).

## Recovery and Rollback

- User-facing rollback:
  - npm: `npm install -g codewhale@X.Y.Z`
  - Cargo: `cargo install codewhale-cli --version X.Y.Z --locked --force`
    and `cargo install codewhale-tui --version X.Y.Z --locked --force`
  - manual assets: download binaries or the platform archive plus the matching
    `codewhale-artifacts-sha256.txt` or `codewhale-bundles-sha256.txt`
    manifest from `https://github.com/Hmbown/CodeWhale/releases/tag/vX.Y.Z`
  - workspace files: use `/restore list [N]` and `/restore <N>` for side-git
    snapshots; this does not change the installed binary version or rewrite
    conversation history
  - keep [docs/INSTALL.md](INSTALL.md#roll-back-to-a-previous-release) in sync
    with these commands
- Crates publish partially:
  - rerun `./scripts/release/publish-crates.sh publish`
  - already-published crate versions will be skipped
- GitHub assets missing or checksum manifest incomplete:
  - fix `.github/workflows/release.yml`
  - retag or upload corrected assets before `npm publish`
- npm packaging-only problem:
  - bump only the npm package version
  - keep `codewhaleBinaryVersion` on the last known-good Rust release
  - repack and republish the wrapper
- A bad npm publish cannot be overwritten:
  - publish a new npm version with corrected metadata or install logic
- CNB mirror failed for the release tag:
  - check the run via `gh run list --workflow=sync-cnb.yml`
  - retrigger with `gh workflow run sync-cnb.yml`, or push the tag
    manually per [docs/CNB_MIRROR.md](CNB_MIRROR.md#manual-fallback)
