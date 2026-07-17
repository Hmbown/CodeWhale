# Provider Registry

This registry describes provider behavior that is wired into the current
Codewhale codebase. It is intentionally conservative: shipped entries are
limited to provider IDs, config keys, auth paths, base URLs, model resolution,
and capability metadata that the code already knows about.

DeepSeek remains the default provider, but every entry in `ProviderKind::ALL`
and `PROVIDER_REGISTRY` is a first-class selectable provider route. Hosted
routes, generic OpenAI-compatible endpoints, the OpenAI Codex/ChatGPT route,
native Anthropic, and local runtimes all run the same terminal harness against
the selected provider/model/base URL.

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

`deepseek`, `deepseek-anthropic`, `nvidia-nim`, `openai`, `atlascloud`,
`wanjie-ark`, `volcengine`, `openrouter`, `xiaomi-mimo`, `novita`, `fireworks`,
`siliconflow`, `arcee`, `siliconflow-CN`, `moonshot`, `sglang`, `vllm`,
`ollama`, `huggingface`, `together`, `qianfan`, `openai-codex`, `anthropic`,
`openmodel`, `zai`, `stepfun`, `minimax`, `deepinfra`, `sakana`, `longcat`,
`opencode-go`, `opencode-zen`, `meta`, and `xai`.

Use any of these surfaces to select a provider:

- CLI: `codewhale --provider <id>`
- TUI: `/provider <id>` or the provider picker
- Env: `CODEWHALE_PROVIDER=<id>`; `DEEPSEEK_PROVIDER=<id>` is the legacy alias
- Config: `provider = "<id>"`

`deepseek-cn`, `deepseek_china`, `deepseekcn`, and `deepseek-china` are accepted
as legacy aliases for `deepseek`. They do not select a different official host;
DeepSeek uses the same official API host worldwide.

`deepseek_anthropic`, `deepseek-claude`, and `deepseek_claude` select
`deepseek-anthropic`, the opt-in DeepSeek route that speaks the Anthropic
Messages API at `https://api.deepseek.com/anthropic`. It keeps the normal
DeepSeek API key path but uses `x-api-key` plus `anthropic-version: 2023-06-01`
instead of Bearer auth.

`huggingface`, `hugging-face`, `hugging_face`, and `hf` all select the
Hugging Face Inference Providers route. This is the OpenAI-compatible router
path for chat/inference, not Hub browsing, model-card inspection, uploads, or
artifact export.

Fresh shared config writes to `~/.codewhale/config.toml`. Existing
`~/.deepseek/config.toml` files are still read for compatibility.

### Wire Protocol Compatibility

Provider selection is explicit. A model string prefix such as
`deepseek-ai/...`, `deepseek/...`, `qwen/...`, or `arcee-ai/...` is a
provider-owned wire ID or catalog namespace hint under the selected provider.
It is not a provider switch and must not be treated as proof that the route is
DeepSeek, OpenRouter, or any other provider.

Set the route with `provider = "<id>"`, `CODEWHALE_PROVIDER=<id>`, or
`codewhale --provider <id>`. Set the request model with `CODEWHALE_MODEL`, a
provider-specific model env var, top-level `default_text_model`, or
`[providers.<table>].model`. Set the endpoint with `CODEWHALE_BASE_URL`, a
provider-specific base URL env var, or `[providers.<table>].base_url`. Set auth
with `codewhale auth set --provider <id>`, `[providers.<table>].api_key`, or
the listed provider env vars.

| Provider ID | TOML table | Wire protocol | Auth env vars |
| --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | OpenAI Chat Completions | `DEEPSEEK_API_KEY` |
| `deepseek-anthropic` | `[providers.deepseek_anthropic]` | Anthropic Messages | `DEEPSEEK_API_KEY` |
| `nvidia-nim` | `[providers.nvidia_nim]` | OpenAI Chat Completions | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, `DEEPSEEK_API_KEY` |
| `openai` | `[providers.openai]` | OpenAI Chat Completions | `OPENAI_API_KEY` |
| `atlascloud` | `[providers.atlascloud]` | OpenAI Chat Completions | `ATLASCLOUD_API_KEY` |
| `wanjie-ark` | `[providers.wanjie_ark]` | OpenAI Chat Completions | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` |
| `volcengine` | `[providers.volcengine]` | OpenAI Chat Completions | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` |
| `openrouter` | `[providers.openrouter]` | OpenAI Chat Completions | `OPENROUTER_API_KEY` |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | OpenAI Chat Completions | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` |
| `novita` | `[providers.novita]` | OpenAI Chat Completions | `NOVITA_API_KEY` |
| `fireworks` | `[providers.fireworks]` | OpenAI Chat Completions | `FIREWORKS_API_KEY` |
| `siliconflow` | `[providers.siliconflow]` | OpenAI Chat Completions | `SILICONFLOW_API_KEY` |
| `arcee` | `[providers.arcee]` | OpenAI Chat Completions | `ARCEE_API_KEY` |
| `siliconflow-CN` | `[providers.siliconflow_cn]` | OpenAI Chat Completions | `SILICONFLOW_API_KEY` |
| `moonshot` | `[providers.moonshot]` | OpenAI Chat Completions | `MOONSHOT_API_KEY`, `KIMI_API_KEY` |
| `sglang` | `[providers.sglang]` | OpenAI Chat Completions | `SGLANG_API_KEY` |
| `vllm` | `[providers.vllm]` | OpenAI Chat Completions | `VLLM_API_KEY` |
| `ollama` | `[providers.ollama]` | Ollama-local OpenAI-compatible Chat Completions | `OLLAMA_API_KEY` |
| `huggingface` | `[providers.huggingface]` | OpenAI Chat Completions | `HUGGINGFACE_API_KEY`, `HF_TOKEN` |
| `together` | `[providers.together]` | OpenAI Chat Completions | `TOGETHER_API_KEY` |
| `qianfan` | `[providers.qianfan]` | OpenAI Chat Completions | `QIANFAN_API_KEY`, `BAIDU_QIANFAN_API_KEY` |
| `openai-codex` | `[providers.openai_codex]` | OpenAI Responses | `OPENAI_CODEX_ACCESS_TOKEN`, `CODEX_ACCESS_TOKEN` |
| `anthropic` | `[providers.anthropic]` | Anthropic Messages | `ANTHROPIC_API_KEY` |
| `openmodel` | `[providers.openmodel]` | Anthropic Messages | `OPENMODEL_API_KEY` |
| `zai` | `[providers.zai]` | OpenAI Chat Completions | `ZAI_API_KEY`, `Z_AI_API_KEY` |
| `stepfun` | `[providers.stepfun]` | OpenAI Chat Completions | `STEPFUN_API_KEY`, `STEP_API_KEY` |
| `minimax` | `[providers.minimax]` | OpenAI Chat Completions | `MINIMAX_API_KEY` |
| `deepinfra` | `[providers.deepinfra]` | OpenAI Chat Completions | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` |
| `sakana` | `[providers.sakana]` | OpenAI Chat Completions | `FUGU_API_KEY`, `SAKANA_API_KEY` |
| `longcat` | `[providers.longcat]` | OpenAI Chat Completions | `LONGCAT_API_KEY` |
| `opencode-go` | `[providers.opencode_go]` | OpenAI Chat Completions | `OPENCODE_GO_API_KEY` |
| `opencode-zen` | `[providers.opencode_zen]` | Model-aware: OpenAI Responses, Anthropic Messages, or OpenAI Chat Completions | `OPENCODE_ZEN_API_KEY`, `OPENCODE_API_KEY` |
| `meta` | `[providers.meta]` | OpenAI Chat Completions | `META_MODEL_API_KEY`, `MODEL_API_KEY` |
| `xai` | `[providers.xai]` | OpenAI Chat Completions | `XAI_API_KEY` |

