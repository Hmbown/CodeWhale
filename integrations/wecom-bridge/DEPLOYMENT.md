# WeCom Bridge — Deployment Guide

## Overview

The WeCom Bridge integrates CodeWhale with WeCom (企业微信) Smart Bot
WebSocket long-connection mode, enabling remote terminal agent interaction
without a public IP.

## Prerequisites

1. **WeCom admin access** to create a Smart Bot (智能机器人)
2. **CodeWhale app-server** running at `http://127.0.0.1:7878`
3. **Node.js 18+** for the bridge runtime

### Create a WeCom Smart Bot

1. Open the [WeCom Admin Console](https://work.weixin.qq.com/wework_admin/frame#apps)
2. Navigate: 应用管理 → 智能机器人 → 创建机器人
3. Choose **API mode** (not Webhook mode)
4. Copy the **BotID** and **Secret** — you will need these

## Quick Start

```powershell
# 1. Start the CodeWhale runtime
.\integrations\wecom-bridge\start-runtime.ps1

# 2. Start the WeCom Bridge (separate terminal)
.\integrations\wecom-bridge\start-bridge.ps1
```

Or with batch files:
```cmd
start-bridge-only.bat
```

## Configuration

Copy the environment template and edit:

```bash
cp .env.example .env
# Edit .env with your credentials
```

### Required variables

| Variable | Example | Description |
|----------|---------|-------------|
| `WECOM_BOT_ID` | `wb-xxxxxxxxxxxxxxxx` | Smart Bot BotID from WeCom Admin |
| `WECOM_BOT_SECRET` | `your-secret` | Smart Bot Secret from WeCom Admin |
| `CODEWHALE_RUNTIME_TOKEN` | `rand-xxxxxxxx` | Bearer token for Runtime API (generate a random string) |

### Optional variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CODEWHALE_RUNTIME_URL` | `http://127.0.0.1:7878` | Runtime API address |
| `CODEWHALE_WORKSPACE` | `(cwd)` | Workspace directory |
| `CODEWHALE_MODEL` | `auto` | Default model name |
| `WECOM_CHAT_ALLOWLIST` | `""` | Comma-separated allowed UserIDs |
| `WECOM_ALLOW_UNLISTED` | `false` | Enable first-pairing mode |
| `WECOM_MAX_REPLY_CHARS` | `3500` | Max characters per reply message |
| `CODEWHALE_TURN_TIMEOUT_MS` | `900000` | Turn timeout in ms (15 min) |
| `CODEWHALE_APPROVAL_TIMEOUT_MS` | `300000` | Approval timeout in ms (5 min) |

## First Pairing

1. Set `WECOM_ALLOW_UNLISTED=true` and start the bridge.
2. Send `/status` to the bot in WeCom.
3. The bridge will respond with your `user_id` (or `chat_id`).
4. Add the `user_id` to `WECOM_CHAT_ALLOWLIST`.
5. Set `WECOM_ALLOW_UNLISTED=false` and restart.

## Verify Installation

```bash
# Check syntax
node --check src/index.mjs

# Run bridge tests
node --test test/*.test.mjs
```

Expected output: `ℹ tests 16  ℹ pass 16  ℹ fail 0`

## Architecture

```
WeCom Client → Smart Bot WebSocket → WeCom Bridge ──HTTP──→ codewhale serve --http
                ◀── aibot_respond_msg ◀──                   (127.0.0.1:7878)
```

The bridge:
1. Authenticates via BotID + Secret to obtain an `access_token`
2. Establishes a WebSocket long connection to the WeCom Smart Bot API
3. Receives `aibot_msg_callback` events, processes them through the Runtime API
4. Replies via `aibot_respond_msg` commands

## Security Boundaries

- **No public port exposure**: `codewhale serve --http` binds to `127.0.0.1` only
- **Token authentication**: all `/v1/*` runtime calls require `CODEWHALE_RUNTIME_TOKEN`
- **Chat allowlist**: only chats/users in `WECOM_CHAT_ALLOWLIST` are served
- **Approval gate**: tool calls from WeCom require explicit approval (`/allow` or natural-language keywords)
- **WeCom only sees**: prompts, status summaries, thread listings, and approval requests — workspace contents, shell output, and runtime internals stay on your local machine

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| "not paired" warning | `WECOM_CHAT_ALLOWLIST` is empty | Add your user_id or enable `WECOM_ALLOW_UNLISTED=true` |
| 404 on `/allow` | Approval ID expired (5 min) | Respond faster, or increase `CODEWHALE_APPROVAL_TIMEOUT_MS` |
| Bridge exits immediately | Missing env vars | Run `node src/index.mjs` directly to see validation errors |
| Messages not received | Secret or BotID wrong | Verify credentials in WeCom Admin Console |
| WebSocket disconnect | Network flakiness | Bridge auto-reconnects; check `bridge.log` for details |

## Production Deployment

### As a Windows Service (recommended)

Use the provided VBS scripts to run the bridge as a background process:

```cmd
start-mydpsk.bat   # Start the AI backend
start-mydpsk.vbs   # Start as hidden window
```

### Logging

The bridge logs to `bridge.log` and `runtime.log` in the bridge directory.
Stderr goes to `bridge_err.log` and `runtime_err.log`.

### Auto-restart

The `restart-bridge.ps1` and `restart.ps1` scripts can be scheduled via Task Scheduler
for automatic recovery after system reboots.

## Related Documentation

- [WeCom Bridge README](README.md)
- [CodeWhale Security Policy](../../SECURITY.md)
- [CodeWhale Contributing Guide](../../CONTRIBUTING.md)
