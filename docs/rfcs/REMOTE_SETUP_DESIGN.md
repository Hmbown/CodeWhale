# `codewhale remote-setup` - Tailscale-first design

Status: **design / revision**. This RFC revises the earlier cloud-first
`remote-setup` plan. Keep the accurate implementation work already present:
`codewhale remote-setup` exists today as a generate-only bundle wizard for
cloud plus chat bridge deployments, and `--apply` is still not implemented.

## Goal

Give users a guided, education-forward way to reach a local-first Codewhale
runtime from another surface without accidentally publishing their agent.

Default posture:

1. **Local-first by default.**
2. **Tailnet-private when remote.**
3. **Public only when explicitly chosen.**

The wizard should ask:

> How do you want to reach Codewhale?

and offer these paths, in this order:

1. This machine only (localhost)
2. Private devices with Tailscale (**Recommended**)
3. Telegram bot
4. Feishu/Lark bot
5. Weixin personal bridge
6. Public webhook / Funnel (**Advanced**)

The recommended remote answer is Tailscale Serve with the backend still bound
to `127.0.0.1`. Tailscale supplies device identity and encrypted transport.
Tailscale Funnel is public internet exposure and must stay advanced.

## Current implementation checkpoint

### `/setup` → Remote runtime step (#3409)

The setup wizard's `RemoteRuntime` card presents exactly four modes, each with a
status derived from observable state — never from intent:

| mode | status source |
|------|---------------|
| this machine only | always `ready`; nothing is exposed and nothing to configure |
| runtime API | presence (never value) of `CODEWHALE_RUNTIME_TOKEN` / `DEEPSEEK_RUNTIME_TOKEN` |
| phone on your network | `disabled` unless `CODEWHALE_RUNTIME_HOST` is set, because the shipped unit binds loopback |
| chat app | per-bridge `secret_keys` presence from `remote_setup::registry` |

Contract for that step:

- **Secret values are never read or rendered.** Only variable *names* and
  set/unset are used, so a hostile token cannot reach the UI or the persisted
  step result.
- **`R` renders a plan preview in memory** through `remote_setup::bundle::render_bundle`
  with every secret replaced by `<redacted>`. It never calls `write_bundle`,
  never runs a provisioning command, and creates no files.
- **Missing tokens/config are `NeedsAction`, never blocking.** Local-only is the
  default and settles the step in one key, so a user who wants no remote access
  is finished immediately. `/setup`'s report and `doctor` inherit the recorded
  status verbatim.

Verified against the codebase:

- `codewhale app-server --http` is the canonical HTTP/SSE runtime API entrypoint.
  It delegates to the mature `serve --http` implementation.
- `codewhale app-server --mobile` is real and serves the phone control page at
  `/mobile`.
- `--host`, `--port`, `--workers`, `--auth-token`, `--insecure-no-auth`, and
  repeatable `--cors-origin` exist on `app-server --http` / `--mobile`.
- `--mobile` without `--host` binds to `0.0.0.0` by design. Use
  `--host 127.0.0.1` when putting Tailscale in front of the runtime.
- `/health` and `/v1/runtime/info` are public bootstrap/supervision endpoints.
  `/v1/*` control routes require the runtime bearer token unless auth is
  explicitly disabled on a trusted loopback bind.
- `codewhale doctor --json` exists as the machine-readable local diagnostic.
- `codewhale remote-setup` exists, but today it is generate-only. Its current
  matrix is cloud target (`lighthouse`, `azure`, `digitalocean`) x bridge
  (`feishu`, `telegram`) x provider registry. It does **not** yet model
  localhost, Tailscale, Weixin, or Funnel as first-class choices.
- Telegram and Feishu bridge validators exist as `npm run validate:config`.
  Weixin currently has `npm run check`, but no validate-config script.

Accuracy note for the Tailscale recommendation: the requested setup uses
`app-server --http`, but the current runtime serves `/mobile` only in mobile
mode. This RFC keeps the target command shape for the recommended loopback
runtime, and documents the verified current-binary variant when the mobile page
is required:

