# CNB Cool mirror

`cnb.cool/deepseek-tui.com/DeepSeek-TUI` is a one-way mirror of this
GitHub repository for users on networks where GitHub is slow or blocked
(primarily mainland China). The mirror receives every push to `main`, every
`v*` release tag, and Tencent release-candidate branches used by the
Lighthouse/Feishu setup.

## How it works

The mirror is maintained by the [`Sync to CNB`](../.github/workflows/sync-cnb.yml)
GitHub Actions workflow:

- **Trigger:** `push` to `main`, `push` of any `v*` tag,
  Tencent setup branches matching `work/v*-feishu-*` or
  `work/v*-lighthouse*`, or `workflow_dispatch` for manual recovery.
- **Auth:** HTTPS basic auth as user `cnb` with the `CNB_GIT_TOKEN`
  repository secret as the password.
- **Scope:** only the ref that triggered the run is pushed. Tag pushes
  push exactly that tag. Branch pushes mirror `main` or an explicitly
  matched Tencent setup branch. Other feature branches and dependabot refs
  are intentionally *not* mirrored.
- **Concurrency:** runs are serialized via a `cnb-sync` concurrency
  group so the back-to-back `main` push and tag push from
  `auto-tag.yml` cannot race each other.
- **Retry:** each push is retried up to three times with linear
  backoff (5s, 10s) before the workflow gives up.

CNB pipeline configuration is also source-controlled in GitHub at
[`/.cnb.yml`](../.cnb.yml). This is deliberate: the sync workflow force-mirrors
GitHub refs to CNB, so pipeline files created only on the CNB side will be
overwritten. Submit `.cnb.yml` changes through GitHub PRs and let the one-way
mirror carry them to CNB.

## CNB tag releases

When CNB receives a `v*` tag, the root `.cnb.yml` tag pipeline builds Linux x64
release assets from source and publishes a CNB release with:

- `deepseek-linux-x64`
- `deepseek-tui-linux-x64`
- `deepseek-artifacts-sha256.txt`

This gives users who can reach CNB but not GitHub a CNB-native release path.
GitHub remains the canonical full release matrix; the CNB tag pipeline is the
China-friendly Linux x64 fallback.

## Verifying the mirror after a release

After `release.yml` completes for a `vX.Y.Z` tag, the CNB mirror
should have both the new commit on `main` and the new tag:

```bash
# Quick check: does the new tag exist on CNB?
git ls-remote https://cnb.cool/deepseek-tui.com/DeepSeek-TUI.git \
    refs/tags/vX.Y.Z

# Quick check: is CNB's main at the same commit as origin/main?
gh_main=$(git ls-remote https://github.com/Hmbown/DeepSeek-TUI.git refs/heads/main | awk '{print $1}')
cnb_main=$(git ls-remote https://cnb.cool/deepseek-tui.com/DeepSeek-TUI.git refs/heads/main | awk '{print $1}')
test "$gh_main" = "$cnb_main" && echo "in sync" || echo "DIVERGED: gh=$gh_main cnb=$cnb_main"
```

Or check the workflow run directly:

```bash
gh run list --workflow=sync-cnb.yml --repo Hmbown/DeepSeek-TUI --limit 5
```

If the most recent run for the release tag is `success`, the mirror
caught it. If it's `failure`, follow the manual fallback below.

## Manual fallback

If the workflow fails for any reason (CNB rate-limit, token expired,
GitHub outage, etc.), the maintainer can push to CNB by hand from
their local checkout. This works because the CNB token is a personal
PAT — the same token used by the workflow lives in the maintainer's
password manager.

### One-time setup

```bash
# Add the CNB remote alongside origin.
git remote add cnb https://cnb:${CNB_TOKEN}@cnb.cool/deepseek-tui.com/DeepSeek-TUI.git

# Or, if you don't want the token in your shell history:
git remote add cnb https://cnb.cool/deepseek-tui.com/DeepSeek-TUI.git
# (you'll be prompted for username `cnb` and password ${CNB_TOKEN}
#  on the first push; subsequent pushes use the credential helper.)
```

