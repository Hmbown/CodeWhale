# CodeWhale

> An open source terminal coding agent, built to bring the best available models
> to as many people as possible.

CodeWhale is a terminal coding agent — a TUI and a CLI. You point it at a model
and a project, and it gets to work: reading code, making edits, running
commands, checking results, planning multi-step tasks, and correcting itself
when something fails.

It's open source (MIT, Rust), it runs on your machine, and it works with the
models people actually use. DeepSeek and open-weight models are first-class,
but Claude, GPT, Kimi, and a local vLLM/Ollama box on your LAN are all full
peers. The goal is simple: stay current with the best research and features in
commercial coding agents, and surpass them.

Developers from all over the world have shaped CodeWhale into what it is. If
there's a model, endpoint, or feature you don't see that you want, open an issue
— that's how the project grows.

[简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md) · [codewhale.net](https://codewhale.net/) · [Install guide](docs/INSTALL.md) · [Provider registry](docs/PROVIDERS.md) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![CodeWhale running in a terminal](assets/screenshot.png)

## Install

```bash
npm install -g codewhale
codewhale --version   # 0.8.63
```

The npm wrapper (Node 18+) downloads SHA-256-verified binaries from GitHub
Releases and installs `codewhale`, `codew`, and `codewhale-tui`. Prefer building
from source? Use cargo (Rust 1.88+):

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

> **Linux users:** install system build dependencies first:
> `sudo apt-get install -y build-essential pkg-config libdbus-1-dev`.
> See [INSTALL.md](docs/INSTALL.md#4-install-via-cargo-any-tier-1-rust-target).

Every other path:

```bash
# Docker
docker pull ghcr.io/hmbown/codewhale:latest

# Nix
nix run github:Hmbown/CodeWhale

# Windows
scoop install codewhale        # or the NSIS installer from GitHub Releases

# CNB mirror for users who cannot reliably reach GitHub
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.63 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.63 codewhale-tui --locked --force

# Legacy Homebrew compatibility while the formula is renamed
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Prebuilt archives for every platform — including Linux riscv64 — are attached
to [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases). Checksums,
China mirrors, Windows specifics, and troubleshooting live in
[docs/INSTALL.md](docs/INSTALL.md).

**Upgrading from the legacy `deepseek-tui` package?** Your config, sessions,
skills, and MCP settings are preserved. See [docs/REBRAND.md](docs/REBRAND.md),
then run `codewhale doctor` to confirm.

## First Run

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Every provider is the same one-line shape: `--provider openrouter`,
`--provider moonshot`, or point `vllm`, `sglang`, or `ollama` at your own
localhost runtime with no key at all. Have a Claude key instead? Run
`codewhale auth set --provider anthropic` — or just export
`ANTHROPIC_API_KEY` — and the native Messages adapter takes it from there.

Keys land in `~/.codewhale/config.toml`; legacy `~/.deepseek/` config is still
read for compatibility.

Useful in-session commands:

- `/provider` and `/model` switch the route and model mid-session.
- `/restore` rolls back a prior turn from side-git snapshots.
- `/skills` loads reusable workflows from `~/.codewhale/skills/`.
- `/config` edits runtime settings; `/statusline` shows the current route,
  cost, and session state.
- `! cargo test -p codewhale-tui` runs any shell command through the normal
  approval and sandbox path.

Headless, for scripts and CI:

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## The models

Twenty-five providers route through the same harness and the same tools. If the
one you want isn't here, that's a good issue to open.

- **Open models, hosted:** `deepseek` (first among equals), `openrouter`,
  `huggingface` (Inference Providers), `moonshot` (Kimi — OAuth temporarily
  broken), `zai` (GLM — recommended), `minimax`, `volcengine` (Ark),
  `nvidia-nim`, `together`, `fireworks`, `novita`, `siliconflow` /
  `siliconflow-CN`, `arcee`, `xiaomi-mimo`, `deepinfra`, `stepfun`,
  `atlascloud`, `wanjie-ark`, plus a generic `openai`-compatible route for any
  gateway.
- **Open models, self-hosted:** `vllm`, `sglang`, and `ollama` against your own
  localhost endpoints — no key required.
- **Closed providers, natively:** `anthropic` through a dedicated
  `/v1/messages` adapter with adaptive thinking, prompt-cache breakpoints, and
  signed-thinking replay — and `openai-codex`, which reuses an existing
  ChatGPT/Codex CLI login (working).

Routing is more than a base URL swap: `/reasoning` effort is translated into
each provider's wire dialect, sub-agent tiers resolve per provider, and the
system prompt's model facts are templated per-model instead of hardcoded.
Switch mid-session with `/provider` and `/model`. The full registry —
credentials, base URLs, capability boundaries — lives in
[docs/PROVIDERS.md](docs/PROVIDERS.md).

Sub-agent fanout is config-first. Set global `[subagents]` defaults, then add
`[subagents.providers.deepseek]`, `[subagents.providers.glm]`,
`[subagents.providers.openrouter]`, or other provider profiles to match the API
you are actually using. Direct DeepSeek can stay wide; subscription or
rate-limited routes can stay at 3-5 concurrent agents without changing prompts
or code. See [docs/SUBAGENTS.md](docs/SUBAGENTS.md#concurrency-cap).

Atlas Cloud is included as an OpenAI-compatible hosted route for users who want
its curated catalog behind one key: set `DEEPSEEK_PROVIDER=atlascloud`,
`ATLASCLOUD_API_KEY`, and optionally `ATLASCLOUD_MODEL`, for example
`deepseek-ai/deepseek-v4-pro`. Atlas model IDs pass through as selected; use
Atlas's model catalog or Coding Plan page for the current list and pricing.

## What makes CodeWhale different

As a project evolves, the instructions pile up and they inevitably conflict: the
original spec, a later refactor that contradicts it, stale memory, a previous
agent's handoff, your current request, and fresh test output that doesn't match
what the handoff claimed. A flat system prompt makes the model resolve that by
guess. CodeWhale uses a **nested constitution** so there's a defined rank instead
of vibes.

The system prompt is layered, most-static first, and the order is enforced in
code (there are tests asserting it can't drift):

1. **Global constitution** — the base law, compiled into every binary. Its
   priority article fixes the authority order for any conflict.
2. **Your project's law** — drop a `.codewhale/constitution.json` in a repo to
   declare `protected_invariants`, `branch_policy`, `verification_policy`, and
   `escalate_when`. It's loaded as its own authority block, above memory and
   handoffs.
3. **Your current request** — the operative instruction this turn.
4. **Live evidence** — what the tools actually returned. Ground truth; the model
   may be ordered past it, but it may never report a fact that isn't there.

When two instructions conflict, each yields to the one above. The model isn't
renegotiating the stack each turn — the order is fixed, so it can act on the
mountain of overlapping context without being paralyzed or quietly wrong. And
because the law lives in the harness, not the model, swapping models keeps the
structure intact.

## Features

- **Three modes.** Plan (read-only investigation), Agent (executes, asks per
  action), YOLO (auto-approve). Switch with `Tab` or `/mode`.
- **Persistent goal loop.** Set an objective with `/goal` and the agent keeps
  working across turns — reading, editing, running, checking results — until the
  goal is done, it's blocked, or you stop it. No turn cap. `/task` tracks
  background tasks; the Work sidebar shows live plan and checklist state.
- **Sub-agents.** Independent investigations and implementation slices run in
  parallel with provider-specific fanout caps, clean context, and
  provider-aware model tiers (big vs. cheap).
- **25 providers.** DeepSeek, GLM, Claude, GPT, Kimi, MiniMax, OpenRouter, and
  local vLLM/SGLang/Ollama, all behind the same harness and tools. Switch
  mid-session with `/provider` and `/model`.
- **Rollback.** Side-git snapshots and `/restore`, kept outside your repo's
  `.git` — undoing a turn never touches your history.
- **Sandboxing & approval gates.** OS sandboxing (bwrap, Landlock, Seatbelt,
  seccomp) and a `.codewhale/hooks.toml` hook system that can allow, deny, or ask
  before any tool call.
- **Durable sessions.** Persist across restarts and system sleep; a task that
  takes forty tool calls survives the forty-first.
- **Headless mode.** `codewhale exec` with `--allowed-tools`, `--disallowed-tools`
  (deny wins), `--max-turns`, and `--append-system-prompt` for scripts and CI.
- **MCP, bidirectionally.** Consume tools from external servers, or expose
  CodeWhale itself as an MCP server via `codewhale mcp`.
- **Skills.** Reusable workflows in `~/.codewhale/skills/`, loaded with `/skills`.
- **Embedded everywhere.** HTTP/SSE and ACP runtime APIs, a VS Code extension,
  and Telegram/Feishu bridges (Weixin experimental).

## The project

CodeWhale started as one person's DeepSeek side project. Developers from
countries all over the world have made it what it is — the contributor list on
every release is the proof. The project is built in the open, issues are
triaged in the open, and releases cut from `main`.

Something I learned early in teaching: **all feedback is a gift.** Issues, PRs,
bug reports, feature ideas, "first PR"s, and curious questions all count as real
project work. Maintainers treat every report as a contribution even when the
final patch has to be narrowed, delayed, or folded into a maintainer commit —
and recurring contributors stay credited in the public record. If you hit
something that doesn't work, or you want a model that isn't listed, that's the
most useful thing you can tell the project.

- [Open issues](https://github.com/Hmbown/CodeWhale/issues) — good first
  contributions live here.
- [CONTRIBUTING.md](CONTRIBUTING.md) — set up a dev loop and open a PR.
- [Code of Conduct](CODE_OF_CONDUCT.md) — be excellent to each other.
- [Contributors](docs/CONTRIBUTORS.md) — the people who've shaped CodeWhale.

Support: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

## Where details live

The README is the short version. The rest is in docs and on
[codewhale.net](https://codewhale.net/):

- [User guide](docs/GUIDE.md) · [Install guide](docs/INSTALL.md) ·
  [Configuration](docs/CONFIGURATION.md) · [Provider registry](docs/PROVIDERS.md)
- [Modes](docs/MODES.md) — Agent, Plan, and YOLO.
- [Sub-agents](docs/SUBAGENTS.md) — roles, lifecycle, output contract, and
  recovery behavior.
- [Architecture](docs/ARCHITECTURE.md) — crate layout, runtime flow, tool system,
  extension points, and security model.
- [Fleet](docs/FLEET.md) · [WhaleFlow authoring](docs/WHALEFLOW_AUTHORING.md) ·
  [MCP](docs/MCP.md) · [Runtime API](docs/RUNTIME_API.md) ·
  [Model Lab](docs/MODEL_LAB.md)
- [Keybindings](docs/KEYBINDINGS.md) · [Sandbox & approvals](docs/SANDBOX.md)
  · [Accessibility](docs/ACCESSIBILITY.md) · [Docker](docs/DOCKER.md)
  · [Memory](docs/MEMORY.md)
- [Full docs index](docs) — everything else.

## Thanks

CodeWhale exists because of the people who use it, break it, and fix it.

- **[DeepSeek](https://github.com/deepseek-ai)** — the models and support that
  got this project started. 感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — for the support and for
  welcoming us into the Whale Brother family. 感谢 DataWhale 的支持。
- **[OpenWarp](https://github.com/zerx-lab/warp)** and
  **[Open Design](https://github.com/nexu-io/open-design)** — for collaborating
  on a better terminal-agent experience.
- **Every contributor** — the full per-PR record lives in
  [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md). Thank you.

## License

[MIT](LICENSE)

> *CodeWhale is an independent community project and is not affiliated with any
> model provider.*

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
