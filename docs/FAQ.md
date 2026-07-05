# FAQ

Frequently asked questions, adapted from the
[codewhale.net FAQ](https://codewhale.net/faq) (source:
`web/app/[locale]/faq/page.tsx`, which also carries the 中文 version). Answers
are sourced from real code, docs, and GitHub issues.

## What is CodeWhale?

CodeWhale is a terminal-native coding agent for open-source and open-weight
models. It runs from the `codewhale` command, streams reasoning blocks, edits
local workspaces with approval gates, and can auto-route each turn to the
right model and thinking level. DeepSeek is the first-class model path;
OpenRouter, Hugging Face, self-hosted runtimes, and other OpenAI-compatible
routes are additive. See the [root README](../README.md) and
[ARCHITECTURE.md](ARCHITECTURE.md).

## How do I install CodeWhale?

Four paths, same result:

```bash
# npm (recommended — no Rust toolchain needed)
npm install -g codewhale

# Cargo (needs Rust 1.88+)
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked

# Homebrew (macOS, legacy tap while the formula is renamed)
brew tap Hmbown/deepseek-tui && brew install deepseek-tui

# Direct download
# https://github.com/Hmbown/CodeWhale/releases
```

Run `codewhale` to start. First run creates `~/.codewhale/` automatically;
legacy `~/.deepseek/` is still read as a compatibility fallback. See
[INSTALL.md](INSTALL.md) for China mirrors, Docker, and troubleshooting.

## What's the difference between `codewhale` and `codewhale-tui`?

`codewhale` is the dispatcher CLI — it manages config, auth, updates, and
launches the TUI. `codewhale-tui` is the terminal UI binary that runs the
agent loop. When you type `codewhale`, the dispatcher spawns `codewhale-tui`
for you. Both are installed together; you rarely need to think about the
split.

## Is CodeWhale the same as DeepSeek TUI? What about the rename?

Yes. CodeWhale is the new name for what was previously called DeepSeek TUI.
The canonical command is now `codewhale`; legacy `deepseek` and `deepseek-tui`
commands remain as compatibility shims. Config lives at `~/.codewhale/`,
legacy `~/.deepseek/` config is still read, and `DEEPSEEK_*` env vars continue
to work. DeepSeek is not deprecated — the rename reflects CodeWhale as an
agentic terminal for open models across providers, not a narrowing away from
DeepSeek. See [REBRAND.md](REBRAND.md).

## How do I set my API key?

```bash
# Method 1: Environment variable
export DEEPSEEK_API_KEY=sk-...

# Method 2: Saved config (recommended — survives shell restarts)
codewhale auth set --provider deepseek --api-key sk-...

# Method 3: config.toml — add to ~/.codewhale/config.toml:
# api_key = "sk-..."

# Check what's active:
codewhale auth status    # shows config, keyring, and env-var state
codewhale doctor         # full connectivity check
```

Saved config keys take precedence over environment variables. Use
`codewhale auth clear --provider deepseek` to remove a saved key.

## Which providers does CodeWhale support?

- **DeepSeek** — first-class, native API: reasoning streaming, cache metrics,
  thinking effort control.
- **OpenRouter** — unified API for DeepSeek models and other open-model
  routes.
- **OpenAI-compatible**, **NVIDIA NIM**, **AtlasCloud**, **Wanjie Ark**,
  **Volcengine Ark**, **Xiaomi MiMo**, **Novita**, **Fireworks**,
  **SiliconFlow** / **SiliconFlow CN**, **Arcee AI**, **Moonshot/Kimi**,
  **Hugging Face**, **DeepInfra**, **Together AI**, **Z.ai**, **StepFun**,
  **MiniMax**, **OpenAI Codex**, **Anthropic**, **SGLang**, **vLLM**,
  **Ollama**.

Set the corresponding env var (e.g. `OPENROUTER_API_KEY`) and your provider in
`~/.codewhale/config.toml`. Self-hosted OpenAI-compatible endpoints are
supported through the provider config. Full registry:
[PROVIDERS.md](PROVIDERS.md).

## How do I use OpenRouter with CodeWhale?

```bash
# 1. Set your OpenRouter key
export OPENROUTER_API_KEY=sk-or-v1-...

# 2. In ~/.codewhale/config.toml:
[providers.openrouter]
api_key = "sk-or-v1-..."

# 3. Run with an OpenRouter model:
codewhale --model openrouter/deepseek/deepseek-v4-pro

# Or set it as default in config.toml:
default_text_model = "openrouter/deepseek/deepseek-v4-pro"
```

OpenRouter uses the same reasoning/cache parser as the native DeepSeek
provider. Model IDs follow the `provider/model-id` pattern.

## Can I use self-hosted or local models (vLLM, Ollama, llama.cpp)?

Yes. Use the `vllm`, `sglang`, or `ollama` providers with your local endpoint
— no key required. For other OpenAI-compatible endpoints (llama.cpp server,
text-generation-webui, Aphrodite, etc.), use the `openai` provider with a
custom `base_url`. CodeWhale also respects `DEEPSEEK_ALLOW_INSECURE_HTTP=true`
for local HTTP endpoints. Hugging Face Inference Providers are available
through the `huggingface` provider.

## What are Plan, Agent, and YOLO modes?

- **Plan** — read-only investigation. Can grep, read files, list directories,
  fetch URLs. Cannot write or execute shell.
