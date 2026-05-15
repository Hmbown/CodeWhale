# DeepSeek TUI Release Runbook

This runbook is the source of truth for shipping Rust crates, GitHub release assets,
and the `deepseek-tui` npm wrapper.

Current packaging note:
- `deepseek-tui` is the live runtime and TUI package shipped to users today.
- `deepseek-tui-core` is a supporting workspace crate for the extraction/parity effort, not a replacement for the shipping runtime.

## Canonical Publish Targets

- End-user crates:
  - `deepseek-tui`
  - `deepseek-tui-cli`
- Supporting crates published from this workspace:
  - `deepseek-secrets`
  - `deepseek-config`
  - `deepseek-protocol`
  - `deepseek-state`
  - `deepseek-agent`
  - `deepseek-execpolicy`
  - `deepseek-hooks`
  - `deepseek-mcp`
  - `deepseek-tools`
  - `deepseek-core`
  - `deepseek-app-server`
  - `deepseek-tui-core`
- `deepseek-cli` on crates.io is an unrelated crate and is not part of this release flow.

## Version Coordination

- Rust crates inherit the shared workspace version from [Cargo.toml](../Cargo.toml).
- Internal path dependency versions should match the shared workspace version; stale older pins are release blockers once the workspace version moves.
- The npm wrapper version lives in [npm/deepseek-tui/package.json](../npm/deepseek-tui/package.json).
- `deepseekBinaryVersion` controls which GitHub release binaries the npm wrapper downloads.
- Packaging-only npm releases are allowed:
  - bump the npm package version
  - leave `deepseekBinaryVersion` pinned to the previously released Rust binaries
  - rerun `npm pack` smoke checks before `npm publish`

## Preflight

Run these from the repository root before cutting a tag:

```bash
./scripts/release/check-versions.sh   # version drift between workspace, npm, lockfile
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo publish --dry-run --locked --allow-dirty -p deepseek-tui
./scripts/release/publish-crates.sh dry-run
```

`check-versions.sh` also runs in CI on every push/PR (the `versions` job in
`.github/workflows/ci.yml`), so drift between `Cargo.toml`, the per-crate
manifests, `npm/deepseek-tui/package.json`, and `Cargo.lock` is caught before
release time rather than at it.

`publish-crates.sh dry-run` performs a full `cargo publish --dry-run` for crates
without unpublished workspace dependencies and a packaging preflight for dependent
workspace crates. That avoids false negatives from crates.io not yet containing the
new workspace version while still validating package contents before publish.

For npm wrapper verification, build the two shipped binaries and run the
cross-platform smoke harness. This packs the npm wrapper, installs it into a
clean temporary project, serves local release assets over HTTP, and checks both
the dispatcher-to-TUI path (`deepseek doctor --help`) and the direct TUI
entrypoint (`deepseek-tui --help`).

```bash
cargo build --release --locked -p deepseek-tui-cli -p deepseek-tui
node scripts/release/npm-wrapper-smoke.js
```

Set `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1` to keep the temporary pack/install
directory for inspection.

To exercise `npm run release:check` locally as well, regenerate the local asset
directory with a full asset matrix fixture before starting the server:

```bash
DEEPSEEK_TUI_PREPARE_ALL_ASSETS=1 node scripts/release/prepare-local-release-assets.js
cd npm/deepseek-tui
DEEPSEEK_TUI_VERSION=X.Y.Z DEEPSEEK_TUI_RELEASE_BASE_URL=http://127.0.0.1:8123/ npm run release:check
```

Set `DEEPSEEK_TUI_VERSION` to the npm package version you are verifying for that local run.

The CI workflow runs the same tarball install + delegated-entrypoint smoke test
on Linux, macOS, and Windows.

After publishing, prove the release is visible in both registries:

```bash
./scripts/release/check-published.sh X.Y.Z
```

Do not mark a Rust release complete until that command sees `deepseek-tui@X.Y.Z`
on npm and every `deepseek-*` crate at `X.Y.Z` on crates.io. For a rare
npm packaging-only release, run with `--allow-npm-binary-mismatch` and keep the
release notes explicit that no new Rust binary version shipped.

## Rust Crates Release

Crate publishing to crates.io is **manual** — there is no automated
`crates-publish` GitHub workflow. Operators run the helpers in
`scripts/release/` from a developer workstation that has `cargo login`
configured.

1. Update the workspace version in [Cargo.toml](../Cargo.toml).
2. Run `./scripts/release/check-versions.sh` and
   `./scripts/release/publish-crates.sh dry-run` locally; both must be clean.
3. Tag the release as `vX.Y.Z` (typically by pushing the version bump to
   `main` and letting `auto-tag.yml` create the tag — see the npm wrapper
   release section below for the `RELEASE_TAG_PAT` requirement).
