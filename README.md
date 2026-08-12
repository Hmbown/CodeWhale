# Codewhale

An open source coding agent for your terminal — bring your own model.

Codewhale started as a native experience for DeepSeek. It has since grown into a
community-driven project: one coding harness that fits a growing international
community and supports as many models and providers as possible — open models
first, hosted or local, none privileged over the rest.

Give it a provider, a model, and a task. It reads your code, edits files, runs
commands, and checks its own work, then stops when the job is done or it needs
you. Switch models mid-task with `/model`. Work interactively in the TUI, or run
`codewhale exec` in scripts and CI. It's written in Rust, licensed MIT, and runs
on your machine.

The part that isn't like other harnesses: **you pick the model for each role,
and they don't have to match.** A fleet pins a provider, a model, and a
reasoning tier per role — so a cheap fast model can direct an expensive
reasoning one, or a GLM builder can work the same job as a Kimi reviewer.
Write your own roles, your own constitution, and the harness is yours rather
than ours.

We're always looking for contributors and ways to improve. If a model or
provider you use is missing, or something breaks, telling us is one of the most
useful things you can do — see [Contributing](#contributing).

[简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![Codewhale running in a terminal](assets/screenshot.png)

## Install

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, prebuilt archives, Android/Termux, and a CNB mirror
for anyone who can't reach GitHub are covered in
[docs/INSTALL.md](docs/INSTALL.md). Coming from `deepseek-tui`? Your config and
sessions carry over — see [docs/REBRAND.md](docs/REBRAND.md).

## Use

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```

In the TUI: `/model` switches provider and model together, `/fleet` builds and
runs the team — one role at a time, each with its own model — `/undo` reverts
the last turn, and `/restore <N>` rolls the workspace back to an earlier
snapshot (bare `/restore` lists them). `Tab`
cycles Plan / Work / Operate when the composer is empty — with text in it, `Tab`
completes slash commands and `@` mentions instead. `Shift+Tab` cycles the
Ask / Auto-Review / Full Access permission posture at any time. `!` runs a
shell command through the normal approval path.

## What it does

- **Any model, any provider — and any mix of them.** DeepSeek, Claude, GPT,
  Kimi, GLM, and 30+ providers, plus your own vLLM, SGLang, or Ollama with no
  key, all through one runtime and one toolset. A saved role records its
  `provider`, `model`, and reasoning tier explicitly, so a fleet can span
  vendors in a single run and a role's route never depends on whichever
  provider happens to be active. Context limits and prices come from the real
  route, and an unknown price shows as unknown rather than $0.
- **A harness you author.** Roles are files you can read and edit — a model, a
  tool posture, and standing instructions per role — kept in the project so the
  team shares them, or beside your other personal settings so they follow you
  between repos. A constitution records how you want the agent to behave across
  every session, so the harness matches your practice instead of ours.
- **Read-only until you allow more.** Plan mode can't change files, and
  approvals gate risky commands. When an OS sandbox actually wraps a command,
  Codewhale says so: Seatbelt on macOS where available, opt-in bubblewrap on
  Linux. A repo's `constitution.json` compiles into write holds that even Full
  Access can't skip.
- **Work you can resume.** A fleet records every step to an append-only ledger,
  so `fleet resume` picks up where you left off.

## Learn more

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — every provider route: hosted,
  gateway, and local
- [docs/FLEET.md](docs/FLEET.md) — fleets, the ledger, and resume
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — frozen, provider-neutral experimental search within Workflow
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, hooks, and
  the constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — how modes,
  hooks, permission rules, safety floors, repo law, approvals, and sandboxing
  compose
- [docs/HOOKS.md](docs/HOOKS.md) — the eleven TUI lifecycle hook events, their
  payloads, and which three of them can steer a turn (`codewhale exec` and the
  CLI subcommands do not fire hooks)
- [docs/WEB.md](docs/WEB.md) — the loopback-only browser client and its one-time
  authentication boundary

Everything else — modes, keybindings, sandbox details, MCP, the runtime API,
and architecture — lives in [docs](docs) and on
[codewhale.net](https://codewhale.net/).

## Contributing

Issues, PRs, repro steps, logs, and feature requests are all real project work,
and first contributions are welcome. When a PR can't merge as-is, maintainers
harvest what works and keep the author credited — in the commit, the changelog,
and [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md).

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

[MIT](LICENSE). An independent community project, not affiliated with any model
provider.

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
