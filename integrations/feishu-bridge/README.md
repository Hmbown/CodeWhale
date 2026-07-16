# Feishu / Lark Bridge

This bridge lets a Feishu or Lark chat control a local `codewhale serve --http`
runtime from a phone. It uses the official Lark/Feishu Node SDK long-connection
mode, so the first version does not need a public webhook URL.

Security model:

- `codewhale serve --http` stays bound to `127.0.0.1`.
- `/v1/*` runtime calls use `CODEWHALE_RUNTIME_TOKEN`.
- Feishu/Lark chats must be allowlisted in `CODEWHALE_CHAT_ALLOWLIST` unless
  `CODEWHALE_ALLOW_UNLISTED=true`
  is set for first pairing.
- Direct messages are the intended MVP control surface. Group chat control is
  disabled unless `FEISHU_ALLOW_GROUPS=true`.
- Tool approvals are text commands: `/allow <approval_id>` or `/deny <approval_id>`.
- Feishu/Lark only sees the prompts, status, thread summaries, and approval
  messages the bridge sends. The workspace, shell, and runtime HTTP listener
  stay local behind the Codewhale runtime token.

## Setup

```bash
cd /opt/codewhale/feishu-bridge
npm install --omit=dev
cp .env.example /etc/codewhale/feishu-bridge.env
sudoedit /etc/codewhale/feishu-bridge.env
node src/index.mjs
```

Validate the env files before starting the service:

```bash
npm run validate:config -- \
  --env /etc/codewhale/feishu-bridge.env \
  --runtime-env /etc/codewhale/runtime.env \
  --workspace-root /opt/whalebro \
  --check-filesystem
```

For first pairing, temporarily set `CODEWHALE_ALLOW_UNLISTED=true`, send the
bot `/status`, copy the returned `chat_id`, `open_id`, or `union_id` into
`CODEWHALE_CHAT_ALLOWLIST`, then turn `CODEWHALE_ALLOW_UNLISTED=false`.

For a Tencent Lighthouse deployment, use:

```bash
sudo systemctl enable --now codewhale-runtime codewhale-feishu-bridge
sudo journalctl -u codewhale-feishu-bridge -f
```

## Commands

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
messages should start with the Codewhale prefix `/cw`, for example:

```text
/cw check git status and tell me what is dirty
```
