# Provider Registry

This registry describes provider behavior that is wired into the current
CodeWhale codebase. It is intentionally conservative: shipped entries are
limited to provider IDs, config keys, auth paths, base URLs, model resolution,
and capability metadata that the code already knows about.

DeepSeek remains the first-class default provider. NVIDIA NIM, OpenRouter,
Volcengine Ark, Xiaomi MiMo, Novita, Fireworks, SiliconFlow, Arcee AI, generic
OpenAI-compatible endpoints, self-hosted runtimes, Moonshot/Kimi, and Hugging
Face Inference Providers are additive routes for running the same terminal
harness against other hosted or local model endpoints.

Sources to keep in sync:

- `crates/config/src/lib.rs` - shared provider IDs, defaults, env precedence.
- `crates/tui/src/config.rs` - TUI provider IDs, provider capability metadata,
  and provider-specific env handling.
- `crates/agent/src/lib.rs` - static `ModelRegistry` used by
  `codewhale model list` and `codewhale model resolve`.
- `config.example.toml` and `docs/CONFIGURATION.md` - user-facing config
  examples and environment variable reference.
- `scripts/check-provider-registry.py` - drift check for canonical provider
  IDs, live TUI provider IDs, TOML table names, static registry rows, and
  documented defaults.

## Provider Selection

The canonical provider IDs are:

`deepseek`, `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `volcengine`,
`openrouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`,
`siliconflow-CN`, `arcee`, `moonshot`, `sglang`, `vllm`, `ollama`, and
`huggingface`.

Use any of these surfaces to select a provider:

- CLI: `codewhale --provider <id>`
- TUI: `/provider <id>` or the provider picker
- Env: `CODEWHALE_PROVIDER=<id>`; `DEEPSEEK_PROVIDER=<id>` is the legacy alias
- Config: `provider = "<id>"`

`deepseek-cn`, `deepseek_china`, `deepseekcn`, and `deepseek-china` are accepted
as legacy aliases for `deepseek`. They do not select a different official host;
DeepSeek uses the same official API host worldwide.

Fresh shared config writes to `~/.codewhale/config.toml`. Existing
`~/.deepseek/config.toml` files are still read for compatibility.

## Auth And Env Rules

For hosted providers, `codewhale auth set --provider <id>` saves an API key for
that provider. API-key environment variables are fallback inputs after saved
config and keyring credentials; an explicit process-level `--api-key` still
wins for that launch.

For base URL and model selection, prefer:

- `CODEWHALE_BASE_URL` / `CODEWHALE_MODEL` for the active provider.
- Provider-specific base URL/model env vars when listed below.
- `DEEPSEEK_BASE_URL`, `DEEPSEEK_MODEL`, and `DEEPSEEK_DEFAULT_TEXT_MODEL` as
  legacy aliases.

Non-local `http://` base URLs are rejected unless
`DEEPSEEK_ALLOW_INSECURE_HTTP=1` is set. Loopback HTTP URLs are allowed for
self-hosted runtimes.

## Custom DeepSeek-Compatible Endpoints

Most custom DeepSeek-compatible deployments can use an existing provider ID.
Do not create `[providers.deepseek_custom]`; the provider table names are fixed.
Instead, choose the closest shipped route and override its endpoint/model:

- DeepSeek-compatible hosted API: keep `provider = "deepseek"` and set
  `[providers.deepseek].base_url` plus `[providers.deepseek].model`, or launch
  with `DEEPSEEK_BASE_URL` and `DEEPSEEK_MODEL`.
- Generic OpenAI-compatible gateway: use `provider = "openai"` with
  `[providers.openai].base_url` plus `[providers.openai].model`, or launch with
  `OPENAI_BASE_URL` and `OPENAI_MODEL`.
- Local OpenAI-compatible runtimes: use `provider = "vllm"`, `"sglang"`, or
  `"ollama"` with the matching provider-specific base URL/model values.

Example user config for a DeepSeek-compatible host:

```toml
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_API_KEY"
base_url = "https://your-provider.example/v1"
model = "deepseek-ai/DeepSeek-V4-Pro"
```

