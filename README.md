# CodeWhale

> Terminal coding agent for DeepSeek V4. It runs from the `codewhale` command, streams reasoning blocks, edits local workspaces with approval gates, and includes an auto mode that chooses both model and thinking level per turn.

[简体中文 README](README.zh-CN.md)
[日本語 README](README.ja-JP.md)
[Tiếng Việt README](README.vi.md)


## Install

`codewhale` installs as a matched pair of self-contained Rust release binaries:
the `codewhale` dispatcher command and the sibling `codewhale-tui` runtime it
launches for interactive sessions. npm and Docker install both for you; Cargo
and manual installs must put both binaries in the same directory
(normally a directory on your `PATH`). The npm package is only an
installer/wrapper for those release binaries; the agent does not run on Node.

```bash
# 1. npm — easiest if you already use Node. The package downloads the
#    matching prebuilt Rust binaries from GitHub Releases.
npm install -g codewhale

# 2. Cargo — no Node needed. Requires Rust 1.88+ (the crates use the
#    2024 edition; older toolchains fail with "feature `edition2024` is
#    required"). Run `rustup update` first, or use a non-Cargo path below.
cargo install codewhale-cli --locked   # `codewhale` (entry point)
cargo install codewhale-tui     --locked   # `codewhale-tui` (TUI binary)

# 3. Homebrew — legacy compatibility only.
#    The tap/formula still uses the old deepseek-tui name. Prefer npm, Cargo,
#    Docker, or direct downloads for new installs until the formula is renamed.
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. Direct download — platform archive from GitHub Releases.
#    https://github.com/Hmbown/CodeWhale/releases
#    Archives include both codewhale and codewhale-tui plus an install script.
#    Individual binaries are also attached for scripts; keep the pair together.

# 5. Docker — prebuilt release image.
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
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
brew update && brew upgrade deepseek-tui  # legacy Homebrew installs only
cargo install codewhale-cli --locked --force
cargo install codewhale-tui     --locked --force
```

> codewhale update now supports --proxy, update through a proxy
> eg: codewhale update --proxy https://localhost:7897

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[DeepWiki project index](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

---

## What Is It?

A model answers a question. An agent finishes a task. The difference is
the harness — a system of rules, evidence, and feedback that keeps the
model oriented instead of drifting.

CodeWhale is that harness, built around DeepSeek V4 and guided by three ideas:

| Principle | How it works |
|---|---|
| **Start with trust** | Every turn begins with "A" — possibility before certainty, craft before convenience |
| **Clear jurisdiction** | A written Constitution with nine tiers of authority. User intent outranks stale instructions. Verification outranks confidence. |
| **Recursive improvement** | V4 helped write the harness. As the harness improves, V4 becomes more effective — and helps improve the harness further. Each turn starts stronger. |

It's open source, terminal-native, and packaged as a matched `codewhale` /
`codewhale-tui` Rust binary pair.

## How the Harness Works

Agentic models deal with conflicting information at scale: user intent,
project rules, system defaults, tool output, and stale memory all compete
for authority in a single turn. LLM-as-a-judge needs jurisdiction — which
source wins when they disagree?

CodeWhale answers this with a **Constitution** (`prompts/base.md`). It's a
formal hierarchy of law — Article VII ranks nine tiers from the
Constitution's own articles down to prior-session handoffs. The user's
current message outranks stale project instructions. Live tool output
outranks assumptions. Verification outranks confidence. The model inherits
a clear chain of authority every turn and never has to guess which
directive to follow.

Six Articles define the model's identity, duties, and agency (Article VII
is the hierarchy itself): a verification mandate (Article V — every action
leaves evidence, never declare success on faith), a coordination legacy
(Article VI — leave the workspace cleaner and the handoff truthful for the
next intelligence), and a primacy-of-truth clause (Article II —
non-negotiable; not even a user request may override the duty of truth).

DeepSeek V4's prefix caching makes this practical. The Constitution is long
and detailed, but once cached it costs roughly 100× less per turn than a
cold read. The model references it recursively — peeking, scanning, and
querying through RLM sessions — revisiting information on demand rather
than relying on a single memorized pass. It performs more like an
open-book test than a closed one.

Because the authority structure is explicit, failure isn't hidden. Non-zero
exit codes, type errors from rust-analyzer arriving between turns, sandbox
denials — these are fed back as correction vectors. The model uses its own
drift to self-correct.

