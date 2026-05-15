# CNB Deploy Templates

The root `.cnb.yml` is intentionally source-controlled in GitHub because CNB is
a one-way mirror from GitHub. Do not add or edit `.cnb.yml` only on the CNB
side; the next GitHub sync will overwrite it.

The active root `.cnb.yml` does two things:

- runs Feishu bridge and version-drift checks when CNB receives `main`;
- builds Linux x64 release assets from `v*` tags, creates the CNB release, and
  uploads `deepseek-linux-x64`, `deepseek-tui-linux-x64`, and
  `deepseek-artifacts-sha256.txt`.

The files in this directory are retained as deploy-button templates for Tencent
Lighthouse. Copy only the deploy environment file after the Lighthouse instance
is already working manually:

```bash
mkdir -p .cnb
cp deploy/tencent-lighthouse/cnb/tag_deploy.yml.example .cnb/tag_deploy.yml
```

If you also need to customize `.cnb.yml`, edit the root file in GitHub and let
the one-way mirror carry it to CNB.

## Required CNB Secrets

Configure these as protected CNB environment variables or secrets:

- `LIGHTHOUSE_HOST`: public IP or DNS name of the Lighthouse instance
- `LIGHTHOUSE_SSH_TARGET`: SSH target, for example `ubuntu@203.0.113.10`
- `LIGHTHOUSE_SSH_PRIVATE_KEY`: private deploy key allowed to update the server
- `DEEPSEEK_REPO_BRANCH`: branch or tag to deploy, for example `main`

Optional:

- `DEEPSEEK_REPO_URL`: defaults to the CNB mirror URL
- `LIGHTHOUSE_SSH_PORT`: defaults to `22`

The server side should already have `/opt/whalebro/deepseek-tui`,
`/etc/deepseek/runtime.env`, `/etc/deepseek/feishu-bridge.env`, and the
systemd services from `docs/TENCENT_LIGHTHOUSE_HK.md`.

## Safety Notes

- Do not store Feishu App Secret or DeepSeek API keys in CNB. They belong in
  `/etc/deepseek/*.env` on Lighthouse.
- Do not expose `127.0.0.1:7878` through EdgeOne, a security group, or a public
  reverse proxy.
- Start with a manual deploy button. Automatic deploy on every `main` push is
  convenient later, but it can consume CNB quota and restart the phone bridge
  while a turn is active.
