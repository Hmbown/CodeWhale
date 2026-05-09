# CI/CD Integration Guide — Running DeepSeek TUI on CNB Platform

This guide explains how to run DeepSeek TUI as a headless AI agent on the
[CNB (Cloud Native Build)](https://cnb.cool) platform. It covers pipeline
configuration, NPC bot setup, the local proxy pattern for the CNB AI gateway,
and production best practices learned from real-world deployments.

## Overview

DeepSeek TUI's `--yolo exec` mode makes it possible to run the agent
non-interactively inside CNB pipeline runners. In this mode the agent:

- Automatically approves all tool executions (no human in the loop)
- Trusts the workspace without confirmation prompts
- Accepts a prompt as a CLI argument and exits when done

On the CNB platform, this enables **NPC (AI Bot)** use cases:

- **Code review bots** that comment on merge requests
- **Issue triage agents** that label and respond to new issues
- **Refactoring agents** that create sub-issues or merge requests
- **Documentation generators** triggered by `@npc` mentions

## Prerequisites

| Requirement | Minimum | Notes |
|-------------|---------|-------|
| DeepSeek TUI | v0.8.20+ | `npm install -g deepseek-tui` or binary release |
| Node.js | 18+ | Only if using the npm installer |
| glibc | 2.39+ | Debian 13 (Trixie) or Ubuntu 24.04+; Alpine is unsupported |
| Git | 2.25+ | Required for workspace rollback features |
| cnb CLI | latest | `npm install -g @cnbcool/cnb-cli` for Issue/PR commenting |

> **Tip:** On Debian 12 (Bookworm) the prebuilt binary will fail to load due to
> glibc 2.36. Use `node:22-trixie-slim` or a newer base image.

## Architecture

```
CNB Pipeline Runner
┌──────────────────────────────────────────────────────────┐
│                                                          │
│  ┌──────────┐     ┌─────────────┐     ┌──────────────┐  │
│  │ Launcher │────▶│ Local Proxy │────▶│ CNB AI       │  │
│  │ (Node.js)│     │ :18903      │     │ Gateway      │  │
│  └──────────┘     └─────────────┘     └──────────────┘  │
│       │                                                  │
│       ▼                                                  │
│  ┌──────────────────┐                                    │
│  │  deepseek-tui    │                                    │
│  │  --yolo exec     │──── tools ───▶ cnb CLI (comment)   │
│  └──────────────────┘                                    │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**Data flow:**

1. CNB platform triggers the pipeline when a user `@npc` in an Issue or MR
2. The launcher reads `CNB_*` environment variables (comment body, repo info, etc.)
3. It starts a local proxy to adapt the CNB AI gateway protocol
4. It launches `deepseek-tui --yolo exec` with a constructed prompt
5. DeepSeek TUI executes tools (including `cnb` CLI) to respond to the user

## Dockerfile

```dockerfile
FROM node:22-trixie-slim AS builder

WORKDIR /srv
COPY package.json ./
RUN npm install

COPY tsconfig.json ./
COPY src ./src
RUN npm run build

# ---

# Runtime stage: Debian 13+ for glibc >= 2.39
FROM node:22-trixie-slim

# System dependencies
# - git / ca-certificates / curl: basics
# - libdbus-1-3: runtime library required by DeepSeek TUI binary
RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates curl libdbus-1-3 \
    && rm -rf /var/lib/apt/lists/*

# Install DeepSeek TUI (Rust binary distributed via npm)
RUN npm install -g deepseek-tui \
    && deepseek --version \
    && deepseek-tui --version

# Install CNB CLI for Issue/PR interactions
RUN npm install -g @cnbcool/cnb-cli \
    && cnb --version

ENV PATH="/usr/local/bin:${PATH}"
WORKDIR /workspace

COPY --from=builder /srv/dist /srv/dist
COPY --from=builder /srv/node_modules /srv/node_modules

# Pre-install skills (optional)
RUN npx -y skills add https://cnb.cool/cnb/skills/cnb-skill.git --agent -y -g

# Default runtime config
ENV AI_MODEL=deepseek-v4-flash \
    BASE_URL_PATH=/ai-ide/v2 \
    DEEPSEEK_SKIP_UPDATE_CHECK=1

ENTRYPOINT ["node", "/srv/dist/index.js"]
```

## CNB Pipeline Configuration (`.cnb.yml`)

### Basic NPC Bot Pipeline

```yaml
main:
  push:
    - docker:
        image: your-registry/deepseek-npc:latest
  npc:
    - docker:
        image: your-registry/deepseek-npc:latest
```

### NPC Settings in Repository

Configure your NPC bot in the CNB repository settings. The platform injects
these as environment variables when the pipeline runs:

| CNB Environment Variable | Purpose |
|--------------------------|---------|
| `CNB_COMMENT_BODY` | The comment content that triggered the NPC |
| `CNB_ISSUE_DESCRIPTION` | Issue body (for issue creation events) |
| `CNB_REPO_SLUG` | Repository identifier (e.g., `owner/repo`) |
| `CNB_EVENT` | Event type (`issue.comment`, `pull_request.comment`, etc.) |
| `CNB_BRANCH` | Current branch name |
| `CNB_WEB_ENDPOINT` | CNB web base URL |
| `CNB_ISSUE_IID` | Issue number (for issue events) |
| `CNB_PULL_REQUEST_IID` | MR number (for merge request events) |
| `CNB_BUILD_USER` | Username who triggered the event |
| `CNB_BUILD_USER_NICKNAME` | Nickname of the trigger user |
| `CNB_NPC_SLUG` | The NPC's own slug |
| `CNB_NPC_NAME` | The NPC's display name |
| `CNB_NPC_PROMPT` | Character prompt (role settings) |
| `CNB_NPC_ENABLE_WORKMODE` | `true` = work mode (can create Issues/PRs) |
| `CNB_TOKEN` | Authentication token for CNB API/CLI |
| `CNB_API_ENDPOINT` | CNB API base (default: `https://api.cnb.cool`) |

## Runtime Configuration

### Config File Generation

Generate `~/.deepseek/config.toml` at container startup:

```toml
# Auto-generated for CI — do not edit manually
model = "deepseek-v4-flash"
api_base = "http://127.0.0.1:18903"   # points to local proxy

thinking_mode = "normal"
temperature = 0.1
top_p = 0.95
max_tokens = 8192

auto_approve = true
enable_git = true
enable_mcp = true
enable_web_search = false
enable_auto_compress = true

disable_auto_update = true
telemetry_enabled = false
```

### Environment Variables for DeepSeek TUI

Set these in the child process environment when spawning `deepseek-tui`:

```bash
DEEPSEEK_BASE_URL="http://127.0.0.1:18903"   # local proxy
DEEPSEEK_API_KEY="$CNB_TOKEN"                 # or AI_API_TOKEN
DEEPSEEK_MODEL="deepseek-v4-flash"
DEEPSEEK_SKIP_UPDATE_CHECK=1
DEEPSEEK_CLI_TRUST_WORKSPACE=true
CI=true
TERM=dumb
```

## The Local Proxy — CNB AI Gateway Adaptation

DeepSeek TUI speaks the standard OpenAI `/v1/chat/completions` protocol, but
the CNB AI gateway has protocol differences that require a local proxy.

### Why a Proxy Is Needed

| Issue | DeepSeek TUI expects | CNB AI Gateway does |
|-------|---------------------|---------------------|
| URL prefix | Appends `/v1` to base URL | No `/v1` prefix |
| Streaming | Sends `stream: false` by default | Only supports streaming (SSE) |
| SSE fields | Standard OpenAI format | Non-standard fields that break parsing |

### Proxy Responsibilities

The proxy listens on `127.0.0.1:18903` and forwards to the CNB AI gateway:

#### 1. URL Rewriting

Strip the `/v1` prefix that DeepSeek TUI hardcodes:

```
Client request: POST /v1/chat/completions
Proxy forwards: POST /chat/completions  →  CNB gateway
```

#### 2. Stream Conversion (Non-stream → SSE Aggregation)

When DeepSeek TUI sends `"stream": false`:

1. Proxy rewrites to `"stream": true` in the request body
2. Proxy sets `Accept: text/event-stream` header
3. Collects all `data:` SSE events from the gateway
4. Aggregates deltas into a complete `chat.completion` JSON response
5. Returns the JSON to DeepSeek TUI with `content-type: application/json`

#### 3. SSE Field Normalization

For streaming pass-through, clean non-standard fields per event:

| Field | Issue | Fix |
|-------|-------|-----|
| `finish_reason: ""` | DeepSeek TUI treats as end-of-message | Convert to `null` |
| `delta.refusal` | Unknown field causes parsing issues | Remove if empty/null |
| `delta.extra_fields` | Non-standard extension | Remove if null |
| `delta.function_call` | Deprecated field | Remove if null |
| `delta.content: ""` with `tool_calls` | Corrupts tool call parsing | Set `content` to `null` |

#### 4. tool_calls Slot Allocation

The CNB gateway sometimes sends concurrent tool_calls with the same `index` but
different `id` values. Standard OpenAI merge-by-index would incorrectly combine
them. Use this strategy:

- **Has `id`:** Find existing slot with same `id` → append; else create new slot
- **No `id`, has `index`:** Find last slot with matching `index` → append
- **Neither:** Append to the last slot

### Upstream URL Construction

The CNB AI gateway URL is constructed from environment variables:

```
{CNB_API_ENDPOINT}/{CNB_REPO_SLUG}/-/ai-ide/v2
```

Example: `https://api.cnb.cool/myorg/myrepo/-/ai-ide/v2`

## Process Management

### Timeout Strategy

In CI environments, always implement timeout management:

- **Total timeout:** `maxTurns × 60 seconds` (default: 20 turns = 20 minutes)
- **Silence detection:** Kill if no stdout/stderr output for 120 seconds
- **Heartbeat logging:** Log activity status every 15 seconds for debugging

### Signal Handling

Send `SIGTERM` first, then `SIGKILL` after a 5-second grace period:

```typescript
child.kill('SIGTERM');
setTimeout(() => {
  if (!child.killed) child.kill('SIGKILL');
}, 5000);
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| non-zero | Failure (check stderr for details) |

> **Tip:** If the process was killed by timeout but produced meaningful output
> (stdout is non-empty), you may still want to treat the run as successful —
> the agent likely already posted its response via `cnb` CLI.

## Bootstrap Steps

Before launching DeepSeek TUI, perform these initialization steps:

### 1. Initialize Git Repository

DeepSeek TUI expects a git repository for its workspace rollback feature:

```bash
cd /workspace
git init -q
git config user.email "npc@bot"
git config user.name "npc-bot"
git add -A
git commit -q -m "initial" --allow-empty
```

### 2. Sync Skills

If skills were installed at build time as root, copy them to the runtime user:

```bash
cp -r /root/.agents/skills ~/.agents/skills
```

### 3. Write Config

Generate `~/.deepseek/config.toml` with the correct `api_base` pointing to the
local proxy (see [Runtime Configuration](#runtime-configuration) above).

## Prompt Engineering for NPC Bots

When running as a CNB NPC bot, the agent's stdout is **invisible to users**.
The only way to communicate is via `cnb` CLI commands. Structure your prompt
with hard constraints:

```
You are an AI assistant for repository `{repo_slug}`, serving Issue #{iid}.

<user_input>
{comment_body}
</user_input>

【Hard Constraints — Highest Priority】

1. Your stdout is invisible to users.
2. The ONLY way to reply is via exec_shell:
   cnb issues comment --body "<your reply>"
3. Reply must start with @{username} to notify them.
4. Do not @ other NPCs in your reply.
5. Task is only complete after `cnb issues comment` exits with code 0.
6. Use `cnb issues view {iid}` to inspect issue details — do not guess.
```

## Calling Convention

> **Important:** Call `deepseek-tui` directly, not the `deepseek` dispatcher.
> The dispatcher does not accept `--yolo` at the top level.

```bash
deepseek-tui --yolo exec --model deepseek-v4-flash "$PROMPT"
```

| Flag | Effect |
|------|--------|
| `--yolo` | Agent mode + auto-approve all tools + trust workspace |
| `exec` | Non-interactive one-shot execution |
| `--model <name>` | Override the model |

## Best Practices

### Security

- **Never** run `--yolo` mode on completely untrusted input without sandboxing
- Delete sensitive tokens (`CNB_TOKEN_FOR_CODEBUDDY`, etc.) from child env
- Scope API keys to minimum required permissions
- Consider running the container as non-root in production

### Reliability

- Always set a total timeout (recommended: 10–20 minutes)
- Implement silence detection (kill after 2 minutes of no output)
- Log heartbeat status for post-mortem debugging
- Treat timeout-with-output as a soft success if applicable

### Performance

- Pre-install DeepSeek TUI in your Docker image (avoid download at runtime)
- Enable `enable_auto_compress = true` for long-running sessions
- Set `temperature = 0.1` for deterministic CI behavior
- Use `thinking_mode = "normal"` to balance quality and speed

### Observability

- Capture both stdout and stderr separately
- Fold verbose tool output blocks into summary lines for cleaner logs
- Log request/response metadata (message count, tool count, model used)
- Track execution time and exit codes for metrics

## Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| `GLIBC_2.39 not found` | Base image too old | Use Debian 13+ or Ubuntu 24.04+ |
| Binary hangs at startup | Missing `TERM=dumb` | Set `TERM=dumb` and `CI=true` |
| Empty responses from gateway | Non-stream request to stream-only gateway | Use local proxy to aggregate SSE |
| `tool_calls` corrupted | Gateway returns non-standard SSE | Normalize SSE fields via proxy |
| Process never exits | No timeout configured | Add silence detection + total timeout |
| `Permission denied` on tools | Workspace not trusted | Set `DEEPSEEK_CLI_TRUST_WORKSPACE=true` |
| `cnb` command not found | CLI not installed | `npm install -g @cnbcool/cnb-cli` |
| 404 from AI gateway | `/v1` prefix not stripped | Ensure proxy strips `/v1` before forwarding |

## Further Reading

- [Configuration Reference](../CONFIGURATION.md)
- [Runtime API](../RUNTIME_API.md) — for programmatic headless workflows
- [Docker Guide](../DOCKER.md)
- [Modes (Plan / Agent / YOLO)](../MODES.md)
- [Tool Surface](../TOOL_SURFACE.md)