Example user config for a generic gateway:

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_GATEWAY_API_KEY"
base_url = "https://gateway.example/v1"
model = "your-deepseek-compatible-model"
```

Keep `provider`, `api_key`, and `base_url` in user config or process
environment. Project-local config overlays intentionally cannot set those keys,
so a repository cannot silently redirect prompts or credentials to another
endpoint.

## Shipped Providers

| Provider ID | TOML table | Auth env | Base URL env and default | Default or static models | Notes |
| --- | --- | --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | `DEEPSEEK_API_KEY` | `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL`; default `https://api.deepseek.com/beta` | `deepseek-v4-pro`, `deepseek-v4-flash`; compatibility aliases `deepseek-chat`, `deepseek-reasoner` | First-class default. Beta URL enables strict tool mode, chat prefix completion, and FIM completion. Set `https://api.deepseek.com` or `/v1` explicitly to opt out of beta-only features. |
| `nvidia-nim` | `[providers.nvidia_nim]` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, fallback `DEEPSEEK_API_KEY` | `NVIDIA_NIM_BASE_URL`, `NIM_BASE_URL`, `NVIDIA_BASE_URL`; default `https://integrate.api.nvidia.com/v1` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | Hosted DeepSeek V4 through NVIDIA NIM. `NVIDIA_NIM_MODEL` is accepted by the TUI config path. |
| `openai` | `[providers.openai]` | `OPENAI_API_KEY` | `OPENAI_BASE_URL`; default `https://api.openai.com/v1` | Registry entries: `deepseek-v4-pro`, `deepseek-v4-flash`; default config model `deepseek-v4-pro` | Generic OpenAI-compatible route for gateways and custom endpoints. Use this for explicit third-party OpenAI-compatible routes instead of inventing a new provider ID. `OPENAI_MODEL` is accepted. |
| `atlascloud` | `[providers.atlascloud]` | `ATLASCLOUD_API_KEY` | `ATLASCLOUD_BASE_URL`; default `https://api.atlascloud.ai/v1` | Default `deepseek-ai/deepseek-v4-flash`; explicit `vendor/model-id` values pass through when AtlasCloud is selected | OpenAI-compatible hosted route. `ATLASCLOUD_MODEL` is accepted by the TUI config path, the static `ModelRegistry` keeps DeepSeek V4 fallback rows, and provider-hinted CLI model IDs are sent to AtlasCloud exactly as requested. |
| `wanjie-ark` | `[providers.wanjie_ark]` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` | `WANJIE_ARK_BASE_URL`, `WANJIE_BASE_URL`, `WANJIE_MAAS_BASE_URL`; default `https://maas-openapi.wanjiedata.com/api/v1` | `deepseek-reasoner` | OpenAI-compatible hosted route. `WANJIE_ARK_MODEL`, `WANJIE_MODEL`, and `WANJIE_MAAS_MODEL` are accepted. |
| `volcengine` | `[providers.volcengine]` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` | `VOLCENGINE_BASE_URL`, `VOLCENGINE_ARK_BASE_URL`, `ARK_BASE_URL`; default `https://ark.cn-beijing.volces.com/api/coding/v3` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | Volcengine/Volcano Engine Ark OpenAI-compatible coding endpoint. `VOLCENGINE_MODEL` and `VOLCENGINE_ARK_MODEL` are accepted. |
| `openrouter` | `[providers.openrouter]` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`; default `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`; recent large IDs include `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `google/gemma-4-31b-it`, `z-ai/glm-5.1`, `moonshotai/kimi-k2.6` | Additive open-model routing layer. It does not replace DeepSeek; it lets users route supported model IDs through OpenRouter when they choose it. |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` | `XIAOMI_MIMO_BASE_URL`, `MIMO_BASE_URL`; default `https://token-plan-sgp.xiaomimimo.com/v1` | Chat: `mimo-v2.5-pro`, `mimo-v2.5`; speech/TTS: `mimo-v2.5-tts`, `mimo-v2.5-tts-voicedesign`, `mimo-v2.5-tts-voiceclone`, `mimo-v2-tts` | Xiaomi MiMo OpenAI-compatible chat completions route. Token Plan keys (`tp-...`) use the token-plan endpoint by default; pay-as-you-go keys can set `base_url = "https://api.xiaomimimo.com/v1"`. It sends `max_completion_tokens` and uses MiMo's `thinking` field for reasoning control. `codewhale speech` / `tts` uses the TTS models. |
| `novita` | `[providers.novita]` | `NOVITA_API_KEY` | `NOVITA_BASE_URL`; default `https://api.novita.ai/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | OpenAI-compatible hosted route for DeepSeek model IDs. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `fireworks` | `[providers.fireworks]` | `FIREWORKS_API_KEY` | `FIREWORKS_BASE_URL`; default `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro` | OpenAI-compatible hosted route. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `siliconflow` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`; default `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | OpenAI-compatible hosted route. Official docs use the `.com` endpoint. `SILICONFLOW_MODEL` is accepted. Reasoning aliases `deepseek-reasoner` and `deepseek-r1` map to Pro; `deepseek-chat` and `deepseek-v3` map to Flash. |
| `siliconflow-CN` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`; default `https://api.siliconflow.cn/v1` | Uses the SiliconFlow model set | China regional SiliconFlow route. This intentionally shares `[providers.siliconflow]` and `SILICONFLOW_API_KEY`; do not create `[providers.siliconflow_CN]`. Select it with `provider = "siliconflow-CN"` or `CODEWHALE_PROVIDER=siliconflow-CN`. |
| `arcee` | `[providers.arcee]` | `ARCEE_API_KEY` | `ARCEE_BASE_URL`; default `https://api.arcee.ai/api/v1` | `trinity-large-thinking`, `trinity-large-preview` | Arcee AI direct OpenAI-compatible route, tracked as 256K-context BF16 serving. `ARCEE_MODEL` is accepted. OpenRouter's `arcee-ai/trinity-large-thinking` remains the OpenRouter namespaced model ID; direct Arcee uses the bare `trinity-large-thinking` ID. |
| `moonshot` | `[providers.moonshot]` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` | `MOONSHOT_BASE_URL`, `KIMI_BASE_URL`; default `https://api.moonshot.ai/v1` | `kimi-k2.6`; Kimi Code path uses `kimi-for-coding` at `https://api.kimi.com/coding/v1` | Moonshot/Kimi route. `MOONSHOT_MODEL`, `KIMI_MODEL_NAME`, and `KIMI_MODEL` are accepted. `[providers.moonshot] auth_mode = "kimi_oauth"` reads Kimi CLI OAuth credentials when present. |
| `sglang` | `[providers.sglang]` | Optional `SGLANG_API_KEY` | `SGLANG_BASE_URL`; default `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted OpenAI-compatible route. Localhost deployments commonly omit auth. `SGLANG_MODEL` is accepted. |
| `vllm` | `[providers.vllm]` | Optional `VLLM_API_KEY` | `VLLM_BASE_URL`; default `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted vLLM OpenAI-compatible route. Localhost deployments commonly omit auth. `VLLM_MODEL` is accepted. |
| `ollama` | `[providers.ollama]` | Optional `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`; default `http://localhost:11434/v1` | `deepseek-coder:1.3b`; provider-hinted custom tags pass through | Self-hosted Ollama OpenAI-compatible route. Localhost deployments commonly omit auth. `OLLAMA_MODEL` is accepted. |
| `huggingface` | `[providers.huggingface]` | `HUGGINGFACE_API_KEY`, `HF_TOKEN` | `HUGGINGFACE_BASE_URL`; default `https://router.huggingface.co/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Hugging Face Inference Providers OpenAI-compatible route. Org-prefixed model IDs pass through. |

### Hugging Face MCP (Model Context Protocol)

Hugging Face provides an MCP server for Hub search, model-card lookups,
dataset discovery, and community tools — separate from the inference provider
route above. CodeWhale can detect whether it is configured and print a safe
config skeleton via the `/hf` command:

- `/hf mcp status` — check whether the HF MCP server is configured
- `/hf mcp setup` — print a config skeleton with placeholder tokens
- `/hf concepts` — explain HF provider vs MCP vs Hub

Add the following to your MCP config (`mcp.json` or the CodeWhale MCP config
file) to enable HF MCP:

```jsonc
{
  "servers": {
    "huggingface": {
      "url": "https://huggingface.co/api/mcp",
      "headers": {
        "Authorization": "Bearer ${HF_TOKEN}"
      }
    }
  }
}
```

`${HF_TOKEN}` is a placeholder — replace it with your Hugging Face token.
Never commit your real token.

Once configured, the HF MCP server provides tools for:

- Searching the Hub for models, datasets, and Spaces
- Inspecting model cards and documentation
- Accessing community tools and resources

For more details, see https://huggingface.co/docs/hub/hf-mcp-server.

### Xiaomi MiMo Notes

`xiaomi-mimo` defaults to `mimo-v2.5-pro` for long-context reasoning and coding
work. The chat picker also exposes the latest Omni model `mimo-v2.5`. Xiaomi MiMo
TTS is available through `codewhale --provider xiaomi-mimo speech "text"
--model tts` (or the `tts` alias) plus model-visible `speech` / `tts` tools in
Agent/YOLO mode.
Voice-design and voice-clone shorthands map to `mimo-v2.5-tts-voicedesign` and
`mimo-v2.5-tts-voiceclone`. Xiaomi's current
[image-understanding guide](https://platform.xiaomimimo.com/docs/en-US/usage-guide/multimodal-understanding/image-understanding)
includes `mimo-v2.5` for image input. CodeWhale exposes image analysis through the
separate `[vision_model]` / `image_analyze` path; set that model to
`mimo-v2.5` when using MiMo for vision.

### Recent OpenRouter Large Models

OpenRouter completions and static registry rows include the April 2026 onward
large models verified through OpenRouter's model metadata:
`arcee-ai/trinity-large-thinking`, `qwen/qwen3.6-flash`,
`qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`,
`qwen/qwen3.6-plus`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`,
`xiaomi/mimo-v2.5`, `moonshotai/kimi-k2.6`, `z-ai/glm-5.1`, `tencent/hy3-preview`,
`google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, and
`nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`.
`minimax/minimax-m3` was added from OpenRouter's May 31, 2026 listing as a 1M
context multimodal model for coding, tool use, and long-horizon agentic work.

## Static Model Registry

`codewhale model list` and `codewhale model resolve` use the static registry in
`crates/agent/src/lib.rs`. This is not the same as live `/models` discovery.
Use `/models` or `codewhale models` to fetch model IDs from the active API
endpoint when the endpoint supports model listing.

| Provider | Static registry entries | Tool calls | Registry reasoning flag |
| --- | --- | --- | --- |
| `deepseek` | `deepseek-v4-pro`, `deepseek-v4-flash` | yes | yes |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | yes | yes |
| `openai` | `deepseek-v4-pro`, `deepseek-v4-flash` | yes | yes |
| `atlascloud` | `deepseek-ai/deepseek-v4-flash`, `deepseek-ai/deepseek-v4-pro` | yes | yes |
| `wanjie-ark` | `deepseek-reasoner` | yes | yes |
| `volcengine` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | yes | yes |
| `openrouter` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`, `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`, `xiaomi/mimo-v2.5`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `moonshotai/kimi-k2.6`, `z-ai/glm-5.1`, `tencent/hy3-preview`, `google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` | yes | yes |
| `xiaomi-mimo` | `mimo-v2.5-pro`, `mimo-v2.5`; speech/TTS IDs are selected through `codewhale speech` / `tts` | yes | yes for chat models; no for speech/TTS models |
| `novita` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | yes | yes |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` | yes | yes |
| `siliconflow` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `arcee` | `trinity-large-thinking`, `trinity-large-preview`; provider-hinted custom model IDs pass through | yes | yes for `trinity-large-thinking`; no for `trinity-large-preview` |
| `moonshot` | `kimi-k2.6` | yes | yes |
| `sglang` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `vllm` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `ollama` | `deepseek-coder:1.3b`; custom tags pass through when provider hint is `ollama` | yes | no |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | no |

AtlasCloud keeps the same default model as the config layer and adds
provider-scoped aliases for the Pro and Flash rows. Other AtlasCloud model IDs
should still be selected through `ATLASCLOUD_MODEL`, config, or live model
listing when available.

## Capability Metadata

`codewhale-tui doctor --json` exposes the `capability` object. It is static
metadata, not a live API probe. Current fields are:

`resolved_provider`, `resolved_model`, `context_window`, `max_output`,
`thinking_supported`, `cache_telemetry_supported`, and `request_payload_mode`.

All shipped providers use the Chat Completions request payload mode today.

| Provider/model class | Context window | Max output metadata | Thinking support | Cache telemetry | FIM endpoint |
| --- | --- | --- | --- | --- | --- |
| DeepSeek V4 (`deepseek-v4-pro`, `deepseek-v4-flash`) | 1,000,000 | 384,000 | yes | yes | DeepSeek beta only |
| DeepSeek compatibility aliases (`deepseek-chat`, `deepseek-reasoner`) | 1,000,000 | 384,000 | yes | yes | DeepSeek beta only |
| NVIDIA NIM V4 registry models | 1,000,000 | 384,000 | yes | yes | not documented in code |
| Volcengine Ark V4 model IDs | 1,000,000 | 384,000 | yes | yes | not documented in code |
| OpenRouter, Novita, Fireworks, SiliconFlow, SGLang, and vLLM V4 model IDs | 1,000,000 | 384,000 | yes | no | not documented in code |
| Xiaomi MiMo `mimo-v2.5-pro`, `mimo-v2.5` | 1,000,000 | 131,072 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 Flash / Plus | 1,000,000 | 65,536 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 35B / 27B | 262,144 | 262,140 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 Max Preview | 262,144 | 65,536 | yes | no | not documented in code |
| Wanjie Ark `reasoner` / `r1` model IDs | 128,000 | 4,096 | yes | no | not documented in code |
| Direct Arcee API `trinity-large-thinking` | 262,144 | 262,144 | yes | no | not documented in code |
| Direct Arcee API `trinity-large-preview` | 262,144 | 4,096 | no in doctor capability metadata | no | not documented in code |
| Generic `openai`, AtlasCloud, and Moonshot/Kimi | 128,000 | 4,096 | no in doctor capability metadata | no | not documented in code |
| Ollama | 8,192 | 4,096 | no | no | not documented in code |
| Hugging Face Inference Providers V4 model IDs | 131,072 | 4,096 | yes | no | not documented in code |
| Other recognized DeepSeek model IDs | 128,000 unless the model name carries an explicit `Nk` hint | 4,096 | no unless V4/reasoner logic matches | DeepSeek/NIM only | DeepSeek beta only |

Tool-call support is tracked separately by the static `ModelRegistry` and by
the endpoint's ability to accept OpenAI-compatible `tools` payloads. A custom
OpenAI-compatible or local endpoint can still reject tool calls even if
CodeWhale can send the schema.

### When a Local Model Prints Tool JSON

CodeWhale only executes tools when the provider returns Chat Completions
`tool_calls` or streamed `delta.tool_calls`. If a local model prints text such
as `{"name":"grep_files","arguments":{...}}` in the assistant message, that is
ordinary model output, not an executable tool request.

For OpenAI-compatible or local runtimes, check:

- The endpoint accepts the `tools` array in `/v1/chat/completions` requests.
- The selected model or chat template is configured for function/tool calls.
- The server returns `tool_calls` in the response rather than plain JSON text.
- The compatibility layer does not strip tools before forwarding the request.
- If in doubt, test a small `read_file` or `grep_files` request against a known
  tool-calling model before debugging CodeWhale's tool registry.

Changing `provider`, `base_url`, or `model` can select a route that supports the
OpenAI-compatible payload shape, but CodeWhale cannot convert arbitrary JSON
text into a trusted tool call after the model has emitted it as prose.

DeepSeek compatibility aliases `deepseek-chat` and `deepseek-reasoner` map to
`deepseek-v4-flash` capability metadata and are scheduled to retire on
2026-07-24 at 2026-07-24T15:59:00Z.

## Drift Check

Run this before changing provider IDs, provider TOML tables, static model
registry rows, or provider default strings:

```bash
python3 scripts/check-provider-registry.py
```

The check fails when:

- `docs/PROVIDERS.md` omits a canonical `ProviderKind::as_str()` ID.
- `crates/tui/src/config.rs` `ApiProvider::as_str()` diverges from
  `ProviderKind::as_str()` except for the explicit `deepseek-cn` legacy alias.
- The shipped-provider table omits or adds a `[providers.*]` TOML table.
- The static model registry table drifts from providers used by
  `crates/agent/src/lib.rs`.
- A provider default model or base URL constant in `crates/tui/src/config.rs`
  is no longer mentioned here.

## Planned, Not Shipped Yet

These items belong to the v0.8.48+ provider-abstraction milestone or related
provider docs work, but they are not native shipped behavior in this checkout:

- A unified `Provider` trait in `codewhale-agent` that owns env precedence,
  secret resolution, base URL normalization, auth-header construction, and
  provider metadata. Those responsibilities are still split across
  `crates/config`, `crates/secrets`, and `crates/tui/src/client.rs`.
- Hugging Face model passport metadata in the picker, including license, base
  model, context length, chat template, tool-call support, reasoning support,
  and gated/private status.
