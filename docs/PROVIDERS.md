# Provider Registry

This registry describes provider behavior that is wired into the current
CodeWhale codebase. It is intentionally conservative: shipped entries are
limited to provider IDs, config keys, auth paths, base URLs, model resolution,
and capability metadata that the code already knows about.

DeepSeek remains the first-class default provider. NVIDIA NIM, OpenRouter,
Volcengine Ark, Xiaomi MiMo, Novita, Fireworks, SiliconFlow, generic
OpenAI-compatible endpoints, self-hosted runtimes, and Moonshot/Kimi are
additive routes for running the same terminal harness against other hosted or
local model endpoints. Hugging Face Inference Providers are a planned additive
open-model routing layer; they are not a native provider in this checkout yet.

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
`openrouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`, `moonshot`,
`sglang`, `vllm`, and `ollama`.

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
| `atlascloud` | `[providers.atlascloud]` | `ATLASCLOUD_API_KEY` | `ATLASCLOUD_BASE_URL`; default `https://api.atlascloud.ai/v1` | Default config model `deepseek-ai/deepseek-v4-flash` | OpenAI-compatible hosted route. `ATLASCLOUD_MODEL` is accepted by the TUI config path. The static `ModelRegistry` does not currently list AtlasCloud rows. |
| `wanjie-ark` | `[providers.wanjie_ark]` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` | `WANJIE_ARK_BASE_URL`, `WANJIE_BASE_URL`, `WANJIE_MAAS_BASE_URL`; default `https://maas-openapi.wanjiedata.com/api/v1` | `deepseek-reasoner` | OpenAI-compatible hosted route. `WANJIE_ARK_MODEL`, `WANJIE_MODEL`, and `WANJIE_MAAS_MODEL` are accepted. |
| `volcengine` | `[providers.volcengine]` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` | `VOLCENGINE_BASE_URL`, `VOLCENGINE_ARK_BASE_URL`, `ARK_BASE_URL`; default `https://ark.cn-beijing.volces.com/api/coding/v3` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | Volcengine/Volcano Engine Ark OpenAI-compatible coding endpoint. `VOLCENGINE_MODEL` and `VOLCENGINE_ARK_MODEL` are accepted. |
| `openrouter` | `[providers.openrouter]` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`; default `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | Additive open-model routing layer. It does not replace DeepSeek; it lets users route supported model IDs through OpenRouter when they choose it. |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | `XIAOMI_MIMO_API_KEY`, `MIMO_API_KEY` | `XIAOMI_MIMO_BASE_URL`, `MIMO_BASE_URL`; default `https://api.xiaomimimo.com/v1` | `mimo-v2.5-pro`, `mimo-v2.5` | Xiaomi MiMo OpenAI-compatible chat completions route. It sends `max_completion_tokens` and uses MiMo's `thinking` field for reasoning control. |
| `novita` | `[providers.novita]` | `NOVITA_API_KEY` | `NOVITA_BASE_URL`; default `https://api.novita.ai/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | OpenAI-compatible hosted route for DeepSeek model IDs. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `fireworks` | `[providers.fireworks]` | `FIREWORKS_API_KEY` | `FIREWORKS_BASE_URL`; default `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro` | OpenAI-compatible hosted route. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `siliconflow` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`; default `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | OpenAI-compatible hosted route. Official docs use the `.com` endpoint; users who need the regional endpoint can set `https://api.siliconflow.cn/v1` explicitly. `SILICONFLOW_MODEL` is accepted. Reasoning aliases `deepseek-reasoner` and `deepseek-r1` map to Pro; `deepseek-chat` and `deepseek-v3` map to Flash. |
| `moonshot` | `[providers.moonshot]` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` | `MOONSHOT_BASE_URL`, `KIMI_BASE_URL`; default `https://api.moonshot.ai/v1` | `kimi-k2.6`; Kimi Code path uses `kimi-for-coding` at `https://api.kimi.com/coding/v1` | Moonshot/Kimi route. `MOONSHOT_MODEL`, `KIMI_MODEL_NAME`, and `KIMI_MODEL` are accepted. `[providers.moonshot] auth_mode = "kimi_oauth"` reads Kimi CLI OAuth credentials when present. |
| `sglang` | `[providers.sglang]` | Optional `SGLANG_API_KEY` | `SGLANG_BASE_URL`; default `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted OpenAI-compatible route. Localhost deployments commonly omit auth. `SGLANG_MODEL` is accepted. |
| `vllm` | `[providers.vllm]` | Optional `VLLM_API_KEY` | `VLLM_BASE_URL`; default `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted vLLM OpenAI-compatible route. Localhost deployments commonly omit auth. `VLLM_MODEL` is accepted. |
| `ollama` | `[providers.ollama]` | Optional `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`; default `http://localhost:11434/v1` | `deepseek-coder:1.3b`; provider-hinted custom tags pass through | Self-hosted Ollama OpenAI-compatible route. Localhost deployments commonly omit auth. `OLLAMA_MODEL` is accepted. |

