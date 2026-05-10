# DeepSeek TUI

> Terminal coding agent for DeepSeek V4. It runs from the `deepseek` command, streams reasoning blocks, edits local workspaces with approval gates, and includes an auto mode that chooses both model and thinking level per turn.

[简体中文 README](README.zh-CN.md)

## Install

`deepseek` is distributed as Rust binaries: the dispatcher command
(`deepseek`) and the companion TUI runtime (`deepseek-tui`). Pick whichever
install path you already use; they all put the same commands on your `PATH`.
The npm package is an installer/wrapper for the release binaries, not the
agent runtime itself.

```bash
# 1. npm — easiest if you already use Node. The package downloads the
#    matching prebuilt Rust binaries from GitHub Releases.
npm install -g deepseek-tui

# 2. Cargo — no Node needed.
cargo install deepseek-tui-cli --locked   # `deepseek` (entry point)
cargo install deepseek-tui     --locked   # `deepseek-tui` (TUI binary)

# 3. Homebrew — macOS package manager.
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. Direct download — no package manager or toolchain.
#    https://github.com/Hmbown/DeepSeek-TUI/releases
#    Prebuilt for Linux x64/ARM64, macOS x64/ARM64, Windows x64.

# 5. Docker — prebuilt release image.
docker run --rm -it \
  -e DEEPSEEK_API_KEY \
  -v "$PWD:/workspace" \
  ghcr.io/hmbown/deepseek-tui:latest
```

