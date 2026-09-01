# Codewhale Release Runbook

This runbook is the source of truth for shipping Rust crates, GitHub release assets,
and the `codewhale` npm wrapper.

Current packaging note:
- `codewhale-tui` is the live runtime crate linked into the installed
  `codewhale`/`codew` commands; it is not a third installed command.
- `codewhale-app-server` is a supporting library crate. The shipped entrypoint
  is `codewhale app-server`; do not add or publish a standalone app-server binary.

## Canonical Publish Targets

- End-user crates:
  - `codewhale-tui`
  - `codewhale-cli`
- Supporting crates published from this workspace:
  - `codewhale-build-support`
  - `codewhale-mcp`
  - `codewhale-paths`
  - `codewhale-protocol`
  - `codewhale-release`
  - `codewhale-secrets`
  - `codewhale-state`
  - `codewhale-telemetry`
  - `codewhale-workflow`
  - `codewhale-workflow-js`
  - `codewhale-execpolicy`
  - `codewhale-hooks`
  - `codewhale-tools`
  - `codewhale-config`
  - `codewhale-lane`
  - `codewhale-agent`
  - `codewhale-core`
  - `codewhale-command-contract`
  - `codewhale-app-server`

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
./scripts/release/check-versions.sh   # workspace/npm/SDK/VS Code/generated-fact/lock drift
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
./scripts/release/publish-crates.sh dry-run
```

`check-versions.sh` also runs in CI on every push/PR (the `versions` job in
`.github/workflows/ci.yml`), so drift between `Cargo.toml`, the per-crate
manifests, the npm wrapper and Runtime SDK, the VS Code extension and lock,
generated web facts, and `Cargo.lock` is caught before release time rather than
at it.

The source-controlled CNB pipeline mirrors the heavy Linux version/fmt/check/
clippy/test/npm-smoke gates for `fix/*`, `rebrand/*`, `work/v*`, and `main`.
GitHub Actions keeps the cheap drift/fmt statuses plus macOS and Windows
coverage, while CNB carries the Linux work.

`publish-crates.sh dry-run` first validates the maintained publication order
against the locked Cargo workspace graph. It then performs a full
`cargo publish --dry-run` for crates without unpublished workspace dependencies
and a packaging preflight for dependent workspace crates. That avoids false
negatives from crates.io not yet containing the new workspace version while
still validating package contents before publish.

For npm wrapper verification, build the single runtime and run the
cross-platform smoke harness. This packs the npm wrapper, installs it into a
clean temporary project, serves local release assets over HTTP, and checks both
published commands against that runtime: `codewhale doctor --help` and
`codew --version`.

```bash
cargo build --release --locked -p codewhale-cli -p codewhale-tui
node scripts/release/npm-wrapper-smoke.js
```

Set `DEEPSEEK_TUI_KEEP_SMOKE_DIR=1` to keep the temporary pack/install
directory for inspection.

## Exact-head GitHub proof before publication

Two manual workflows provide exact-head evidence without crossing the public
release boundary. Run them only after the intended source is on a named ref
(normally the frozen `main`), and pass the full commit SHA as an independent
guard against that ref moving between inspection and dispatch:

```bash
git fetch origin main
candidate_sha="$(git rev-parse origin/main)"

gh workflow run ci.yml --ref main \
  -f expected_sha="${candidate_sha}"
gh workflow run release-candidate.yml --ref main \
  -f expected_sha="${candidate_sha}"
```

The manual `ci.yml` path verifies that the dispatch resolved to
`expected_sha`, disables light-change shortcuts, and forces the heavy Rust,
workflow, mobile, Actions, Linux, macOS, Windows, npm-wrapper, and documentation
gates. A mismatch fails before those gates start; it never silently tests a
different head.

`release-candidate.yml` also fails unless the selected ref resolves to the
exact requested SHA. It invokes the same reusable artifact workflow as the
public release, building all seven targets (including Android arm64 and native
Windows arm64), staging `codewhale` and `codew` (single binary), building the
NSIS installer and nine platform archives, and validating the authoritative
34-file inventory from `npm/codewhale/scripts/artifacts.js` (27 current
artifacts and manifests plus seven compatibility-only `codewhale-tui-*`
filenames containing the same compiled `codewhale` bytes for v0.9.4 update
clients). It then installs
the packed npm wrapper against those assembled local assets and exercises its
delegated entrypoints. The resulting `codewhale-release-assets` bundle is a
short-lived GitHub Actions artifact only.

This candidate workflow does not create a tag or GitHub Release, publish a
crate or npm package, push a container, update Homebrew, deploy anything, or
write repository contents. Its green result is evidence, not publication
authorization. The stop line remains explicit Hunter approval: do not create
the `vX.Y.Z` tag, dispatch `release.yml`, or run any registry publication step
until that approval is given.

The Android target is cross-built and included in the checksum/bundle gates,
but GitHub's Linux runner cannot execute the Android binary as a real Termux
user. Keep the real-device limitation in the release packet unless separate
device evidence exists.

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
   version-bearing file (workspace + crate pins + npm wrapper + Runtime SDK +
   VS Code extension/lock + remote-smoke default + public source-candidate
   facts + README install tags), refreshes the Cargo/npm locks and generated
   files, and runs
   the version and OHOS gates. It is safe to rerun after the workspace already
   equals `X.Y.Z`: the second run skips replacements, refreshes the packaged
   changelog and web facts, and reruns both gates.
2. Run `./scripts/release/publish-crates.sh dry-run` locally; it must be clean.
3. Merge the release PR into `main` before tagging. After the same-version
   queue is frozen and `main` is at the intended source SHA, create `vX.Y.Z`
   from `main` with the manual **Create release tag** workflow or with a signed
   local tag push from a developer machine.
   - If `RELEASE_TAG_PAT` is configured, the tag push starts `release.yml`.
   - If no Release run appears and the tag already exists, first confirm that
     no tag-triggered run is queued or active, then dispatch the exact tag:
     `gh workflow run release.yml --ref vX.Y.Z -f version=X.Y.Z`.
   - Never dispatch from `main`, and do not start a duplicate while the
     tag-triggered run is merely delayed. The workflow serializes runs for the
     same tag. It also refuses to start release work when that tag already owns
     any GitHub Release asset, rechecks immediately before upload, and disables
     the release action's overwrite behavior. A normal rerun must never replace
     public bytes.
4. Wait for the GitHub Release workflow and all public assets to finish, then
   fetch the release tag and run the public asset gate. Do not publish any
   Cargo or npm package until it passes:

   ```bash
   git fetch --force origin +refs/tags/vX.Y.Z:refs/tags/vX.Y.Z
   ./scripts/release/verify-release-assets.sh X.Y.Z
   ```

5. Create a clean detached checkout of the immutable release tag, then publish
   the Rust crates from that checkout only:

   ```bash
   git worktree add --detach ../codewhale-release-vX.Y.Z vX.Y.Z
   cd ../codewhale-release-vX.Y.Z
   ./scripts/release/require-release-tag-checkout.sh X.Y.Z
   ./scripts/release/publish-crates.sh publish
   ```

   Both Cargo and npm publication fail closed unless `HEAD`, the clean local
   checkout, and the remote `vX.Y.Z` tag still agree. The authoritative 21-crate
   dependency order lives in `scripts/release/crates.sh`; do not maintain a
   second handwritten order in this runbook. The helper waits for each new
   version to appear on crates.io before moving to dependents and safely skips
   versions that are already public on a rerun.

The publish helper is idempotent for reruns: already-published crate versions are skipped.

## GitHub Release Assets

`.github/workflows/release.yml` builds and stages these artifacts:

- one `codewhale-*` runtime binary for Linux x64/arm64, Android arm64, macOS
  x64/arm64, and Windows x64/arm64
- byte-identical `codew-*` command assets copied from that runtime
- byte-identical `codewhale-tui-*` compatibility filenames so installed v0.9.4
  clients can discover and complete the one-runtime upgrade; current installers
  never expose those filenames as a third command
- `codewhale.bat` for the Windows npm/GitHub x64 launcher, and the same
  filename inside Windows zip archives and the NSIS install (those copies
  launch `codewhale.exe` and prefer Windows Terminal)
- platform `.tar.gz` / `.zip` archives and `CodeWhaleSetup.exe`

The release job also uploads `codewhale-artifacts-sha256.txt` and
`codewhale-bundles-sha256.txt`. The npm installer and release verification
script depend on those manifests. The authoritative release asset list lives in
`npm/codewhale/scripts/artifacts.js`.

Before any Cargo or npm publish, prove that the public GitHub Release assets
belong to the tag commit you are publishing:

```bash
./scripts/release/verify-release-assets.sh X.Y.Z
```

That gate compares the local and remote `vX.Y.Z` tag SHAs, confirms a
successful `Release` workflow run used that SHA, then runs the npm wrapper's
release check against the public GitHub asset URLs. The npm check fails if the
release is missing a required binary, archive, installer, or manifest; either
manifest omits a required row; or the assets predate the matching release
workflow run. If the command fails, rerun or repair `release.yml`; do not
publish Cargo or npm against stale assets.

## AUR / Omarchy Package

`codewhale-bin` is a downstream package of the same Linux release, not a new
Codewhale semantic version. After the public asset gate above passes, render
its AUR metadata from the verified release directory:

```bash
./packaging/aur/render.sh /path/to/release-assets /tmp/codewhale-bin
```

The renderer reads the workspace version and extracts the x64/arm64 archive
hashes only after both release checksum manifests agree with the actual files.
It emits no `SKIP` checksums or source-controlled per-release values. Follow
[`packaging/aur/README.md`](../packaging/aur/README.md) for the clean Arch build,
`.SRCINFO` comparison, and package-content checks.

The GitHub release workflows only verify that the AUR metadata can be rendered
from their candidate assets. They do not publish to AUR. AUR publication is a
separate, explicitly authorized maintainer action after the matching tag and
assets are public.

## npm Wrapper Release

`release.yml` publishes `codewhale` through npm Trusted Publishing after the
exact-SHA GitHub Release job succeeds. The job has only `contents: read` and
`id-token: write`; it does not use `NPM_TOKEN`, `NODE_AUTH_TOKEN`, or a
long-lived bypass-2FA credential.

Before the first automated publish, configure the `codewhale` package's npm
Trusted Publisher with these exact values:

- organization or user: `Hmbown`
- repository: `CodeWhale`
- workflow filename: `release.yml`
- environment: leave blank (the workflow does not claim a GitHub environment)

That npm-side binding is an external release gate. If it is missing or differs
in case, repository, workflow filename, or environment, the publish job must
fail; do not add a token fallback to make it pass.

### Steps

1. Set the npm package version in [npm/codewhale/package.json](../npm/codewhale/package.json) to match the workspace `Cargo.toml`. CI's version-drift guard will catch mismatches before tag.
2. Set `codewhaleBinaryVersion` to the GitHub release tag that should supply binaries.
3. Push the version bump to `main`. After the release source is frozen, create
   the matching `vX.Y.Z` tag from `main`; `release.yml` then builds the binary
   matrix and drafts the GitHub Release.
4. **Wait for the GitHub Release to finalize** with the full binary and archive
   matrix, Windows installer, and both checksum manifests. The dependent `npm`
   job checks the remote tag again, runs the public asset freshness gate and
   package tests, then publishes with OIDC. The package's `prepublishOnly` hook
   repeats the clean exact-tag and public-asset checks immediately before the
   registry write.
5. Confirm the `npm` job succeeded, then prove the published package and binary
   version are visible:

```bash
npm view codewhale@X.Y.Z version codewhaleBinaryVersion --json
./scripts/release/check-published.sh X.Y.Z
```

For a rare packaging-only npm release where the npm package version intentionally
points at older Rust binaries, add `--allow-npm-binary-mismatch` and keep the
release notes explicit that no new binary version shipped. That exception is a
separate manual release path: the normal trusted-publishing job deliberately
does not set `CODEWHALE_ALLOW_NPM_BINARY_MISMATCH`.

Do not publish `npm/deepseek-tui`; it is deprecated compatibility metadata only.

### Manual recovery

If GitHub OIDC is unavailable after the GitHub Release and public-asset gate are
green, use a clean detached checkout of the immutable tag. Authenticate
interactively with npm's normal WebAuthn/2FA flow; never create a long-lived
bypass-2FA token:

```bash
./scripts/release/require-release-tag-checkout.sh X.Y.Z
./scripts/release/verify-release-assets.sh X.Y.Z
npm login
npm whoami
cd npm/codewhale
npm publish --access public
```

The same `prepublishOnly` gates rerun on every attempt. An OIDC or login failure
is not permission to edit the tagged package, move the tag, or skip asset
verification.

## CNB Cool mirror

Every push to `main`, `fix/*`, `rebrand/*`, `work/v*`, and every `v*` tag is mirrored to
`cnb.cool/codewhale.net/codewhale` via the `Sync to CNB` workflow
so users behind GitHub-blocking networks can fetch the source and so CNB can
run the heavy Linux CI lane. After a release tag, **verify the mirror caught
it** before declaring the release shipped:

```bash
git ls-remote https://cnb.cool/codewhale.net/codewhale.git refs/tags/vX.Y.Z
```

If the workflow failed for the release tag, use the exact-tag rerun or
`workflow_dispatch --ref vX.Y.Z` recovery documented in
[docs/CNB_MIRROR.md](CNB_MIRROR.md#manual-fallback).

## Recovery and Rollback

### Re-tagging an UNPUBLISHED release (pulled before npm/crates)

If `vX.Y.Z` was tagged and a GitHub Release was created, but **no package was
published** (npm still on the prior version, `check-published.sh` shows the
crates unpublished), the tag is still recoverable — a bug found after tagging
can be fixed and the same version recut. Confirm first that nothing consumed it:
`npm view codewhale version` and crates.io both show the PRIOR version, no
Homebrew/Winget/mirror points at it, and the GitHub Release download counts are
only the release pipeline's own verification passes. Then, with explicit
maintainer approval:

```bash
# 1. land the fix on main (normal PR + required CI)
# 2. delete the premature Release + tag
gh release delete vX.Y.Z --repo Hmbown/CodeWhale --yes --cleanup-tag
git push origin :refs/tags/vX.Y.Z    # belt-and-suspenders
git tag -d vX.Y.Z                    # local
# 3. recut at the fixed HEAD (workspace version unchanged)
gh workflow run auto-tag.yml --repo Hmbown/CodeWhale --ref main
# 4. release.yml rebuilds assets; rebuild + reinstall locally from the new tag
```

This is the sanctioned path from "do not delete/move/recreate a release tag
implicitly": it is explicit, approved, and only for a tag no registry consumer
has treated as public. Never do it once a crate or the npm wrapper is published
for that version — bump to the next patch instead.

### External publish gates (not code defects)

- **crates.io:** publishing needs a valid `cargo login` token on the operator
  machine (`curl -H "Authorization: <token>" https://crates.io/api/v1/me`
  returning 200). A 403 means the token is missing/expired — `cargo login`,
  then `./scripts/release/publish-crates.sh publish`.
- **npm:** the OIDC job publishes only if the npmjs.com Trusted Publisher for
  `Hmbown` / `CodeWhale` / workflow `release.yml` / blank environment is
  configured. Missing config → the `npm` job fails `E404 No match found`. Fix
  the binding (or use the manual WebAuthn recovery above), then re-run the
  failed `npm` job: `gh run rerun <release-run-id> --failed`.

- User-facing rollback:
  - npm: `npm install -g codewhale@X.Y.Z`
  - Cargo: `cargo install codewhale-cli --version X.Y.Z --locked --force`;
    add an optional `codew` alias as documented in
    [docs/INSTALL.md](INSTALL.md#7-build-from-source)
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
  - fix `.github/workflows/release.yml`, but do not rerun it over an existing
    asset set and do not delete assets merely to make the guard pass
  - if any asset may have been public or consumed, cut a new patch version
  - only after explicit maintainer approval and proof that no downstream
    publication or consumer treated the failed asset set as public may a
    deliberately scoped recovery remove the failed release before an exact-tag
    rerun; record that exception in the release packet
- npm packaging-only problem:
  - bump only the npm package version
  - keep `codewhaleBinaryVersion` on the last known-good Rust release
  - repack and republish the wrapper
- A bad npm publish cannot be overwritten:
  - publish a new npm version with corrected metadata or install logic
- CNB mirror failed for the release tag:
  - check the run via `gh run list --workflow=sync-cnb.yml`
  - rerun the failed tag run, or dispatch
    `gh workflow run sync-cnb.yml --ref vX.Y.Z`; never omit the tag ref
  - follow the proof steps in
    [docs/CNB_MIRROR.md](CNB_MIRROR.md#manual-fallback)
- Workflow runner failure or hung release job:
  - every release-lane job carries an explicit `timeout-minutes` to contain
    unattended runs, but timeouts are containment rather than immediate recovery
  - if a workflow job sits `in_progress` with 404 logs (or produces no useful
    log output for 20 minutes), cancel the run and rerun failed jobs / dispatch
    an exact-ref rerun rather than waiting out the full job timeout
  - check the last-useful-log timestamp before cancelling to distinguish an
    infrastructure failure (runner dropped / HTTP 404 on log stream) from a
    legitimate long build step (e.g. Windows artifact compilation)