Default base URLs and models for each route are listed in the shipped provider
table below. The wire protocol values above are derived from
`crates/config/src/provider.rs`: `ChatCompletions` is the default,
`openai-codex` overrides to `Responses`; `deepseek-anthropic`, `anthropic`, and
`openmodel` override to `AnthropicMessages`; and `opencode-zen` resolves the
protocol from the selected model's curated offering.

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
- Multiple named OpenAI-compatible gateways, or local routes you want to pin
  from an AgentProfile, can use a custom table such as
  `[providers.lm-studio] kind = "openai-compatible"` and select it with
  `provider = "lm-studio"` or a profile `provider = "lm-studio"`.
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

Alibaba Bailian / Model Studio DashScope exposes Qwen through an
OpenAI-compatible Chat Completions endpoint. Configure it as an explicit
`openai` provider route so the API key, base URL, and wire model stay scoped to
that provider:

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_DASHSCOPE_API_KEY"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
context_window = 1000000
```

The Singapore endpoint above sends chat requests to `/chat/completions`;
Alibaba also documents regional `compatible-mode/v1` base URLs for Virginia,
Beijing, Hong Kong, and Frankfurt. Keep the API key and base URL from the same
region. The `qwen-plus` model ID is preserved as an OpenAI provider-scoped wire
ID; Codewhale does not infer a switch to OpenRouter, DeepSeek, or another
provider from the `qwen` prefix. Set `context_window` to the gateway/model's
real total context window when it differs from Codewhale's static model
metadata.

Private gateways with broken or intercepted certificates should use
`SSL_CERT_FILE` with a trusted CA bundle. The legacy
`insecure_skip_tls_verify = true` key is still parsed so `codewhale doctor` can
report stale configs, but provider clients reject it instead of skipping TLS
certificate verification.

Keep `provider`, `api_key`, and `base_url` in user config or process
environment. Project-local config overlays intentionally cannot set those keys,
so a repository cannot silently redirect prompts or credentials to another
endpoint.

## Credential Links

Provider setup surfaces should link users to provider-owned credential pages
instead of leaving them to search from a missing-key error. The runtime copy uses
the same links where possible.

| Provider ID | Credential or console link |
| --- | --- |
| `deepseek` | [DeepSeek API keys](https://platform.deepseek.com/api_keys) |
| `nvidia-nim` | [NVIDIA NIM API keys](https://build.nvidia.com/settings/api-keys) |
| `openai` | [OpenAI API keys](https://platform.openai.com/api-keys) |
| `atlascloud` | [Atlas Cloud API keys](https://atlascloud.ai/docs/en/api-keys) |
| `wanjie-ark` | [Wanjie MaaS APIKEY docs](https://docs.wanjiedata.com/maas/maas-openapi-v1.html) |
| `volcengine` | [Volcengine Ark console](https://console.volcengine.com/ark) |
| `openrouter` | [OpenRouter keys](https://openrouter.ai/settings/keys) |
| `xiaomi-mimo` | [Xiaomi MiMo Token Plan](https://platform.xiaomimimo.com/token-plan) |
| `novita` | [Novita quickstart](https://novita.ai/docs/guides/quickstart) |
| `fireworks` | [Fireworks API keys](https://fireworks.ai/account/api-keys) |
| `siliconflow`, `siliconflow-CN` | [SiliconFlow API keys](https://cloud.siliconflow.com/account/ak) |
| `arcee` | [Arcee API key guide](https://docs.arcee.ai/other/create-your-first-api-key) |
| `moonshot` | [Kimi Open Platform](https://platform.kimi.ai/) |
| `zai` | [Z.ai model API](https://z.ai/model-api) |
| `stepfun` | [StepFun Open Platform](https://platform.stepfun.ai/) |
| `minimax` | [MiniMax prerequisites](https://platform.minimax.io/docs/guides/quickstart-preparation) |
| `minimax-anthropic` | [MiniMax prerequisites](https://platform.minimax.io/docs/guides/quickstart-preparation) |
| `huggingface` | [Hugging Face tokens](https://huggingface.co/settings/tokens) |
| `deepinfra` | [DeepInfra API keys](https://deepinfra.com/dash/api_keys) |
| `together` | [Together API keys](https://api.together.ai/settings/api-keys) |
| `anthropic` | [Anthropic API keys](https://console.anthropic.com/settings/keys) |
| `openmodel` | [OpenModel API key guide](https://docs.openmodel.ai/en/docs/guides/api-key) |
| `openai-codex` | Reuses `codex login`; no Codewhale API key is stored. |
| `sglang`, `vllm`, `ollama` | Local OpenAI-compatible endpoints can run without an API key on localhost. |
| `sakana` | [Sakana AI API](https://api.sakana.ai/) |
| `longcat` | [Meituan LongCat platform](https://longcat.chat/platform) |
| `opencode-go` | [OpenCode Go](https://opencode.ai/docs/go/) |
| `opencode-zen` | [OpenCode Zen](https://opencode.ai/docs/zen/) |
| `meta` | [Meta Model API](https://developer.meta.com/ai/) |
| `xai` | [xAI Console](https://console.x.ai/) |

## Shipped Providers

| Provider ID | TOML table | Auth env | Base URL env and default | Default or static models | Notes |
| --- | --- | --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | `DEEPSEEK_API_KEY` | `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL`; default `https://api.deepseek.com/beta` | `deepseek-v4-pro`, `deepseek-v4-flash`; compatibility aliases `deepseek-chat`, `deepseek-reasoner` | First-class default. Beta URL enables strict tool mode, chat prefix completion, and FIM completion. Set `https://api.deepseek.com` or `/v1` explicitly to opt out of beta-only features. |
| `deepseek-anthropic` | `[providers.deepseek_anthropic]` | `DEEPSEEK_API_KEY` | `DEEPSEEK_ANTHROPIC_BASE_URL`; default `https://api.deepseek.com/anthropic` | `deepseek-v4-pro`, `deepseek-v4-flash`; compatibility aliases `deepseek-chat`, `deepseek-reasoner` | Opt-in DeepSeek route for the Anthropic Messages wire protocol. Uses `/v1/messages`, `x-api-key`, and `anthropic-version: 2023-06-01`. Keep `provider = "deepseek"` for the default Chat Completions path. |
| `nvidia-nim` | `[providers.nvidia_nim]` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, fallback `DEEPSEEK_API_KEY` | `NVIDIA_NIM_BASE_URL`, `NIM_BASE_URL`, `NVIDIA_BASE_URL`; default `https://integrate.api.nvidia.com/v1` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | Hosted DeepSeek V4 through NVIDIA NIM. `NVIDIA_NIM_MODEL` is accepted by the TUI config path. |
| `openai` | `[providers.openai]` | `OPENAI_API_KEY` | `OPENAI_BASE_URL`; default `https://api.openai.com/v1` | Registry entries: `deepseek-v4-pro`, `deepseek-v4-flash`, `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`; default config model `deepseek-v4-pro` | Generic OpenAI-compatible route for gateways and custom endpoints, including Alibaba Bailian / Model Studio DashScope when configured with that endpoint. The [GPT-5.6 family](https://developers.openai.com/api/docs/models/gpt-5.6-sol) uses OpenAI's documented 1.05M context, 128K max output, and reasoning levels. Use this for explicit third-party OpenAI-compatible routes instead of inventing a new provider ID. `OPENAI_MODEL` is accepted. |
| `atlascloud` | `[providers.atlascloud]` | `ATLASCLOUD_API_KEY` | `ATLASCLOUD_BASE_URL`; default `https://api.atlascloud.ai/v1` | Default `deepseek-ai/deepseek-v4-flash`; explicit `vendor/model-id` values pass through when AtlasCloud is selected | OpenAI-compatible hosted route. `ATLASCLOUD_MODEL` is accepted by the TUI config path, the static `ModelRegistry` keeps DeepSeek V4 fallback rows, and provider-hinted CLI model IDs are sent to AtlasCloud exactly as requested. Use Atlas Cloud's own catalog or Coding Plan page for the current provider-owned model list and pricing. |
| `wanjie-ark` | `[providers.wanjie_ark]` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` | `WANJIE_ARK_BASE_URL`, `WANJIE_BASE_URL`, `WANJIE_MAAS_BASE_URL`; default `https://maas-openapi.wanjiedata.com/api/v1` | `deepseek-reasoner` | OpenAI-compatible hosted route. `WANJIE_ARK_MODEL`, `WANJIE_MODEL`, and `WANJIE_MAAS_MODEL` are accepted. |
| `volcengine` | `[providers.volcengine]` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` | `VOLCENGINE_BASE_URL`, `VOLCENGINE_ARK_BASE_URL`, `ARK_BASE_URL`; default `https://ark.cn-beijing.volces.com/api/coding/v3` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | Volcengine/Volcano Engine Ark OpenAI-compatible coding endpoint. `VOLCENGINE_MODEL` and `VOLCENGINE_ARK_MODEL` are accepted. |
| `openrouter` | `[providers.openrouter]` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`; default `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`; recent large IDs include `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `google/gemma-4-31b-it`, `z-ai/glm-5.1`, `z-ai/glm-5.2`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6` | Additive open-model routing layer. It does not replace DeepSeek; it lets users route supported model IDs through OpenRouter when they choose it. |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` | `XIAOMI_MIMO_BASE_URL`, `MIMO_BASE_URL`, `XIAOMI_MIMO_MODE`, `MIMO_MODE`; default `https://token-plan-sgp.xiaomimimo.com/v1` | Chat: `mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5`; speech/TTS: `mimo-v2.5-tts`, `mimo-v2.5-tts-voicedesign`, `mimo-v2.5-tts-voiceclone`, `mimo-v2-tts` | Xiaomi MiMo OpenAI-compatible chat completions route. Token Plan keys (`tp-...`) use `api-key` auth and the token-plan endpoint by default; pay-as-you-go mode uses standard API keys (`sk-...`) and `https://api.xiaomimimo.com/v1`. It sends `max_completion_tokens` and uses MiMo's `thinking` field for reasoning control. Token Plan cost/usage is credit/quota based; Codewhale shows it as unknown until Xiaomi exposes a reliable balance API. `codewhale speech` / `tts` uses the TTS models. |
| `novita` | `[providers.novita]` | `NOVITA_API_KEY` | `NOVITA_BASE_URL`; default `https://api.novita.ai/openai/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | OpenAI-compatible hosted route for DeepSeek model IDs. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `fireworks` | `[providers.fireworks]` | `FIREWORKS_API_KEY` | `FIREWORKS_BASE_URL`; default `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro` | OpenAI-compatible hosted route. Use config or `CODEWHALE_MODEL` / `DEEPSEEK_MODEL` for model overrides. |
| `siliconflow` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`; default `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | OpenAI-compatible hosted route. Official docs use the `.com` endpoint. `SILICONFLOW_MODEL` is accepted. Reasoning aliases `deepseek-reasoner` and `deepseek-r1` map to Pro; `deepseek-chat` and `deepseek-v3` map to Flash. |
| `siliconflow-CN` | `[providers.siliconflow_cn]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`; default `https://api.siliconflow.cn/v1` | Uses the SiliconFlow model set | China regional SiliconFlow route. Falls back to `[providers.siliconflow]` for api_key / base_url / model when unset. Select it with `provider = "siliconflow-CN"` or `CODEWHALE_PROVIDER=siliconflow-CN`. |
| `arcee` | `[providers.arcee]` | `ARCEE_API_KEY` | `ARCEE_BASE_URL`; default `https://api.arcee.ai/api/v1` | `trinity-large-thinking`, `trinity-large-preview` | Arcee AI direct OpenAI-compatible route, tracked as 256K-context BF16 serving. `ARCEE_MODEL` is accepted. OpenRouter's `arcee-ai/trinity-large-thinking` remains the OpenRouter namespaced model ID; direct Arcee uses the bare `trinity-large-thinking` ID. |
| `moonshot` | `[providers.moonshot]` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` | `MOONSHOT_BASE_URL`, `KIMI_BASE_URL`; default `https://api.moonshot.ai/v1` | `kimi-k2.7-code`, `kimi-k2.6`; Kimi Code path uses `kimi-for-coding` at `https://api.kimi.com/coding/v1` | Moonshot/Kimi route. `kimi` and `kimi-k2` aliases select `kimi-k2.7-code`; `MOONSHOT_MODEL`, `KIMI_MODEL_NAME`, and `KIMI_MODEL` are accepted. Kimi thinking streams through `reasoning_content`; Codewhale keeps it in Thinking cells and replays it for thinking/tool-call continuity. `[providers.moonshot] auth_mode = "kimi_oauth"` reads Kimi Code OAuth credentials from `KIMI_CODE_HOME`/`~/.kimi-code`, with legacy `KIMI_SHARE_DIR`/`~/.kimi` fallback. |
| `zai` | `[providers.zai]` | `ZAI_API_KEY`, `Z_AI_API_KEY` | `ZAI_BASE_URL`, `Z_AI_BASE_URL`; default `https://api.z.ai/api/coding/paas/v4`; general API `https://api.z.ai/api/paas/v4` | `GLM-5.2` default; `GLM-5.1`, `GLM-5-Turbo` available | Z.AI GLM Coding Plan route. `GLM-5.2` is the default; set `model = "GLM-5.1"` or `ZAI_MODEL=GLM-5.1` for the smaller model, or `GLM-5-Turbo` for the fast variant used by faster/explore sub-agents. |
| `stepfun` | `[providers.stepfun]` | `STEPFUN_API_KEY`, `STEP_API_KEY` | `STEPFUN_BASE_URL`, `STEP_BASE_URL`; default `https://api.stepfun.ai/v1`; Coding Plan endpoint `https://api.stepfun.ai/step_plan/v1` | `step-3.7-flash` | StepFun / StepFlash direct OpenAI-compatible route. Set `[providers.stepfun].base_url` or `STEP_BASE_URL` to the Coding Plan URL when using that plan. Offline accounting labels recognized routes as `stepfun-payg` or `stepfun-plan` without persisting the raw endpoint, and only the standard PAYG route receives token pricing. `STEPFUN_MODEL` and `STEP_MODEL` are accepted. |
| `minimax` | `[providers.minimax]` | `MINIMAX_API_KEY` | `MINIMAX_BASE_URL`; default `https://api.minimax.io/v1`; China `https://api.minimaxi.com/v1` | `MiniMax-M3`, `MiniMax-M2.7`, `MiniMax-M2.7-highspeed`, `MiniMax-M2.5`, `MiniMax-M2.5-highspeed`, `MiniMax-M2.1`, `MiniMax-M2.1-highspeed`, `MiniMax-M2` | MiniMax direct OpenAI-compatible route. Codewhale sends `reasoning_split = true` so MiniMax thinking arrives separately from answer text. Official M3 input modalities are text, image, and video; M2.7 is text-only. |
| `minimax-anthropic` | `[providers.minimax_anthropic]` | `MINIMAX_API_KEY` | `MINIMAX_ANTHROPIC_BASE_URL`; default `https://api.minimax.io/anthropic`; China `https://api.minimaxi.com/anthropic` | `MiniMax-M3`, `MiniMax-M2.7` | MiniMax direct Anthropic-compatible Messages route. Keep the `/anthropic` suffix because Codewhale appends `/v1/messages`; the route uses `x-api-key`. M3 supports adaptive or disabled thinking. M2.7 always keeps thinking enabled. |
| `sglang` | `[providers.sglang]` | Optional `SGLANG_API_KEY` | `SGLANG_BASE_URL`; default `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted OpenAI-compatible route. Localhost deployments commonly omit auth. `SGLANG_MODEL` is accepted. |
| `vllm` | `[providers.vllm]` | Optional `VLLM_API_KEY` | `VLLM_BASE_URL`; default `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Self-hosted vLLM OpenAI-compatible route. Localhost deployments commonly omit auth. `VLLM_MODEL` is accepted. |
| `ollama` | `[providers.ollama]` | Optional `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`; default `http://localhost:11434/v1` | `deepseek-coder:1.3b`; provider-hinted custom tags pass through | Self-hosted Ollama OpenAI-compatible route. Localhost deployments commonly omit auth. `OLLAMA_MODEL` is accepted. |
| `huggingface` | `[providers.huggingface]` | `HUGGINGFACE_API_KEY`, `HF_TOKEN` | `HUGGINGFACE_BASE_URL`, `HF_BASE_URL`; default `https://router.huggingface.co/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Hugging Face Inference Providers OpenAI-compatible router route. Accepted aliases: `huggingface`, `hugging-face`, `hugging_face`, `hf`. Org-prefixed model IDs pass through. `HUGGINGFACE_MODEL` and `HF_MODEL` are accepted. Hub browsing/export are separate future features. |
| `deepinfra` | `[providers.deepinfra]` | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` | `DEEPINFRA_BASE_URL`; default `https://api.deepinfra.com/v1/openai` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | DeepInfra OpenAI-compatible route. Drop-in replacement for OpenAI SDK. |
| `together` | `[providers.together]` | `TOGETHER_API_KEY` | `TOGETHER_BASE_URL`; default `https://api.together.xyz/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash`, `thinkingmachines/inkling` | Together AI OpenAI-compatible route. `TOGETHER_MODEL` is accepted. Model aliases `deepseek-v4-pro` and `deepseek-v4-flash` normalize to Together's org-prefixed IDs; `inkling` and `together-inkling` normalize to Together's published lowercase Inkling wire ID. Inkling uses the exact `none`/`minimal`/`low`/`medium`/`high`/`max` reasoning vocabulary from Thinking Machines' [official model repository](https://huggingface.co/thinkingmachines/Inkling). Together's [launch post](https://www.together.ai/blog/together-ai-brings-thinking-machines-labs-new-model-inkling-on-day-0) currently says Inkling is live with 1M context, while its [model detail page](https://www.together.ai/models/inkling) says coming soon with 256K context and publishes no price. Until Together's active `/models` endpoint and the Models.dev catalog resolve that conflict, Inkling is not seeded into Codewhale's offline picker and no route-specific context or cost is inferred. |
| `qianfan` | `[providers.qianfan]` | `QIANFAN_API_KEY`, `BAIDU_QIANFAN_API_KEY` | `QIANFAN_BASE_URL`, `BAIDU_QIANFAN_BASE_URL`; default `https://api.baiduqianfan.ai/v1` | `ernie-4.0-turbo-8k`; provider-scoped custom Qianfan service/model IDs pass through | Baidu Qianfan OpenAI-compatible route. Requests use Bearer auth and Chat Completions payloads. `QIANFAN_MODEL` and `BAIDU_QIANFAN_MODEL` are accepted; aliases `baidu-qianfan`, `baidu_qianfan`, and `baidu` resolve to this provider. Tool/function calling is model-scoped in Qianfan docs, so Codewhale preserves the selected wire model and leaves live capability proof to follow-up route/capability work. |
| `openai-codex` | `[providers.openai_codex]` | OAuth via `codex login` (`~/.codex/auth.json`); env override `OPENAI_CODEX_ACCESS_TOKEN`, `CODEX_ACCESS_TOKEN` | `OPENAI_CODEX_BASE_URL`/`CODEX_BASE_URL`; default `https://chatgpt.com/backend-api` | `gpt-5.5` | **Experimental.** Reuses your existing ChatGPT/Codex CLI OAuth login and talks to the OpenAI Responses API at `/codex/responses`. The access token is read and refreshed from `~/.codex/auth.json`; no API key is stored. `OPENAI_CODEX_MODEL`/`CODEX_MODEL` and `OPENAI_CODEX_ACCOUNT_ID`/`CODEX_ACCOUNT_ID` are accepted. Codewhale budgets this route with the 400K Codex-family effective context window even when the public API model table lists a larger native `gpt-5.5` window. |
| `anthropic` | `[providers.anthropic]` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL`; default `https://api.anthropic.com` | `claude-opus-4-8`, `claude-sonnet-4-6` (default), `claude-haiku-4-5` | Native Anthropic Messages API route (`/v1/messages`, `x-api-key` + `anthropic-version: 2023-06-01`) — not OpenAI-compatible. Prompt caching via `cache_control` breakpoints, adaptive thinking + `output_config.effort`, signed thinking blocks replayed verbatim, cache telemetry normalized per #2961. `ANTHROPIC_MODEL` is accepted. |
| `openmodel` | `[providers.openmodel]` | `OPENMODEL_API_KEY` | `OPENMODEL_BASE_URL`; default `https://api.openmodel.ai` | `deepseek-v4-flash`; provider-scoped custom model IDs pass through | OpenModel Anthropic-compatible Messages route. Uses `/v1/messages`, Bearer auth, and `anthropic-version: 2023-06-01`; OpenModel selects DeepSeek, DashScope, Xiaomi, Claude, and other routes by model id. `OPENMODEL_MODEL` is accepted. |
| `sakana` | `[providers.sakana]` | `FUGU_API_KEY`, `SAKANA_API_KEY` | `SAKANA_BASE_URL`; default `https://api.sakana.ai/v1` | `fugu` (default), `fugu-ultra-20260615` | Sakana AI Fugu OpenAI-compatible route. Standard Chat Completions wire protocol; streaming supported. `fugu-ultra-20260615` is the heavy/reasoning variant. Env var aliases: `FUGU_API_KEY` (primary), `SAKANA_API_KEY`; provider aliases: `sakana-ai`, `sakana_ai`, `fugu`. |
| `longcat` | `[providers.longcat]` | `LONGCAT_API_KEY` | `LONGCAT_BASE_URL`; default `https://api.longcat.chat/openai/v1` | `LongCat-2.0` (default) | Meituan LongCat curated model gateway. OpenAI-compatible Chat Completions wire protocol. Sign up at https://longcat.chat/platform for an API key. Provider aliases: `long-cat`, `meituan-longcat`, `meituan`. |
| `opencode-go` | `[providers.opencode_go]` | `OPENCODE_GO_API_KEY` | `OPENCODE_GO_BASE_URL`; default `https://opencode.ai/zen/go/v1` | `deepseek-v4-pro` (default), `glm-5.2`, `glm-5.1`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro` | [OpenCode Go](https://opencode.ai/docs/go/) subscription route using OpenAI-compatible Chat Completions. `OPENCODE_GO_MODEL` is accepted. Codewhale uses bare wire IDs; familiar `opencode-go/<model-id>` input aliases normalize to the bare ID. Go models documented only on the Anthropic `/messages` endpoint are deliberately not advertised by this route until Codewhale supports per-model wire selection. Billing surfaces show the Go allowance instead of token-price estimates. |
| `opencode-zen` | `[providers.opencode_zen]` | `OPENCODE_ZEN_API_KEY`, fallback `OPENCODE_API_KEY` | `OPENCODE_ZEN_BASE_URL`; default `https://opencode.ai/zen/v1` | `gpt-5.5` (default); current documented GPT, Claude, Qwen, DeepSeek, MiniMax, GLM, Kimi, Grok, and free-model IDs | [OpenCode Zen](https://opencode.ai/docs/zen/) model-aware gateway. `OPENCODE_ZEN_MODEL` is accepted, and official `opencode/<model-id>` selectors normalize to bare wire IDs. GPT rows use `/responses`; Claude and Qwen rows use `/messages`; DeepSeek, MiniMax, GLM, Kimi, Grok, and the listed free rows use `/chat/completions`. All use the Zen API key with Bearer auth; the route never uses ChatGPT/Codex OAuth guidance or headers. Gemini currently fails closed because its model-specific Google wire protocol is not implemented. Unknown models also fail closed until their protocol is present in the curated catalog. |
| `meta` | `[providers.meta]` | `META_MODEL_API_KEY`, `MODEL_API_KEY` | `META_MODEL_API_BASE_URL`, `MODEL_API_BASE_URL`; default `https://api.meta.ai/v1` | `muse-spark-1.1` (default) | [Meta Model API](https://developer.meta.com/ai/resources/blog/build-with-muse-spark/) public-preview route using OpenAI-compatible Chat Completions. Muse Spark 1.1 keeps its wire ID, tool support, 1M context, 32K output metadata, and `none` through `xhigh` reasoning effort. `META_MODEL_API_MODEL` and `MODEL_API_MODEL` are accepted. Provider aliases: `meta-ai`, `meta_model_api`, `muse`, `muse-spark`. |
| `xai` | `[providers.xai]` | `XAI_API_KEY` **or** OAuth via `auth_mode = "oauth"` (`~/.grok/auth.json` / device-code) | `XAI_BASE_URL`; default `https://api.x.ai/v1` | `grok-4.5` (default), `grok-4.3`, `grok-build`, `grok-composer-2.5-fast`, `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning` | xAI/Grok OpenAI-compatible Chat Completions route. **API-key** (default): Bearer token from console.x.ai via `XAI_API_KEY` / keyring / `api_key`. **OAuth**: set `[providers.xai] auth_mode = "oauth"` to reuse the official Grok CLI token file (`~/.grok/auth.json`, `$GROK_HOME/auth.json`, or `GROK_AUTH_PATH`); tokens refresh against `https://auth.x.ai/oauth2/token` before expiry. Device-code login is available via `crate::xai_oauth::device_code_login` (verification URL + user code; no localhost callback — SSH/headless friendly). OAuth may return HTTP 403 on some SuperGrok tiers — keep API-key as the reliable fallback. `XAI_MODEL` is accepted. Provider aliases: `x-ai`, `x_ai`, `grok`. |

### OpenCode Zen protocol catalog

The bundled Zen transport snapshot follows the [official endpoint
table](https://opencode.ai/docs/zen/) and is intentionally explicit:

- Responses: `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`,
  `gpt-5.5-pro`, `gpt-5.4`, `gpt-5.4-pro`, `gpt-5.4-mini`, `gpt-5.4-nano`,
  `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.2`, `gpt-5.2-codex`,
  `gpt-5.1`, `gpt-5.1-codex`, `gpt-5.1-codex-max`,
  `gpt-5.1-codex-mini`, `gpt-5`, `gpt-5-codex`, `gpt-5-nano`.
- Anthropic Messages: `claude-fable-5`, `claude-opus-4-8`,
  `claude-opus-4-7`, `claude-opus-4-6`, `claude-opus-4-5`,
  `claude-sonnet-5`, `claude-sonnet-4-6`, `claude-sonnet-4-5`,
  `claude-haiku-4-5`, `qwen3.7-max`, `qwen3.7-plus`, `qwen3.6-plus`,
  `qwen3.5-plus`.
- Chat Completions: `deepseek-v4-pro`, `deepseek-v4-flash`, `minimax-m3`,
  `minimax-m2.7`, `minimax-m2.5`, `glm-5.2`, `glm-5.1`, `glm-5`,
  `kimi-k2.5`, `kimi-k2.6`, `kimi-k2.7-code`, `grok-4.5`,
  `grok-build-0.1`, `big-pickle`, `mimo-v2.5-free`,
  `north-mini-code-free`, `nemotron-3-ultra-free`,
  `deepseek-v4-flash-free`.

Gemini entries are excluded because the official table assigns them Google's
model-specific protocol. A catalog miss never falls back to another Zen wire
shape, including when a custom Zen base URL is configured.

### Hugging Face Provider vs MCP vs Hub

Codewhale's `huggingface` provider ID is only the OpenAI-compatible chat
inference route through Hugging Face Inference Providers. It is selected with
`/provider huggingface`, `CODEWHALE_PROVIDER=huggingface`, or
`provider = "huggingface"`.

Hugging Face MCP is a separate external-tool route. Configure it through the
MCP config described in `docs/MCP.md`, preferably using the settings-generated
snippet from <https://huggingface.co/settings/mcp>. In the TUI, `/hf mcp status`
checks whether the Hugging Face MCP server appears in the resolved MCP config,
`/hf mcp setup` prints the settings workflow and a placeholder-only shape, and
`/hf concepts` explains the provider/MCP/Hub distinction.

Hub publishing or repository management remains explicit user action through
Hub-native tooling such as `huggingface_hub` or git. The `/hf` helper does not
upload to Hugging Face and does not perform direct Hugging Face Hub HTTP search.

### Xiaomi MiMo Notes

`xiaomi-mimo` defaults to `mimo-v2.5-pro` for long-context reasoning and coding
work. The chat picker also exposes `mimo-v2.5-pro-ultraspeed` and the latest
Omni model `mimo-v2.5`. Xiaomi MiMo TTS is available through
`codewhale --provider xiaomi-mimo speech "text" --model tts` (or the `tts`
alias) plus model-visible `speech` / `tts` tools in Agent/YOLO mode.

`/provider xiaomi-mimo ultraspeed` and `/provider xiaomi-mimo pro-ultraspeed`
both select `mimo-v2.5-pro-ultraspeed`. Speech aliases such as `tts`,
`voice-design`, and `voice-clone` are separate from normal chat defaults.

Token Plan keys default to the Singapore endpoint
`https://token-plan-sgp.xiaomimimo.com/v1`. If your MiMo account is provisioned
for the China region, set `base_url = "https://token-plan-cn.xiaomimimo.com/v1"`
explicitly in `[providers.xiaomi_mimo]` or set `mode = "token-plan-cn"`. Europe
Token Plan accounts can set
`base_url = "https://token-plan-ams.xiaomimimo.com/v1"` or use
`mode = "token-plan-ams"`; `mode = "pay-as-you-go"`
selects the standard API endpoint and standard MiMo key family. Xiaomi Token
Plan docs and console expose credit/quota semantics, but Codewhale does not
currently have a documented balance endpoint to poll, so cost display remains
unknown rather than reusing token-price estimates from another provider.
Evidence captured from Xiaomi's official docs on 2026-06-23 lives in
[`docs/evidence/xiaomi-mimo-2026-06-23/`](evidence/xiaomi-mimo-2026-06-23/);
those notes override the secondary workbook snapshot where they disagree.

Voice-design and voice-clone shorthands map to `mimo-v2.5-tts-voicedesign` and
`mimo-v2.5-tts-voiceclone`. Xiaomi's current
[image-understanding guide](https://platform.xiaomimimo.com/docs/en-US/usage-guide/multimodal-understanding/image-understanding)
includes `mimo-v2.5` for image input. Codewhale exposes image analysis through the
separate `[vision_model]` / `image_analyze` path; set that model to
`mimo-v2.5` when using MiMo for vision.

### OpenRouter-Compatible Base URLs

OpenRouter-compatible gateways should usually stay on the `openrouter`
provider with a provider-scoped `base_url` override instead of moving through
the generic `openai` route. That keeps OpenRouter-style reasoning, streaming,
cache usage, and namespaced wire model parsing attached to the selected route:

```toml
provider = "openrouter"

[providers.openrouter]
api_key = "sk-..."
base_url = "https://openrouter-compatible.example/v1"
model = "deepseek/deepseek-v4-pro"
```

Codewhale preserves the `deepseek/` wire-model prefix under the OpenRouter
provider scope; it does not infer a switch to the direct DeepSeek provider from
that model string. Cache fields such as `prompt_cache_hit_tokens`,
`prompt_cache_miss_tokens`, and `prompt_tokens_details.cached_tokens` are
parsed when the upstream gateway sends them. If a key/account type omits those
fields, Codewhale treats them as absent for that response rather than as a
different provider route.

### Recent OpenRouter Large Models

OpenRouter completions and static registry rows include the April 2026 onward
large models verified through OpenRouter's model metadata:
`arcee-ai/trinity-large-thinking`, `qwen/qwen3.6-flash`,
`qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`,
`qwen/qwen3.6-plus`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`,
`xiaomi/mimo-v2.5`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6`,
`z-ai/glm-5.1`, `z-ai/glm-5.2`, `z-ai/glm-5-turbo`, `tencent/hy3-preview`,
`google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, and
`nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`.
`minimax/minimax-m3` was added from OpenRouter's May 31, 2026 listing as a 1M
context multimodal model for coding, tool use, and long-horizon agentic work.
`z-ai/glm-5.2` is now the default GLM route on both the Z.AI Coding Plan and
OpenRouter; `GLM-5.1` / `z-ai/glm-5.1` remain available as the smaller model,
and `GLM-5-Turbo` / `z-ai/glm-5-turbo` serve as the faster same-family sibling
used by faster/explore sub-agents.

## Static Model Registry

`codewhale model list` and `codewhale model resolve` use the static registry in
`crates/agent/src/lib.rs`. This is not the same as live `/models` discovery.
Use `/models` or `codewhale models` to fetch model IDs from the active API
endpoint when the endpoint supports model listing.

| Provider | Static registry entries | Tool calls | Registry reasoning flag |
| --- | --- | --- | --- |
| `deepseek` | `deepseek-v4-pro`, `deepseek-v4-flash` | yes | yes |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | yes | yes |
| `openai` | `deepseek-v4-pro`, `deepseek-v4-flash`, `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | yes | yes |
| `atlascloud` | `deepseek-ai/deepseek-v4-flash`, `deepseek-ai/deepseek-v4-pro` | yes | yes |
| `wanjie-ark` | `deepseek-reasoner` | yes | yes |
| `volcengine` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | yes | yes |
| `openrouter` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`, `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `minimax/minimax-m2.7`, `xiaomi/mimo-v2.5-pro`, `xiaomi/mimo-v2.5`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `qwen/qwen3.7-max`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6`, `z-ai/glm-5.1`, `z-ai/glm-5.2`, `z-ai/glm-5-turbo`, `tencent/hy3-preview`, `google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`, `nvidia/nemotron-3-ultra-550b-a55b` | yes | yes |
| `xiaomi-mimo` | `mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5`; speech/TTS IDs are selected through `codewhale speech` / `tts` | yes | yes for chat models; no for speech/TTS models |
| `novita` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | yes | yes |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` | yes | yes |
| `siliconflow` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `arcee` | `trinity-large-thinking`, `trinity-large-preview`; provider-hinted custom model IDs pass through | yes | yes for `trinity-large-thinking`; no for `trinity-large-preview` |
| `moonshot` | `kimi-k2.7-code`, `kimi-k2.6` | yes | yes |
| `zai` | `GLM-5.2`, `GLM-5.1`, `GLM-5-Turbo`; provider-hinted custom model IDs pass through | yes | yes |
| `stepfun` | `step-3.7-flash` | yes | no |
| `minimax` | `MiniMax-M3`, `MiniMax-M2.7`, `MiniMax-M2.7-highspeed`, `MiniMax-M2.5`, `MiniMax-M2.5-highspeed`, `MiniMax-M2.1`, `MiniMax-M2.1-highspeed`, `MiniMax-M2` | yes | yes |
| `minimax-anthropic` | `MiniMax-M3`, `MiniMax-M2.7` | yes | yes |
| `sglang` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `vllm` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `ollama` | `deepseek-coder:1.3b`; custom tags pass through when provider hint is `ollama` | yes | no |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | no |
| `deepinfra` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | yes | yes |
| `together` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash`, `thinkingmachines/inkling` | yes | yes |
| `openai-codex` | `gpt-5.5` | yes | yes |
| `anthropic` | `claude-opus-4-8`, `claude-sonnet-4-6`, `claude-haiku-4-5` | yes | yes for `claude-opus-4-8` and `claude-sonnet-4-6`; no for `claude-haiku-4-5` |
| `openmodel` | `deepseek-v4-flash`; provider-scoped custom model IDs pass through | yes | model-dependent |
| `sakana` | `fugu`, `fugu-ultra-20260615` | yes | yes for `fugu-ultra-20260615` |
| `longcat` | `LongCat-2.0` | yes | yes |
| `opencode-go` | `glm-5.2`, `glm-5.1`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-pro`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro` | yes | yes |
| `meta` | `muse-spark-1.1` | yes | yes |
| `xai` | `grok-4.5`, `grok-4.3`, `grok-build`, `grok-composer-2.5-fast`, `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning` | yes | yes for `grok-4.5`, `grok-4.3`, `grok-build`, and `grok-4.20-0309-reasoning` |

AtlasCloud keeps the same default model as the config layer and adds
provider-scoped aliases for the Pro and Flash rows. Other AtlasCloud model IDs
should still be selected through `ATLASCLOUD_MODEL`, config, or live model
listing when available.

## Capability Metadata

`codewhale-tui doctor --json` exposes the `capability` object. It is static
metadata, not a live API probe. Current fields are:

`resolved_provider`, `resolved_model`, `context_window`, `max_output`,
`thinking_supported`, `cache_telemetry_supported`, and `request_payload_mode`.

Most shipped providers use the Chat Completions request payload mode. Native
Messages routes, including `minimax-anthropic`, use `/v1/messages`, and
`openai-codex` uses Responses.

For OpenAI-compatible gateways or self-hosted runtimes whose real window
differs from the static table, set `[providers.<name>] context_window = N`.
The configured value becomes the route-effective context window for prompts,
context-pressure checks, compaction, and output-cap budgeting.

| Provider/model class | Context window | Max output metadata | Thinking support | Cache telemetry | FIM endpoint |
| --- | --- | --- | --- | --- | --- |
| DeepSeek V4 (`deepseek-v4-pro`, `deepseek-v4-flash`) | 1,000,000 | 384,000 | yes | yes | DeepSeek beta only |
| DeepSeek compatibility aliases (`deepseek-chat`, `deepseek-reasoner`) | 1,000,000 | 384,000 | yes | yes | DeepSeek beta only |
| NVIDIA NIM V4 registry models | 1,000,000 | 384,000 | yes | yes | not documented in code |
| Volcengine Ark V4 model IDs | 1,000,000 | 384,000 | yes | yes | not documented in code |
| OpenRouter, Novita, Fireworks, SiliconFlow, SGLang, and vLLM V4 model IDs | 1,000,000 | 384,000 | yes | no | not documented in code |
| Xiaomi MiMo `mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5` | 1,000,000 | 131,072 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 Flash / Plus | 1,000,000 | 65,536 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 35B / 27B | 262,144 | 262,140 | yes | no | not documented in code |
| OpenRouter Qwen 3.6 Max Preview | 262,144 | 65,536 | yes | no | not documented in code |
| OpenAI API `gpt-5.5` | 1,050,000 | 128,000 | yes | no | not documented in code |
| OpenAI API `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | 1,050,000 | 128,000 | yes | no | not documented in code |
| Meta Model API `muse-spark-1.1` | 1,000,000 | 32,000 | yes | no | not documented in code |
| OpenAI Codex / ChatGPT route (`openai-codex`) | 400,000 effective | 128,000 | yes | no | route uses Responses payload at `/codex/responses` |
| OpenModel default/custom model IDs | 200,000 fallback unless model metadata or config overrides it | 64,000 fallback | model-dependent | no | route uses Messages payload at `/v1/messages` |
| Wanjie Ark `reasoner` / `r1` model IDs | 128,000 | 4,096 | yes | no | not documented in code |
| Direct Arcee API `trinity-large-thinking` | 262,144 | 262,144 | yes | no | not documented in code |
| Direct Arcee API `trinity-large-preview` | 262,144 | 4,096 | no in doctor capability metadata | no | not documented in code |
| Direct Moonshot/Kimi `kimi-k2.7-code`, `kimi-k2.6`, `kimi-for-coding` | 262,144 | 262,144 | yes | no | not documented in code |
| Direct Z.AI `GLM-5.2` (default) | 1,000,000 | 131,072 | yes | no | not documented in code |
| Direct Z.AI `GLM-5.1` | 202,752 | 131,072 | yes | no | not documented in code |
| Direct Z.AI `GLM-5-Turbo` | 202,752 | 131,072 | yes | no | faster/explore sub-agent sibling |
| Direct MiniMax `MiniMax-M3` | 1,000,000 | 524,288 | yes | no | not documented in code |
| Direct MiniMax M2.x models | 204,800 | 4,096 fallback until MiniMax output metadata is promoted | yes | no | not documented in code |
| MiniMax Messages route (`MiniMax-M3`, `MiniMax-M2.7`) | model-specific values above | model-specific values above | yes | no | route uses `/anthropic/v1/messages` |
| Generic `openai` and AtlasCloud | 128,000 | 4,096 | no in doctor capability metadata | no | not documented in code |
| Ollama | 8,192 | 4,096 | no | no | not documented in code |
| Hugging Face Inference Providers V4 model IDs | 131,072 | 4,096 | yes | no | not documented in code |
| Other recognized DeepSeek model IDs | 128,000 unless the model name carries an explicit `Nk` hint | 4,096 | no unless V4/reasoner logic matches | DeepSeek/NIM only | DeepSeek beta only |

MiniMax M3 uses input-length and service tiers. Codewhale omits
`service_tier`, so requests use the standard tier and cost estimates select the
correct standard rate from total input usage. Priority rates are listed to keep
the official tier structure visible. Prices are USD per million tokens.

| Model / service tier | Input length | Input | Output | Cache read | Cache write |
| --- | --- | ---: | ---: | ---: | ---: |
| `MiniMax-M3` standard | up to 512,000 input tokens | $0.30 | $1.20 | $0.06 | not published |
| `MiniMax-M3` standard | over 512,000 input tokens | $0.60 | $2.40 | $0.12 | not published |
| `MiniMax-M3` priority | up to 512,000 input tokens | $0.45 | $1.80 | $0.09 | not published |
| `MiniMax-M3` priority | over 512,000 input tokens | $0.90 | $3.60 | $0.18 | not published |
| `MiniMax-M2.7` standard | all supported inputs | $0.30 | $1.20 | $0.06 | $0.375 |

These values come from the [MiniMax pay-as-you-go pricing
guide](https://platform.minimax.io/docs/guides/pricing-paygo). M3 thinking is
adaptive or disabled; the OpenAI-compatible API defaults to adaptive and the
Anthropic-compatible API defaults to disabled. M2.7 thinking cannot be
disabled. Codewhale sends explicit controls when the user selects a reasoning
mode.

Tool-call support is tracked separately by the static `ModelRegistry` and by
the endpoint's ability to accept OpenAI-compatible `tools` payloads. A custom
OpenAI-compatible or local endpoint can still reject tool calls even if
Codewhale can send the schema.

### Hugging Face Inference Providers Notes

The shipped Hugging Face route targets the OpenAI-compatible Inference Providers
router at `https://router.huggingface.co/v1`. Configure auth with
`HUGGINGFACE_API_KEY` first, or `HF_TOKEN` as a fallback. Configure the endpoint
with `HUGGINGFACE_BASE_URL` first, or `HF_BASE_URL` as a fallback; configure the
model with `HUGGINGFACE_MODEL` first, or `HF_MODEL` as a fallback.

This route does not imply Hub browsing, model-card metadata, dataset access,
Jobs, uploads, or export. Those remain explicit Model Lab work items so
provider auth and artifact movement stay separate.

### When a Local Model Prints Tool JSON

Codewhale only executes tools when the provider returns Chat Completions
`tool_calls` or streamed `delta.tool_calls`. If a local model prints text such
as `{"name":"grep_files","arguments":{...}}` in the assistant message, that is
ordinary model output, not an executable tool request.

For OpenAI-compatible or local runtimes, check:

- The endpoint accepts the `tools` array in `/v1/chat/completions` requests.
- The selected model or chat template is configured for function/tool calls.
- The server returns `tool_calls` in the response rather than plain JSON text.
- The compatibility layer does not strip tools before forwarding the request.
- If in doubt, test a small `read_file` or `grep_files` request against a known
  tool-calling model before debugging Codewhale's tool registry.

Changing `provider`, `base_url`, or `model` can select a route that supports the
OpenAI-compatible payload shape, but Codewhale cannot convert arbitrary JSON
text into a trusted tool call after the model has emitted it as prose.

DeepSeek will retire `deepseek-chat` and `deepseek-reasoner` on 2026-07-24 at
15:59 UTC. Codewhale migrates either name to `deepseek-v4-flash` before a
request reaches DeepSeek's first-party OpenAI or Anthropic endpoint. If no
reasoning tier was configured, `deepseek-chat` also migrates to `off` and
`deepseek-reasoner` to `high`, preserving their former non-thinking / thinking
intent; an explicit `reasoning_effort` remains authoritative. The mapping is
deliberately not global: Wanjie Ark, aggregators, self-hosted runtimes, and
custom endpoints continue to own their model ids.

## Reasoning Effort

`/reasoning <effort>` (and the `reasoning_effort` config key) is translated to
each provider's wire dialect by the client before the request is sent. `off`
disables thinking where the dialect supports it; providers marked "omitted"
receive no reasoning fields at all for that tier.

| Provider | `off` | `low`/`medium`/`high` | `max`/`xhigh` |
| --- | --- | --- | --- |
| `deepseek`, `deepseek-cn`, `siliconflow`, `siliconflow-CN`, `sglang`, `volcengine`, `atlascloud` | `thinking: {type: disabled}` | `reasoning_effort: "high"` + `thinking: {type: enabled}` | `reasoning_effort: "max"` + `thinking: {type: enabled}` |
| `openrouter`, `novita`, other `together` models | `thinking: {type: disabled}` | `reasoning_effort` pass-through + `thinking: {type: enabled}` | `reasoning_effort: "xhigh"` + `thinking: {type: enabled}` |
| `together` + `thinkingmachines/inkling` | `reasoning_effort: "none"` | exact `minimal`/`low`/`medium`/`high` `reasoning_effort` | `reasoning_effort: "max"` |
| `moonshot` | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| `ollama` | `think: false` | `think: true` | `think: true` |
| `xiaomi-mimo` | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| `minimax` | `reasoning_split: true` + `thinking: {type: disabled}` | `reasoning_split: true` + `thinking: {type: adaptive}` | `reasoning_split: true` + `thinking: {type: adaptive}` |
| `nvidia-nim` | `chat_template_kwargs.thinking: false` | `chat_template_kwargs`: `thinking: true` + `reasoning_effort: "high"` | `chat_template_kwargs`: `thinking: true` + `reasoning_effort: "max"` |
| `vllm` | `chat_template_kwargs.enable_thinking: false` | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort` low/medium/high | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort: "high"` (vLLM has no max tier) |
| `arcee`, `huggingface` | omitted | `reasoning_effort` pass-through | `reasoning_effort: "high"` |
| `fireworks` | omitted | `reasoning_effort: "high"` | `reasoning_effort: "max"` |
| `openai`, `wanjie-ark` | omitted | omitted | omitted |
| `openmodel` | Anthropic Messages adapter handles thinking/output configuration | Anthropic Messages adapter handles thinking/output configuration | Anthropic Messages adapter handles thinking/output configuration |
| `openai-codex` | Responses API `reasoning` field (handled by the Responses bridge) | Responses API `reasoning` field | Responses API `reasoning` field |

AtlasCloud serves DeepSeek models, so it speaks the DeepSeek reasoning dialect,
including the `max` tier (#3024).

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
