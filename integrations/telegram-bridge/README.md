# Telegram Bridge

This bridge lets a Telegram chat control a local `codewhale serve --http`
runtime from a phone. It uses Telegram Bot API long polling, so the first
version does not need a public webhook URL or inbound port.

Security model:

- `codewhale serve --http` stays bound to `127.0.0.1`.
- `/v1/*` runtime calls use `CODEWHALE_RUNTIME_TOKEN`. Legacy
  `DEEPSEEK_RUNTIME_TOKEN` is accepted only as a compatibility fallback.
- Telegram chats must be allowlisted unless `TELEGRAM_ALLOW_UNLISTED=true` is
  set for first pairing.
- Direct messages are the intended MVP control surface. Group chat control is
  disabled unless `TELEGRAM_ALLOW_GROUPS=true`.
- Tool approvals are text commands: `/allow <approval_id>` or `/deny <approval_id>`.
- The bridge also sends inline button controls for common actions. Text
  commands remain the fallback.

## Setup

Create a bot with Telegram's `@BotFather`, then configure the bridge:

```bash
cd /opt/codewhale/telegram-bridge
npm install --omit=dev
cp .env.example /etc/codewhale/telegram-bridge.env
sudoedit /etc/codewhale/telegram-bridge.env
node src/index.mjs
```

Validate env files before starting the service:

```bash
npm run validate:config -- \
  --env /etc/codewhale/telegram-bridge.env \
  --runtime-env /etc/codewhale/runtime.env \
  --workspace-root /opt/whalebro \
  --check-filesystem
```

For first pairing, temporarily set `TELEGRAM_ALLOW_UNLISTED=true`, send the bot
`/status`, copy the returned `chat_id` or `user_id` into
`TELEGRAM_CHAT_ALLOWLIST`, then turn `TELEGRAM_ALLOW_UNLISTED=false`.

## Commands

- `/menu`
- `/status`
- `/threads`
- `/new`
- `/resume <thread_id>`
- `/model <name|default>`
- `/interrupt`
- `/compact`
- `/allow <approval_id> [remember]`
- `/deny <approval_id>`

Anything else is sent as a prompt. If group control is explicitly enabled,
messages must start with `/cw` by default, for example:

```text
/cw check git status and tell me what is dirty
```

The `/menu`, `/status`, `/threads`, active-turn, and approval messages include
tap targets for common actions. Approval buttons map to the same runtime API as
`/allow` and `/deny`; they do not enable blanket auto-approval unless you tap
the explicit "Allow + remember" button.
