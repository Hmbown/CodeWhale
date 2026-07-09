# CodeWhale

> The terminal coding agent for any model — open models first.

CodeWhale is a terminal coding agent — a TUI and a CLI. You point it at a model
and a project, and it gets to work: reading code, making edits, running
commands, checking results, planning multi-step tasks, and correcting itself
when something fails.

It's open source (MIT, Rust), it runs on your machine, and it works with the
models people actually use. DeepSeek and open-weight models are first-class, and
a local vLLM/SGLang/Ollama box on your LAN needs no key at all — but Claude, GPT,
Kimi, and GLM are full peers through the same runtime and the same tools. You
pick a provider and a model; CodeWhale resolves a real route and runs.

The project began as `deepseek-tui`, a coding harness built around DeepSeek
workflows. The developer community — much of it in China — adopted it, filed
reports, and contributed fixes, and it became clear the harness was bigger than
one model. Multi-provider support followed, and the project became CodeWhale to
match. If there's a model, endpoint, or feature you don't see that you want,
open an issue — that's how the project grows.

[简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md) · [한국어 README](README.ko-KR.md) · [codewhale.net](https://codewhale.net/) · [Install guide](docs/INSTALL.md) · [Provider registry](docs/PROVIDERS.md) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![Security audit](https://github.com/Hmbown/CodeWhale/actions/workflows/security-audit.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/security-audit.yml)
[![cargo-deny](https://github.com/Hmbown/CodeWhale/actions/workflows/cargo-deny.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/cargo-deny.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![CodeWhale running in a terminal](assets/screenshot.png)

## Install

```bash
npm install -g codewhale
codewhale --version   # 0.8.68
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
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.68 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.68 codewhale-tui --locked --force

# Legacy Homebrew compatibility while the formula is renamed
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Prebuilt archives for Linux x64/arm64, macOS x64/arm64, and Windows x64 are
attached to [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases).
Android / Termux is a separate Android arm64 target, not the Linux arm64 asset.
Linux riscv64 prebuilts are temporarily paused while upstream QuickJS bindings
catch up. Checksums, Termux notes, China mirrors, Windows specifics, and
troubleshooting live in [docs/INSTALL.md](docs/INSTALL.md).

**Upgrading from the legacy `deepseek-tui` package?** Your config, sessions,
skills, and MCP settings are preserved. See [docs/REBRAND.md](docs/REBRAND.md),
then run `codewhale doctor` to confirm.

## First run

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Every provider is the same one-line shape: `--provider openrouter`,
`--provider moonshot`, `--provider openmodel`, or point `vllm`, `sglang`, or `ollama` at your own
localhost runtime with no key at all. Have a Claude key instead? Run
`codewhale auth set --provider anthropic` — or just export
`ANTHROPIC_API_KEY` — and the native Messages adapter takes it from there.

Keys land in `~/.codewhale/config.toml`; legacy `~/.deepseek/` config is still
read for compatibility.

Useful in-session commands:

- `/provider` opens the readiness dashboard — per provider it shows auth state,
  the resolved default route, and the cost/usage meter. `/model` picks the model
  and reasoning effort. Both also take arguments (`/provider nvidia-nim`,
  `/model auto`) to switch mid-session.
- `/restore` rolls back a prior turn from side-git snapshots.
- `/fleet` opens the Fleet setup view — roles, profiles, loadouts, and policy.
- `/skills` loads reusable workflows from `~/.codewhale/skills/`.
- `/config` edits runtime settings; `/statusline` chooses which footer chips
  show route, cost, and session state.
- `! cargo test -p codewhale-tui` runs any shell command through the normal
  approval and sandbox path.

Headless, for scripts and CI:

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## Providers and routing

You pick a provider and a model, and CodeWhale resolves a **real route** — a
concrete endpoint, wire protocol, model ID, context limit, and price — instead
of just swapping a base URL. A `RouteResolver` is the only thing that can mint a
resolved route, so the same selection logic backs the TUI picker, the CLI, and
headless runs. The catalog behind it is a committed, network-free snapshot in
the Models.dev shape, optionally refreshed from a provider's live `/models`
endpoint.

Because the route is resolved, the rest of the harness can be honest about it:

- **Route-aware context budgets.** The compaction threshold and usable window
  come from the resolved route's real context limit, not a hardcoded guess.
- **Honest cost display.** A route reports exactly one cost state: per-token
  pricing, a subscription/quota meter, account credits, *local / not
  applicable*, or *unknown / stale*. CodeWhale never invents a price it doesn't
  have — an unmatched model shows as unknown rather than $0.
- **Explicit wire protocol.** Whether a route speaks Chat Completions, the
  OpenAI Responses API, or native Anthropic Messages is carried on the resolved
  route, not inferred from a prompt. Reasoning effort is translated into each
  provider's own dialect.

Switch the route mid-session with `/provider` and `/model`. The full registry —
credentials, base URLs, capability boundaries — lives in
[docs/PROVIDERS.md](docs/PROVIDERS.md).

### Supported providers

Every provider routes through the same runtime and the same tools. If the one
you want isn't here, that's a good issue to open.

- **Open models, hosted:** `deepseek` (the default), `openrouter`,
  `huggingface` (Inference Providers), `moonshot` (Kimi), `zai` (GLM),
  `minimax`, `volcengine` (Ark), `nvidia-nim`, `together`, `fireworks`,
  `novita`, `siliconflow` / `siliconflow-CN`, `arcee`, `xiaomi-mimo`,
  `openmodel`, `deepinfra`, `stepfun`, `atlascloud`, `qianfan`, `wanjie-ark`, plus a generic
  `openai`-compatible route for any gateway.
- **Open models, self-hosted:** `vllm`, `sglang`, and `ollama` against your own
  localhost endpoints — no key required.
- **Closed providers, natively:** `anthropic` through a dedicated
  `/v1/messages` adapter with adaptive thinking, prompt-cache breakpoints, and
  signed-thinking replay; `deepseek-anthropic`, DeepSeek's opt-in Messages-API
  route; and `openai-codex` (experimental), which reuses an existing
  ChatGPT/Codex CLI login instead of an API key.

## Fleet

Fleet is CodeWhale's durable control plane for multi-worker runs. A fleet worker
is a headless `codewhale exec` run, but the fleet launches and tracks it durably:
work is recorded in an append-only ledger (`.codewhale/fleet.jsonl`), so a run
survives a manager exit, laptop sleep, or a runtime restart.

```bash
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet resume <run-id>
```

`fleet resume` replays the ledger, reconciles any in-flight task whose worker
stopped heartbeating (retrying within budget, else failing and escalating), and
is idempotent — safe to run after anything that interrupted the manager. Each
worker records a typed receipt (`pass` / `fail` / `partial` / `skip` /
`timeout`) so `fleet status` can report what actually happened.

Workers are shaped by **roles**, **profiles**, **loadouts**, and **slots**,
configured under `[fleet]` in your config or authored from the in-app Fleet
setup view. Loadouts express model intent as a class — `strong`, `balanced`, or
`fast` — and the route resolver turns that into a concrete provider/model. This
is the same headless runtime that backs in-session sub-agents; Fleet is the
durable layer on top. See [docs/FLEET.md](docs/FLEET.md).

### Workflow

Workflow is the resumable, inspectable layer for larger jobs. It plans a goal
into bounded steps, can fan those steps out through Fleet workers, and records
progress and verification in `.codewhale/workflow-runs.jsonl`:

```bash
codewhale workflow run my-workflow.js --verify
codewhale workflow status
codewhale workflow logs <run-id>
```

Workflow is an overlay, not another permission mode: Plan / Act / Operate and
the Ask / Auto-Review / Full Access posture stay orthogonal to orchestration.

## Safety

CodeWhale edits files and runs commands, so the safety posture is part of the
product, not an afterthought.

- **Three modes.** Plan (read-only investigation), Act (multi-step execution),
  and Operate (Fleet/Workflow orchestration). Switch with `Tab` or `/mode`.
  `Shift+Tab` independently cycles Ask, Auto-Review, and Full Access approval
  posture; legacy YOLO remains a compatibility alias for Act + Full Access.
- **Approval-gated tools.** A `.codewhale/hooks.toml` hook system can allow,
  deny, or ask before any tool call, and the exec policy decides whether a
  command runs, needs approval, or is forbidden outright.
- **OS sandboxing.** Seatbelt on macOS, Landlock plus a seccomp syscall filter
  on Linux, and bubblewrap (bwrap) where it's available.
- **Rollback.** Side-git snapshots live outside your repo's `.git`, so
  `/restore` can undo a turn without ever touching your real history.

## Features

- **Persistent goal loop.** Set an objective with `/goal` and the agent keeps
  working across turns — reading, editing, running, checking results — until the
  goal is done, it's blocked, or you stop it. No turn cap. `/task` tracks
  background tasks; the Work sidebar shows live plan and checklist state.
- **Durable sessions.** Persist across restarts and system sleep; a task that
  takes forty tool calls survives the forty-first.
- **Headless mode.** `codewhale exec` with `--allowed-tools`,
  `--disallowed-tools` (deny wins), `--max-turns`, and `--append-system-prompt`
  for scripts and CI.
- **MCP, bidirectionally.** Consume tools from external MCP servers, or expose
  CodeWhale itself as an MCP server via `codewhale mcp`.
- **Skills.** Reusable workflows in `~/.codewhale/skills/`, loaded with
  `/skills`.
- **Embedded everywhere.** HTTP/SSE and ACP runtime APIs, a VS Code extension,
  and Telegram/Feishu bridges (Weixin experimental).

## CodeWhale for VS Code — GUI frontend

Prefer a graphical IDE experience over the terminal? [**CodeWhale for VS Code**](https://github.com/HengQuWorld/CodeWhale-VSCode) is a community-maintained GUI frontend that wraps the same CodeWhale engine into a native VS Code sidebar — chat, slash commands, threaded conversations, live diffs, task management, and a settings UI, all without leaving the editor.

The GUI talks to the same local `codewhale` runtime over the [Runtime API](docs/RUNTIME_API.md), so sessions, providers, modes, and skills stay in sync between terminal and IDE. If you live in VS Code, give it a try:

```bash
npm install -g codewhale        # install the engine first
# then search "CodeWhale" in the VS Code extensions panel
```

> The minimal scaffold under [`extensions/vscode/`](extensions/vscode/) in this
> repo is a separate, read-only Phase 0 viewer. For the full chat experience,
> use the linked GUI project above.

## How instructions are ranked

As a project evolves, the instructions pile up and they inevitably conflict: the
original spec, a later refactor that contradicts it, stale memory, a previous
agent's handoff, your current request, and fresh test output that doesn't match
what the handoff claimed. A flat system prompt makes the model resolve that by
guess. CodeWhale uses a **nested constitution** so there's a defined rank instead
of vibes.

The system prompt is layered, most-static first, and the order is enforced in
code (there are tests asserting it can't drift):

1. **Bundled global Constitution** — the base law, compiled into every binary.
   Its priority article fixes the authority order for any conflict.
2. **Your user-global constitution** — managed through `/constitution` and
   `/setup`, saved as structured data under `$CODEWHALE_HOME/constitution.json`,
   and rendered into a separate model-facing prose block. It is normal guided
   setup output, not a raw prompt editor.
3. **Your project's law** — drop a `.codewhale/constitution.json` in a repo to
   declare `protected_invariants`, `branch_policy`, `verification_policy`, and
   `escalate_when`. It's loaded as its own repo-local authority block, above
   project instructions, memory, and handoffs. A `protected_invariants` entry
   that carries path globs is not just prose: it compiles into a mechanical,
   tighten-only write hold in the tool gate (`ask` force-prompts even in YOLO,
   `block` denies) with a receipt naming the invariant. See
   [Configuration](docs/CONFIGURATION.md#enforced-repo-law-invariants).
4. **Project instructions** — `AGENTS.md` and compatibility fallbacks explain
   how agents should work in this repo.
5. **Memory and handoffs** — useful recalled state, lower authority than
   constitution layers and project instructions.

Your current request and live tool evidence still control the active turn: the
model may be given many layers, but it may never report a fact that the tools did
not return. Runtime approval, sandbox, network, and trust controls are enforced
in code and are not changed by constitution text.

There is also an expert-only full base-prompt override at
`$CODEWHALE_HOME/prompts/constitution.md` behind an explicit opt-in flag. It is
not the normal guided setup path. When two instructions conflict, each yields to
the higher authority layer. Because the law lives in the harness, not the model,
swapping models keeps the structure intact.

## Where details live

The README is the short version. The rest is in docs and on
[codewhale.net](https://codewhale.net/):

- [User guide](docs/GUIDE.md) · [Install guide](docs/INSTALL.md) ·
  [Configuration](docs/CONFIGURATION.md) · [Provider registry](docs/PROVIDERS.md)
- [Modes](docs/MODES.md) — Plan, Act, Operate, and orthogonal approval posture.
- [Fleet](docs/FLEET.md) · [Sub-agents](docs/SUBAGENTS.md) — roles, lifecycle,
  output contract, and recovery behavior.
- [Architecture](docs/ARCHITECTURE.md) — crate layout, runtime flow, tool system,
  extension points, and security model.
- [Automatic Workflows](docs/AUTOMATIC_WORKFLOWS.md) · [Workflow authoring](docs/WORKFLOW_AUTHORING.md) · [MCP](docs/MCP.md) ·
  [Runtime API](docs/RUNTIME_API.md) · [Model Lab](docs/MODEL_LAB.md)
- [Keybindings](docs/KEYBINDINGS.md) · [Sandbox & approvals](docs/SANDBOX.md)
  · [Accessibility](docs/ACCESSIBILITY.md) · [Docker](docs/DOCKER.md)
  · [Memory](docs/MEMORY.md)
- [Full docs index](docs) — everything else.

## The project

CodeWhale started as one person's DeepSeek side project. Developers from
countries all over the world have made it what it is — the contributor list on
every release is the proof. The project is built in the open, issues are triaged
in the open, and releases cut from `main`.

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