```bash
# Runtime API only, verified:
codewhale app-server --http --host 127.0.0.1 --port 7878 --auth-token "$CODEWHALE_RUNTIME_TOKEN"

# Runtime API plus /mobile, verified:
codewhale app-server --mobile --host 127.0.0.1 --port 7878 --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

## Common runtime base

Every path starts from the same local runtime trust boundary.

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

For the current binary, use `--mobile --host 127.0.0.1` instead of `--http` if
the path needs the built-in `/mobile` page.

Doctor-style local validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  http://127.0.0.1:7878/v1/runtime/info
```

Runtime mental model:

- Exposed by Codewhale: only the address it binds. The recommended bind is
  `127.0.0.1:7878`.
- Auth token: `CODEWHALE_RUNTIME_TOKEN`, passed as `Authorization: Bearer ...`
  by clients and bridges. Legacy `DEEPSEEK_RUNTIME_TOKEN` remains a fallback.
- Provider secrets: stay in runtime configuration, not in bridge env files.
- Bridge secrets: stay in transport-specific env files.

## Guided flow

### 1. This machine only (localhost)

Use this when the TUI, SDK, browser, or local script runs on the same machine as
Codewhale.

Setup:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

Env template:

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<same value used to start app-server>
```

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  http://127.0.0.1:7878/v1/runtime/info
```

Trust boundary:

- Exposed: loopback only.
- Not exposed: LAN, tailnet, or public internet.
- Token used: `CODEWHALE_RUNTIME_TOKEN` for control routes; local `/health` and
  `/v1/runtime/info` are public bootstrap endpoints.

### 2. Private devices with Tailscale (Recommended)

Use this to reach Codewhale from your phone or laptop without opening a LAN or
public port. Tailscale authenticates devices in your tailnet; Codewhale still
binds to localhost.

Target setup to feature in the wizard:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

tailscale serve --bg --https=443 localhost:7878
```

Then open the Tailscale Serve URL from a phone or laptop in the same tailnet.
For the current binary's mobile page, start Codewhale with the verified mobile
variant:

```bash
codewhale app-server --mobile \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"
```

Then open (put the token in the URL **fragment**, not a query param — the
`/mobile` page reads it from `location.hash`, and a fragment is never sent to
the Tailscale serving layer or to any proxy log):

```text
https://<machine>.<tailnet>.ts.net/mobile#token=<CODEWHALE_RUNTIME_TOKEN>
```

Env template:

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<openssl-rand-hex-32>
TAILSCALE_SERVE_TARGET=localhost:7878
TAILSCALE_SERVE_URL=https://<machine>.<tailnet>.ts.net
```

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS https://<machine>.<tailnet>.ts.net/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  https://<machine>.<tailnet>.ts.net/v1/runtime/info
tailscale serve status
```

Trust boundary:

- Exposed: an HTTPS endpoint reachable by devices authorized in your tailnet.
- Not exposed: the raw Codewhale listener; it stays on `127.0.0.1`.
- Token used: Tailscale identity gates network reachability; Codewhale still
  uses `CODEWHALE_RUNTIME_TOKEN` for runtime control.
- Caveat: Tailscale Serve is private to the tailnet. Tailscale Funnel is public
  internet exposure and belongs only in the advanced path below.

### 3. Telegram bot

Use this when a Telegram DM should control a local Codewhale runtime. The bridge
uses Telegram Bot API long polling, so it does not require a public webhook URL
or inbound port.

Setup:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/telegram-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
npm start
```

Env template:

```env
TELEGRAM_BOT_TOKEN=replace-with-botfather-token

CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<same value used to start app-server>
CODEWHALE_WORKSPACE=/path/to/workspace
# Optional override; leave blank to inherit the runtime's configured provider/model.
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # grants shell execution from the bridge; set false for text-only chat
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

TELEGRAM_CHAT_ALLOWLIST=
TELEGRAM_ALLOW_UNLISTED=false
TELEGRAM_ALLOW_GROUPS=false
```

First pairing:

```bash
# Temporarily in .env:
TELEGRAM_ALLOW_UNLISTED=true
```

