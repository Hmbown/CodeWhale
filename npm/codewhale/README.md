# codewhale

Install and run CodeWhale, the agentic terminal for DeepSeek and other
OpenAI-compatible coding models, from GitHub release artifacts.

This npm package is a small launcher: it downloads the matching native
CodeWhale binaries for your platform and exposes the `codewhale` and
`codewhale-tui` commands. The application state and credentials still live in
CodeWhale's normal config files, not inside `node_modules`.

> Previously published as `deepseek-tui`. See `docs/REBRAND.md` in the upstream
> repository for the migration notes; the legacy `deepseek-tui` npm package is
> deprecated and receives no further releases.

## Install

```bash
npm install -g codewhale
# or
pnpm add -g codewhale
```

For project-local usage:

```bash
npm install codewhale
npx codewhale --help
```

`postinstall` tries to download platform binaries into `bin/downloads/`. If
GitHub release assets are temporarily unreachable, install continues and the
wrapper retries the download on first run.

## First run

```bash
codewhale login --api-key "YOUR_DEEPSEEK_API_KEY"
codewhale doctor
codewhale
```

The `codewhale` facade and `codewhale-tui` binary share
`~/.codewhale/config.toml` for DeepSeek auth and default model settings. Legacy
`~/.deepseek/config.toml` installs are still read as a compatibility fallback.
Common TUI commands are available directly through the facade, including
`codewhale doctor`, `codewhale models`, `codewhale sessions`, and
`codewhale resume --last`.

The app talks to DeepSeek's documented OpenAI-compatible Chat Completions API.
Set `DEEPSEEK_BASE_URL` only if you need the China endpoint or DeepSeek beta
features such as strict tool mode, chat prefix completion, or FIM completion.

NVIDIA NIM-hosted DeepSeek V4 Pro is also supported:

```bash
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim
```

For a single process, set `DEEPSEEK_PROVIDER=nvidia-nim` and `NVIDIA_API_KEY`
or `NVIDIA_NIM_API_KEY` (with `DEEPSEEK_API_KEY` as a compatibility fallback).
The NIM default model is `deepseek-ai/deepseek-v4-pro` and the default base URL
is `https://integrate.api.nvidia.com/v1`. With `--provider nvidia-nim`,
`--model deepseek-v4-flash` maps to `deepseek-ai/deepseek-v4-flash`.

## Supported platforms

Prebuilt binaries for the GitHub release are downloaded automatically:

- Linux x64
- Linux arm64 (v0.8.8+)
- macOS x64 / arm64
- Windows x64

Other platform/architecture combinations (musl, riscv64, FreeBSD, …) aren't
shipped as prebuilts. Unsupported platforms, checksum failures, and glibc
compatibility problems still fail with a clear error pointing you at
`cargo install codewhale-cli codewhale-tui --locked` and the full
[docs/INSTALL.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/INSTALL.md)
build-from-source guide.

## Wrapper configuration

| Setting | What it does |
| --- | --- |
| `codewhaleBinaryVersion` in `package.json` | Default native binary version. `deepseekBinaryVersion` is still read as a backward-compat fallback. |
| `DEEPSEEK_TUI_VERSION` or `DEEPSEEK_VERSION` | Override the GitHub release version to download. |
| `DEEPSEEK_TUI_GITHUB_REPO` or `DEEPSEEK_GITHUB_REPO` | Override the source repo. Defaults to `Hmbown/CodeWhale`. |
| `DEEPSEEK_TUI_RELEASE_BASE_URL` | Use an internal or mirrored release-asset directory when GitHub Releases is unavailable. The directory must contain `codewhale-artifacts-sha256.txt` and the platform binaries. |
| `DEEPSEEK_TUI_FORCE_DOWNLOAD=1` | Force download even when the cached binary is already present. |
| `DEEPSEEK_TUI_DISABLE_INSTALL=1` | Skip install-time download. |
| `DEEPSEEK_TUI_OPTIONAL_INSTALL=1` | Make install-time retryable download failures warn and exit `0` instead of failing `npm install`. |

## Release integrity

- `npm publish` runs a release-asset check to ensure all required binary assets
  exist for the target GitHub release before publishing.
- Install-time downloads are verified against the release checksum manifest before
  the wrapper marks them executable.
