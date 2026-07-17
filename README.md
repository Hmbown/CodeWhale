# Codewhale

A coding agent for your terminal. Works with any model; open models first.

You give it a provider, a model, and a task. It reads code, edits files, runs
commands, checks the results, and keeps going until the task is done or it
needs you. TUI for interactive work, `codewhale exec` for scripts and CI.
Rust, MIT, runs entirely on your machine.

It started as `deepseek-tui`. The community that formed around it needed more
providers, so now DeepSeek, Claude, GPT, Kimi, GLM, and 30+ others run through
the same runtime and tools.

[简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![Codewhale TUI idle screen with GLM-5.2 selected through Z.ai](assets/screenshot.png)

## Install

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, prebuilt archives, Android/Termux, and a CNB mirror
for users who cannot reach GitHub are covered in
[docs/INSTALL.md](docs/INSTALL.md). Coming from `deepseek-tui`? Your config and
sessions carry over — see [docs/REBRAND.md](docs/REBRAND.md).

## Use

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
```

In the TUI: `/model` switches provider and model together, `/fleet` runs a
team of workers, `/restore` undoes a turn, `Tab` cycles Plan / Act / Operate,
`Shift+Tab` cycles the Ask / Auto-Review / Full Access approval posture, and
`!` runs a shell command through the normal approval path.

## What it does

- Resolves your provider + model choice to a concrete route: endpoint, wire
  protocol, context limit, price. Context budgets and cost display come from
  the real route; an unknown price shows as unknown, not $0.
  ([docs/PROVIDERS.md](docs/PROVIDERS.md))
- Talks to hosted open-model providers (`deepseek`, `openrouter`, `moonshot`,
  `zai`, `minimax`, `nvidia-nim`, …), to your own `vllm` / `sglang` / `ollama`
  with no key, and to Anthropic natively over the Messages API with thinking
  and prompt caching.
- Runs multiple workers durably: Fleet records work in an append-only ledger,
  so runs survive restarts and `fleet resume` picks up where things stopped.
  Workflow plans bigger jobs into resumable, verifiable lanes.
  ([docs/FLEET.md](docs/FLEET.md))
- Gates risk in code, not vibes: three modes (Plan is read-only), a separate
  approval posture, OS sandboxing (Seatbelt, Landlock + seccomp, bwrap),
  hooks that can allow/deny/ask per tool call, and side-git snapshots so
  `/restore` never touches your real history.
- Lets a repo declare its own law: `.codewhale/constitution.json` invariants
  compile into write holds that even Full Access can't skip.
  ([docs/CONFIGURATION.md](docs/CONFIGURATION.md))
- Speaks MCP in both directions, loads reusable skills, exposes HTTP/SSE and
  ACP runtime APIs, and backs a community
  [VS Code GUI](https://github.com/HengQuWorld/CodeWhale-VSCode).
- The TUI shows work as receipts you can inspect, keeps one live row moving,
  has a real context inspector, 12 themes, reduced-motion and ASCII-safe
  modes, and ships in English, 简体中文, 日本語, Tiếng Việt, Español,
  Português, 한국어, and partial 繁體中文.

Everything else — configuration, keybindings, sandbox details, architecture —
is in [docs](docs) and on [codewhale.net](https://codewhale.net/).

## Contributing

All feedback is a gift. Issues, PRs, repro steps, logs, feature requests, and
first contributions are all real project work here. When a PR can't merge
as-is, maintainers harvest what works and the author stays credited — in the
commit, the changelog, and [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md). If a
model or provider you use is missing, or something breaks on your machine,
telling us is the most useful thing you can do.

- [Open issues](https://github.com/Hmbown/CodeWhale/issues) — good first
  contributions live here
- [CONTRIBUTING.md](CONTRIBUTING.md) — dev setup and PR flow
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — everyone who has shaped this
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

Thanks to [DeepSeek](https://github.com/deepseek-ai) for the models and support
that started the project, [DataWhale](https://github.com/datawhalechina) 🐋 for
welcoming us into the Whale Brother family, and
[OpenWarp](https://github.com/zerx-lab/warp) and
[Open Design](https://github.com/nexu-io/open-design) for collaborating on the
terminal-agent experience.

## License

[MIT](LICENSE). Independent community project; not affiliated with any model
provider.

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