DM the bot `/status`, copy the returned `chat_id` or `user_id` into
`TELEGRAM_CHAT_ALLOWLIST`, then set `TELEGRAM_ALLOW_UNLISTED=false` and restart
the bridge.

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
```

Trust boundary:

- Exposed: no inbound Codewhale port. Telegram sees messages sent to the bot.
- Not exposed: Codewhale remains on `127.0.0.1`; provider keys stay in the
  runtime env, not the Telegram env.
- Tokens used: `TELEGRAM_BOT_TOKEN` for Telegram, `CODEWHALE_RUNTIME_TOKEN` for
  bridge-to-runtime calls, and `TELEGRAM_CHAT_ALLOWLIST` for user/chat gating.
- Caveat: direct messages are the intended MVP control surface. Group control is
  off unless `TELEGRAM_ALLOW_GROUPS=true`.

### 4. Feishu/Lark bot

Use this when a Feishu or Lark chat should control the local runtime. The bridge
uses the Lark/Feishu long-connection SDK, so the first version does not need a
public webhook URL.

Setup:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/feishu-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
npm start
```

Env template:

```env
FEISHU_APP_ID=cli_xxxxxxxxxxxxxxxx
FEISHU_APP_SECRET=replace-with-app-secret
FEISHU_DOMAIN=feishu               # international Lark users: set to "lark"

CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<same value used to start app-server>
CODEWHALE_WORKSPACE=/path/to/workspace
# Optional override; leave blank to inherit the runtime's configured provider/model.
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # grants shell execution from the bridge; set false for text-only chat
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

CODEWHALE_CHAT_ALLOWLIST=
CODEWHALE_ALLOW_UNLISTED=false
FEISHU_ALLOW_GROUPS=false
```

First pairing:

Temporarily set `CODEWHALE_ALLOW_UNLISTED=true`, message the app once, copy the
logged open id into `CODEWHALE_CHAT_ALLOWLIST`, then set
`CODEWHALE_ALLOW_UNLISTED=false` and restart the bridge.

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run validate:config -- \
  --env .env \
  --workspace-root "$PWD/../.." \
  --check-filesystem
```

Trust boundary:

- Exposed: no inbound Codewhale port. Feishu/Lark sees messages sent to the app.
- Not exposed: Codewhale remains on `127.0.0.1`; provider keys stay in runtime
  config.
- Tokens used: `FEISHU_APP_ID` / `FEISHU_APP_SECRET` for the platform,
  `CODEWHALE_RUNTIME_TOKEN` for bridge-to-runtime calls, and
  `CODEWHALE_CHAT_ALLOWLIST` for chat gating.
- Caveat: group control is off unless explicitly enabled.

### 5. Weixin personal bridge

Use this when a personal Weixin account should control the local runtime by QR
login. This is not a public account webhook. The bridge initiates long polling
and does not need a public port.

Setup:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --http \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

cd integrations/weixin-bridge
npm install --omit=dev
cp .env.example .env
$EDITOR .env
npm run check
npm start
```

Env template:

```env
CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
CODEWHALE_RUNTIME_TOKEN=<same value used to start app-server>
CODEWHALE_WORKSPACE=/path/to/workspace
# Optional override; leave blank to inherit the runtime's configured provider/model.
CODEWHALE_MODEL=
CODEWHALE_MODE=agent
CODEWHALE_ALLOW_SHELL=true     # grants shell execution from the bridge; set false for text-only chat
CODEWHALE_TRUST_MODE=false
CODEWHALE_AUTO_APPROVE=false

WEXIN_CHAT_ALLOWLIST=
WEXIN_ALLOW_UNLISTED=false
WEXIN_STATE_DIR=/var/lib/codewhale-weixin-bot-bridge
```

First pairing:

