# CodeWhale

> DeepSeek-first agentic terminal for open source and open-weight coding models. It runs from the `codewhale` command, streams reasoning blocks, edits local workspaces with approval gates, and can auto-route each turn to the right DeepSeek model and thinking level.

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![Sponsor](https://img.shields.io/badge/Sponsor-GitHub%20Sponsors-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Hmbown)
[DeepWiki project index](https://deepwiki.com/Hmbown/CodeWhale)

[简体中文 README](README.zh-CN.md)
[日本語 README](README.ja-JP.md)

[Install](#install) · [Quickstart](#quickstart) · [Usage](#usage) · [Documentation](#documentation) · [Contributing](#contributing) · [Support](#support)

## Install

`codewhale` is distributed as Rust binaries: the dispatcher command
(`codewhale`) and the companion TUI runtime (`codewhale-tui`). Pick whichever
install path you already use; they all put the same commands on your `PATH`.
The npm package is an installer/wrapper for the release binaries, not the
agent runtime itself.

```bash
# 1. npm — easiest if you already use Node. The package downloads the
#    matching prebuilt Rust binaries from GitHub Releases.
npm install -g codewhale

# 2. Cargo — no Node needed. Requires Rust 1.88+ (the crates use the
#    2024 edition; older toolchains fail with "feature `edition2024` is
#    required"). Run `rustup update` first, or use a non-Cargo path below.
cargo install codewhale-cli --locked   # `codewhale` (entry point)
cargo install codewhale-tui     --locked   # `codewhale-tui` (TUI binary)

# 3. Homebrew — macOS package manager.
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. Direct download — no package manager or toolchain.
#    https://github.com/Hmbown/CodeWhale/releases
#    Prebuilt for Linux x64/ARM64, macOS x64/ARM64, Windows x64.

# 5. Docker — prebuilt release image.
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

> In mainland China, speed up the npm path with
> `--registry=https://registry.npmmirror.com`, or use the
> [Cargo mirror](#china--mirror-friendly-installation) below.
>
> Download safety: official release binaries live under
> `https://github.com/Hmbown/CodeWhale/releases`. For manual downloads,
> verify the SHA-256 manifest and avoid look-alike repositories or search-result
> mirrors. See [download safety and checksums](docs/INSTALL.md#2-download-safety-and-checksums).

Already installed? Use the updater that matches the install path:

```bash
codewhale update                         # release-binary updater
npm install -g codewhale@latest      # npm wrapper
brew update && brew upgrade deepseek-tui
cargo install codewhale-cli --locked --force
cargo install codewhale-tui     --locked --force
```

![codewhale screenshot](assets/screenshot.png)

---

## What Is It?

CodeWhale is a DeepSeek-first coding agent for open source and open-weight models that runs in your terminal. It can read and edit files, run shell commands, search the web, manage git, and coordinate sub-agents from a keyboard-driven TUI.

It is built around DeepSeek V4 (`deepseek-v4-pro` / `deepseek-v4-flash`), including 1M-token context windows, streaming reasoning blocks, and prefix-cache-aware cost reporting.

### Key Features

- **Model auto-routing** — `--model auto` / `/model auto` chooses both the model and thinking level for each turn
- **Thinking-mode streaming** — see DeepSeek reasoning blocks as the model works
- **Full tool suite** — file ops, shell execution, git, web search/browse, apply-patch, sub-agents, MCP servers
- **1M-token context** — context tracking, manual or configured compaction, and prefix-cache telemetry
- **Prefix-cache stability tracking** — an optional `/statusline` footer chip surfaces how stable the cached prefix has been across recent turns so cost-busting edits are visible before they land
- **Three modes** — Plan (read-only explore), Agent (interactive with approval), YOLO (auto-approved)
- **Reasoning-effort tiers** — cycle through `off → high → max` with `Shift + Tab`
- **Session save/resume/fork** — checkpoint long-running sessions and fork saved conversations into sibling paths with parent lineage shown in the picker
- **Workspace rollback** — side-git pre/post-turn snapshots with `/restore` and `revert_turn`, without touching your repo's `.git`
- **OS-level sandbox** — Seatbelt on macOS, Landlock on Linux, Job Objects on Windows; shell commands run with workspace-scoped filesystem access only
- **Durable task queue** — background tasks can survive restarts
- **HTTP/SSE runtime API** — `codewhale serve --http` for headless agent workflows
- **MCP protocol** — connect to Model Context Protocol servers for extended tooling; please see [docs/MCP.md](docs/MCP.md)
- **Fin-powered seams** — cheap `deepseek-v4-flash` with thinking off handles routing, RLM child calls, summaries, and other fast coordination work
- **Native RLM** (`rlm_session_objects`/`rlm_open`/`rlm_eval`) — persistent REPL sessions for batched analysis with bounded helpers like `peek`, `search`, `chunk`, and `sub_query_batch`; active prompt/history objects are opened by symbolic refs instead of pasted into the parent transcript
- **LSP diagnostics** — inline error/warning surfacing after every edit via rust-analyzer, pyright, typescript-language-server, gopls, clangd
- **User memory** — optional persistent note file injected into the system prompt for cross-session preferences
- **Localized UI** — `en`, `ja`, `zh-Hans`, `pt-BR` with auto-detection
- **Live cost tracking** — per-turn and session-level token usage and cost estimates; cache hit/miss breakdown; CNY display when the session locale is `zh-Hans`
- **Skills system** — composable, installable instruction packs from GitHub; ships with a bundled starter set (`skill-creator`, `mcp-builder`, `plugin-creator`, `v4-best-practices`, `documents`, `presentations`, `spreadsheets`, `pdf`, `feishu`, `skill-installer`, `delegate`) so `/skills` is useful from first launch
- **Terminal-native notifications** — OSC 9 (iTerm2/WezTerm/Ghostty), OSC 99 (Kitty), OSC 777 (Ghostty), plus desktop notification fallback
- **Built-in theme picker** — Catppuccin, Tokyo Night, Dracula, Gruvbox alongside the original light/dark palettes; switch live with `/theme`

---

## How It's Wired

`codewhale` (dispatcher CLI) → `codewhale-tui` (companion binary) → ratatui interface ↔ async engine ↔ OpenAI-compatible streaming client. Tool calls route through a typed registry (shell, file ops, git, web, sub-agents, MCP, RLM) and results stream back into the transcript. The engine manages session state, turn tracking, the durable task queue, and an LSP subsystem that feeds post-edit diagnostics into the model's context before the next reasoning step.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full walkthrough.

### Sub-agents: Concurrent Background Execution

CodeWhale can dispatch multiple sub-agents that run in parallel — like a concurrent task queue:

- **Non-blocking launch.** `agent_open` returns immediately. The child gets its own fresh context and tool registry and runs independently. The parent keeps working.
- **Background execution.** Sub-agents execute concurrently (default cap: 10, configurable to 20). The engine manages the pool — no polling loop needed.
- **Completion notification.** When a sub-agent finishes, the runtime delivers a structured `<codewhale:subagent.done>` event with a summary, evidence list, and execution metrics. The parent model reads the `summary` field and integrates findings.
- **Bounded result retrieval.** Large transcripts are parked behind `var_handle` references. The model calls `handle_read` for slices, ranges, or JSONPath projections — keeping the parent context lean.

See [docs/SUBAGENTS.md](docs/SUBAGENTS.md) for the full sub-agent reference.

---

## Quickstart

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

Prebuilt binaries are published for **Linux x64**, **Linux ARM64** (v0.8.8+), **macOS x64**, **macOS ARM64**, and **Windows x64**. For other targets (musl, riscv64, FreeBSD, etc.), see [Install from source](#install-from-source) or [docs/INSTALL.md](docs/INSTALL.md).

On first launch you'll be prompted for your [DeepSeek API key](https://platform.deepseek.com/api_keys). The key is saved to `~/.deepseek/config.toml` so it works from any directory without OS credential prompts.

You can also set it ahead of time:

```bash
codewhale auth set --provider deepseek   # saves to ~/.deepseek/config.toml
codewhale auth status                    # shows the active credential source

export DEEPSEEK_API_KEY="YOUR_KEY"      # env var alternative; use ~/.zshenv for non-interactive shells
codewhale

codewhale doctor                         # verify setup
```

If `codewhale doctor` says the rejected key came from `DEEPSEEK_API_KEY`, remove
the stale export from your shell startup file, open a fresh shell, or run
`codewhale auth set --provider deepseek`. Use `codewhale auth status` to see the
config, keyring, and env-var source state without printing the key. Saved config
keys take precedence over the keyring and environment and are easier to rotate.

> To rotate or remove a saved key: `codewhale auth clear --provider deepseek`.

### Tencent Cloud / CNB Remote-First Path

For an always-on workspace you can control from a phone, use the Tencent-native
path: CNB mirror/source, Tencent Lighthouse HK, a Feishu/Lark long-connection
bridge, and optional EdgeOne for a deliberate public HTTPS edge. The runtime API
stays bound to localhost; EdgeOne is not used to expose `/v1/*`.

Start with [docs/TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md),
then use [docs/TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) for the
server runbook.

### Model Auto-Routing and Fin

Use `codewhale --model auto` or `/model auto` when you want codewhale to decide how much model and reasoning power a turn needs.

Model auto-routing controls two settings together:

- Model: `deepseek-v4-flash` or `deepseek-v4-pro`
- Thinking: `off`, `high`, or `max`

Before the real turn is sent, the app makes a small `deepseek-v4-flash` routing call with thinking off. That fast path is called **Fin**: a low-latency seam for model selection, summaries, RLM children, context maintenance, and other coordination work that should not spend a full reasoning turn. Fin looks at the latest request and recent context, then selects a concrete model and thinking level for the real request. Short/simple turns can stay on Flash with thinking off; coding, debugging, release work, architecture, security review, or ambiguous multi-step tasks can move up to Pro and/or higher thinking.

`--model auto` and `/model auto` are local to codewhale. The upstream API never receives `model: "auto"`; it receives the concrete model and thinking setting chosen for that turn. The TUI shows the selected route, and cost tracking is charged against the model that actually ran. If the Fin route fails or returns an invalid answer, the app falls back to a local heuristic. Sub-agents inherit model auto-routing unless you assign them an explicit model.

Use a fixed model or fixed thinking level when you want repeatable benchmarking, a strict cost ceiling, or a specific provider/model mapping.

### Linux ARM64 (Raspberry Pi, Asahi, Graviton, HarmonyOS PC)

`npm i -g codewhale` works on glibc-based ARM64 Linux from v0.8.8 onward. You can also download prebuilt binaries from the [Releases page](https://github.com/Hmbown/CodeWhale/releases) and place them side by side on your `PATH`.

### China / Mirror-friendly Installation

If GitHub or npm downloads are slow from mainland China, use a Cargo registry mirror:

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

Then install both binaries (the dispatcher delegates to the TUI at runtime):

```bash
cargo install codewhale-cli --locked   # provides `codewhale`
cargo install codewhale-tui     --locked   # provides `codewhale-tui`
codewhale --version
```

Prebuilt binaries can also be downloaded from [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases). Use `DEEPSEEK_TUI_RELEASE_BASE_URL` for mirrored release assets.

### Windows (Scoop)

[Scoop](https://scoop.sh) is a Windows package manager. The `codewhale` package is listed
in Scoop's main bucket, but that manifest updates independently and can lag the
GitHub/npm/Cargo release. Run `scoop update` first, then verify the installed
version with `codewhale --version`:

```bash
scoop update
scoop install deepseek-tui
codewhale --version
```

Use npm or direct GitHub release downloads when you need the newest release
before Scoop's manifest catches up.


<details id="install-from-source">
<summary>Install from source</summary>

Works on any Tier-1 Rust target — including musl, riscv64, FreeBSD, and older ARM64 distros.

```bash
# Linux build deps (Debian/Ubuntu/RHEL):
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # requires Rust 1.88+; provides `codewhale`
cargo install --path crates/tui --locked   # provides `codewhale-tui`
```

Both binaries are required. Cross-compilation and platform-specific notes: [docs/INSTALL.md](docs/INSTALL.md).

</details>

### Other API Providers

Official DeepSeek remains the default and first-class path. Other providers are
additive, with OpenRouter starting from DeepSeek Pro/Flash before broader
open-model catalogs are enabled.

```bash
# NVIDIA NIM
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim

# AtlasCloud
codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
codewhale --provider atlascloud

# Wanjie Ark
codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"
codewhale --provider wanjie-ark --model deepseek-reasoner

# OpenRouter
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro

# Novita
codewhale auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
codewhale --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
codewhale --provider fireworks --model deepseek-v4-pro

# Generic OpenAI-compatible endpoint
codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" codewhale --provider openai --model glm-5

# Self-hosted SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" codewhale --provider sglang --model deepseek-v4-flash

# Self-hosted vLLM
VLLM_BASE_URL="http://localhost:8000/v1" codewhale --provider vllm --model deepseek-v4-flash

# Self-hosted Ollama
ollama pull codewhale-coder:1.3b
codewhale --provider ollama --model codewhale-coder:1.3b
```

Inside the TUI, `/provider` opens the provider picker and `/model` opens the
local model/thinking picker. `/provider openrouter` and `/model <id>` switch
directly, while `/models` explicitly fetches and lists live API models when the
active provider supports model listing.

---

## Release Notes

Release-specific changes live in [CHANGELOG.md](CHANGELOG.md). This README
stays focused on current install paths, core workflows, provider setup, runtime
interfaces, and extension points.

---

## Usage

All examples use `codewhale`. The short form `codew` works everywhere —
it's a silent alias that forwards to `codewhale`.

```bash
codewhale                                         # interactive TUI
codewhale "explain this function"                 # one-shot prompt
codewhale exec --auto --output-format stream-json "fix this bug"  # agentic exec with tool auto-approvals
codewhale swebench run --instance-id <ID> --issue-file issue.md  # write all_preds.jsonl for SWE-bench
codewhale exec --resume <SESSION_ID> "follow up"  # continue a non-interactive session
codewhale --model deepseek-v4-flash "summarize"   # model override
codewhale --model auto "fix this bug"             # auto-route model + thinking
codewhale --yolo                                  # auto-approve tools
codewhale auth set --provider deepseek            # save API key
codewhale doctor                                  # check setup & connectivity
codewhale doctor --json                           # machine-readable diagnostics
codewhale setup --status                          # read-only setup status
codewhale setup --tools --plugins                 # scaffold tool/plugin dirs
codewhale models                                  # list live API models
codewhale sessions                                # list saved sessions
codewhale resume --last                           # resume the most recent session in this workspace
codewhale resume <SESSION_ID>                     # resume a specific session by UUID
codewhale fork <SESSION_ID>                       # fork a saved session into a sibling path
codewhale serve --http                            # HTTP/SSE API server
codewhale serve --acp                             # ACP stdio adapter for Zed/custom agents
codewhale run pr <N>                              # fetch PR and pre-seed review prompt
codewhale mcp list                                # list configured MCP servers
codewhale mcp validate                            # validate MCP config/connectivity
codewhale mcp-server                              # run dispatcher MCP stdio server
codewhale update                                  # check for and apply binary updates
```

### Branching Conversations

Saved sessions are intentionally branchable. `codewhale fork <SESSION_ID>` copies
an existing saved session into a new sibling session, records the parent session
id in metadata, and opens that fork so you can explore an alternate direction
without polluting the original path. The session picker and `codewhale sessions`
mark forked sessions with their parent id.

Inside the TUI, Esc-Esc backtrack can rewind the active transcript to a prior
user prompt and put that prompt back in the composer for editing. `/restore`
and `revert_turn` are separate workspace rollback tools: they restore files
from side-git snapshots but do not rewrite conversation history.

Docker images are published to GHCR for release builds:

```bash
docker volume create codewhale-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.deepseek \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

See [docs/DOCKER.md](docs/DOCKER.md) for pinned tags, local image builds,
volume ownership notes, and non-interactive pipeline usage.

### SWE-bench

CodeWhale can emit SWE-bench-compatible prediction JSONL from a checked-out
task workspace:

```bash
codewhale swebench run \
  --instance-id django__django-12345 \
  --issue-file issue.md \
  --predictions-path all_preds.jsonl
```

`run` uses the same tool-backed automation path as `codewhale exec --auto`,
then exports the final working-tree diff as `model_patch`. Use
`codewhale swebench export --instance-id <ID>` when you have already produced
the diff yourself. See [docs/SWEBENCH.md](docs/SWEBENCH.md) for the full flow.

### Zed / ACP

DeepSeek can run as a custom Agent Client Protocol server for editors that
spawn local ACP agents over stdio. In Zed, add a custom agent server:

```json
{
  "agent_servers": {
    "DeepSeek": {
      "type": "custom",
      "command": "codewhale",
      "args": ["serve", "--acp"],
      "env": {}
    }
  }
}
```

The first ACP slice supports new sessions and prompt responses through your
existing DeepSeek config/API key. Tool-backed editing and checkpoint replay are
not exposed through ACP yet.

Community-maintained adapter: [acp-codewhale-adapter](https://github.com/rockeverm3m/acp-codewhale-adapter)
bridges `codewhale exec --auto` to `cc-connect` for users who need tool-backed
ACP workflows outside the built-in Zed slice.

### Keyboard Shortcuts

| Key | Action |
|---|---|
| `Tab` | Complete `/` or `@` entries; while running, queue draft as follow-up; otherwise cycle mode |
| `Shift+Tab` | Cycle reasoning-effort: off → high → max |
| `F1` | Searchable help overlay |
| `Esc` | Back / dismiss |
| `Ctrl+K` | Command palette |
| `Ctrl+R` | Resume an earlier session |
| `Alt+R` | Search prompt history and recover cleared drafts |
| `Ctrl+S` | Stash current draft (`/stash list`, `/stash pop` to recover) |
| `@path` | Attach file/directory context in composer |
| `↑` (at composer start) | Select attachment row for removal |

Voice input is available from the command palette (`Ctrl+K`, then search
`Voice input`) after configuring `voice_input_command`; the helper
records/transcribes audio, CodeWhale shows a listening status while it runs, and
the final transcript is inserted into the composer for editing.

Full shortcut catalog: [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).

---

## Modes

| Mode | Behavior |
| --- | --- |
| **Plan** 🔍 | Read-only investigation — model explores and proposes a plan before making changes; multi-step investigations use `checklist_write` |
| **Agent** 🤖 | Default interactive mode — multi-step tool use with approval gates; substantial work is tracked with `checklist_write` |
| **YOLO** ⚡ | Auto-approve all tools in a trusted workspace; multi-step work still keeps a visible checklist |

Modes are separate from model auto-routing. `Tab` cycles Plan / Agent / YOLO,
while `/model auto` controls model and thinking selection. The `/goal` command
tracks a session objective and token budget today; a fuller Goal work surface is
the right future home for persistent objective progress rather than another
meaning of "auto".

---

## Configuration

User config: `~/.deepseek/config.toml`. Project overlay: `<workspace>/.deepseek/config.toml` (denied: `api_key`, `base_url`, `provider`, `mcp_config_path`). [config.example.toml](config.example.toml) has every option.

Key environment variables:

| Variable | Purpose |
|---|---|
| `DEEPSEEK_API_KEY` | API key |
| `DEEPSEEK_BASE_URL` | API base URL |
| `DEEPSEEK_HTTP_HEADERS` | Optional custom model request headers, e.g. `X-Model-Provider-Id=your-model-provider` |
| `DEEPSEEK_MODEL` | Default model |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | Stream idle timeout in seconds, default `300`, clamped to `1..=3600` |
| `DEEPSEEK_PROVIDER` | `codewhale` (default), `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `openrouter`, `novita`, `fireworks`, `sglang`, `vllm`, `ollama` |
| `DEEPSEEK_PROFILE` | Config profile name |
| `DEEPSEEK_MEMORY` | Set to `on` to enable user memory |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | Allow non-local `http://` API base URLs on trusted networks |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `OPENROUTER_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | Provider auth |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | Generic OpenAI-compatible endpoint and model ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud endpoint and model override |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark endpoint and model override |
| `OPENROUTER_BASE_URL` | OpenRouter endpoint override |
| `NOVITA_BASE_URL` | Novita endpoint override |
| `FIREWORKS_BASE_URL` | Fireworks endpoint override |
| `SGLANG_BASE_URL` | Self-hosted SGLang endpoint |
| `SGLANG_MODEL` | Self-hosted SGLang model ID |
| `VLLM_BASE_URL` | Self-hosted vLLM endpoint |
| `VLLM_MODEL` | Self-hosted vLLM model ID |
| `OLLAMA_BASE_URL` | Self-hosted Ollama endpoint |
| `OLLAMA_MODEL` | Self-hosted Ollama model tag |
| `NO_ANIMATIONS=1` | Force accessibility mode at startup |
| `SSL_CERT_FILE` | Custom CA bundle for corporate proxies |

Set `locale` in `settings.toml`, use `/config locale zh-Hans`, or rely on `LC_ALL`/`LANG` to choose UI chrome and the fallback language sent to V4 models. The latest user message still wins for natural-language reasoning and replies, so Chinese user turns stay Chinese even on an English system locale. See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) and [docs/MCP.md](docs/MCP.md).

---

## Models & Pricing

| Model | Context | Input (cache hit) | Input (cache miss) | Output |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M | $0.435 / 1M | $0.87 / 1M |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

DeepSeek Platform defaults to `https://api.deepseek.com/beta` so beta-gated API features can be tested without extra setup. Set `base_url = "https://api.deepseek.com"` to opt out.

Legacy aliases `deepseek-chat` / `deepseek-reasoner` map to `deepseek-v4-flash` and retire after July 24, 2026. NVIDIA NIM variants use your NVIDIA account terms.

> [!Note]
> DeepSeek's pricing page now lists the V4 Pro rates above as the permanent prices: the previous 75% promotional discount has been folded into a one-quarter base-rate adjustment as the promotion window closes on 15:59 UTC on 31 May 2026. The TUI cost estimator already uses these values, so no behavioural change is required. For any future price changes, consult the official [DeepSeek pricing page](https://api-docs.deepseek.com/zh-cn/quick_start/pricing).

---

## Publishing Your Own Skill

codewhale discovers skills from workspace directories (`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills` → `.cursor/skills`) and global directories (`~/.agents/skills` → `~/.claude/skills` → `~/.deepseek/skills`). Each skill is a directory with a `SKILL.md` file:

```text
~/.agents/skills/my-skill/
└── SKILL.md
```

Frontmatter required:

```markdown
---
name: my-skill
description: Use this when DeepSeek should follow my custom workflow.
---

# My Skill
Instructions for the agent go here.
```

Commands: `/skills` (list), `/skill <name>` (activate), `/skill new` (scaffold), `/skill install github:<owner>/<repo>` (community), `/skill update` / `uninstall` / `trust`. Community installs from GitHub require no backend service. Installed skills appear in the model-visible session context; the agent can auto-select relevant skills via the `load_skill` tool when your task matches their descriptions.

First launch also installs bundled system skills for common workflows:
`skill-creator`, `delegate`, `v4-best-practices`, `plugin-creator`,
`skill-installer`, `mcp-builder`, `documents`, `presentations`,
`spreadsheets`, `pdf`, and `feishu`. These live under
`~/.deepseek/skills` and are versioned so new bundles are added on upgrade
without recreating skills the user deliberately deleted.

---

## Documentation

| Doc | Topic |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Codebase internals |
| [CONFIGURATION.md](docs/CONFIGURATION.md) | Full config reference |
| [MODES.md](docs/MODES.md) | Plan / Agent / YOLO modes |
| [MCP.md](docs/MCP.md) | Model Context Protocol integration |
| [RUNTIME_API.md](docs/RUNTIME_API.md) | HTTP/SSE API server |
| [INSTALL.md](docs/INSTALL.md) | Platform-specific install guide |
| [DOCKER.md](docs/DOCKER.md) | GHCR image, volumes, and Docker usage |
| [CNB_MIRROR.md](docs/CNB_MIRROR.md) | CNB mirror and China-friendly install notes |
| [TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md) | Tencent/CNB/Lighthouse/Feishu remote-first path |
| [TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) | Lighthouse Hong Kong server setup |
| [MEMORY.md](docs/MEMORY.md) | User memory feature guide |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | Sub-agent role taxonomy and lifecycle |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | Full shortcut catalog |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | Release process |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | UI locale matrix & switching |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | Ops & recovery |
| [RECURSIVE_SELF_IMPROVEMENT.md](docs/RECURSIVE_SELF_IMPROVEMENT.md) | Copyable prompts for agent-assisted CodeWhale improvements |

Full Changelog: [CHANGELOG.md](CHANGELOG.md).

---

## Support

CodeWhale is MIT-licensed and usable without sponsorship. If it saves you time,
the clearest way to support ongoing maintenance is
[GitHub Sponsors](https://github.com/sponsors/Hmbown). One-time support is also
available through [Buy Me a Coffee](https://www.buymeacoffee.com/hmbown).

Sponsorship helps cover release builds, CI/runtime testing, package publishing,
and maintainer time for issue triage and review. Feature requests, bug reports,
and pull requests do not require sponsorship.

---

## Thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — thank you for the models and support that power every turn. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — thank you for your support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — thank you for prioritizing codewhale support and for collaborating on a better terminal-agent experience.
- **[Open Design](https://github.com/nexu-io/open-design)** — thank you for support and collaboration around design-forward agent workflows.

This project ships with help from a growing community of contributors:

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 28 PRs spanning features, fixes, and VS Code extension scaffolding (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Markdown rendering for tables, bold/italic, and horizontal rules (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows + China install documentation (#578)
- **[20bytes](https://github.com/20bytes)** — User memory docs and help polish (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc compatibility preflight (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc compatibility improvements and terminal restoration on SIGINT/SIGTERM (#565, #1586)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Shell `cwd` boundary validation (#524)
- **[toi500](https://github.com/toi500)** — Windows paste fix report
- **[xsstomy](https://github.com/xsstomy)** — Terminal startup repaint report
- **[melody0709](https://github.com/melody0709)** — Slash-prefix Enter activation report
- **[lloydzhou](https://github.com/lloydzhou)** and **[jeoor](https://github.com/jeoor)** — Compaction cost reports and npm installer stream-pause race fix (#1860); lloydzhou also contributed deterministic environment context (#813, #922) and KV prefix-cache stabilisation (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README clarity pass (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows install documentation (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — Pricing/discount info update (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — model ID case-sensitivity compatibility report (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — stale `working...` state bug report, Windows clipboard fallback, MCP Streamable HTTP session fixes, and Homebrew tap automation (#738, #850, #1643, #1631)
- **[reidliu41](https://github.com/reidliu41)** — resume hint, workspace trust persistence, Ollama provider support, thinking-block stream finalization, CI cache hardening, streaming wrap, DeepSeek model completions, and help picker selection polish (#863, #870, #921, #1078, #1603, #1628, #1601, #1964)
- **[cyq1017](https://github.com/cyq1017)** — Unicode `git_status` paths, local/configured skill discovery, and mode-switch toast dedupe (#1953, #1956, #1957)
- **[xieshutao](https://github.com/xieshutao)** — plain Markdown skill fallback (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper `--version` fallback (#885)
- **[y0sif](https://github.com/y0sif)** — parent turn-loop wakeup after direct child sub-agent completion (#901)
- **[mac119](https://github.com/mac119)** and **[leo119](https://github.com/leo119)** — `codewhale update` command documentation (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** / **浩淼的mac** — command-safety null-byte hardening (#706, #918)
- **macworkers** — fork confirmation with the new session id (#600, #919)
- **zero** and **[zerx-lab](https://github.com/zerx-lab)** — notification condition config and richer OSC 9 notification body (#820, #920)
- **[chnjames](https://github.com/chnjames)** — cached @mention completions, config recovery polish, and Windows UTF-8 shell output (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — config safety, async cleanup, Docker hardening, and command-safety fixes (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — UTF-8 decoding and deterministic test coverage (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — `/rename` command for custom session titles (#836)
- **[banqii](https://github.com/banqii)** — `.cursor/skills` discovery path support (#817)
- **[junskyeed](https://github.com/junskyeed)** — dynamic `max_tokens` calculation for API requests (#826)
- **Hafeez Pizofreude** — SSRF protection in `fetch_url` and Star History chart
- **Unic (YuniqueUnic)** — Schema-driven config UI (TUI + web)
- **Jason** — SSRF security hardening
- **[axobase001](https://github.com/axobase001)** — snapshot orphan cleanup, npm install guards, session telemetry fixes, model-scope cache clear, symlinked skill support, npm mirror-escape-hatch guidance, and proxy preservation for child tasks (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056, #1608)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` command foundation and SSE gzip/brotli decompression (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan-mode read-only sandbox safety fix (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — precise paste-Enter auto-submit reproducer (#1073)
- **[Duducoco](https://github.com/Duducoco)** and **[AlphaGogoo](https://github.com/AlphaGogoo)** — skills slash-menu and `/skills` coverage fix (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — window-resize artifact fix for macOS Terminal.app and ConHost (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter and custom-endpoint model-ID preservation (#1066)
- **[Jefsky](https://github.com/Jefsky)** — DeepSeek endpoint correction report (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM provider API-key preference diagnosis (#1081)
- **[Horace Liu](https://github.com/liuhq)** — Nix package support and install documentation (#1173)
- **[jieshu666](https://github.com/jieshu666)** — terminal repaint flicker reduction (#1563)
- **[gordonlu](https://github.com/gordonlu)** — Windows Enter / CSI-u input fix (#1612)
- **[mdrkrg](https://github.com/mdrkrg)** — first-run onboarding crash fix when the API key is missing (#1598)
- **[Aitensa](https://github.com/Aitensa)** — CJK wrapping propagation for diff and pager output (#1622)
- **[qiyan233](https://github.com/qiyan233)** — legacy DeepSeek CN provider alias compatibility (#1645)
- **[zlh124](https://github.com/zlh124)** — WSL2/headless startup report and clipboard-init fix (#1772, #1773)
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen logging, Home/End composer, and runtime log follow-ups (#1774, #1776, #1748, #1749, #1782, #1783)
- **[LeoLin990405](https://github.com/LeoLin990405)** — provider model passthrough, reasoning replay, thinking-only turn, and Windows quoting fixes (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Ctrl+C prompt restore fix (#1764)
- **[h3c-hexin](https://github.com/h3c-hexin)** — streaming batch tool-call preservation and CLI reasoning-effort passthrough (#1686, #1511)
- **[hxy91819](https://github.com/hxy91819)** — prefix-cache preservation during tool-result pruning (#1514)
- **[JiarenWang](https://github.com/JiarenWang)** — Plan-mode read-only enforcement, approval-takeover clamping, Ctrl+H delete fix, and undo context sync (#1123, #962, #958, #1150)
- **[Liu-Vince](https://github.com/Liu-Vince)** — MCP pagination, markdown indentation preservation, zh-Hans i18n polish, and env-var documentation (#1256, #1179, #1274, #1178)
- **[linzhiqin2003](https://github.com/linzhiqin2003)** — `--model auto` cost-saving bias, execution-discipline prompts, and declarative-fact memory hygiene (#1385, #1384, #1381)
- **[lbcheng888](https://github.com/lbcheng888)** — cost persistence across save/restore and transcript scroll fix (#1192, #1211)
- **[pengyou200902](https://github.com/pengyou200902)** — UTF-8-safe memory truncation, truncation-marker precision, and keybinding docs (#968, #1122, #1095)
- **[ChaceLyee2101](https://github.com/ChaceLyee2101)** — reasoning-token cost tracking with auto-CNY on zh-Hans and zh-CN README sync (#1505, #1504)
- **[CrepuscularIRIS](https://github.com/CrepuscularIRIS)** — low-motion mode for Termius/SSH and npx MCP server sandbox fix (#1479, #1346)
- **[laoye2020](https://github.com/laoye2020)** — Catppuccin, Tokyo Night, Dracula, and Gruvbox themes with `/theme` picker (#1534)
- **[punkcanyang](https://github.com/punkcanyang)** — Kitty (OSC 99) and Ghostty (OSC 777) desktop notification support (#1426)
- **[Rene-Kuhm](https://github.com/Rene-Kuhm)** — Spanish (es-419) Latin American localization (#1452)
- **[sternelee](https://github.com/sternelee)** — DeepSeek prefix-cache stability tracking (#1517)
- **[ComeFromTheMars](https://github.com/ComeFromTheMars)** — Shift+Up/Down transcript scroll shortcuts (#1432)
- **[sockerch](https://github.com/sockerch)** — pinyin aliases for all slash commands (#1306)
- **[Apeiron0w0](https://github.com/Apeiron0w0)** — FocusGained debounce for Tabby terminal flicker loop (#1560)
- **[greyfreedom](https://github.com/greyfreedom)** — jump-to-latest-transcript button (#969)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — explicit hidden-file mention completion (#1270)
- **[dst1213](https://github.com/dst1213)** — quota-error HTTP 400 retry (#1203)
- **[fuleinist](https://github.com/fuleinist)** — `--yolo` flag forwarding from CLI to TUI (#1233)
- **[heloanc](https://github.com/heloanc)** — Home/End key composer support (#1246)
- **[jinpengxuan](https://github.com/jinpengxuan)** — active provider credential preservation during onboarding (#1265)
- **[lixiasky-back](https://github.com/lixiasky-back)** — verified npm binary adoption (#1339)
- **[J3y0r](https://github.com/J3y0r)** — workspace-switch command (#1065)
- **[KhalidAlnujaidi](https://github.com/KhalidAlnujaidi)** — delegate skill bundling (#1144)
- **[Wenjunyun123](https://github.com/Wenjunyun123)** — docs anchor-offset preservation (#1282)
- **[whtis](https://github.com/whtis)** — zh-CN README dispatcher-path sync (#1235)
- **[aqilaziz](https://github.com/aqilaziz)** — memory skill-link fix (#1095)
- **[wuwuzhijing](https://github.com/wuwuzhijing)** — rsproxy rustup workaround install docs (#1011)
- **[eltociear](https://github.com/eltociear)** — Japanese README translation (#746)
- **[Ling](https://github.com/LING71671)** — `grep_files` cancellation-token support and Ctrl+Z composer-draft recovery (#1839, #1911)
- **[Ben Younes](https://github.com/ousamabenyounes)** — Linux Wayland (non-wlroots) clipboard support (#1938)
- **[Matt Van Horn](https://github.com/mvanhorn)** — Docker first-run permission fix and runtime system-prompt regression tests (#1699, #1702)
- **[Kristopher Clark](https://github.com/krisclarkdev)** — compaction user-query preservation fix (#1704)
- **[tdccccc](https://github.com/tdccccc)** — composer scroll fix and pager mouse-wheel support (#1715, #1716)
- **[LittleBlacky](https://github.com/LittleBlacky)** — provider-gated `reasoning_content` stream fix (#1680)
- **[Anaheim](https://github.com/AnaheimEX)** — `rlm_open` blank-source schema validation report (#1712)
- **[THatch26](https://github.com/THatch26)** — terminal resize paging fix (#1724)
- **[Alvin](https://github.com/alvin1)** — Zed ACP id compatibility report (#1696)
- **[knqiufan](https://github.com/knqiufan)** — sub-agent file-write delegation work (#1833)
- **[IIzzaya](https://github.com/IIzzaya)** — exact-alias-first slash-completion ordering idea (#1811)
- **[DC](https://github.com/duanchao-lab)** — terminal cleanup-guard idea (#1630)
- **[imkingjh999](https://github.com/imkingjh999)** — provider/model switching fixes (#1642)
- **[Photo](https://github.com/eng2007)** — provider-aware `/model` picker catalog work (#1201)
- **[chennest](https://github.com/chennest)** — diagnostics schema report (#1685)
- **[kunpeng-ai-lab](https://github.com/kunpeng-ai-lab)** — Windows composer scroll fix (#1578)
- **[WuMing](https://github.com/asdfg314284230)** — Windows PowerShell flicker fix (#1591)
- **[maker316](https://github.com/maker316)** — LoopGuard/checklist loop report (#1574)
- **[lalala](https://github.com/lalala-233)** — approval denial regression report (#1617)
- **[muyuliyan](https://github.com/muyuliyan)** — `pandoc_convert` validation fix (#1523)
- **[czf0718](https://github.com/czf0718)** — resize and turn-completion flicker fix (#1537)
- **[MeAiRobot](https://github.com/MeAiRobot)** — toast overlay composer-input fix (#1485)
- **[tiger-dog](https://github.com/tiger-dog)** — approval modal collapse and markdown identifier fixes (#1455)
- **[MMMarcinho](https://github.com/MMMarcinho)** — opt-in `image_analyze` vision tool (#1467)
- **[lucaszhu-hue](https://github.com/lucaszhu-hue)** — AtlasCloud provider integration (#1436)
- **[sandofree](https://github.com/sandofree)** — Tavily and Bocha `web_search` backends (#1294)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — `/change` release-notes command (#1416)
- **[NorethSea](https://github.com/NorethSea)** — updater companion-binary refresh fix (#1492)
- **[Jianfengwu2024](https://github.com/Jianfengwu2024)** — Windows MSVC toolchain environment preservation (#1487)
- **[Fire-dtx](https://github.com/Fire-dtx)** — npm postinstall recoverability work (#1059)
- **[oooyuy92](https://github.com/oooyuy92)** — long-session palette readability report (#1070, #936)
- **[qinxianyuzou](https://github.com/qinxianyuzou)** — zh-Hans destructive approval wording (#1087, #1091)
- **[tyouter](https://github.com/tyouter)** — session title/history preview cleanup (#1510)
- **[xulongzhe](https://github.com/xulongzhe)** — issue-template and vision-boundary follow-ups (#1530, #1544)
- **[YaYII](https://github.com/YaYII)** — trusted media path work (#1462)
- **[47Cid](https://github.com/47Cid)** and **[Jafar Akhondali](https://github.com/JafarAkhondali)** — responsible security disclosures and hardening reports

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests welcome — check the [open issues](https://github.com/Hmbown/CodeWhale/issues) for good first contributions.

If you want CodeWhale to help improve CodeWhale, start with the
[recursive self-improvement prompt](docs/RECURSIVE_SELF_IMPROVEMENT.md). It is
designed to turn one DeepSeek V4 Pro session, or another capable open-weight
path, into one small, reviewable patch.

> [!Note]
> *Not affiliated with DeepSeek Inc.*

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
