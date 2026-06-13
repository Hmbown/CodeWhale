# Tencent Cloud Remote-First Quickstart

This is the opinionated Tencent-native teaching path for codewhale users
who want an always-on agent workspace, a phone control surface, and a stack
that works well from mainland China.

It complements the local install path. If you only want to use `codewhale` on a
laptop, start with the README quickstart. If you want "CodeWhale as a remote
workbench I can control from my phone", start here.

## Default Stack

```text
GitHub main/tags
  -> CNB mirror: cnb.cool/codewhale.net/codewhale
  -> optional CNB build/deploy pipeline
  -> Tencent Lighthouse HK
       /opt/whalebro/codewhale
       /opt/whalebro/worktrees
       codewhale-runtime.service on 127.0.0.1:7878
       codewhale-feishu-bridge.service or codewhale-telegram-bridge.service
  -> Feishu/Lark or Telegram phone DM

EdgeOne is optional:
  public HTTPS domain -> EdgeOne -> Caddy/Nginx on Lighthouse
```

## What Each Piece Does

- **CNB** is the Tencent-side source and automation lane. The existing
  `cnb.cool` mirror is useful for clones and tagged installs when GitHub is
  slow. Optional CNB deploy templates live under
  `deploy/tencent-lighthouse/cnb/`.
- **Lighthouse** is the private always-on host. It owns `/opt/whalebro`,
  systemd, Rust/Node installs, and the `codewhale serve --http` runtime.
- **Telegram** is the simplest phone MVP. The bridge uses long polling, so the
  first setup does not need a public webhook URL.
- **Feishu/Lark** is the Tencent-native enterprise phone UI. The bridge uses
  long-connection mode, so the first setup does not need a public webhook URL.
- **EdgeOne** is the public edge only when you intentionally expose a web
  surface such as docs, a status page, or a future webhook endpoint. Do not put
  the runtime API behind EdgeOne.

## First Lesson: Get a Remote Agent Running

1. Buy or reuse a Tencent Lighthouse instance in Hong Kong.
2. Clone from CNB by default when the branch or tag exists there:

   ```bash
   export CODEWHALE_REPO_URL=https://cnb.cool/codewhale.net/codewhale.git
   git ls-remote "$CODEWHALE_REPO_URL" refs/heads/main
   ```

   Tencent setup branches matching `work/v*-feishu-*` or
   `work/v*-lighthouse*` are mirrored by the GitHub CNB sync workflow. Use
   the GitHub URL only when the CNB workflow or credentials are unhealthy.

3. Bootstrap `/opt/whalebro` on the server:

   ```bash
   export CODEWHALE_BRANCH=main
   git clone --branch "$CODEWHALE_BRANCH" "$CODEWHALE_REPO_URL" /tmp/codewhale
   cd /tmp/codewhale
   sudo CODEWHALE_REPO_URL="$CODEWHALE_REPO_URL" \
     CODEWHALE_REPO_BRANCH="$CODEWHALE_BRANCH" \
     bash scripts/tencent-lighthouse/bootstrap-ubuntu.sh
   ```

4. Install Rust for the `codewhale` user, build both binaries, and install the
   systemd units using `docs/TENCENT_LIGHTHOUSE_HK.md`.
5. Configure either a Telegram bot (`CODEWHALE_BRIDGE=telegram` and
   `/etc/codewhale/telegram-bridge.env`) or a Feishu/Lark self-built app
   (`CODEWHALE_BRIDGE=feishu` and `/etc/codewhale/feishu-bridge.env`), run the
   validator, then run the VPS doctor.
6. From your phone DM, validate `/status`, a harmless prompt, `/interrupt`,
   `/threads`, `/resume`, approval allow/deny, service restart, and reboot
   persistence.

## Second Lesson: Make CNB the Deploy Button

Once the manual Lighthouse path works, copy the non-active examples from
`deploy/tencent-lighthouse/cnb/` into the CNB repository:

- `cnb.yml.example` -> `.cnb.yml`
- `tag_deploy.yml.example` -> `.cnb/tag_deploy.yml`

The intended deploy button should:

1. Run bridge validation/tests and lightweight release-version checks.
2. SSH to Lighthouse with a deploy key stored as a CNB secret.
3. Update `/opt/whalebro/codewhale`.
4. Rebuild/install both binaries.
5. Reinstall/restart systemd services.
6. Run `scripts/tencent-lighthouse/doctor.sh`.

Do not enable this on `main` until the deploy key, target host, billing/quota,
and rollback policy are explicit.

## Third Lesson: Add EdgeOne Only For Public HTTPS

The Feishu/Lark long-connection bridge works without EdgeOne. Add EdgeOne when
you want a public domain in front of a deliberate HTTP service:

- a public tutorial/docs site
- a small operator status page
- a future webhook-mode bridge
- a demo app running on the same Lighthouse origin

Keep these rules:

- `codewhale serve --http` stays bound to `127.0.0.1`.
- `/v1/*` runtime endpoints are never public.
- `CODEWHALE_RUNTIME_TOKEN` never leaves the server env files.
- Phone-bridge group control stays off until a specific group allowlist is set.
- Auto-approval stays off for the phone bridge unless a maintainer explicitly
  accepts the risk.

## Teaching Order

Use this sequence when explaining codewhale to a new remote-first user:

1. **Local mental model:** `codewhale` is the dispatcher, `codewhale-tui` is the
   companion runtime, and both binaries matter.
2. **Agent safety:** Plan/Agent/YOLO are separate from approval mode and
   sandboxing.
3. **Remote runtime:** `codewhale serve --http` is a localhost runtime API, not
   a public web app.
4. **Phone bridge:** Telegram or Feishu/Lark messages become runtime requests
   through an allowlisted bridge.
5. **CNB automation:** once manual setup is proven, CNB turns the setup into a
   repeatable deploy button.
6. **EdgeOne edge:** add the public edge after you know exactly what public
   surface you are exposing.

## References

- CNB mirror details: `docs/CNB_MIRROR.md`
- Lighthouse implementation runbook: `docs/TENCENT_LIGHTHOUSE_HK.md`
- Telegram bridge: `integrations/telegram-bridge/README.md`
- Feishu/Lark bridge: `integrations/feishu-bridge/README.md`
- CNB templates: `deploy/tencent-lighthouse/cnb/`