Three modes control the action space. Plan is read-only. Agent gates
destructive operations behind approval. YOLO auto-approves in trusted
workspaces. macOS Seatbelt is the active sandbox; Linux Landlock is
detected but not yet enforced; Windows sandboxing is not yet advertised.

Fin — a cheap Flash call with thinking off — handles model auto-routing per
turn. `--model auto` is the default.

Every turn records a side-git snapshot outside your repo's `.git`.
`/restore` and `revert_turn` roll back the workspace.

Sub-agents run concurrently (up to 20). `agent_open` returns immediately;
results arrive inline as completion sentinels with a summary. Full
transcripts stay behind bounded handles through `agent_eval`. See
[docs/SUBAGENTS.md](docs/SUBAGENTS.md).

The rest of the surface: LSP diagnostics after every edit (rust-analyzer,
pyright, typescript-language-server, gopls, clangd, jdtls,
vue-language-server), RLM sessions for batched analysis, MCP protocol,
HTTP/SSE runtime API, persistent task queue, ACP adapter for Zed,
SWE-bench export, and live cost tracking with cache hit/miss breakdowns.

---

## The Harness

`codewhale` (dispatcher CLI) → `codewhale-tui` (companion binary) → ratatui interface ↔ async engine ↔ OpenAI-compatible streaming client. Tool calls route through a typed registry (shell, file ops, git, web, sub-agents, MCP, RLM) and results stream back into the transcript. The engine manages session state, turn tracking, the durable task queue, and an LSP subsystem that feeds post-edit diagnostics into the model's context before the next reasoning step.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full walkthrough.

### Sub-agents: Concurrent Background Execution

CodeWhale can dispatch multiple sub-agents that run in parallel — like a concurrent task queue:

- **Non-blocking launch.** `agent_open` returns immediately. The child gets its own fresh context and tool registry and runs independently. The parent keeps working.
- **Background execution.** Sub-agents execute concurrently (default cap: 10, configurable to 20). The engine manages the pool — no polling loop needed.
- **Completion notification.** When a sub-agent finishes, the runtime injects a `<codewhale:subagent.done>` sentinel into the parent's transcript. The human-readable summary — including the child's findings, changed files, and any risks — sits on the line immediately before the sentinel. The parent model reads that summary and integrates findings without an extra tool call.
- **Bounded result retrieval.** The full child transcript lives behind a `transcript_handle` accessible through `agent_eval`. When the summary isn't enough, the parent calls `handle_read` for slices, line ranges, or JSONPath projections — keeping the parent context lean without losing access to the details.

See [docs/SUBAGENTS.md](docs/SUBAGENTS.md) for the full sub-agent reference.

---

## Quickstart

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