Set `WEXIN_ALLOW_UNLISTED=true`, start the bridge, scan the QR code, send
`/status`, copy the returned `user_id` into `WEXIN_CHAT_ALLOWLIST`, then set
`WEXIN_ALLOW_UNLISTED=false` and restart the bridge.

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
npm run check
```

Trust boundary:

- Exposed: no inbound Codewhale port. The personal Weixin session and the
  bridge state directory become sensitive local state.
- Not exposed: Codewhale remains on `127.0.0.1`; provider keys stay in runtime
  config.
- Tokens used: the scanned Weixin login/session state for platform access,
  `CODEWHALE_RUNTIME_TOKEN` for bridge-to-runtime calls, and
  `WEXIN_CHAT_ALLOWLIST` for user gating.
- Caveat: this is a personal-account bridge. Treat the host and state directory
  like a logged-in phone session.

### 6. Public webhook / Funnel (Advanced)

Use this only when the user explicitly chooses public internet reachability,
understands that the URL can be reached outside the tailnet, and has a reason
that Tailscale Serve or long polling cannot satisfy.

Preferred advanced pattern:

```bash
CODEWHALE_RUNTIME_TOKEN="$(openssl rand -hex 32)"
export CODEWHALE_RUNTIME_TOKEN

codewhale app-server --mobile \
  --host 127.0.0.1 \
  --port 7878 \
  --auth-token "$CODEWHALE_RUNTIME_TOKEN"

tailscale funnel --bg --https=443 localhost:7878
```

Env template:

```env
CODEWHALE_RUNTIME_URL=https://<public-name>
CODEWHALE_RUNTIME_TOKEN=<openssl-rand-hex-32>
PUBLIC_EXPOSURE_ACK=true
```

Validation:

```bash
codewhale doctor --json
curl -fsS http://127.0.0.1:7878/health
curl -fsS https://<public-name>/health
curl -fsS \
  -H "Authorization: Bearer $CODEWHALE_RUNTIME_TOKEN" \
  https://<public-name>/v1/runtime/info
tailscale funnel status
```

Trust boundary:

- Exposed: a public HTTPS endpoint, not just your tailnet.
- Not exposed by Codewhale directly: the backend still binds to `127.0.0.1`,
  but the fronting layer makes selected routes reachable from the internet.
- Token used: `CODEWHALE_RUNTIME_TOKEN` remains mandatory for control routes.
- Caveat: public does not mean safe. Do not use `--insecure-no-auth`, do not bind
  Codewhale to `0.0.0.0`, and do not call this the default.

## Cloud/VPS posture

Cloud/VPS is a placement choice, not a trust model. The old RFC's cloud work is
still useful, but it should sit behind the same reachability choices:

- A VPS can run the runtime bound to `127.0.0.1`.
- Recommended remote access from personal devices is still Tailscale Serve.
- Bot bridges should use long polling / long connection where available, keeping
  the runtime localhost-only on the host.
- SSH tunnels remain acceptable for ad hoc validation:

```bash
ssh -L 7878:127.0.0.1:7878 <host>
```

Public inbound listeners, public webhooks, and Tailscale Funnel are advanced
choices, not the default cloud path.

## Prior art: Hermes Agent (reference only - do not copy)

Nous Research's Hermes Agent validates the table-driven part of this design.
Use it for ideas; keep Codewhale's style: Rust core, local runtime, zero-dep
Node bridges where possible, and plain-text replies.

- `gateway/platform_registry.py` maps to our `BridgeSpec` / access-path
  registry: one row per platform, with setup hints, required env, validation,
  and adapter factory.
- `gateway/pairing.py` maps to our allowlist / first-pairing flow.

Telegram hardening carried forward from the original RFC:

| Edge case | In Hermes | In our Telegram bridge |
|---|---|---|
| 409 polling conflict | `_looks_like_polling_conflict` | done - poll loop backs off and warns |
| 429 `retry_after` | rate-limit handling | done - `telegramApi` honors `parameters.retry_after` |
| Forum General topic id handling | send/typing split | done - omit `message_thread_id` when id is 1 on send |
| Stale reply anchor after restart | retry without anchor | sidestepped - no `reply_to_message_id` |
| Network/connect timeout retry | network error detection | partial - generic poll-loop backoff |
| Text batching / progress edit | progress-edit tests | deferred - plain periodic chunks |
| MarkdownV2 escaping | escaping helpers | deferred - plain text |
| Webhook mode | webhook adapter | out of default scope - long polling first |

## Design principle: table-driven, like `ProviderSpec`

The provider registry is the model to preserve: adding a provider is one row.
Apply the same idea to access paths, bridges, and cloud placements so the matrix
grows by data.

```text
AccessPath x Placement x BridgeSpec + ProviderSpec
----------   ---------   ----------   ------------
localhost    local       none         deepseek / openai / ...
tailscale    local/vps   none         provider lives in runtime.env
telegram     local/vps   telegram     bridge is pure transport
feishu       local/vps   feishu       bridge is pure transport
weixin       local/vps   weixin       bridge is pure transport
funnel       local/vps   optional     explicit public exposure
```

Clean separation:

- **Provider = runtime env.** The runtime resolves provider/model/API key from
  `CODEWHALE_PROVIDER`, provider key vars, and the provider registry. Bridges do
  not need provider keys.
- **Access path = reachability.** Localhost, Tailscale Serve, chat long polling,
  and Funnel are separate choices with different trust boundaries.
- **Bridge = transport.** A chat bridge forwards allowed chat messages to
  `http://127.0.0.1:7878` with `CODEWHALE_RUNTIME_TOKEN`.