### Xiaomi MiMo Notes

`xiaomi-mimo` defaults to `mimo-v2.5-pro` for long-context reasoning and coding
work, while the static registry also exposes `mimo-v2.5`. Xiaomi's current
[image-understanding guide](https://platform.xiaomimimo.com/docs/en-US/usage-guide/multimodal-understanding/image-understanding)
includes `mimo-v2.5` for image input. CodeWhale exposes image analysis through the
separate `[vision_model]` / `image_analyze` path; set that model to
`mimo-v2.5` when using MiMo for vision.

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
| `wanjie-ark` | `deepseek-reasoner` | yes | yes |
| `volcengine` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | yes | yes |
| `openrouter` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | yes | yes |
| `xiaomi-mimo` | `mimo-v2.5-pro`, `mimo-v2.5` | yes | yes |
| `novita` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | yes | yes |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` | yes | yes |
| `siliconflow` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `moonshot` | `kimi-k2.6` | yes | yes |
| `sglang` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `vllm` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `ollama` | `deepseek-coder:1.3b`; custom tags pass through when provider hint is `ollama` | yes | no |

The registry currently has no AtlasCloud entry even though AtlasCloud is a
supported provider in config and TUI selection. AtlasCloud should use the
configured model or live model listing.

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
| Xiaomi MiMo models | 1,000,000 | 128,000 | yes | no | not documented in code |
| Wanjie Ark `reasoner` / `r1` model IDs | 128,000 | 4,096 | yes | no | not documented in code |
| Generic `openai`, AtlasCloud, and Moonshot/Kimi | 128,000 | 4,096 | no in doctor capability metadata | no | not documented in code |
| Ollama | 8,192 | 4,096 | no | no | not documented in code |
| Other recognized DeepSeek model IDs | 128,000 unless the model name carries an explicit `Nk` hint | 4,096 | no unless V4/reasoner logic matches | DeepSeek/NIM only | DeepSeek beta only |

Tool-call support is tracked separately by the static `ModelRegistry` and by
the endpoint's ability to accept OpenAI-compatible `tools` payloads. A custom
OpenAI-compatible or local endpoint can still reject tool calls even if
CodeWhale can send the schema.

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

These items belong to the v0.8.47 provider-abstraction milestone or related
provider docs work, but they are not native shipped behavior in this checkout:

- A unified `Provider` trait in `codewhale-agent` that owns env precedence,
  secret resolution, base URL normalization, auth-header construction, and
  provider metadata. Those responsibilities are still split across
  `crates/config`, `crates/secrets`, and `crates/tui/src/client.rs`.
- A native Hugging Face provider such as `[providers.huggingface]`.
- Native Hugging Face auth envs such as `HF_TOKEN` or `HUGGINGFACE_API_KEY`.
- A default Hugging Face router base URL such as
  `https://router.huggingface.co/v1`.
- Hugging Face model passport metadata in the picker, including license, base
  model, context length, chat template, tool-call support, reasoning support,
  and gated/private status.

Until native Hugging Face support lands, users can only reach an explicitly
configured Hugging Face-compatible OpenAI route through the generic `openai`
provider. That is an explicit user-selected route, not built-in Hub discovery
or a replacement for DeepSeek.
