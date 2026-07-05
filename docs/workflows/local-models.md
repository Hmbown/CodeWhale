# Run against a local model

CodeWhale treats local OpenAI-compatible servers — Ollama, vLLM, SGLang — as
first-class providers. Localhost endpoints commonly run without an API key.

Point `~/.codewhale/config.toml` at your server (the defaults already match a
local Ollama at `http://localhost:11434/v1`):

```toml
provider = "ollama"

[providers.ollama]
model = "deepseek-coder:1.3b"   # any local tag passes through
# base_url = "http://localhost:11434/v1"   # default; override if remote
```

vLLM defaults to `http://localhost:8000/v1` (`[providers.vllm]`) and SGLang to
`http://localhost:30000/v1` (`[providers.sglang]`). Any other
OpenAI-compatible gateway works via `[providers.openai]` with `base_url`.

Then launch and confirm the route:

```bash
codewhale --provider ollama
/model        # verify the active provider/model in the TUI
```

**Done when:** `/model` shows your local provider and a turn completes without
any hosted-API key configured.

See also: [PROVIDERS.md](../PROVIDERS.md) — the full provider table, base-URL
env vars, and defaults.
