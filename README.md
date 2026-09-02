# Codewhale

Codewhale is an open source coding agent for your terminal, built in Rust and
improved in public with the people who use it.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="brand/wordmark-inverted.svg">
  <img src="brand/wordmark.svg" alt="Codewhale" width="360">
</picture>

[简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/screenshot.webp">
  <img src="assets/screenshot.webp" alt="A Codewhale terminal session" width="720">
</picture>

## Install

```bash
npm install -g codewhale
codewhale
```

The first run helps you connect a provider or stay offline. Codewhale also
supports Cargo, Docker, Nix, Scoop, prebuilt archives, Android/Termux, and a CNB
mirror. See [the installation guide](docs/INSTALL.md).

Tab completion is one command per shell — `codewhale completion bash|zsh|fish|powershell|elvish`.
See [shell completions](docs/INSTALL.md#8-shell-completions).

## Use

Talk to Codewhale the same way you would talk to a teammate:

```text
Fix the failing tests and explain what changed.
```

Or run a task without opening the TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale can read your repository, edit files, run commands, inspect results,
and keep working toward a goal. You decide how much access it has.

## Why Codewhale

- **Use the model you want.** Connect hosted providers or local models through
  Ollama, vLLM, or SGLang. Switch provider and model with `/model`.
- **Stay in control.** Plan is read-only. Ask, Auto-Review, and Full Access make
  approval behavior visible. `/undo` reverts the last turn and `/restore`
  returns the workspace to an earlier snapshot.
- **Keep long work organized.** Save sessions, set a durable `/goal`, review
  workflows before they run, and coordinate agents without turning their
  internal instructions into your transcript.
- **Extend the agent you already have.** Connect MCP servers and skills,
  configure hooks, and keep agent roles as readable files in your project or
  personal settings.

Run `/help` in the TUI for commands and keyboard shortcuts.

## Safety

Codewhale runs on your machine with the access you grant it. Approval modes and
repository rules limit what the agent may do; optional OS sandboxing adds a
stronger execution boundary where supported. Unknown model prices stay unknown
instead of being reported as free.

Read [authorization order](docs/AUTHORIZATION_ORDER.md) for the exact policy
stack and [configuration](docs/CONFIGURATION.md) for local settings.

## Documentation

- [Providers and local models](docs/PROVIDERS.md)
- [Agent teams](docs/FLEET.md)
- [MCP](docs/MCP.md), [hooks](docs/HOOKS.md), and [configuration](docs/CONFIGURATION.md)
- [Local web client](docs/WEB.md)
- [All documentation](docs)

## Join the community

Codewhale gets better when people use it, report what feels wrong, and help fix
it. If a provider is missing, a workflow is awkward, or the terminal UI gets in
your way, [open an issue](https://github.com/Hmbown/CodeWhale/issues). If you
know how to improve it, [open a pull request](CONTRIBUTING.md). First
contributions are welcome, and contributors keep credit for the work that
lands.

Join the [Discord](https://discord.gg/37gfS3ksug), or add Hunter on WeChat
(`hunterbown`) and ask to join the Whale Brothers group.

## Project history

Codewhale began as `deepseek-tui` and still preserves that configuration and
session compatibility. It is now provider-neutral and independently maintained;
it is not affiliated with any model provider.

Thanks to every contributor and to the open source communities that helped the
project grow. See [the contributor record](docs/CONTRIBUTORS.md).

## License

[MIT](LICENSE). Portions adapted from other open-source projects are recorded
in [third-party notices](docs/THIRD_PARTY_NOTICES.md).
