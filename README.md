# CodeWhale

> DeepSeek-first terminal coding agent with a durable harness: approval-gated
> local edits, sub-agents, provider/model routing, live verification, rollback,
> relay/continuity handoffs, and a v0.9 track for typed WhaleFlow workflows.

[简体中文 README](README.zh-CN.md)
[日本語 README](README.ja-JP.md)
[Tiếng Việt README](README.vi.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[DeepWiki project index](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

## What CodeWhale Does

CodeWhale is a terminal-native coding harness for agentic model work. It gives
the model a durable prompt constitution, a typed tool surface, approval gates,
side-git rollback, LSP feedback after edits, cost/cache telemetry, and
concurrent sub-agents that can investigate or implement without blocking the
parent turn.

It is DeepSeek-first, not DeepSeek-only. The default path targets DeepSeek V4,
while provider routes such as OpenRouter, NVIDIA NIM, Arcee, Xiaomi MiMo,
SiliconFlow, Fireworks, OpenAI-compatible gateways, self-hosted SGLang/vLLM, and
Hugging Face stay explicit. Provider, model, base URL, and credentials are
separate choices so direct-provider APIs do not get blurred with OpenRouter
aliases.

The product goal is practical continuity. A long CodeWhale task should survive
model routing, compaction, shell noise, branch experiments, contributor review,
and a fresh maintainer session without losing the reason the work started or
who helped move it forward.

## Active v0.9 Track

v0.9.0 is not released yet. The current branch is a stewardship lane for making
long-running CodeWhale work easier to continue, review, and hand off without
turning the README into release notes.

The v0.9 track keeps the same DeepSeek-first harness and adds work in these
areas:

| Track | What is changing |
| --- | --- |
| Relay and continuity | `/relay`, fork-state handoff, and rich PlanArtifact context preserve the goal, why it matters, evidence, constraints, blockers, changed files, verification state, and the next action. |
| Transcript calmness | Dense read/search/list-style tool runs can collapse into expandable groups, while failures, running work, shell commands, writes, diffs, plans, and reviews stay visible. |
| Runtime sessions and workspaces | Branch work extends session/thread runtime APIs, including workspace-aware thread updates, completed-thread session saves, and safer guards around active turns. Treat this as v0.9-track capability until the release ships. |
| Sub-agent recovery | Live per-step timeout recovery can preserve checkpoint metadata and let `agent_eval { continue: true }` resume an interrupted child in the same runtime. Cold-restart continuation is still a follow-up; persisted child tasks are not rehydrated yet. |
| Project context stability | Bounded project-context packs and generated instructions keep large/noisy repositories from turning the first turn into an unbounded filesystem walk. |
| HarmonyOS / OHOS | The lane carries safe OpenHarmony setup, OHOS platform guards, self-update disablement on OHOS, and target gating for PTY and Starlark execpolicy paths. Full OHOS target builds still require a host with the OpenHarmony native SDK configured. |
| Nix and Starlark compatibility | Dependency stewardship keeps OHOS builds from pulling incompatible Nix-chain crates through PTY or Starlark paths where those features are gated. |
| HarnessProfile | The branch carries the typed `HarnessPosture` / `HarnessProfile` config data model and strict schema validation. Provider/model posture selection, prompt/tool/runtime behavior, telemetry, and docs remain follow-up work. |
| Contributor stewardship | Harvested PRs stay credited, contributor identity mapping is machine-readable, and community gates remain dry-run and human-toned while the branch is reviewed. |
| WhaleFlow | Typed branch/leaf workflows, deterministic replay, pod-style workflow monitoring, provider/model posture, and evidence-backed profile evolution remain the larger v0.9 workbench goal. |

The current execution map lives in
[docs/V0_9_0_EXECUTION_MAP.md](docs/V0_9_0_EXECUTION_MAP.md).

## Release Status

The latest published release line is still separate from the v0.9 integration
branch. v0.9.0 work in this README describes the current integration track, not
a published release artifact. Release-specific detail belongs in
[CHANGELOG.md](CHANGELOG.md); this README summarizes the current user-facing
surface and links to deeper docs.

Release channels can lag each other. Before making release claims, verify the
intended surface directly: GitHub Releases and checksums, npm `codewhale`,
Cargo crates, Docker/GHCR images, CNB mirrors, and any legacy Homebrew formula.
No tag, GitHub Release, npm/Cargo publish, Docker publish, or release artifact
push should happen without explicit maintainer approval.

## Quickstart

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

On first launch, CodeWhale prompts for a DeepSeek API key and saves it to
`~/.codewhale/config.toml`; the legacy `~/.deepseek/config.toml` path is still
read for compatibility. You can also set credentials directly:

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
```

Use `/provider`, `/model`, `/config`, `/statusline`, `/skills`, and `/restore`
inside the TUI. Prefix a composer line with `!` to run a shell command through
the normal approval and sandbox path, for example `! cargo test -p codewhale-tui`.

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

`codewhale update --proxy https://localhost:7897` routes update checks and
downloads through a proxy.

---

## Harness Model

A model answers a question. An agent finishes a task. The difference is the
harness: the rules, tools, evidence, and feedback that keep the model oriented
when user intent, repo instructions, tool output, stale memory, and prior
handoffs all compete inside one turn.

CodeWhale's harness has four practical parts:

| Part | What it does |
| --- | --- |
| Prompt constitution | `crates/tui/src/prompts/base.md` gives the model a stable authority hierarchy: live user intent beats stale instructions, live tool output beats assumptions, and verification beats confidence. |
| Typed tool surface | Shell, file, git, web, MCP, RLM, image, and sub-agent tools are registered with explicit schemas, visibility rules, and compatibility aliases. |
| Runtime evidence loop | Side-git snapshots, LSP diagnostics, command output, cost/cache accounting, and task state are fed back into the transcript instead of hidden behind the UI. |
| Approval and sandbox posture | Plan is read-only, Agent uses approval gates, and YOLO auto-approves in trusted workspaces. macOS Seatbelt is enforced; Linux Landlock is detected but not yet enforced; Windows sandboxing is not advertised. |

### Relay And Continuity

Relay is intentional compaction for human and agent handoff. Use `/relay` before
a long break, a fresh thread, a fork, or a handoff to another agent. It keeps the
important story small: the objective, why the work is being done, current state,
changed files, evidence checked, constraints, blockers, and the next concrete
action.

Automatic compaction protects context windows. Relay protects continuity. In
the v0.9 track, rich PlanArtifact fields feed the transcript card, Plan-mode
confirmation, `/relay`, fork-state handoff, and saved-session replay so the
plan, the evidence, and the next step do not become separate stories.

`codewhale` is the dispatcher CLI. `codewhale-tui` is the companion runtime
binary it launches for interactive sessions. The TUI talks to an async engine,
an OpenAI-compatible streaming client, the tool registry, the durable task
queue, the LSP subsystem, and optional HTTP/SSE or ACP servers. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full walkthrough.

### Auto Model Routing

`--model auto` is the default. Before the real turn is sent, CodeWhale makes a
small `deepseek-v4-flash` routing call with thinking off. That local router
selects the concrete model and thinking level for the real request:

- Model: `deepseek-v4-flash` or `deepseek-v4-pro`
- Thinking: `off`, `high`, or `max`

The upstream API never receives `model: "auto"`; it receives the concrete route
chosen for that turn. Use a fixed model or thinking level for repeatable
benchmarking, strict cost ceilings, or exact provider/model mapping.

### Sub-agents

Sub-agents run concurrently in the background. `agent_open` returns immediately;
the child receives its own context and tool registry, then reports back with a
completion sentinel and a human-readable summary. The full child transcript
stays behind a bounded handle that the parent can inspect through `agent_eval`.

Default concurrency is 10 and configurable up to 20. See
[docs/SUBAGENTS.md](docs/SUBAGENTS.md) for role taxonomy, lifecycle, wait/eval
tools, and transcript-handle details.

## Provider Routes

For the full provider registry, model IDs, auth variables, base URLs, and
capability boundaries, see [docs/PROVIDERS.md](docs/PROVIDERS.md).

Provider and model are deliberately separate choices. `provider` is the route,
account, endpoint, and credential source; `model` is the model ID on that route.
That distinction matters when the same model family appears through direct APIs
and OpenRouter aliases.

| Provider | Typical model IDs | Notes |
| --- | --- | --- |
| `deepseek` | `deepseek-v4-pro`, `deepseek-v4-flash` | Default direct DeepSeek route. |
| `openrouter` | `deepseek/deepseek-v4-pro`, `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3` | OpenRouter route; keep these IDs distinct from direct provider IDs. |
| `arcee` | `trinity-large-thinking`, `trinity-large-preview`, `trinity-mini` | Direct Arcee API at `https://api.arcee.ai/api/v1`. |
| `xiaomi-mimo` | `mimo-v2.5-pro`, `mimo-v2.5`, TTS IDs | Token Plan keys (`tp-...`) use `api-key` auth and default to the Token Plan endpoint; pay-as-you-go keys can set the MiMo API endpoint explicitly. |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro` | Uses NVIDIA account terms and model IDs. |
| `siliconflow` / `siliconflow-CN` | `deepseek-ai/DeepSeek-V4-Pro` | SiliconFlow global and China routes. |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` | Fireworks route. |
| `openai` | Your gateway's model ID | Generic OpenAI-compatible endpoint. |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro` | Hugging Face router route. |
| `sglang`, `vllm`, `ollama` | Local model IDs/tags | Self-hosted routes. |

```bash
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro

codewhale auth set --provider arcee --api-key "YOUR_ARCEE_API_KEY"
codewhale --provider arcee --model trinity-large-thinking

codewhale auth set --provider xiaomi-mimo --api-key "YOUR_XIAOMI_KEY"
codewhale --provider xiaomi-mimo --model mimo-v2.5-pro
codewhale --provider xiaomi-mimo speech "Hello from MiMo" --model tts -o hello.wav
XIAOMI_MIMO_TOKEN_PLAN_API_KEY="tp-..." XIAOMI_MIMO_MODE="token-plan-sgp" \
  codewhale --provider xiaomi-mimo --model mimo-v2.5-pro

codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" \
  codewhale --provider openai --model glm-5

SGLANG_BASE_URL="http://localhost:30000/v1" \
  codewhale --provider sglang --model deepseek-v4-flash
```

Inside the TUI, `/provider` opens the provider picker and `/model` opens the
model/thinking picker. `/models` fetches live API model lists when the active
provider supports listing.

## Platform Notes

Prebuilt binary pairs and platform archives are published for Linux x64, Linux
ARM64, macOS x64, macOS ARM64, and Windows x64. For other targets, see
[docs/INSTALL.md](docs/INSTALL.md).

For HarmonyOS PC and OpenHarmony cross-build setup, see [docs/HarmonyOS.md](docs/HarmonyOS.md).

### China / Mirror-friendly Installation

If GitHub or npm downloads are slow from mainland China, use
`npm install -g codewhale --registry=https://registry.npmmirror.com`, download
from GitHub Releases, or configure a Cargo registry mirror:

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

Then install both binaries:

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
```

Use `DEEPSEEK_TUI_RELEASE_BASE_URL` for mirrored release assets.

### Windows

The Scoop `codewhale` manifest can lag GitHub/npm/Cargo releases. Run
`scoop update` first, then verify with `codewhale --version`. Use npm or direct
GitHub release downloads when you need the newest release immediately.

### Remote-first Workspaces

For an always-on workspace you can control from a phone, use the Tencent-native
path: CNB mirror/source, Tencent Lighthouse HK, a Feishu/Lark long-connection
bridge, and optional EdgeOne for a deliberate public HTTPS edge. The runtime API
stays bound to localhost; EdgeOne is not used to expose `/v1/*`.

Start with [docs/TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md),
then use [docs/TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) for the
server runbook.

<details id="install-from-source">
<summary>Install from source</summary>

Works on any Tier-1 Rust target including musl, riscv64, FreeBSD, and older
ARM64 distros.

```bash
# Linux build deps (Debian/Ubuntu/RHEL):
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked
cargo install --path crates/tui --locked
```

Both binaries are required. Rust 1.88+ is required because the crates use the
2024 edition.

</details>

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

CodeWhale can run as a custom Agent Client Protocol server for editors that
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
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | Legacy stream idle timeout env override, default `300`, clamped to `1..=3600`; `[tui].stream_chunk_timeout_secs` takes precedence when configured |
| `CODEWHALE_PROVIDER` / `DEEPSEEK_PROVIDER` | `deepseek` (default), `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `volcengine`, `openrouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`, `siliconflow-CN`, `arcee`, `moonshot`, `sglang`, `vllm`, `ollama`, `huggingface` |
| `DEEPSEEK_PROFILE` | Config profile name |
| `DEEPSEEK_MEMORY` | Set to `on` to enable user memory |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | Allow non-local `http://` API base URLs on trusted networks |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY` / `OPENROUTER_API_KEY` / `XIAOMI_MIMO_TOKEN_PLAN_API_KEY` / `MIMO_TOKEN_PLAN_API_KEY` / `XIAOMI_MIMO_API_KEY` / `XIAOMI_API_KEY` / `MIMO_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SILICONFLOW_API_KEY` / `ARCEE_API_KEY` / `MOONSHOT_API_KEY` / `KIMI_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` / `HUGGINGFACE_API_KEY` / `HF_TOKEN` | Provider auth |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | Generic OpenAI-compatible endpoint and model ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud endpoint and model override |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark endpoint and model override |
| `VOLCENGINE_BASE_URL` / `VOLCENGINE_ARK_BASE_URL` / `ARK_BASE_URL` / `VOLCENGINE_MODEL` / `VOLCENGINE_ARK_MODEL` | Volcengine Ark endpoint and model override |
| `OPENROUTER_BASE_URL` | OpenRouter endpoint override |
| `XIAOMI_MIMO_BASE_URL` / `MIMO_BASE_URL` / `XIAOMI_MIMO_MODEL` / `MIMO_MODEL` / `XIAOMI_MIMO_MODE` / `MIMO_MODE` | Xiaomi MiMo endpoint, model, and Token Plan mode override; Token Plan default is `https://token-plan-sgp.xiaomimimo.com/v1` |
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

## Models & Cost Tracking

CodeWhale tracks the provider route, concrete model, prompt-cache hit/miss
estimate, input tokens, and output tokens for the turn that actually ran. Auto
mode is resolved before the upstream request, so the footer and session summary
charge against `deepseek-v4-flash`, `deepseek-v4-pro`, or the explicit provider
model selected for that turn.

Pricing changes over time and can vary by account, region, provider route, and
promotion. Use [docs/PROVIDERS.md](docs/PROVIDERS.md) for supported model IDs
and the provider's official pricing pages for billing decisions. Treat the TUI
cost display as a local estimate, not a receipt.

DeepSeek Platform defaults to `https://api.deepseek.com/beta` so beta-gated API
features can be tested without extra setup. Set `base_url =
"https://api.deepseek.com"` to opt out. Legacy aliases `deepseek-chat` /
`deepseek-reasoner` remain compatibility shims; prefer V4 model IDs for new
config. NVIDIA NIM variants use your NVIDIA account terms.

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
| [AGENT_ETHOS.md](docs/AGENT_ETHOS.md) | Maintainer and agent stewardship posture |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | Sub-agent role taxonomy and lifecycle |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | Full shortcut catalog |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | Release process |
| [LOCALIZATION.md](docs/LOCALIZATION.md) | UI locale matrix & switching |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | Ops & recovery |
| [V0_9_0_EXECUTION_MAP.md](docs/V0_9_0_EXECUTION_MAP.md) | v0.9.0 issue lanes, PR harvest state, and release gates |
| [2574-provider-fallback-chain.md](docs/rfcs/2574-provider-fallback-chain.md) | Provider fallback chain RFC |

Full Changelog: [CHANGELOG.md](CHANGELOG.md).

---

## Thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — thank you for the models and support that power every turn. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — thank you for your support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — thank you for prioritizing codewhale support and for collaborating on a better terminal-agent experience.
- **[Open Design](https://github.com/nexu-io/open-design)** — thank you for support and collaboration around design-forward agent workflows.

This project ships with help from a growing community of contributors. The
maintainer rule is simple: reports and PRs are real project work, even when the
final patch has to be narrowed, delayed, or harvested into a maintainer branch.

For the v0.9 track, harvested PRs should keep visible credit in the commit or
PR body, changelog or release notes, and relevant issue/PR comments. Contributor
credit should use mappable GitHub identities from `.github/AUTHOR_MAP` or
numeric noreply addresses, not placeholder local emails. The contribution gate
is kept in dry-run mode unless a maintainer deliberately enables enforcement;
when it comments, the tone should be warm and practical rather than treating
the reporter as the problem. Recurring contributors should be recognized so the
automation gets out of their way and the public record shows their repeated
help.

Current v0.9 track credits:

- **[xyuai](https://github.com/xyuai)** — canonical CodeWhale settings path,
  provider persistence, provider picker, logout-scope, and MiMo auth cleanup
  work (#2730, #2714, #2715, #2717, #2718)
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS /
  OpenHarmony porting work and MatePad Edge validation trail (#2634)
- **[sximelon](https://github.com/sximelon)** — saved-session resume footer
  hint work and provider-trait direction reviewed for the v0.9 track (#2758,
  #2760, #2479)
- **[aboimpinto](https://github.com/aboimpinto)** — sidebar command polish and
  pausable custom-command lifecycle direction harvested into the v0.9 track
  (#2788, #2732)
- **[AdityaVG13](https://github.com/AdityaVG13)** — WhaleFlow orchestration and
  cost-tracking drafts that shaped the maintained v0.9 WhaleFlow IR and
  TraceStore foundation (#2482, #2486)
- **[lbcheng888](https://github.com/lbcheng888)**,
  **[AiurArtanis](https://github.com/AiurArtanis)**, and
  **[nasus9527](https://github.com/nasus9527)** — VS Code extension scaffold
  direction, Agent View request, and IDE plugin request that shaped the
  official Phase 0 extension (#1022, #1584, #2580)
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — `web_run` cache-state
  lock-splitting, turn-metadata prefix-cache stability, and project-context
  cache work (#2502, #2517, #2636)
- **[idling11](https://github.com/idling11)** — PlanArtifact continuity,
  dense tool-call transcript collapse, sidebar detail popovers, and
  HarnessPosture provider/model policy direction (#2733, #2738, #2734,
  #2741, #2692, #2694, #2693)
- **[h3c-hexin](https://github.com/h3c-hexin)** — sub-agent model inheritance,
  configured `skills_dir` discovery, and prompt-environment stability work
  (#2736, #2737)
- **[gaord](https://github.com/gaord)** — runtime thread workspace updates and
  completed-thread saved-session API work (#2640, #2639)
- **[cyq1017](https://github.com/cyq1017)** — restore-listing and
  pending-input delivery-mode label work (#2513, #2532, #2054)
- **[NASLXTO](https://github.com/NASLXTO)** and
  **[wuxixing](https://github.com/wuxixing)** — large-workspace startup
  reports that shaped the bounded project-context fallback (#697, #1827)
- **[shuxiangxuebiancheng](https://github.com/shuxiangxuebiancheng)**,
  **[hongqitai](https://github.com/hongqitai)**, and
  **[cyq1017](https://github.com/cyq1017)** — third-party
  OpenAI-compatible path-suffix report and follow-up review trail (#1874,
  #2508, #2506)

Current and recurring contributors include:

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
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen
  logging, Home/End composer, runtime log follow-ups, sidebar command polish,
  and pausable command lifecycle work (#1774, #1776, #1748, #1749, #1782,
  #1783, #2788, #2732)
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
- **[tdccccc](https://github.com/tdccccc)** — approval prompt key-detail and shell-preview work harvested into the maintained approval path (#1991, #2269)
- **[AresNing](https://github.com/AresNing)** — first-run guide, message-submit hook transform design, and turn-end observer hook work harvested into the maintained hooks path (#2278, #2318, #2434, #2578)
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

CodeWhale gets a lot of good reports and PRs. The maintainer posture is to keep
that door open while protecting release quality:

- Issues should stay human-readable and actionable. Intake automation is
  advisory unless a maintainer deliberately enables enforcement.
- PRs are reviewed from code, tests, linked issues, and runtime behavior, not
  from title alone.
- If a PR is too broad to merge directly, maintainers may harvest the safe part
  into a narrower branch, then credit the author and explain what landed.
- Co-author trailers should use mappable GitHub noreply identities from
  `.github/AUTHOR_MAP`; reporters and repro authors should be thanked in
  changelogs, release notes, and closure comments.
- Recurring contributors can be added to `.github/APPROVED_CONTRIBUTORS` so
  dry-run gates stay out of their way.

Support: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!Note]
> *Not affiliated with DeepSeek Inc.*

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