4. Publish crates in this order with `./scripts/release/publish-crates.sh publish`:
   - `deepseek-secrets`
   - `deepseek-config`
   - `deepseek-protocol`
   - `deepseek-state`
   - `deepseek-agent`
   - `deepseek-execpolicy`
   - `deepseek-hooks`
   - `deepseek-mcp`
   - `deepseek-tools`
   - `deepseek-core`
   - `deepseek-app-server`
   - `deepseek-tui-core`
   - `deepseek-tui-cli`
   - `deepseek-tui`
5. Wait for each published crate version to appear on crates.io before publishing dependents.

The publish helper is idempotent for reruns: already-published crate versions are skipped.

## GitHub Release Assets

`.github/workflows/release.yml` builds these binaries:

- `deepseek-linux-x64`
- `deepseek-macos-x64`
- `deepseek-macos-arm64`
- `deepseek-windows-x64.exe`
- `deepseek-tui-linux-x64`
- `deepseek-tui-macos-x64`
- `deepseek-tui-macos-arm64`
- `deepseek-tui-windows-x64.exe`

The release job also uploads `deepseek-artifacts-sha256.txt`. The npm installer and
release verification script both depend on that checksum manifest.

## npm Wrapper Release

**The npm publish step is manual.** `release.yml` no longer runs `npm publish`
because the npm account requires 2FA OTP on every publish, and an automation
token that bypasses 2FA has not been provisioned. The GitHub Release flow
remains fully automated; only the npm wrapper publish requires a developer
on a workstation with `npm login` and an authenticator app.

### Steps

1. Set the npm package version in [npm/deepseek-tui/package.json](../npm/deepseek-tui/package.json) to match the workspace `Cargo.toml`. CI's version-drift guard will catch mismatches before tag.
2. Set `deepseekBinaryVersion` to the GitHub release tag that should supply binaries.
3. Push the version bump to `main`. `auto-tag.yml` creates the matching `vX.Y.Z` tag, and `release.yml` builds the binary matrix and drafts the GitHub Release.
4. **Wait for the GitHub Release to finalize** with all eight signed binaries plus `deepseek-artifacts-sha256.txt`. The npm `prepublishOnly` hook (`scripts/verify-release-assets.js`) requires every asset to be present.
5. From a developer machine, publish the npm wrapper manually:

```bash
cd npm/deepseek-tui
npm publish --access public
# (you will be prompted for the npm OTP from your authenticator)
```

### Why not automated?

- `release.yml`'s old `publish-npm` job used `secrets.NPM_TOKEN`, but npm's 2FA-by-default policy means a publish token must be either an automation token with "Bypass 2FA for token authentication" enabled OR an account-level 2FA-disabled state. We don't have either configured.
- The standalone `publish-npm.yml` and `crates-publish.yml` workflows have been removed; no inert automation plumbing remains. A future move to npm Trusted Publishing (OIDC) would re-introduce a dedicated workflow at that point.

### If you fix the token later

To re-enable automated publish: provision an npm automation token with "Bypass 2FA for token authentication" enabled (or set up npm Trusted Publishing via OIDC), store the corresponding secret on the repo, and re-add a `publish-npm` job to `release.yml` (or a dedicated workflow) along with reverting this section's "manual" framing.

## CNB Cool mirror

Every push to `main` and every `v*` tag is mirrored to
`cnb.cool/deepseek-tui.com/DeepSeek-TUI` via the `Sync to CNB` workflow
so users behind GitHub-blocking networks can fetch the source. After a
release tag, **verify the mirror caught it** before declaring the
release shipped:

```bash
git ls-remote https://cnb.cool/deepseek-tui.com/DeepSeek-TUI.git refs/tags/vX.Y.Z
```

If the workflow failed for the release tag, the manual fallback is
documented in [docs/CNB_MIRROR.md](CNB_MIRROR.md) (one-time `git
remote add cnb …`, then `git push cnb vX.Y.Z`).

## Recovery and Rollback

- Crates publish partially:
  - rerun `./scripts/release/publish-crates.sh publish`
  - already-published crate versions will be skipped
- GitHub assets missing or checksum manifest incomplete:
  - fix `.github/workflows/release.yml`
  - retag or upload corrected assets before `npm publish`
- npm packaging-only problem:
  - bump only the npm package version
  - keep `deepseekBinaryVersion` on the last known-good Rust release
  - repack and republish the wrapper
- A bad npm publish cannot be overwritten:
  - publish a new npm version with corrected metadata or install logic
- CNB mirror failed for the release tag:
  - check the run via `gh run list --workflow=sync-cnb.yml`
  - retrigger with `gh workflow run sync-cnb.yml`, or push the tag
    manually per [docs/CNB_MIRROR.md](CNB_MIRROR.md#manual-fallback)