- **Cloud = where it runs and where secrets live.** It is not permission to
  open port 7878.

## Proposed command surface

Current flags are verified for the generate-only cloud/bridge wizard:

| Flag | Current status |
|---|---|
| `--cloud <lighthouse|azure|digitalocean>` | verified |
| `--bridge <telegram|feishu>` | verified |
| `--provider <slug>` | verified, provider registry-backed |
| `--out <dir>` | verified |
| `--generate-only` | verified |
| `--apply` | verified flag, but not implemented |
| `--yes` | verified flag |
| `--non-interactive` | verified flag |

Proposed Tailscale-first revision:

| Flag | Meaning |
|---|---|
| `--access <localhost|tailscale|telegram|feishu|weixin|funnel>` | Skip the reachability prompt. |
| `--placement <local|vps|lighthouse|azure|digitalocean>` | Where the runtime runs; default local. |
| `--bridge <telegram|feishu|weixin>` | Optional when `--access` implies a bridge. |
| `--provider <slug>` | Provider slug; validated against the existing provider registry. |
| `--out <dir>` | Bundle output dir. |
| `--generate-only` | Emit commands/env/runbook, do not provision. Default. |
| `--apply` | Future cloud CLI provisioning, behind confirmation. Still not implemented. |
| `--yes` | Skip final confirmation gates where safe for CI/non-interactive use. |
| `--non-interactive` | Fail instead of prompting for missing required values. |

The first prompt should be the reachability question, not the cloud question.
Tailscale should be visually marked as recommended.

## Generated bundle

The current bundle model stays useful. Extend it so the generated runbook is
access-path-first.

Files:

- `runtime.env` - provider and runtime config:

  ```env
  CODEWHALE_PROVIDER=openai
  OPENAI_API_KEY=replace-with-provider-key
  # Optional override; leave blank to inherit the runtime's configured provider/model.
CODEWHALE_MODEL=
  CODEWHALE_RUNTIME_TOKEN=<random>
  CODEWHALE_RUNTIME_PORT=7878
  CODEWHALE_RUNTIME_WORKERS=2
  RUST_LOG=info
  ```

- `<bridge>.env` - transport only when a bridge is selected:

  ```env
  CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878
  CODEWHALE_RUNTIME_TOKEN=<same random token>
  CODEWHALE_WORKSPACE=/opt/whalebro
  # Optional override; leave blank to inherit the runtime's configured provider/model.
CODEWHALE_MODEL=
  CODEWHALE_MODE=agent
  CODEWHALE_ALLOW_SHELL=true     # grants shell execution from the bridge; set false for text-only chat
  CODEWHALE_TRUST_MODE=false
  CODEWHALE_AUTO_APPROVE=false
  ```

- `codewhale-runtime.service`
- optional `codewhale-<bridge>.service`
- optional cloud artifacts: `cloud-init.yaml`, `provision.sh`, `cnb.yml`, or
  cloud-specific runbook steps
