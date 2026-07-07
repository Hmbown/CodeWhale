# codewhale

> The terminal coding agent for any model — open models first.

CodeWhale is a Rust TUI and CLI for many model providers — DeepSeek,
OpenRouter, Hugging Face, and local vLLM/SGLang/Ollama are first-class routes,
and it speaks natively to Anthropic Claude and OpenAI when that's what you have
— with approval-gated tools, OS sandboxing, side-git snapshots, and `/restore`
rollback.

This npm package is a small launcher: it downloads the matching native
CodeWhale binaries for your platform, verifies them against the release
SHA-256 manifest, and installs the `codewhale`, `codew`, and `codewhale-tui`
commands. The application state and credentials still live in CodeWhale's
normal config files, not inside `node_modules`.

> Previously published as `deepseek-tui`. See
> [docs/REBRAND.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/REBRAND.md)
> for the migration notes; the legacy `deepseek-tui` npm package is deprecated
> and receives no further releases.

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
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Every provider is the same one-line shape — `--provider openrouter`,
`--provider huggingface`, `--provider ollama`, or `--provider anthropic` for a
Claude key; the full registry lives in
[docs/PROVIDERS.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/PROVIDERS.md).

The `codewhale` facade and `codewhale-tui` binary share
`~/.codewhale/config.toml` for auth and default model settings. Legacy
`~/.deepseek/config.toml` installs are still read as a compatibility fallback.
Common TUI commands are available directly through the facade, including
`codewhale doctor`, `codewhale models`, `codewhale sessions`, and
`codewhale resume --last`.

## Supported platforms

Prebuilt binaries for the GitHub release are downloaded automatically:

- Linux x64
- Linux arm64
- Linux riscv64
- macOS x64 / arm64
- Windows x64

HarmonyOS PC (`openharmony`) is treated as `linux`, so it gets the Linux
binaries matching your CPU architecture (x64, arm64, or riscv64). Other
platform/architecture combinations (musl, FreeBSD, …) aren't
shipped as prebuilts. Unsupported platforms, checksum failures, and glibc
compatibility problems still fail with a clear error pointing you at
`cargo install codewhale-cli codewhale-tui --locked` and the full
[docs/INSTALL.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/INSTALL.md)
build-from-source guide.

## Wrapper configuration

| Setting | What it does |
| --- | --- |
| `codewhaleBinaryVersion` in `package.json` | Default native binary version. `deepseekBinaryVersion` is still read as a backward-compat fallback. |
| `CODEWHALE_RELEASE_BASE_URL` | Canonical override: use an internal or mirrored release-asset directory when GitHub Releases is unavailable. The directory must contain `codewhale-artifacts-sha256.txt` and the platform binaries. `DEEPSEEK_TUI_RELEASE_BASE_URL` and `DEEPSEEK_RELEASE_BASE_URL` are the implemented legacy fallbacks. |
| `CODEWHALE_USE_CNB_MIRROR=1` | Download release assets from the CNB (China-friendly) mirror instead of GitHub. |
| `DEEPSEEK_TUI_VERSION` or `DEEPSEEK_VERSION` | Override the GitHub release version to download. |
| `DEEPSEEK_TUI_GITHUB_REPO` or `DEEPSEEK_GITHUB_REPO` | Override the source repo. Defaults to `Hmbown/CodeWhale`. |
| `DEEPSEEK_TUI_FORCE_DOWNLOAD=1` | Force download even when the cached binary is already present. |
| `DEEPSEEK_TUI_DISABLE_INSTALL=1` | Skip install-time download. |
| `DEEPSEEK_TUI_OPTIONAL_INSTALL=1` | Make install-time retryable download failures warn and exit `0` instead of failing `npm install`. |
| `DEEPSEEK_TUI_SKIP_GLIBC_CHECK=1` | Bypass the Linux glibc preflight check at your own risk (`DEEPSEEK_SKIP_GLIBC_CHECK=1` also works). |

### Proxies

Downloads respect `HTTPS_PROXY` / `HTTP_PROXY` (CONNECT tunneling included)
and `NO_PROXY`, so the wrapper works behind corporate proxies. For fully
offline installs, set `DEEPSEEK_TUI_DISABLE_INSTALL=1` or point
`CODEWHALE_RELEASE_BASE_URL` at a local mirror.

## Release integrity

- `npm publish` runs a release-asset check to ensure all required binary assets
  exist for the target GitHub release before publishing.
- For the default GitHub Release source, `npm run release:check` also verifies
  that the npm-facing assets were updated by a successful `release.yml` run for
  the tag commit. When `CODEWHALE_RELEASE_BASE_URL` or a legacy mirror override
  is set, it checks the mirror asset URLs and checksum manifest instead.
- Install-time downloads are verified against the release checksum manifest before
  the wrapper marks them executable.

## Links

- Repository: <https://github.com/Hmbown/CodeWhale>
- Website: <https://codewhale.net/>
- Provider registry: [docs/PROVIDERS.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/PROVIDERS.md)
- Changelog: [CHANGELOG.md](https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md)

## TormentNexus Extension

CodeWhale 0.8.66+ includes a built-in **TormentNexus extension** (`crates/tn-extension`) compiled directly into the binary — no separate plugin or DLL needed. It provides:

### Lifecycle Hooks
- **SessionStart** — logs session info to TN L2 memory on port 7778
- **BeforeAgentStart** — injects TN system prompt guidance + searches L2 for relevant context
- **ToolCall** — logs tool calls; checks 6 dangerous patterns against TN enterprise RBAC
- **ToolResult** — auto-stores substantial results from key tools to L2
- **TurnEnd** — logs tool usage per turn
- **Input** — expands `@memory:key` inline with L2 content
- **UserBash** — audit-logs shell commands to TN enterprise audit
- **ModelSelect** — tracks model changes
- **SessionCompact** — preserves memory across compaction

### Custom Tools (via Extension API)
- 9 tool definitions: `tn_memory_store`, `tn_memory_search`, `tn_memory_vector_search`, `tn_tool_search`, `tn_session_search`, `tn_skill_manage`, `tn_code_search`, `tn_context_harvest`, `tn_scratchpad`
- 6 slash commands: `/tn-store`, `/tn-search`, `/tn-status`, `/tn-plan`, `/tn-summary`, `/tn-purge`
- 3 keyboard shortcuts: `Ctrl+Shift+M` (memory), `Ctrl+Shift+T` (tools), `Ctrl+Shift+P` (status)

### MCP Server Auto-Registration
The extension automatically registers the `tormentnexus` MCP server pointing at `tormentnexus.exe mcp`. Enable it by keeping the TormentNexus sidecar running on `http://127.0.0.1:7778`.

> Extension source: `crates/tn-extension` — implements `codewhale_extension::Extension` trait.