- **Agent** — the default mode. Multi-step tool calling; shell and side-effect
  tools require approval based on your `approval_mode` setting.
- **YOLO** — auto-approves all operations and enables trust mode. Workspace
  boundaries lift. Use carefully.

Press `Tab` to cycle modes. Approval mode (suggest / auto / never) is
orthogonal — you can be in Agent mode with auto-approval, for example. See
[MODES.md](MODES.md).

## What is model auto-routing? What is Fin?

Use `codewhale --model auto` or `/model auto` to let CodeWhale decide how much
model power each turn needs. **Fin** is the fast non-thinking path
(`deepseek-v4-flash` with thinking off) used for routing decisions, summaries,
RLM children, and other coordination work. Before the real turn is sent, Fin
makes a small routing call to pick the concrete model and thinking level.
Short turns can stay on Flash; coding, debugging, and architecture work can
move up to Pro and/or higher thinking. Fin is local to CodeWhale — the
upstream API never receives `model: "auto"`.

## What does `/goal` do?

`/goal` sets a goal for the current TUI session, and the agent keeps working
across turns until the goal is done, blocked, or stopped. App-server clients
can persist a thread-scoped goal through the `thread/goal/*` methods. It does
not add another app mode; the mode switcher remains Plan, Agent, and YOLO.

## Is my code safe? What sandboxing does CodeWhale use?

CodeWhale runs entirely on your machine — no telemetry, no cloud processing of
your code. Sandbox backends: **seatbelt** (macOS), **landlock** (Linux),
restricted tokens (Windows). Workspace boundaries default to `--workspace`;
`/trust` lifts them. Approval mode is configurable per session, and all
credential/approval/elevation events are written to `~/.codewhale/audit.log`.
See [SANDBOX.md](SANDBOX.md) and [SECURITY.md](../SECURITY.md).

## How do MCP servers work?

CodeWhale is a bidirectional MCP client and server. Define servers in
`~/.codewhale/mcp.json`; their tools appear as `mcp_<server>_<tool>`. You can
also expose CodeWhale itself as an MCP server with `codewhale mcp`. See
[MCP.md](MCP.md).

## How do I contribute?

No CLA required. Fork, branch with conventional commits (`feat:`, `fix:`,
etc.), run the local checks, open a PR. The maintainer reads everything
personally; start with issues labeled `good first issue`. See
[CONTRIBUTING.md](../CONTRIBUTING.md) and [CREDIT.md](CREDIT.md) for how
credit works.

## I'm in China — how do I install? Downloads are slow.

Use mirror registries:

```bash
# npm mirror
npm config set registry https://registry.npmmirror.com
npm install -g codewhale

# Cargo mirror (Tsinghua TUNA) — add to ~/.cargo/config.toml:
[source.crates-io]
replace-with = "tuna"
[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

Prebuilt binaries are also available from
[GitHub Releases](https://github.com/Hmbown/CodeWhale/releases), and a CNB
mirror is maintained for users who cannot reliably reach GitHub — see
[CNB_MIRROR.md](CNB_MIRROR.md).

## Is codewhale.net the official site? What about mirrors?

**codewhale.net** and **www.codewhale.net** are the official sites, deployed
on Cloudflare. The website source is open and lives under `web/` in the
`Hmbown/CodeWhale` repository — anyone can self-deploy it as a mirror. All
official releases and SHA-256 checksums are distributed exclusively through
[GitHub Releases](https://github.com/Hmbown/CodeWhale/releases); the npm
package downloads verified binaries from there. Self-deployed website copies,
mirror sites, and third-party packages are not controlled by the CodeWhale
project — verify download sources and checksums.

## My API key was rejected or I get auth errors on first run

Run `codewhale doctor` — it checks API key, network, sandbox, and MCP servers,
and writes a full report to `~/.codewhale/doctor.log`. Common causes:

- Stale `DEEPSEEK_API_KEY` in a shell startup file — open a fresh shell or use
  `codewhale auth set`.
- Key from the wrong provider — make sure the key matches the provider you're
  using.
- Network connectivity — check `curl https://api.deepseek.com/v1/models`.

## What is Model Lab? What Hugging Face pieces are available?

The `huggingface` provider is the shipped OpenAI-compatible route for Hugging
Face Inference Providers. Model Lab is the planned open-model infrastructure
layer for Hub discovery, model cards, datasets, safetensors adapters, and
Jobs. See [MODEL_LAB.md](MODEL_LAB.md).

## Why is token consumption so high? Why is cache hit rate low?

CodeWhale sends substantial context (system prompt, project instructions,
tool definitions) with each turn. DeepSeek's prefix cache is used aggressively
— the system prompt is layered to maximize cache hits. If you see high token
usage, check whether you're using `deepseek-v4-pro` for simple queries better
suited to Flash; model auto-routing (Fin) can pick the right model per turn.
Cache hit rate depends on prompt stability — modifying the system prompt or
switching models resets the cache.

## How do I update CodeWhale?

```bash
# Release-binary updater (works for npm/release-binary installs)
codewhale update

# npm
npm install -g codewhale@latest

# Cargo
cargo install codewhale-cli --locked --force

# Homebrew
brew update && brew upgrade deepseek-tui
```

If a mirror is lagging, download directly from
[GitHub Releases](https://github.com/Hmbown/CodeWhale/releases).