- `RUNBOOK.md` with:
  - exact setup commands
  - env template
  - doctor-style validation
  - first-pairing steps for bridges
  - trust-boundary summary
  - explicit "public exposure acknowledged" section for Funnel/webhook modes

## Auto-provision

Preserve the original safety model:

- `--generate-only` is the default.
- `--apply` is explicit and is not implemented today.
- Every command is rendered before execution.
- Secrets are not passed through shell history or argv.
- Cloud CLIs are placement helpers, not permission to open runtime ports.

Existing cloud target design remains accurate:

- Tencent Lighthouse: native plus systemd, env-file secrets, CNB-oriented plan.
- Azure VM: Docker image plus Key Vault, managed identity at boot.
- DigitalOcean Droplet: native plus systemd, env-file secrets, `doctl` plan.

All cloud plans should bind Codewhale to `127.0.0.1` and then layer one of the
reachability paths above.

## Namespace migration: `DEEPSEEK_*` to `CODEWHALE_*`

Carry forward the convention already used in code: read `CODEWHALE_X` first,
fall back to `DEEPSEEK_X` where compatibility is needed.

Touch list from the original RFC remains valid:

1. Bridges: read `CODEWHALE_X ?? DEEPSEEK_X` for runtime URL/token, workspace,
   model, mode, shell/trust/approval flags, allowlists, and timeouts. Templates
   should emit `CODEWHALE_*`.
2. Deploy units: prefer `/etc/codewhale/*.env`; keep legacy path reads only for
   compatibility where needed.
3. `.env.example` files and `config.example.toml`: lead with `CODEWHALE_*`,
   document legacy aliases.
4. Drop DeepSeek-shaped defaults in bridge templates except where DeepSeek is
   explicitly the chosen provider. Provider choice belongs in `runtime.env`.

## Tests

Existing bundle tests should stay:

- Every cloud / bridge / provider triple renders.
- Runtime and bridge env files share the same `CODEWHALE_RUNTIME_TOKEN`.
- Env files lead with `CODEWHALE_*`.
- Generated runbooks are non-empty and list the provision plan.
- Provision plans are command data and are not executed in tests.

New tests for this revision:

- Every `AccessPath` row has setup commands, env template, validation commands,
  and trust-boundary copy.
- Tailscale is the recommended remote path in prompt ordering.
- Funnel/webhook mode requires an explicit advanced/public acknowledgement.
- `/mobile` docs use `app-server --mobile --host 127.0.0.1` for current binary
  behavior, or clearly mark any `--http` plus `/mobile` path as proposed.
- Weixin can be documented before it is in the `remote-setup` registry, but the
  wizard must mark it proposed until a `BridgeSpec` row and validation story
  exist.

## Suggested sequencing

1. Revise the RFC and runbook copy to be Tailscale-first.
2. Add an access-path registry above the existing cloud/bridge/provider tables.
3. Add localhost and Tailscale generate-only bundles.
4. Add Weixin as a `BridgeSpec` row or explicitly hide it behind "proposed" in
   the wizard until registry and validation support land.
5. Rework cloud bundles so placement is second and reachability is first.
6. Add Funnel/webhook only as an advanced path with explicit public-exposure
   acknowledgement.
7. Implement `--apply` last, after generate-only output is reviewed.

## Command verification ledger

Verified against Codewhale code/docs in this worktree:

- `codewhale app-server --http --host 127.0.0.1 --port 7878 --auth-token TOKEN`
- `codewhale app-server --mobile --host 127.0.0.1 --port 7878 --auth-token TOKEN`
- `codewhale doctor --json`
- `curl /health` and authenticated `curl /v1/runtime/info`
- `npm run validate:config` for Telegram and Feishu bridges
- `npm run check` for the Weixin bridge
- Existing `remote-setup` generate-only flags listed above

Marked proposed or external:

- `codewhale remote-setup --access ...` and access-path registry
- first-class Tailscale, localhost, Weixin, and Funnel choices in the wizard
- `--apply` execution
- Tailscale CLI commands (`tailscale serve ...`, `tailscale funnel ...`) are
  external Tailscale commands. They are the intended RFC examples, but they are
  not Codewhale CLI flags.