> In mainland China, speed up the npm path with
> `--registry=https://registry.npmmirror.com`, or use the
> [Cargo mirror](#china--mirror-friendly-installation) below.

[![CI](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/DeepSeek-TUI/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/deepseek-tui)](https://www.npmjs.com/package/deepseek-tui)
[![crates.io](https://img.shields.io/crates/v/deepseek-tui-cli?label=crates.io)](https://crates.io/crates/deepseek-tui-cli)
[DeepWiki project index](https://deepwiki.com/Hmbown/DeepSeek-TUI)

![DeepSeek TUI screenshot](assets/screenshot.png)

---

## What Is It?

DeepSeek TUI is a coding agent that runs in your terminal. It can read and edit files, run shell commands, search the web, manage git, and coordinate sub-agents from a keyboard-driven TUI.

It is built around DeepSeek V4 (`deepseek-v4-pro` / `deepseek-v4-flash`), including 1M-token context windows, streaming reasoning blocks, and prefix-cache-aware cost reporting.

### Key Features

- **Auto mode** — `--model auto` / `/model auto` chooses both the model and thinking level for each turn
- **Thinking-mode streaming** — see DeepSeek reasoning blocks as the model works
- **Full tool suite** — file ops, shell execution, git, web search/browse, apply-patch, sub-agents, MCP servers
- **1M-token context** — context tracking, manual or configured compaction, and prefix-cache telemetry
- **Three modes** — Plan (read-only explore), Agent (interactive with approval), YOLO (auto-approved)
- **Reasoning-effort tiers** — cycle through `off → high → max` with `Shift + Tab`
- **Session save/resume** — checkpoint and resume long-running sessions
- **Workspace rollback** — side-git pre/post-turn snapshots with `/restore` and `revert_turn`, without touching your repo's `.git`
- **Durable task queue** — background tasks can survive restarts
- **HTTP/SSE runtime API** — `deepseek serve --http` for headless agent workflows
- **MCP protocol** — connect to Model Context Protocol servers for extended tooling; please see [docs/MCP.md](docs/MCP.md)
- **Native RLM** (`rlm_query`) — run batched analysis through cheap `deepseek-v4-flash` children using the same API client
- **LSP diagnostics** — inline error/warning surfacing after every edit via rust-analyzer, pyright, typescript-language-server, gopls, clangd
- **User memory** — optional persistent note file injected into the system prompt for cross-session preferences
- **Localized UI** — `en`, `ja`, `zh-Hans`, `pt-BR` with auto-detection
- **Live cost tracking** — per-turn and session-level token usage and cost estimates; cache hit/miss breakdown
- **Skills system** — composable, installable instruction packs from GitHub with no backend service required

---

## How It's Wired

`deepseek` (dispatcher CLI) → `deepseek-tui` (companion binary) → ratatui interface ↔ async engine ↔ OpenAI-compatible streaming client. Tool calls route through a typed registry (shell, file ops, git, web, sub-agents, MCP, RLM) and results stream back into the transcript. The engine manages session state, turn tracking, the durable task queue, and an LSP subsystem that feeds post-edit diagnostics into the model's context before the next reasoning step.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full walkthrough.

---

## Quickstart

```bash
npm install -g deepseek-tui
deepseek --version
deepseek --model auto
```

Prebuilt binaries are published for **Linux x64**, **Linux ARM64** (v0.8.8+), **macOS x64**, **macOS ARM64**, and **Windows x64**. For other targets (musl, riscv64, FreeBSD, etc.), see [Install from source](#install-from-source) or [docs/INSTALL.md](docs/INSTALL.md).

On first launch you'll be prompted for your [DeepSeek API key](https://platform.deepseek.com/api_keys). The key is saved to `~/.deepseek/config.toml` so it works from any directory without OS credential prompts.

You can also set it ahead of time:

```bash
deepseek auth set --provider deepseek   # saves to ~/.deepseek/config.toml
deepseek auth status                    # shows the active credential source

export DEEPSEEK_API_KEY="YOUR_KEY"      # env var alternative; use ~/.zshenv for non-interactive shells
deepseek

deepseek doctor                         # verify setup
```

If `deepseek doctor` says the rejected key came from `DEEPSEEK_API_KEY`, remove
the stale export from your shell startup file, open a fresh shell, or run
`deepseek auth set --provider deepseek`. Use `deepseek auth status` to see the
config, keyring, and env-var source state without printing the key. Saved config
keys take precedence over the keyring and environment and are easier to rotate.

> To rotate or remove a saved key: `deepseek auth clear --provider deepseek`.

### Auto Mode

Use `deepseek --model auto` or `/model auto` when you want DeepSeek TUI to decide how much model and reasoning power a turn needs.

Auto mode controls two settings together:

- Model: `deepseek-v4-flash` or `deepseek-v4-pro`
- Thinking: `off`, `high`, or `max`

Before the real turn is sent, the app makes a small `deepseek-v4-flash` routing call with thinking off. That router looks at the latest request and recent context, then selects a concrete model and thinking level for the real request. Short/simple turns can stay on Flash with thinking off; coding, debugging, release work, architecture, security review, or ambiguous multi-step tasks can move up to Pro and/or higher thinking.

`auto` is local to DeepSeek TUI. The upstream API never receives `model: "auto"`; it receives the concrete model and thinking setting chosen for that turn. The TUI shows the selected route, and cost tracking is charged against the model that actually ran. If the router call fails or returns an invalid answer, the app falls back to a local heuristic. Sub-agents inherit auto mode unless you assign them an explicit model.

Use a fixed model or fixed thinking level when you want repeatable benchmarking, a strict cost ceiling, or a specific provider/model mapping.

### Linux ARM64 (Raspberry Pi, Asahi, Graviton, HarmonyOS PC)

`npm i -g deepseek-tui` works on glibc-based ARM64 Linux from v0.8.8 onward. You can also download prebuilt binaries from the [Releases page](https://github.com/Hmbown/DeepSeek-TUI/releases) and place them side by side on your `PATH`.

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
cargo install deepseek-tui-cli --locked   # provides `deepseek`
cargo install deepseek-tui     --locked   # provides `deepseek-tui`
deepseek --version
```

Prebuilt binaries can also be downloaded from [GitHub Releases](https://github.com/Hmbown/DeepSeek-TUI/releases). Use `DEEPSEEK_TUI_RELEASE_BASE_URL` for mirrored release assets.

### Windows (Scoop)

[Scoop](https://scoop.sh) is a Windows package manager. DeepSeek TUI is listed
in Scoop's main bucket, but that manifest updates independently and can lag the
GitHub/npm/Cargo release. Run `scoop update` first, then verify the installed
version with `deepseek --version`:

```bash
scoop update
scoop install deepseek-tui
deepseek --version
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

git clone https://github.com/Hmbown/DeepSeek-TUI.git
cd DeepSeek-TUI

cargo install --path crates/cli --locked   # requires Rust 1.88+; provides `deepseek`
cargo install --path crates/tui --locked   # provides `deepseek-tui`
```

Both binaries are required. Cross-compilation and platform-specific notes: [docs/INSTALL.md](docs/INSTALL.md).

</details>

### Other API Providers

```bash
# NVIDIA NIM
deepseek auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
deepseek --provider nvidia-nim

# Fireworks
deepseek auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
deepseek --provider fireworks --model deepseek-v4-pro

# Generic OpenAI-compatible endpoint
deepseek auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" deepseek --provider openai --model glm-5

# Self-hosted SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" deepseek --provider sglang --model deepseek-v4-flash

# Self-hosted vLLM
VLLM_BASE_URL="http://localhost:8000/v1" deepseek --provider vllm --model deepseek-v4-flash

# Self-hosted Ollama
ollama pull deepseek-coder:1.3b
deepseek --provider ollama --model deepseek-coder:1.3b
```

---

## What's New In v0.8.25

A stabilization + drift-fixes release. [Full changelog](CHANGELOG.md).

- **Markdown tables wrap long cells** instead of truncating with `…`.
  Long cell content is word-wrapped within the column and the grid stays
  intact on every wrapped line.
- **Self-update is `curl`-free and verifies SHA-256** — `deepseek update`
  uses `reqwest` with rustls and parses the aggregated checksum manifest
  to verify each downloaded asset before installing. Drops the v0.8.23
  Schannel `--ssl-no-revoke` Windows hack.
- **MCP JSON-RPC framing centralized** — request/response correlation,
  timeouts, and message framing now live above the byte transports.
  Stdio, SSE, and the new Streamable HTTP transport share one protocol
  layer.
- **Streamable HTTP MCP endpoints** (#1300, thanks **Reid Liu
  (@reidliu41)**) — third MCP transport alongside stdio and SSE.
- **Terminal-mode recovery unified** — startup, `FocusGained`, and
  `resume_terminal` all route through one `recover_terminal_modes()`
  helper. Wheel scroll, keyboard enhancement, bracketed paste, and
  focus events are re-armed in one place after focus round-trips.
- **`recall_archive` available in parent registries** — the read-only
  BM25 archive search tool is now callable from Plan, Agent, and YOLO
  parent registries (was sub-agent only).
- **Onboarding respects the active provider** (#1265, thanks
  **jinpengxuan (@jinpengxuan)**), **Home/End move the cursor**
  (#1246, thanks **heloanc (@heloanc)**), **`/config` view columns
  align to data** (#1290, thanks **Reid Liu (@reidliu41)**),
  **`reasoning_content` replay is cache-stable** (#1297, thanks
  **Duducoco (@Duducoco)**), **docs anchor scroll-margin overrideable**
  (#1282, thanks **Wenjunyun123 (@Wenjunyun123)**), **zh-Hans
  approval-dialog uses 终止** (#1274, thanks **Liu-Vince
  (@Liu-Vince)**).

⚠️ **Known issues carried over to v0.8.26:** Windows 10 conhost flicker
(#1260, #1251), per-turn snapshotting (no write-aware skip yet), `▏`
glyph leak in code blocks (#1212), mouse selection crossing the
sidebar (#1169), drag-select edge auto-scroll (#1163), mid-run MCP
stderr capture.

---

## Usage

```bash
deepseek                                         # interactive TUI
deepseek "explain this function"                 # one-shot prompt
deepseek --model deepseek-v4-flash "summarize"   # model override
deepseek --model auto "fix this bug"             # auto-select model + thinking
deepseek --yolo                                  # auto-approve tools
deepseek auth set --provider deepseek            # save API key
deepseek doctor                                  # check setup & connectivity
deepseek doctor --json                           # machine-readable diagnostics
deepseek setup --status                          # read-only setup status
deepseek setup --tools --plugins                 # scaffold tool/plugin dirs
deepseek models                                  # list live API models
deepseek sessions                                # list saved sessions
deepseek resume --last                           # resume the most recent session in this workspace
deepseek resume <SESSION_ID>                     # resume a specific session by UUID
deepseek fork <SESSION_ID>                       # fork a session at a chosen turn
deepseek serve --http                            # HTTP/SSE API server
deepseek serve --acp                             # ACP stdio adapter for Zed/custom agents
deepseek run pr <N>                              # fetch PR and pre-seed review prompt
deepseek mcp list                                # list configured MCP servers
deepseek mcp validate                            # validate MCP config/connectivity
deepseek mcp-server                              # run dispatcher MCP stdio server
deepseek update                                  # check for and apply binary updates
```

Docker images are published to GHCR for release builds:

```bash
docker volume create deepseek-tui-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v deepseek-tui-home:/home/deepseek/.deepseek \
  ghcr.io/hmbown/deepseek-tui:latest
```

### Zed / ACP

DeepSeek can run as a custom Agent Client Protocol server for editors that
spawn local ACP agents over stdio. In Zed, add a custom agent server:

```json
{
  "agent_servers": {
    "DeepSeek": {
      "type": "custom",
      "command": "deepseek",
      "args": ["serve", "--acp"],
      "env": {}
    }
  }
}
```

The first ACP slice supports new sessions and prompt responses through your
existing DeepSeek config/API key. Tool-backed editing and checkpoint replay are
not exposed through ACP yet.

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

Full shortcut catalog: [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).

---

## Modes

| Mode | Behavior |
| --- | --- |
| **Plan** 🔍 | Read-only investigation — model explores and proposes a plan (`update_plan` + `checklist_write`) before making changes |
| **Agent** 🤖 | Default interactive mode — multi-step tool use with approval gates; model outlines work via `checklist_write` |
| **YOLO** ⚡ | Auto-approve all tools in a trusted workspace; still maintains plan and checklist for visibility |

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
| `DEEPSEEK_PROVIDER` | `deepseek` (default), `nvidia-nim`, `openai`, `openrouter`, `novita`, `fireworks`, `sglang`, `vllm`, `ollama` |
| `DEEPSEEK_PROFILE` | Config profile name |
| `DEEPSEEK_MEMORY` | Set to `on` to enable user memory |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | Provider auth |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | Generic OpenAI-compatible endpoint and model ID |
| `SGLANG_BASE_URL` | Self-hosted SGLang endpoint |
| `VLLM_BASE_URL` | Self-hosted vLLM endpoint |
| `OLLAMA_BASE_URL` | Self-hosted Ollama endpoint |
| `OLLAMA_MODEL` | Self-hosted Ollama model tag |
| `NO_ANIMATIONS=1` | Force accessibility mode at startup |
| `SSL_CERT_FILE` | Custom CA bundle for corporate proxies |

Set `locale` in `settings.toml`, use `/config locale zh-Hans`, or rely on `LC_ALL`/`LANG` to choose UI chrome and the fallback language sent to V4 models. The latest user message still wins for natural-language reasoning and replies, so Chinese user turns stay Chinese even on an English system locale. See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) and [docs/MCP.md](docs/MCP.md).

---

## Models & Pricing

| Model | Context | Input (cache hit) | Input (cache miss) | Output |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M* | $0.435 / 1M* | $0.87 / 1M* |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

DeepSeek Platform defaults to `https://api.deepseek.com/beta` in v0.8.16 so beta-gated API features can be tested without extra setup. Set `base_url = "https://api.deepseek.com"` to opt out.

Legacy aliases `deepseek-chat` / `deepseek-reasoner` map to `deepseek-v4-flash` and retire after July 24, 2026. NVIDIA NIM variants use your NVIDIA account terms.

*DeepSeek Pro rates currently reflect a limited-time 75% discount, which remains valid until 15:59 UTC on 31 May 2026. After that time, the TUI cost estimator will revert to the base Pro rates.*

> [!Note]
> For the latest DeepSeek-V4-Pro pricing, including the current 75% discount valid until 15:59 UTC on 31 May 2026, please consult the official [DeepSeek pricing page](https://api-docs.deepseek.com/zh-cn/quick_start/pricing). All rates listed in the README correspond to the officially published values.

---

## Publishing Your Own Skill

DeepSeek TUI discovers skills from workspace directories (`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills` → `.cursor/skills`) and global directories (`~/.agents/skills` → `~/.claude/skills` → `~/.deepseek/skills`). Each skill is a directory with a `SKILL.md` file:

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
| [MEMORY.md](docs/MEMORY.md) | User memory feature guide |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | Sub-agent role taxonomy and lifecycle |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | Full shortcut catalog |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | Release process |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | UI locale matrix & switching |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | Ops & recovery |

Full Changelog: [CHANGELOG.md](CHANGELOG.md).

---

## Thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — thank you for the models and support that power every turn. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — thank you for your support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。

This project ships with help from a growing community of contributors:

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 28 PRs spanning features, fixes, and VS Code extension scaffolding (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Markdown rendering for tables, bold/italic, and horizontal rules (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows + China install documentation (#578)
- **[20bytes](https://github.com/20bytes)** — User memory docs and help polish (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc compatibility preflight (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc compatibility improvements (#565)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Shell `cwd` boundary validation (#524)
- **[toi500](https://github.com/toi500)** — Windows paste fix report
- **[xsstomy](https://github.com/xsstomy)** — Terminal startup repaint report
- **[melody0709](https://github.com/melody0709)** — Slash-prefix Enter activation report
- **[lloydzhou](https://github.com/lloydzhou)** and **[jeoor](https://github.com/jeoor)** — Compaction cost reports; lloydzhou also contributed deterministic environment context (#813, #922) and KV prefix-cache stabilisation (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README clarity pass (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows install documentation (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — Pricing/discount info update (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — model ID case-sensitivity compatibility report (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — stale `working...` state bug report and Windows clipboard fallback (#738, #850)
- **[reidliu41](https://github.com/reidliu41)** — resume hint, workspace trust persistence, Ollama provider support, and thinking-block stream finalization (#863, #870, #921, #1078)
- **[xieshutao](https://github.com/xieshutao)** — plain Markdown skill fallback (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper `--version` fallback (#885)
- **[y0sif](https://github.com/y0sif)** — parent turn-loop wakeup after direct child sub-agent completion (#901)
- **[mac119](https://github.com/mac119)** and **[leo119](https://github.com/leo119)** — `deepseek update` command documentation (#838, #917)
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
- **[axobase001](https://github.com/axobase001)** — snapshot orphan cleanup, npm install guards, session telemetry fixes, model-scope cache clear, symlinked skill support, and npm mirror-escape-hatch guidance (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` command for dark/light toggle and SSE gzip/brotli decompression (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan-mode read-only sandbox safety fix (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — precise paste-Enter auto-submit reproducer (#1073)
- **[Duducoco](https://github.com/Duducoco)** and **[AlphaGogoo](https://github.com/AlphaGogoo)** — skills slash-menu and `/skills` coverage fix (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — window-resize artifact fix for macOS Terminal.app and ConHost (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter and custom-endpoint model-ID preservation (#1066)
- **[Jefsky](https://github.com/Jefsky)** — DeepSeek endpoint correction report (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM provider API-key preference diagnosis (#1081)

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests welcome — check the [open issues](https://github.com/Hmbown/DeepSeek-TUI/issues) for good first contributions.

Support: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!Note]
> *Not affiliated with DeepSeek Inc.*

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/DeepSeek-TUI&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FDeepSeek-TUI&type=date&logscale=&legend=top-left)
