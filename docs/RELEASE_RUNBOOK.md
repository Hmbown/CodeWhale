# CodeWhale Release Runbook

This runbook is the source of truth for shipping Rust crates, GitHub release assets,
and the `codewhale` npm wrapper.

Current packaging note:
- `codewhale-tui` is the live runtime crate shipped to users today.
- `codewhale-tui-core` is a supporting workspace crate for the extraction/parity effort, not a replacement for the shipping runtime.

## Canonical Publish Targets

- End-user crates:
  - `codewhale-tui`
  - `codewhale-cli`
- Supporting crates published from this workspace:
  - `codewhale-secrets`
  - `codewhale-config`
  - `codewhale-protocol`
  - `codewhale-state`
  - `codewhale-execpolicy`
  - `codewhale-hooks`
  - `codewhale-mcp`
  - `codewhale-tools`
  - `codewhale-core`
  - `codewhale-app-server`
  - `codewhale-tui-core`

## Version Coordination

- Rust crates inherit the shared workspace version from [Cargo.toml](../Cargo.toml).
- Internal path dependency versions should match the shared workspace version; stale older pins are release blockers once the workspace version moves.
- The npm wrapper version lives in [npm/codewhale/package.json](../npm/codewhale/package.json).
- `codewhaleBinaryVersion` controls which GitHub release binaries the npm wrapper downloads.
- Packaging-only npm releases are allowed:
  - bump the npm package version
  - leave `codewhaleBinaryVersion` pinned to the previously released Rust binaries
  - rerun `npm pack` smoke checks before `npm publish`

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
   - `codewhale-secrets`
   - `codewhale-config`
   - `codewhale-protocol`
   - `codewhale-state`
   - `codewhale-execpolicy`
   - `codewhale-hooks`
   - `codewhale-mcp`
   - `codewhale-tools`
   - `codewhale-core`
   - `codewhale-app-server`
   - `codewhale-tui-core`
   - `codewhale-cli`
   - `codewhale-tui`
5. Wait for each published crate version to appear on crates.io before publishing dependents.

The publish helper is idempotent for reruns: already-published crate versions are skipped.

## GitHub Release Assets

`.github/workflows/release.yml` builds these binaries:

- `codewhale-linux-x64`
- `codewhale-macos-x64`
- `codewhale-macos-arm64`
- `codewhale-windows-x64.exe`
- `codewhale-tui-linux-x64`
- `codewhale-tui-macos-x64`
- `codewhale-tui-macos-arm64`
- `codewhale-tui-windows-x64.exe`

The release job also uploads `codewhale-artifacts-sha256.txt`. The npm installer and
release verification script both depend on that checksum manifest.

## npm Wrapper Release

**The npm publish step is manual.** `release.yml` no longer runs `npm publish`
because the npm account requires 2FA OTP on every publish, and an automation
token that bypasses 2FA has not been provisioned. The GitHub Release flow
remains fully automated; only the npm wrapper publish requires a developer
on a workstation with `npm login` and an authenticator app.

### Steps

1. Set the npm package version in [npm/codewhale/package.json](../npm/codewhale/package.json) to match the workspace `Cargo.toml`. CI's version-drift guard will catch mismatches before tag.
2. Set `codewhaleBinaryVersion` to the GitHub release tag that should supply binaries.
3. Push the version bump to `main`. `auto-tag.yml` creates the matching `vX.Y.Z` tag, and `release.yml` builds the binary matrix and drafts the GitHub Release.
4. **Wait for the GitHub Release to finalize** with all eight signed binaries plus `codewhale-artifacts-sha256.txt`. The npm `prepublishOnly` hook (`scripts/verify-release-assets.js`) requires every asset to be present.
5. From a developer machine, publish the npm wrapper manually:

```bash
cd npm/codewhale
npm publish --access public
# (you will be prompted for the npm OTP from your authenticator)
```

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