Prebuilt binary pairs and platform archives are published for **Linux x64**, **Linux ARM64** (v0.8.8+), **macOS x64**, **macOS ARM64**, and **Windows x64**. For other targets (musl, riscv64, FreeBSD, etc.), see [Install from source](#install-from-source) or [docs/INSTALL.md](docs/INSTALL.md).

On first launch you'll be prompted for your [DeepSeek API key](https://platform.deepseek.com/api_keys). The key is saved to `~/.codewhale/config.toml` (legacy `~/.deepseek/config.toml` also supported) so it works from any directory without OS credential prompts.

You can also set it ahead of time:

```bash
codewhale auth set --provider deepseek   # saves to ~/.codewhale/config.toml
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

### Auto Mode

Use `codewhale --model auto` or `/model auto` when you want codewhale to decide how much model and reasoning power a turn needs.

Auto mode controls two settings together:

- Model: `deepseek-v4-flash` or `deepseek-v4-pro`
- Thinking: `off`, `high`, or `max`

Before the real turn is sent, the app makes a small `deepseek-v4-flash` routing call with thinking off. That router looks at the latest request and recent context, then selects a concrete model and thinking level for the real request. Short/simple turns can stay on Flash with thinking off; coding, debugging, release work, architecture, security review, or ambiguous multi-step tasks can move up to Pro and/or higher thinking.

`auto` is local to codewhale. The upstream API never receives `model: "auto"`; it receives the concrete model and thinking setting chosen for that turn. The TUI shows the selected route, and cost tracking is charged against the model that actually ran. If the router call fails or returns an invalid answer, the app falls back to a local heuristic. Sub-agents inherit auto mode unless you assign them an explicit model.

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
scoop install codewhale
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

For the full shipped provider registry, including model IDs, auth variables,
base URLs, and capability boundaries, see [docs/PROVIDERS.md](docs/PROVIDERS.md).

Think of provider and model as separate choices: `provider` is the route,
account, and endpoint; `model` is the model ID on that route. DeepSeek-family
models can be reached through several routes, so `/config` exposes both
`provider` and `provider_url`.

| Route | Typical DeepSeek model ID |
|-------|---------------------------|
| `deepseek` | `deepseek-v4-pro` |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro` |
| `openrouter` | `deepseek/deepseek-v4-pro` |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` |
| `siliconflow` | `deepseek-ai/DeepSeek-V4-Pro` |
| `openai` | Your gateway's model ID |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro` |

```bash
# NVIDIA NIM
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim

# AtlasCloud
codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
codewhale --provider atlascloud
codewhale --provider atlascloud --model vendor/model-id

# Wanjie Ark
codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"
codewhale --provider wanjie-ark --model deepseek-reasoner

# OpenRouter
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro
codewhale --provider openrouter --model arcee-ai/trinity-large-thinking
codewhale --provider openrouter --model minimax/minimax-m3

Arcee AI offers direct API access to its powerful Trinity models, including the reasoning-capable Trinity-Large Thinking. This section provides comprehensive setup instructions and model comparisons.

## Configuration

### API Key
The primary authentication method is the `ARCEE_API_KEY` environment variable or the `[providers.arcee]` configuration section in `~/.codewhale/config.toml`:

```toml
[providers.arcee]
# api_key = "your-arcee-api-key"
# base_url = "https://api.arcee.ai/api/v1"
# model = "trinity-large-thinking"  # or "trinity-large-preview", "trinity-mini"
```

### Environment Variables

- `ARCEE_API_KEY`: Your Arcee API key (required)
- `ARCEE_BASE_URL`: Custom base URL (optional, defaults to `https://api.arcee.ai/api/v1`)
- `ARCEE_MODEL`: Default model to use (optional, defaults to `trinity-large-thinking`)

### Model Support

CodeWhale supports three Arcee models:

| Model | Reasoning | Context Window | Max Output | Best For |
|--------|-----------|----------------|------------|----------|
| `trinity-large-thinking` | ✅ Yes | 262,144 tokens | 262,144 tokens | Complex reasoning, coding, math |
| `trinity-large-preview` | ❌ No | 262,144 tokens | 4,096 tokens | High-accuracy non-reasoning tasks |
| `trinity-mini` | ❌ No | 128,000 tokens | 4,096 tokens | Faster, cost-effective tasks |

**Note:** The `trinity-large-thinking` model supports reasoning (thinking mode) and can handle very large contexts, making it ideal for complex programming tasks. The other models do not support reasoning but offer larger context windows than many other providers.
codewhale auth set --provider arcee --api-key "YOUR_ARCEE_API_KEY"
codewhale --provider arcee --model trinity-large-thinking
codewhale --provider arcee --model trinity-large-preview

# Xiaomi MiMo
codewhale auth set --provider xiaomi-mimo --api-key "YOUR_XIAOMI_KEY"
# Token Plan (`tp-...`) keys default to https://token-plan-sgp.xiaomimimo.com/v1.
# To force a provider endpoint: /config provider_url token-plan --save
# or /config provider_url pay-as-you-go --save.
codewhale --provider xiaomi-mimo --model mimo-v2.5-pro
codewhale --provider xiaomi-mimo --model mimo-v2.5
codewhale --provider xiaomi-mimo speech "Hello from MiMo" --model tts -o hello.wav

# Novita
codewhale auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
codewhale --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
codewhale --provider fireworks --model deepseek-v4-pro

# SiliconFlow
codewhale auth set --provider siliconflow --api-key "YOUR_SILICONFLOW_API_KEY"
codewhale --provider siliconflow --model deepseek-ai/DeepSeek-V4-Pro

# Generic OpenAI-compatible endpoint
codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" codewhale --provider openai --model glm-5

# Custom DeepSeek-compatible endpoint
DEEPSEEK_BASE_URL="https://your-provider.example/v1" \
  DEEPSEEK_MODEL="deepseek-ai/DeepSeek-V4-Pro" \
  codewhale --provider deepseek

# Self-hosted SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" codewhale --provider sglang --model deepseek-v4-flash

# Self-hosted vLLM
VLLM_BASE_URL="http://localhost:8000/v1" codewhale --provider vllm --model deepseek-v4-flash
# Trusted LAN vLLM over HTTP
DEEPSEEK_ALLOW_INSECURE_HTTP=1 VLLM_BASE_URL="http://192.168.0.110:8000/v1" codewhale --provider vllm --model deepseek-v4-flash

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

```bash
codewhale                                         # interactive TUI
codewhale "explain this function"                 # one-shot prompt
codewhale exec --auto --output-format stream-json "fix this bug"  # NDJSON backend stream
codewhale exec --resume <SESSION_ID> "follow up"  # continue a non-interactive session
codewhale --model deepseek-v4-flash "summarize"   # model override
codewhale --model auto "fix this bug"             # auto-select model + thinking
codewhale --yolo                                  # auto-approve tools
codewhale auth set --provider deepseek            # save API key
codewhale doctor                                  # check setup & connectivity
codewhale doctor --json                           # machine-readable diagnostics
codewhale setup --status                          # read-only setup status
codewhale setup --tools --plugins                 # scaffold tool/plugin dirs
codewhale models                                  # list live API models
codewhale sessions                                # list saved sessions with timestamps
codewhale resume --last                           # resume the most recent session in this workspace
codewhale resume <SESSION_ID>                     # resume a specific session by UUID
codewhale fork <SESSION_ID>                       # fork a saved session into a sibling path
codewhale serve --http                            # HTTP/SSE API server
codewhale serve --mobile                          # LAN mobile control page; token-gated by default
codewhale serve --acp                             # ACP stdio adapter for Zed/custom agents
codewhale run pr <N>                              # fetch PR and pre-seed review prompt
codewhale mcp list                                # list configured MCP servers
codewhale mcp validate                            # validate MCP config/connectivity
codewhale mcp-server                              # run dispatcher MCP stdio server
codewhale update                                  # check for and apply binary updates
```

Inside the interactive TUI composer, prefix a line with `!` to run a shell
command through the normal approval, sandbox, and output surfaces, for example
`! cargo test -p codewhale-tui sidebar`.

### Branching Conversations

Saved sessions are intentionally branchable. `codewhale fork <SESSION_ID>` copies
an existing saved session into a new sibling session, records the parent session
id in metadata, and opens that fork so you can explore an alternate direction
without polluting the original path. The session picker and `codewhale sessions`
mark forked sessions with their parent id.

`codewhale sessions` lists saved sessions across workspaces and includes the
last-updated timestamp. `codewhale resume --last` and `codewhale --continue`
choose the latest session for the current workspace; pass an explicit session id
when resuming work from another directory.

Inside the TUI, Esc-Esc backtrack can rewind the active transcript to a prior
user prompt and put that prompt back in the composer for editing. `/restore`
and `revert_turn` are separate workspace rollback tools: they restore files
from side-git snapshots but do not rewrite conversation history.

Docker images are published to GHCR for release builds:

```bash
docker volume create codewhale-home

docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

See [docs/DOCKER.md](docs/DOCKER.md) for pinned tags, local image builds,
volume ownership notes, and non-interactive pipeline usage.

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

Full shortcut catalog: [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md).

---

## Modes

| Mode | Behavior |
| --- | --- |
| **Plan** 🔍 | Read-only investigation — model explores and proposes a plan before making changes; multi-step investigations use `checklist_write` |
| **Agent** 🤖 | Default interactive mode — multi-step tool use with approval gates; substantial work is tracked with `checklist_write` |
| **YOLO** ⚡ | Auto-approve all tools in a trusted workspace; multi-step work still keeps a visible checklist |

---

## Configuration

User config: `~/.codewhale/config.toml` (legacy `~/.deepseek/config.toml` fallback). Project overlay: `<workspace>/.codewhale/config.toml` (legacy `<workspace>/.deepseek/config.toml`) (denied: `api_key`, `base_url`, `provider`, `mcp_config_path`). [config.example.toml](config.example.toml) has every option.

The TUI footer can be trimmed with `/statusline`, or by setting
`[tui].status_items` in config. Current footer customization selects from the
built-in chips such as `mode`, `model`, `status`, `git_branch`, `tokens`, and
`cache`; chip order is controlled by the order of keys in `status_items` in
`config.toml`. The interactive picker writes the canonical order. Multi-line
layouts, custom colors, and external command widgets are not part of the
current statusline surface.

Custom DeepSeek-compatible endpoints usually do not need a new provider. Keep
`provider = "deepseek"` and set `[providers.deepseek].base_url` / `model`, or
use `provider = "openai"` for generic OpenAI-compatible gateways. Keep
`provider`, `api_key`, and `base_url` in user config or environment variables;
project overlays cannot set them.

Key environment variables:

| Variable | Purpose |
|---|---|
| `DEEPSEEK_API_KEY` | API key |
| `DEEPSEEK_BASE_URL` | API base URL |
| `DEEPSEEK_HTTP_HEADERS` | Optional custom model request headers, e.g. `X-Model-Provider-Id=your-model-provider` |
| `DEEPSEEK_MODEL` | Default model |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | Stream idle timeout in seconds, default `300`, clamped to `1..=3600` |
| `CODEWHALE_PROVIDER` / `DEEPSEEK_PROVIDER` | `deepseek` (default), `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `volcengine`, `openrouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`, `siliconflow-CN`, `arcee`, `moonshot`, `sglang`, `vllm`, `ollama`, `huggingface` |
| `DEEPSEEK_PROFILE` | Config profile name |
| `DEEPSEEK_MEMORY` | Set to `on` to enable user memory |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | Allow non-local `http://` API base URLs on trusted networks |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY` / `OPENROUTER_API_KEY` / `XIAOMI_MIMO_API_KEY` / `XIAOMI_API_KEY` / `MIMO_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SILICONFLOW_API_KEY` / `ARCEE_API_KEY` / `MOONSHOT_API_KEY` / `KIMI_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` / `HUGGINGFACE_API_KEY` / `HF_TOKEN` | Provider auth |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | Generic OpenAI-compatible endpoint and model ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud endpoint and model override |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark endpoint and model override |
| `VOLCENGINE_BASE_URL` / `VOLCENGINE_ARK_BASE_URL` / `ARK_BASE_URL` / `VOLCENGINE_MODEL` / `VOLCENGINE_ARK_MODEL` | Volcengine Ark endpoint and model override |
| `OPENROUTER_BASE_URL` | OpenRouter endpoint override |
| `XIAOMI_MIMO_BASE_URL` / `MIMO_BASE_URL` / `XIAOMI_MIMO_MODEL` / `MIMO_MODEL` | Xiaomi MiMo endpoint and model override; Token Plan default is `https://token-plan-sgp.xiaomimimo.com/v1` |
| `NOVITA_BASE_URL` | Novita endpoint override |
| `FIREWORKS_BASE_URL` | Fireworks endpoint override |
| `SILICONFLOW_BASE_URL` / `SILICONFLOW_MODEL` | SiliconFlow endpoint and model override |
| `ARCEE_BASE_URL` / `ARCEE_MODEL` | Arcee AI endpoint and model override |
| `SGLANG_BASE_URL` | Self-hosted SGLang endpoint |
| `SGLANG_MODEL` | Self-hosted SGLang model ID |
| `VLLM_BASE_URL` | Self-hosted vLLM endpoint |
| `VLLM_MODEL` | Self-hosted vLLM model ID |
| `OLLAMA_BASE_URL` | Self-hosted Ollama endpoint |
| `OLLAMA_MODEL` | Self-hosted Ollama model tag |
| `HUGGINGFACE_API_KEY` / `HF_TOKEN` / `HUGGINGFACE_BASE_URL` / `HUGGINGFACE_MODEL` | Hugging Face endpoint and model override |
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

codewhale discovers skills from workspace directories (`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills` → `.cursor/skills`) and global directories (`~/.agents/skills` → `~/.claude/skills` → `~/.codewhale/skills` → `~/.deepseek/skills`). Each skill is a directory with a `SKILL.md` file:

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
`~/.codewhale/skills` (or legacy `~/.deepseek/skills`) and are versioned so new bundles are added on upgrade
without recreating skills the user deliberately deleted.

---

## Documentation

| Doc | Topic |
|---|---|
| [GUIDE.md](docs/GUIDE.md) | First-run user guide |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Codebase internals |
| [CONFIGURATION.md](docs/CONFIGURATION.md) | Full config reference |
| [PROVIDERS.md](docs/PROVIDERS.md) | Provider IDs, auth, model defaults, and capability metadata |
| [MODES.md](docs/MODES.md) | Plan / Agent / YOLO modes |
| [MCP.md](docs/MCP.md) | Model Context Protocol integration |
| [RUNTIME_API.md](docs/RUNTIME_API.md) | HTTP/SSE API server and mobile control page |
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

Full Changelog: [CHANGELOG.md](CHANGELOG.md).

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
- **[lloydzhou](https://github.com/lloydzhou)** and **[jeoor](https://github.com/jeoor)** — Compaction cost reports; lloydzhou also contributed deterministic environment context (#813, #922) and KV prefix-cache stabilisation (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README clarity pass (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows install documentation (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — Pricing/discount info update (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — model ID case-sensitivity compatibility report (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — stale `working...` state bug report, Windows clipboard fallback, MCP Streamable HTTP session fixes, and Homebrew tap automation (#738, #850, #1643, #1631)
- **[reidliu41](https://github.com/reidliu41)** — resume hint, workspace trust persistence, Ollama provider support, thinking-block stream finalization, CI cache hardening, streaming wrap, and DeepSeek model completions (#863, #870, #921, #1078, #1603, #1628, #1601)
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
- **[axobase001](https://github.com/axobase001)** — snapshot orphan cleanup, npm install guards, session telemetry fixes, model-scope cache clear, symlinked skill support, npm mirror-escape-hatch guidance, proxy preservation for child tasks, mobile runtime control, Docker toolbox docs, large-output receipts, and activity detail context (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056, #1608, #1968, #2296, #2297, #2298)
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
- **[zlh124](https://github.com/zlh124)** — WSL2/headless startup report, clipboard-init fix, CodeWhale tab-title polish, localized context-menu labels, and approval-dialog fixes (#1772, #1773, #2319, #2320, #2325)
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen logging, Home/End composer, and runtime log follow-ups (#1774, #1776, #1748, #1749, #1782, #1783)
- **[LeoLin990405](https://github.com/LeoLin990405)** — provider model passthrough, reasoning replay, thinking-only turn, and Windows quoting fixes (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Ctrl+C prompt restore, provider registry drift docs, tool-search defaults, footer git branch display, and startup prompt interactivity (#1764, #2274, #2344, #2347, #2373)
- **[donglovejava](https://github.com/donglovejava)** — paste @file consolidation, CJK panic fix, user feedback, RLM routing, edit_file retry, hidden-worktree discovery skip, IME composer routing, and eager shell companion tools (#2154-#2168, #2302, #2329, #2330, #2331)
- **[encyc](https://github.com/encyc)** — session token breakdown in footer and `/status` (#2152)
- **[saieswar237](https://github.com/saieswar237)** — review pipeline docs (#2178)
- **[sximelon](https://github.com/sximelon)** — paste Enter suppression, key handler extraction (#2174, #2042)
- **[nanookclaw](https://github.com/nanookclaw)** — search provider in doctor output (#2135)
- **[Sskift](https://github.com/Sskift)** — CLI default env override prevention and statusline footer clearing (#2119, #2248)
- **[xin1104](https://github.com/xin1104)** — Homebrew codewhale binary install (#2105)
- **[mrluanma](https://github.com/mrluanma)** — Metaso search provider (#2059)
- **[Lellansin](https://github.com/Lellansin)** — skip config merge at home dir (#2055)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — update release channels and legacy MCP SSE fixes (#2145, #2301)
- **[cy2311](https://github.com/cy2311)** — Windows `.bat` launcher for CodeWhale (#1861)
- **[LING71671](https://github.com/LING71671)** — effective cost currency context, custom provider docs, and core tool taxonomy prompt block (#1902, #2287, #2292)
- **[dzyuan](https://github.com/dzyuan)** — Volcengine provider support with DeepSeek V4 Pro/Flash models (#1993)
- **[mvanhorn](https://github.com/mvanhorn)** — live request-shape test factories and global `~/.agents/AGENTS.md` fallback (#2107, #2236)
- **[malsony](https://github.com/malsony)** — Matrix-inspired theme and theme picker improvements (#2129)
- **[gaord](https://github.com/gaord)** — external GUI runtime event bridge, session detail serialization, and skills API discovery alignment (#2133, #2265, #2285)
- **[yuanchenglu](https://github.com/yuanchenglu)** — Feishu per-chat model switching (#2149)
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — Xiaomi balance/status work, stalled-turn recovery, approval intent summaries, mobile smoke/QR support, Claude theme, and broad docs/test/CI coverage (#2257, #2267, #2283, #2384, #2385, #2389, #2403, #2440-#2458, #2460)
- **[h3c-hexin](https://github.com/h3c-hexin)** — web-search URL decoding, prompt/instructions override hooks, sub-agent guidance, SSRF fake-IP trust configuration, and prompt-cache-friendly environment placement (#2245, #2311, #2313, #2314, #2354, #2355, #2356)
- **[AresNing](https://github.com/AresNing)** — first-run guide and message-submit hook transform design harvested into the maintained hooks path (#2278, #2318, #2434)
- **[Implementist](https://github.com/Implementist)** — Volcengine Ark search provider and reliability hardening (#2426, #2429, #2439)
- **[lihuan215](https://github.com/lihuan215)** — Unix socket hook sink design harvested into the opt-in hook event path (#2333, #2430)
- **[AdityaVG13](https://github.com/AdityaVG13)** — Xiaomi MiMo provider support (#2246)
- **[New2Niu](https://github.com/New2Niu)** — macOS display notifications (#2260)
- **[AiurArtanis](https://github.com/AiurArtanis)** — Solarized Light theme (#2270)
- **[Lee-take](https://github.com/Lee-take)** — task migration and session environment isolation fixes (#2272)
- **[LeoAlex0](https://github.com/LeoAlex0)** — session persistence fixes for message counts and tool-output cache preservation (#2388, #2395)
- **[jimmyzhuu](https://github.com/jimmyzhuu)** — Baidu AI Search backend for `web_search` (#2371)
- **[rockyzhang](https://github.com/rockyzhang)** — RISC-V prebuilt binary support (#2383)
- **[mo-vic](https://github.com/mo-vic)** — `/purge` slash command for agent-driven context pruning (#2387)
- **[hufanexplore](https://github.com/hufanexplore)** — Java and Vue language-server defaults (#2367)
- **[hoclaptrinh33](https://github.com/hoclaptrinh33)** — Vietnamese localization support (#2358)
- **[AccMoment](https://github.com/AccMoment)** — proxy option for the update command (#2281)
- **[idling11](https://github.com/idling11)** — durable SlopLedger and `/hunt` rename/trophy-card work (#2161, #2306)
- **[cyq1017](https://github.com/cyq1017)** — runtime event envelope, render-diff debug logging, and deterministic composer history flushing (#2252, #2332, #2375)
- **[hongqitai](https://github.com/hongqitai)** — state schema parent-entry support and clippy/fmt cleanup (#2308, #2432)
- **[BryonGo](https://github.com/BryonGo)** — effective-model compaction budgeting fix (#2437)
- **[xyuai](https://github.com/xyuai)** — provider persistence to config, /logout scope clarification, provider picker key replacement shortcut, MiMo auth state cleanup (#2714, #2715, #2717, #2718)
- **[RefuseOdd](https://github.com/RefuseOdd)** — configurable `path_suffix` for OpenAI-compatible endpoints (#2558)

Reports, repros, and verification that shaped v0.8.48 also deserve visible
credit: **[@buko](https://github.com/buko)**, **[@yyyCode](https://github.com/yyyCode)**,
**[@gaslebinh-glitch](https://github.com/gaslebinh-glitch)**, **[@Dr3259](https://github.com/Dr3259)**,
**[@lpeng1711694086-lang](https://github.com/lpeng1711694086-lang)**, **[@VerrPower](https://github.com/VerrPower)**,
**[@yan-zay](https://github.com/yan-zay)**, **[@jretz](https://github.com/jretz)**,
**[@Neo-millunnium](https://github.com/Neo-millunnium)**, **[@caeserchen](https://github.com/caeserchen)**,
**[@T-Phuong-Nguyen](https://github.com/T-Phuong-Nguyen)**, **[@zhyuzhyu](https://github.com/zhyuzhyu)**,
**[@0gl20shk0sbt36](https://github.com/0gl20shk0sbt36)**, **[@hatakes](https://github.com/hatakes)**,
**[@goodvecn-dev](https://github.com/goodvecn-dev)**, **[@bevis-wong](https://github.com/bevis-wong)**,
**[@PurplePulse](https://github.com/PurplePulse)**, and **[@nbiish](https://github.com/nbiish)**.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests welcome — check the [open issues](https://github.com/Hmbown/CodeWhale/issues) for good first contributions.

Support: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!Note]
> *Not affiliated with DeepSeek Inc.*

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