### Sync a release manually

```bash
# Make sure main is current.
git fetch origin
git checkout main
git reset --hard origin/main

# Push main first, then the tag. Order matters: CNB should see the
# commit before the tag that points at it.
git push cnb main --force-with-lease
git push cnb vX.Y.Z
```

### Re-trigger the workflow manually

If the workflow is healthy but happened to fail on the release run
(e.g. a transient CNB outage that's since cleared), retrigger it
without pushing anything:

```bash
gh workflow run sync-cnb.yml --repo Hmbown/DeepSeek-TUI
```

`workflow_dispatch` runs against the workflow's default branch
(`main`), so this will sync the current `main` to CNB. To re-sync
a specific tag, the manual `git push cnb` path above is the way.

## Rotating `CNB_GIT_TOKEN`

If the workflow starts failing with auth errors and the token has
expired:

1. Log in to `cnb.cool` and generate a new personal access token
   with `repo` (push) scope.
2. Update the `CNB_GIT_TOKEN` repository secret:
   ```bash
   gh secret set CNB_GIT_TOKEN --repo Hmbown/DeepSeek-TUI
   ```
3. Re-trigger the workflow on a recent commit:
   ```bash
   gh workflow run sync-cnb.yml --repo Hmbown/DeepSeek-TUI
   ```
4. Confirm the run succeeds via `gh run list --workflow=sync-cnb.yml`.

## Binary release assets and `deepseek update`

CNB now builds Linux x64 assets for `v*` tags from the source-controlled
`.cnb.yml` pipeline. GitHub remains the canonical full release matrix. Users
behind GitHub-blocking networks should use one of these paths:

- **`cargo install`** from the CNB mirror:
  ```bash
  cargo install --git https://cnb.cool/deepseek-tui.com/DeepSeek-TUI --tag vX.Y.Z deepseek-tui-cli
  cargo install --git https://cnb.cool/deepseek-tui.com/DeepSeek-TUI --tag vX.Y.Z deepseek-tui
  ```
  (Both binaries are required — the dispatcher and the TUI ship
  separately; see `AGENTS.md` for the two-binary install rationale.)

- **CNB release assets** for Linux x64, when the matching CNB tag pipeline has
  completed successfully. Download `deepseek-linux-x64`,
  `deepseek-tui-linux-x64`, and `deepseek-artifacts-sha256.txt` from the CNB
  release for `vX.Y.Z`, then verify the binaries against the manifest.

- **`DEEPSEEK_TUI_RELEASE_BASE_URL`** environment variable, if a
  CDN mirror of release assets exists. The npm
  wrapper installer and `deepseek update` read this variable to redirect
  binary downloads. For `deepseek update`, also set
  `DEEPSEEK_TUI_VERSION=X.Y.Z` so the updater can label the mirrored
  release without contacting GitHub. The directory pointed to must contain
  `deepseek-artifacts-sha256.txt` and the platform binaries; format matches
  a GitHub Release asset directory.

## Tencent Cloud remote-first path

The Lighthouse + Feishu/Lark tutorial uses CNB as the Tencent-side source and
automation lane. For a stable install, clone `main` or a release tag from:

```bash
https://cnb.cool/deepseek-tui.com/DeepSeek-TUI.git
```

The mirror receives `main`, release tags, and the Tencent setup branch patterns
used by the Lighthouse/Feishu tutorial. Those CNB refs are the default source
for Tencent-side bootstrap; GitHub is the fallback when the CNB workflow or
credentials are unhealthy.

CNB deploy-button examples live in `deploy/tencent-lighthouse/cnb/`. They are
not active until copied into `.cnb.yml` and `.cnb/tag_deploy.yml`, because live
deploy jobs require a Lighthouse deploy key, target host, and explicit CNB
quota/billing policy.
