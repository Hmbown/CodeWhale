# Model & Provider Metadata Audit

Audit date: **2026-07-12** · Repo state: `main` @ `3e97b278e` (v0.8.68 lane)
Scope: every provider and model CodeWhale knows about, their characteristics
(context window, max output, reasoning, tools, modalities, pricing, aliases),
where each fact lives in code, how the metadata layers interact, and every
discrepancy found against the live Models.dev catalog. Intended as the working
reference for a future metadata-consolidation pass.

---

## 1. Executive summary

1. **The "gpt-5.6-luna 272K" display is correct by design, not a data bug.**
   On the ChatGPT/Codex OAuth route, CodeWhale shows the context window that
   OpenAI's account-scoped `/models` endpoint advertises (persisted by the
   Codex CLI at `~/.codex/models_cache.json`). That cache on this machine
   (fetched 2026-07-13) advertises `context_window: 272000` for **all**
   gpt-5.x OAuth models (gpt-5.5, gpt-5.6-sol/terra/luna, gpt-5.4,
   gpt-5.4-mini; gpt-5.3-codex-spark is 128000). The public **API** route for
   the same model ids is 1,050,000 (922,000 input + 128,000 output). The
   deliberate policy — documented in `route_runtime.rs:33-51` and pinned by
   the test `same_model_id_uses_route_effective_api_and_oauth_metadata`
   (`model_picker.rs:1660-1706`) — is to never let the OAuth route inherit
   the API route's bigger window, output cap, or pricing.
   **Possible UX follow-up (see §8, A1):** label the value as the OAuth-route
   window (e.g. "272K ctx (ChatGPT route)") so it doesn't read as wrong data.

2. **Metadata is spread across seven layers** (see §2). The precedence is
   well-defined and test-guarded, but the *facts* are duplicated in at least
   four hand-maintained places (`models.rs`, `model_catalog.bundled.json`,
   `pricing.rs`, `models_dev.bundled.json`), which is where drift creeps in.

3. **Real drift found** against the live Models.dev catalog — 14 candidate
   mismatches (§7), the most defensible being GLM window (202,752 vs
   vendor 200,000), Qwen3.6-27b/35b output caps (we say 262,140; Alibaba says
   65,536), Grok 4.20 window (we say 2M; xAI catalog row says 1M), and
   MiniMax-M2 (we say 204,800; catalog says 196,608).

---

## 2. Metadata architecture — layers and precedence

Effective precedence for model facts (context/output/reasoning/pricing),
lowest → highest, confirmed from `crates/config/src/catalog.rs:9-18`,
`CatalogCompiler::compile` (`catalog.rs:582-641`) and
`crates/tui/src/provider_lake.rs:54-142`:

```
(5) legacy static completion lists (DEFAULT_* consts)   ← only if catalog has zero rows for provider
(4) static code tables      crates/tui/src/models.rs    ← fallback inside context_window_for_model()
(3) bundled Models.dev seed crates/config/assets/models_dev.bundled.json  ("NOT a competing source of truth", #4188)
    + bundled TUI catalog   crates/tui/assets/model_catalog.bundled.json  (31 entries)
(2) live Models.dev catalog https://models.dev/catalog.json  → ~/.codewhale/catalog/models-dev-catalog.json (24 h TTL)
(1) user / custom overrides (pinned models, custom endpoints, explicit facts)
(0) SPECIAL: ChatGPT/Codex OAuth roster  ~/.codex/models_cache.json — bypasses the catalog entirely
    for ApiProvider::OpenaiCodex (provider_lake.rs:131-133, route_runtime.rs:33-51)
```

Key components:

| Component | File | Role |
|---|---|---|
| `ProviderLake` | `crates/tui/src/provider_lake.rs` | Single facade; merges live-over-bundled keyed on `(provider, wire_model_id)`; legacy fallback at `:138-142` |
| Models.dev live fetch | `crates/tui/src/models_dev_live.rs` | Background refresh, 24 h TTL, 15 s timeout, atomic disk cache; env knobs `CODEWHALE_MODELS_DEV_URL` / `_PATH` / `CODEWHALE_DISABLE_MODELS_DEV_FETCH` |
| Catalog compiler + provenance | `crates/config/src/catalog.rs` | `CatalogSource::{Bundled, Live, UserOverride}`; normalizes Models.dev ids (`moonshotai`→`moonshot`, `togetherai`→`together`, `zhipuai`→`zai`) |
| Models.dev schema | `crates/config/src/models_dev.rs` | Network-free deserialization of `{models, providers}` |
| Static fact tables | `crates/tui/src/models.rs` | `context_window_for_model` / `max_output_tokens_for_model` / `model_supports_reasoning`; catalog checked first, then explicit `_Nk` suffix hint, then vendor heuristics |
| Seeded registry | `crates/tui/src/model_registry.rs` | `ModelMetadata` keyed by id, seeded *from* `models.rs` (drift-guarded by tests); intended future single source |
| Pricing | `crates/tui/src/pricing.rs` | Hand-curated USD (+CNY for DeepSeek) rows; catalog USD pricing used when no explicit row |
| Codex OAuth roster | `crates/tui/src/codex_model_cache.rs` | Read-only parse of `~/.codex/models_cache.json`; trusted only when fresh (&lt;24 h), else conservative fallback with `context_window: None` (compat floor 128,000: `config/models.rs:126`) |
| Agent-crate registry | `crates/agent/src/lib.rs` | 93 `ModelInfo` rows (model×provider), aliases, tools/reasoning flags, resolution fallback chain |

Fallback heuristics in `models.rs` when nothing above matches:
explicit `_Nk` name suffix (8k–1024k) → DeepSeek family (v4 → 1M, legacy →
128K) → GPT-5.5/5.6 API → 1.05M → Codex family → 400K → known-model table →
any "claude" → 200K → `None` (compaction default threshold 102,400).

---

## 3. Provider inventory (33 built-in + 1 legacy alias)

Source: `crates/config/src/provider.rs` (`PROVIDER_REGISTRY`, 33 descriptors),
`provider_kind.rs`, `provider_defaults.rs`. Dialect = `WireFormat`
(`provider.rs:33`): CC = OpenAI Chat Completions, RESP = OpenAI Responses,
AM = Anthropic Messages.

| id | Display | Default base URL | Default model | Auth (env vars) | Dialect |
|---|---|---|---|---|---|
| `deepseek` | DeepSeek | `https://api.deepseek.com/beta` | `deepseek-v4-pro` | `DEEPSEEK_API_KEY` | CC |
| `deepseek-anthropic` | DeepSeek (Anthropic-compatible) | `https://api.deepseek.com/anthropic` | `deepseek-v4-pro` | `DEEPSEEK_API_KEY` | AM |
| `deepseek-cn` *(TUI-only legacy alias)* | DeepSeek (legacy alias) | own config table | — | shares `DEEPSEEK_API_KEY` | CC |
| `nvidia-nim` | NVIDIA NIM | `https://integrate.api.nvidia.com/v1` | `deepseek-ai/deepseek-v4-pro` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, `DEEPSEEK_API_KEY` | CC |
| `openai` | OpenAI-compatible | `https://api.openai.com/v1` | `deepseek-v4-pro` | `OPENAI_API_KEY` | CC |
| `openai-codex` | OpenAI Codex (ChatGPT) | `https://chatgpt.com/backend-api` | `gpt-5.5` | OAuth (`~/.codex/auth.json`) or `OPENAI_CODEX_ACCESS_TOKEN`/`CODEX_ACCESS_TOKEN` | RESP |
| `anthropic` | Anthropic | `https://api.anthropic.com` | `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` (no subscription OAuth) | AM |
| `atlascloud` | AtlasCloud | `https://api.atlascloud.ai/v1` | `deepseek-ai/deepseek-v4-flash` | `ATLASCLOUD_API_KEY` | CC |
| `wanjie-ark` | Wanjie Ark | `https://maas-openapi.wanjiedata.com/api/v1` | `deepseek-reasoner` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` | CC |
| `volcengine` | Volcengine Ark | `https://ark.cn-beijing.volces.com/api/coding/v3` | `DeepSeek-V4-Pro` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` | CC |
| `openrouter` | OpenRouter | `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro` | `OPENROUTER_API_KEY` | CC |
| `xiaomi-mimo` | Xiaomi MiMo | `https://token-plan-sgp.xiaomimimo.com/v1` (regional cn/sgp/ams + PAYG `api.xiaomimimo.com`) | `mimo-v2.5-pro` | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` | CC |
| `novita` | Novita AI | `https://api.novita.ai/openai/v1` | `deepseek/deepseek-v4-pro` | `NOVITA_API_KEY` | CC |
| `fireworks` | Fireworks AI | `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro` | `FIREWORKS_API_KEY` | CC |
| `siliconflow` | SiliconFlow | `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `SILICONFLOW_API_KEY` | CC |
| `siliconflow-CN` | SiliconFlow (China) | `https://api.siliconflow.cn/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `SILICONFLOW_API_KEY` | CC |
| `arcee` | Arcee AI | `https://api.arcee.ai/api/v1` | `trinity-large-thinking` | `ARCEE_API_KEY` | CC |
| `moonshot` | Moonshot/Kimi | `https://api.moonshot.ai/v1` (Kimi-for-coding: `https://api.kimi.com/coding/v1`) | `kimi-k2.7-code` | `MOONSHOT_API_KEY`, `KIMI_API_KEY`, or Kimi OAuth | CC |
| `sglang` | SGLang (self-hosted) | `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `SGLANG_API_KEY` | CC |
| `vllm` | vLLM (self-hosted) | `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `VLLM_API_KEY` | CC |
| `ollama` | Ollama (local) | `http://localhost:11434/v1` | `deepseek-v4-flash` | `OLLAMA_API_KEY` | CC |
| `huggingface` | Hugging Face | `https://router.huggingface.co/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `HUGGINGFACE_API_KEY`, `HF_TOKEN` | CC |
| `together` | Together AI | `https://api.together.xyz/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `TOGETHER_API_KEY` | CC |
| `qianfan` | Baidu Qianfan | `https://api.baiduqianfan.ai/v1` | `ernie-4.0-turbo-8k` | `QIANFAN_API_KEY`, `BAIDU_QIANFAN_API_KEY` | CC |
| `openmodel` | OpenModel | `https://api.openmodel.ai` | `deepseek-v4-flash` | `OPENMODEL_API_KEY` | AM |
| `zai` | Zhipu AI / Z.ai | `https://api.z.ai/api/coding/paas/v4` | `GLM-5.2` | `ZAI_API_KEY`, `Z_AI_API_KEY`, `ZHIPU_API_KEY`, `GLM_API_KEY` | CC |
| `stepfun` | StepFun / StepFlash | `https://api.stepfun.ai/v1` | `step-3.7-flash` | `STEPFUN_API_KEY`, `STEP_API_KEY` | CC |
| `minimax` | MiniMax | `https://api.minimax.io/v1` | `MiniMax-M3` | `MINIMAX_API_KEY` | CC |
| `deepinfra` | DeepInfra | `https://api.deepinfra.com/v1/openai` | `deepseek-ai/DeepSeek-V4-Pro` | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` | CC |
| `sakana` | Sakana AI (Fugu) | `https://api.sakana.ai/v1` | `fugu` | `FUGU_API_KEY`, `SAKANA_API_KEY` | CC |
| `longcat` | Meituan LongCat | `https://api.longcat.chat/openai/v1` | `LongCat-2.0` | `LONGCAT_API_KEY` | CC |
| `meta` | Meta Model API | `https://api.meta.ai/v1` | `muse-spark-1.1` | `META_MODEL_API_KEY`, `MODEL_API_KEY` | CC |
| `xai` | xAI | `https://api.x.ai/v1` | `grok-4.5` | `XAI_API_KEY` or Grok OAuth (`~/.grok/auth.json`) | CC |
| `custom` | Custom (OpenAI-compatible) | per `[providers.<name>]` table | per table | per-entry `api_key_env` | CC |

OAuth routes (token precedence: route OAuth → CLI key → provider/root config
→ ambient env; `crates/tui/src/config.rs:3494-3566`):

- **OpenAI Codex/ChatGPT** — Codex CLI login (`~/.codex/auth.json`);
  account-scoped model roster from `~/.codex/models_cache.json`; usage is
  subscription-scoped so **no dollar pricing is shown** on this route
  (`pricing.rs:127-129, 334-338`).
- **xAI Grok** — `[providers.xai] auth_mode = "oauth"`, reuses `~/.grok/auth.json`
  or device-code login.
- **Moonshot/Kimi** — Kimi CLI OAuth for the coding endpoint.
- **No Anthropic/Claude subscription OAuth exists** — API key only.

---

## 4. First-class model metadata (the curated set)

Merged view of the four fact sources for the models CodeWhale makes explicit
promises about. Columns: **Ctx** = context window (tokens), **Out** = max
output, **R** = emits reasoning, pricing = USD per 1M tokens as
**cache-hit / input / output** from `pricing.rs` (catalog rows have no
cache-hit discount → hit = input).

### 4.1 DeepSeek (first-class)

| Model | Ctx | Out | R | Pricing (hit/in/out USD) | Notes |
|---|---|---|---|---|---|
| `deepseek-v4-pro` | 1,000,000 | 384,000 | ✓ | 0.003625 / 0.435 / 0.87 (+CNY 0.025/3/6) | post-2026-05-31 adjusted rate is permanent (#2489) |
| `deepseek-v4-flash` | 1,000,000 | 384,000 | ✓ | 0.0028 / 0.14 / 0.28 (+CNY 0.02/1/2) | |
| `deepseek-reasoner`, legacy v3.x, `deepseek-coder*` | 128,000 | — | v4-only | flash rates for non-pro | legacy fallback window (`models.rs:7`) |
| `deepseek-ai/*` (NIM-hosted) | 1,000,000 | 384,000 | ✓ | **intentionally unpriced** | NVIDIA terms ≠ DeepSeek platform pricing (`pricing.rs:133-137`) |
| any id with `-Nk` suffix | N×1000 | — | — | — | vendor-agnostic served-name hint, 8k–1024k |

### 4.2 OpenAI

| Model | Ctx (API route) | Ctx (OAuth route) | Out | R | Pricing (hit/in/out) | Notes |
|---|---|---|---|---|---|---|
| `gpt-5.6` (alias → sol) | 1,050,000 | 272,000¹ | 128,000 | ✓ | 0.50 / 5.00 / 30.00 | Models.dev: input limit 922,000 |
| `gpt-5.6-sol` | 1,050,000 | 272,000¹ | 128,000 | ✓ | 0.50 / 5.00 / 30.00 | efforts low→ultra on OAuth |
| `gpt-5.6-terra` | 1,050,000 | 272,000¹ | 128,000 | ✓ | 0.25 / 2.50 / 15.00 | |
| `gpt-5.6-luna` | 1,050,000 | 272,000¹ | 128,000 | ✓ | 0.10 / 1.00 / 6.00 | cost-efficient tier; **the "272k looks wrong" report — see §6** |
| `gpt-5.5` | 1,050,000 | 272,000¹ | 128,000 | ✓ | 0.50 / 5.00 / 30.00 | date snapshots (`gpt-5.5-YYYY-MM-DD`) too |
| `gpt-5.5-pro` | 1,050,000 | n/a | 128,000 | ✓ | 30.00 / 30.00 / 180.00 | no cached-input discount |
| `gpt-5-codex` | 400,000 | per roster | 128,000 | ✓ | 0.125 / 1.25 / 10.00 | deprecated upstream on OAuth path |
| `gpt-5.3-codex` | 400,000 | per roster | 128,000 | ✓ | 0.175 / 1.75 / 14.00 | |
| other codex ids (`gpt-5.1-codex[-mini/-max]`, `gpt-5.2-codex`, `codex-gpt-5.5`, `chatgpt-gpt-5.5`, `gpt-5.5-codex[-preview]`…) | 400,000 | per roster | 128,000 | ✓ | unpriced | recognized by `is_openai_codex_model` (`models.rs:491-507`) |
| `gpt-5.5-nano` | *unknown* | *unknown* | — | ✗ | — | deliberately unrecognized (`models.rs:788-790`) |

¹ OAuth window is whatever `~/.codex/models_cache.json` advertises for the
account; on this machine (2026-07-13) it is 272,000 for all gpt-5.x list
models and 128,000 for `gpt-5.3-codex-spark`. Stale/missing cache → no ctx
shown in picker; runtime compat floor 128,000. OAuth route never shows pricing.

### 4.3 Anthropic

| Model | Ctx | Out | R | Pricing (hit/in/out) | Notes |
|---|---|---|---|---|---|
| `claude-opus-4-8` | 1,000,000 | 128,000 | ✓ | 0.50 / 5.00 / 25.00 | |
| `claude-sonnet-4-6` | 1,000,000 | 128,000 | ✓ | 0.30 / 3.00 / 15.00 | out raised 64K→128K (2026-07-09 audit) |
| `claude-sonnet-5` | 1,000,000 | 128,000 | ✓ | intro 0.20/2.00/10.00 until 2026-08-31, then 0.30/3.00/15.00 | time-aware in `pricing.rs:263-273` |
| `claude-fable-5` | 1,000,000 | 128,000 | ✓ | 1.00 / 10.00 / 50.00 | tokenizer yields ~30% more tokens — raw rate comparisons undercount cost (`pricing.rs:178-182`) |
| `claude-haiku-4-5` | 200,000 | 64,000 | ✗ | 0.10 / 1.00 / 5.00 | |
| any other `claude*` | 200,000 | — | ✗ | — | family fallback |

### 4.4 Moonshot / Kimi

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `kimi-k2.7-code` (± `moonshotai/`) | 262,144 | 262,144 | ✓ | 0.19 / 0.95 / 4.00 | |
| `kimi-k2.6` (± prefix, `:free`) | 262,144 | 262,144 | ✓ | 0.16 / 0.95 / 4.00 | |
| `kimi-for-coding` | 262,144 | 262,144 | ✓ | — | stable coding route; rides K2.7 path; **not in agent-crate registry or catalogs** (§7 D-12) |
| any bare `kimi-*` | — | — | ✓ | — | prefix rule: always reasoning (#3016) |

### 4.5 Z.ai / GLM

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `glm-5.2` (± `z-ai/`) | 1,000,000 | 131,072 | ✓ | 0.26 / 1.40 / 4.40 | |
| `glm-5.3` (± `z-ai/`) | 1,000,000 | 131,072 | ✓ |  | **not released by Z.ai as of 2026-08-03** — registered id only; limits/reasoning inherited wholesale from `glm-5.2` pending official Z.ai release metadata, not measured; no published price |
| `glm-5.1` (± `z-ai/`) | 202,752 | 131,072 | ✓ | 0.26 / 1.40 / 4.40 | vendor page says 200K (§7 D-4) |
| `glm-5-turbo` (± `z-ai/`) | 202,752 | 131,072 | ✓ | 0.24 / 1.20 / 4.00 | fast **text** sibling |
| `glm-5v-turbo` (± `z-ai/`) | 202,752 | — | ✗ | — | **vision** model, distinct from 5-turbo |

### 4.6 MiniMax

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `minimax-m3` (± `minimax/`, `MiniMax-M3`) | 1,000,000 | 524,288 | ✓ | 0.06 / 0.30 / 1.20 | catalog says out 128,000 (§7 D-5) |
| `minimax-m2.7` (± prefix, `-highspeed`) | 204,800 | 131,072 (catalog) | ✓ | 0.3 / 0.3 / 1.2 (catalog) | |
| `minimax-m2.5` / `m2.1` (± `-highspeed`) | 204,800 | — | ✓ | — | |
| `minimax-m2` | 204,800 | — | ✓ | — | catalog says 196,608 (§7 D-6) |

### 4.7 Qwen (OpenRouter-routed)

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `qwen/qwen3.6-flash` | 1,000,000 | 65,536 | ✓ | 0.1875 / 0.1875 / 1.125 | |
| `qwen/qwen3.6-plus` | 1,000,000 | 65,536 | ✓ | 0.325 / 0.325 / 1.95 | |
| `qwen/qwen3.6-35b-a3b` | 262,144 | 262,140 | ✓ | 0.05 / 0.14 / 1.00 | out cap suspect (§7 D-7) |
| `qwen/qwen3.6-27b` | 262,144 | 262,140 | ✓ | 0.15 / 0.285 / 2.40 | out cap suspect (§7 D-7) |
| `qwen/qwen3.6-max-preview` | 262,144 | 65,536 | ✓ | 1.04 / 1.04 / 6.24 | catalog ctx 245,800 (§7 D-8) |
| `qwen/qwen3.7-plus` | — | — | — | 0.064 / 0.32 / 1.28 | **priced but no ctx/out/reasoning rows** (§7 D-13) |
| `qwen/qwen3.7-max` | — | — | — | 0.25 / 1.25 / 3.75 | in agent registry; same gap (§7 D-13) |

### 4.8 Xiaomi MiMo

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `mimo-v2.5-pro` (± `xiaomi/`, `-ultraspeed`) | 1,000,000 | 131,072 | ✓ | intentionally unknown | Token-Plan credit billing, no balance endpoint |
| `mimo-v2.5` (± `xiaomi/`) | 1,000,000 | 131,072 | ✓ | unknown | omni (text+image) |
| `mimo-v2.5-asr` | 8,000 | 2,048 | ✗ | unknown | speech-to-text |
| `mimo-v2.5-tts[-voicedesign/-voiceclone]`, `mimo-v2-tts` | 8,000 | 8,192 | ✗ | unknown | TTS family |

### 4.9 xAI / Grok

| Model | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|
| `grok-4.5` | 500,000 | — | ✓ | — | |
| `grok-4.3` | 1,000,000 | — | ✓ | — | |
| `grok-build` | 512,000 | — | ✓ | — | not in Models.dev |
| `grok-composer-2.5-fast` | 200,000 | — | ✗ | — | not in Models.dev |
| `grok-4.20-0309-reasoning` / `-non-reasoning` | 2,000,000 | — | ✓/✗ | — | catalog row says 1M (§7 D-9); **no Grok pricing rows at all** (§7 D-14) |

### 4.10 Others

| Model | Provider | Ctx | Out | R | Pricing | Notes |
|---|---|---|---|---|---|---|
| `trinity-large-thinking` (± `arcee-ai/`) | Arcee | 262,144 | 262,144 | ✓ | 0.25 / 0.25 / 0.80 | out=ctx suspect (§7 D-2) |
| `trinity-large-preview` | Arcee | 262,144 | — | ✗ | — | catalog row says 131,000 (§7 D-3) |
| `trinity-mini` | Arcee | 128,000 | 64,000 (bundled) | ✗ | 0.045 / 0.045 / 0.15 | catalog says ctx 131,072 (§7 D-1) |
| `step-3.7-flash` | StepFun | 256,000 | 256,000 | ✗ | 0.2 / 0.2 / 1.15 | third-party sourced (models.dev + AA) |
| `fugu` | Sakana | — | — | ✗ | — | in agent registry only |
| `fugu-ultra` / `fugu-ultra-20260615` | Sakana | 1,000,000 | 131,000 | ✓ | 5.0 / 5.0 / 30.0 | limits third-party sourced (Requesty); Sakana's own >272K price tier confirms ctx > 272K |
| `muse-spark-1.1` | Meta | 1,000,000 | 32,000 | ✓ | 1.25 / 1.25 / 4.25 | |
| `LongCat-2.0` | LongCat | — | — | ✓ | — | agent registry only; no fact rows (§7 D-15) |
| `tencent/hy3-preview` | OpenRouter | 262,144 | — | ✓ | 0.021 / 0.063 / 0.21 | catalog says 256,000 (§7 D-10) |
| `google/gemma-4-31b-it` (± `:free`) | OpenRouter | 262,144 | 16,384 (paid) / 32,768 (free) | ✓ | 0.09 / 0.12 / 0.35 | out cap vs catalog 131,072 (§7 D-11) |
| `google/gemma-4-26b-a4b-it` (± `:free`) | OpenRouter | 262,144 | 32,768 (free) | ✓ | 0.06 / 0.06 / 0.33 | |
| `nvidia/nemotron-3-ultra-550b-a55b` (± `:free`) | OpenRouter | 1,000,000 | 16,384 (paid) / 65,536 (free) | ✓ | 0.10 / 0.50 / 2.20 | catalog out 65,000 (§7 D-11) |
| `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` | OpenRouter | 262,144 | 65,536 | ✓ | — | |
| `ernie-4.0-turbo-8k` | Qianfan | 8,000 (via `-8k` hint) | — | ✗ | — | Qianfan default; no explicit rows |

---

## 5. Bundled TUI catalog (`crates/tui/assets/model_catalog.bundled.json`)

31 entries, `fetched_at: 2026-07-06`, effectively-infinite TTL. Adds
modalities and (for some rows) USD pricing on top of §4 facts. Highlights:

- Modalities `text,image`: gpt-5.6 family, muse-spark-1.1, minimax-m3, mimo-v2.5.
- `text,image,audio`: mimo-v2.5-pro (the only audio-input row).
- `gpt-5.6` carries `provider_model_id: gpt-5.6-sol` (alias resolution).
- Pricing present only for: codex models, kimi-k2.7-code, glm-5.2,
  minimax-m2.7, trinity-mini, claude family, step-3.7-flash, fugu-ultra.
- The bundled `models_dev.bundled.json` (config crate) is smaller: 2 models
  (deepseek-v4-pro/flash), 14 providers, 42 chat offerings; demoted to
  offline-fallback-only by #4188.

## 6. The gpt-5.6-luna 272K finding (root cause, resolved)

- **What you saw:** model picker hint "272K ctx" for gpt-5.6-luna.
- **Where it comes from:** `~/.codex/models_cache.json` — the ChatGPT/Codex
  OAuth `/models` roster persisted by the Codex CLI. On this machine every
  gpt-5.x OAuth entry advertises `context_window: 272000`.
- **Why CodeWhale shows it:** for `ApiProvider::OpenaiCodex` the picker and
  runtime use the OAuth-advertised window **exclusively** — never the API
  route's 1,050,000 — because the OAuth offering genuinely has the smaller
  window and different (subscription) billing. Precedence code:
  `model_picker.rs:1105-1111`; runtime: `route_runtime.rs:33-51`; compaction
  consumes it via `effective_context_window` (`compaction.rs:98-104`).
  Pinned by tests (`model_picker.rs:1660-1706`,
  `commands/groups/debug/tests.rs:96-111`).
- **Is 272,000 plausible?** Yes: 400,000 (codex-class window) − 128,000
  (output) = 272,000 usable input; OpenAI advertises the input budget as the
  OAuth "context_window". The same-name API models are 1,050,000 total /
  922,000 input.
- **Verdict:** data is correct per route; the *presentation* invites the
  "that looks wrong" reaction. See action A1.

## 7. Discrepancies & gaps (vs live Models.dev catalog, fetched 2026-07-12)

Confidence key: **vendor** = the live row is the vendor's own provider entry
(strong signal); **aggregator** = row from a reseller (Vercel/OpenRouter etc.,
weaker — verify against the vendor's docs before changing anything).

| # | Model | CodeWhale says | Live catalog says | Source | Assessment |
|---|---|---|---|---|---|
| D-1 | `trinity-mini` | ctx 128,000 / out 64,000 (bundled) | 131,072 / 131,072 | aggregator | verify vs Arcee docs |
| D-2 | `trinity-large-thinking` | out 262,144 (= full ctx — suspicious) | ctx 262,100 / out 80,000 | aggregator | out=ctx is a common data-entry smell; verify |
| D-3 | `trinity-large-preview` | ctx 262,144 | 131,000 / 131,000 | aggregator | verify |
| D-4 | `glm-5.1`, `glm-5-turbo`, `glm-5v-turbo` | ctx 202,752 | 200,000 | **vendor (zai)** | 202,752 = 198×1024; likely fine (marketing 200K vs binary), document choice |
| D-5 | `minimax-m3` | out 524,288 | out 128,000 | vendor (coding-plan row) | route-dependent; verify per MiniMax platform docs |
| D-6 | `minimax-m2` | ctx 204,800 | 196,608 | vendor (coding-plan row) | verify |
| D-7 | `qwen3.6-35b-a3b`, `qwen3.6-27b` | out 262,140 | out 65,536 | **vendor (alibaba)** | our 262,140 looks wrong (≈ctx); likely fix to 65,536 |
| D-8 | `qwen3.6-max-preview` | ctx 262,144 | 245,800 | vendor (alibaba-cn) | verify |
| D-9 | `grok-4.20-0309-*` | ctx 2,000,000 | 1,000,000 | vendor (xai) | 2M matches xAI's fast-endpoint marketing; catalog row may be the standard endpoint — verify |
| D-10 | `tencent/hy3-preview` | ctx 262,144 | 256,000 | aggregator | minor; verify |
| D-11 | `gemma-4-31b-it` out 16,384; `nemotron-3-ultra` out 16,384 | 131,072 / 65,000 | aggregator | per-host caps differ; ours were OpenRouter-specific — document as route-scoped |
| D-12 | `kimi-for-coding` | in `models.rs` only | absent from live catalog, agent registry, bundled catalogs | — | add to agent registry + bundled catalog or document as models.rs-only route |
| D-13 | `qwen/qwen3.7-plus`, `qwen/qwen3.7-max` | priced (and 3.7-max in agent registry) but **no ctx/out/reasoning rows** | present upstream | — | add fact rows |
| D-14 | all Grok models | **no pricing rows** | xai: e.g. grok-4.5 $2/$6, grok-4.3 $1.25/$2.5 | vendor | add pricing (catalog passthrough may already cover once live rows resolve — verify `resolved_usd_pricing` path) |
| D-15 | `LongCat-2.0`, `fugu` (base), `ernie-4.0-turbo-8k` | agent-registry/default only; no metadata rows | — | — | add facts or mark best-effort |
| D-16 | `mimo-v2.5*` | ctx 1,000,000 | 1,048,576 | vendor (token-plan) | cosmetic (1M vs 2^20); document choice |
| D-17 | `fugu-ultra` | out 131,000 | sakana row: out 1,000,000 | vendor row suspect (out=ctx) | keep ours (Requesty-sourced) unless Sakana docs say otherwise |

Structural observations:

- **Four hand-maintained fact stores** must currently be updated in lockstep:
  `models.rs`, `model_catalog.bundled.json`, `pricing.rs`,
  `models_dev.bundled.json`. `model_registry.rs` is the intended chokepoint
  but is not yet consumed by production call sites (its module docs say so).
- The agent crate's `ModelRegistry` (93 rows) duplicates provider/alias data
  with its own `supports_tools`/`supports_reasoning` flags — a fifth store.
- `CurrencyPricing` has no **cache-write** field, so Anthropic (1.25–2× input)
  and Qwen 3.7 cache-write rates are silently dropped from cost estimates.
- The bundled TUI catalog cannot carry a cache-read rate, forcing Anthropic
  rows to live in `pricing.rs` above the catalog.

## 8. Action items (for the implementation pass)

| ID | Action | Where |
|---|---|---|
| A1 | Label OAuth-route context in the picker (e.g. "272K ctx · ChatGPT route") so account-scoped windows don't read as wrong data; optionally show the API-route window alongside | `model_picker.rs:1187-1192` hint renderer |
| A2 | Fix Qwen3.6-27b/35b output caps (262,140 → 65,536, pending vendor-doc check) | `models.rs:371`, tests |
| A3 | Verify & reconcile D-1/2/3 (Arcee), D-5/6 (MiniMax), D-8 (Qwen max-preview), D-9 (Grok 4.20), D-10 (HY3) against vendor docs | `models.rs`, bundled catalogs |
| A4 | Add fact rows for `qwen3.7-plus/max`, `LongCat-2.0`, Grok pricing | `models.rs`, `pricing.rs` |
| A5 | Register `kimi-for-coding` in the agent-crate registry + bundled catalog | `crates/agent/src/lib.rs`, assets |
| A6 | Add cache-write field to `CurrencyPricing` and to the catalog schema; move Anthropic rows into the catalog once it can carry cache rates | `pricing.rs:97-109`, `catalog.rs` |
| A7 | Complete the #3071/#3073 migration: make production call sites consume `model_registry::lookup` so the fact stores collapse to one | `model_registry.rs` |
| A8 | Consider folding the agent-crate `ModelRegistry` flags into the same chokepoint (or generating them from it) | `crates/agent/src/lib.rs` |
| A9 | Refresh `model_catalog.bundled.json` + `models_dev.bundled.json` from the 2026-07-12 live snapshot as part of each release lane | assets |

## Appendix A — agent-crate registry (model × provider, 93 rows)

Source: `crates/agent/src/lib.rs` `ModelRegistry::default()`. Flags:
T = supports tools, R = supports reasoning. Aliases are case-insensitive.

| Wire model id | Provider | T | R | Aliases |
|---|---|---|---|---|
| `deepseek-v4-pro` | Deepseek | ✓ | ✓ | — |
| `deepseek-v4-flash` | Deepseek | ✓ | ✓ | deepseek-chat, deepseek-reasoner, deepseek-r1, deepseek-v3, deepseek-v3.2 |
| `deepseek-ai/deepseek-v4-pro` | NvidiaNim | ✓ | ✓ | deepseek-v4-pro, nvidia-deepseek-v4-pro, nim-deepseek-v4-pro |
| `deepseek-ai/deepseek-v4-flash` | NvidiaNim | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, nvidia-deepseek-v4-flash, nim-deepseek-v4-flash |
| `deepseek-v4-pro` | Openai | ✓ | ✓ | openai-compatible-deepseek-v4-pro |
| `deepseek-v4-flash` | Openai | ✓ | ✓ | openai-compatible-deepseek-v4-flash |
| `gpt-5.6` | Openai | ✓ | ✓ | gpt56 |
| `gpt-5.6-sol` | Openai | ✓ | ✓ | gpt56-sol |
| `gpt-5.6-terra` | Openai | ✓ | ✓ | gpt56-terra |
| `gpt-5.6-luna` | Openai | ✓ | ✓ | gpt56-luna |
| `deepseek-ai/deepseek-v4-flash` | Atlascloud | ✓ | ✓ | deepseek-v4-flash, atlascloud-deepseek-v4-flash |
| `deepseek-ai/deepseek-v4-pro` | Atlascloud | ✓ | ✓ | deepseek-v4-pro, atlascloud-deepseek-v4-pro |
| `deepseek-reasoner` | WanjieArk | ✓ | ✓ | wanjie-deepseek-reasoner, ark-wanjie-deepseek-reasoner |
| `DeepSeek-V4-Pro` | Volcengine | ✓ | ✓ | deepseek-v4-pro, volcengine-deepseek-v4-pro, ark-deepseek-v4-pro |
| `DeepSeek-V4-Flash` | Volcengine | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, volcengine-deepseek-v4-flash, ark-deepseek-v4-flash |
| `trinity-large-thinking` | Arcee | ✓ | ✓ | trinity, arcee-trinity, arcee-trinity-large-thinking |
| `trinity-large-preview` | Arcee | ✓ | ✗ | arcee-trinity-large-preview |
| `deepseek/deepseek-v4-pro` | Openrouter | ✓ | ✓ | deepseek-v4-pro, openrouter-deepseek-v4-pro |
| `deepseek/deepseek-v4-flash` | Openrouter | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, openrouter-deepseek-v4-flash |
| `arcee-ai/trinity-large-thinking` | Openrouter | ✓ | ✓ | trinity, trinity-large-thinking, arcee-trinity-large-thinking |
| `xiaomi/mimo-v2.5-pro` | Openrouter | ✓ | ✓ | openrouter-mimo-v2.5-pro, openrouter-xiaomi-mimo-v2.5-pro |
| `xiaomi/mimo-v2.5` | Openrouter | ✓ | ✓ | openrouter-mimo-v2.5, openrouter-xiaomi-mimo-v2.5 |
| `qwen/qwen3.6-flash` | Openrouter | ✓ | ✓ | qwen3.6-flash, qwen-3.6-flash |
| `qwen/qwen3.6-35b-a3b` | Openrouter | ✓ | ✓ | qwen3.6-35b-a3b, qwen-3.6-35b-a3b |
| `qwen/qwen3.6-max-preview` | Openrouter | ✓ | ✓ | qwen3.6-max-preview, qwen-3.6-max-preview, qwen-max-preview |
| `qwen/qwen3.6-27b` | Openrouter | ✓ | ✓ | qwen3.6-27b, qwen-3.6-27b |
| `qwen/qwen3.6-plus` | Openrouter | ✓ | ✓ | qwen3.6-plus, qwen-3.6-plus |
| `qwen/qwen3.7-max` | Openrouter | ✓ | ✓ | qwen3.7-max, qwen-3.7-max |
| `moonshotai/kimi-k2.7-code` | Openrouter | ✓ | ✓ | kimi-k2.7-code, openrouter-kimi-k2.7-code |
| `moonshotai/kimi-k2.6` | Openrouter | ✓ | ✓ | openrouter-kimi-k2.6 |
| `minimax/minimax-m3` | Openrouter | ✓ | ✓ | minimax-m3, minimax-m-3, openrouter-minimax-m3 |
| `minimax/minimax-m2.7` | Openrouter | ✓ | ✓ | minimax-2.7, minimax-2-7, openrouter-minimax-2.7 |
| `z-ai/glm-5.1` | Openrouter | ✓ | ✓ | glm-5.1, zai-glm-5.1 |
| `z-ai/glm-5.2` | Openrouter | ✓ | ✓ | glm-5.2, zai-glm-5.2 |
| `z-ai/glm-5.3` | Openrouter | ✓ | ✓ | glm-5.3, zai-glm-5.3 |
| `z-ai/glm-5-turbo` | Openrouter | ✓ | ✓ | glm-5-turbo, zai-glm-5-turbo |
| `tencent/hy3-preview` | Openrouter | ✓ | ✓ | hy3-preview, tencent-hy3-preview |
| `google/gemma-4-31b-it` | Openrouter | ✓ | ✓ | gemma-4-31b, gemma-4-31b-it |
| `google/gemma-4-26b-a4b-it` | Openrouter | ✓ | ✓ | gemma-4-26b-a4b, gemma-4-26b-a4b-it |
| `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` | Openrouter | ✓ | ✓ | nemotron-3-nano-omni, nemotron-3-nano-omni-reasoning |
| `nvidia/nemotron-3-ultra-550b-a55b` | Openrouter | ✓ | ✓ | nvidia/nemotron-3-ultra, nemotron-3-ultra, nemotron-3-ultra-550b-a55b, nvidia-nemotron-3-ultra, nvidia-nemotron-3-ultra-550b-a55b |
| `GLM-5.2` | Zai | ✓ | ✓ | glm-5.2, glm-5-2, zai-glm-5.2, zai-glm-5-2 |
| `GLM-5.3` | Zai | ✓ | ✓ | glm-5.3, glm-5-3, zai-glm-5.3, zai-glm-5-3 |
| `GLM-5.1` | Zai | ✓ | ✓ | glm-5.1, glm-5-1, zai-glm-5.1, zai-glm-5-1 |
| `GLM-5-Turbo` | Zai | ✓ | ✓ | glm-5-turbo, glm-5turbo, zai-glm-5-turbo |
| `mimo-v2.5-pro` | XiaomiMimo | ✓ | ✓ | mimo, pro, xiaomi-mimo-v2.5-pro, xiaomi-mimo-v2-5-pro |
| `mimo-v2.5` | XiaomiMimo | ✓ | ✓ | omni, mimo-omni, v2.5-omni, mimo-v2.5-omni, xiaomi-mimo-v2.5, xiaomi-mimo-v2.5-omni |
| `mimo-v2.5-asr` | XiaomiMimo | ✗ | ✗ | asr, speech-to-text, transcribe |
| `mimo-v2.5-tts` | XiaomiMimo | ✗ | ✗ | tts, speech, mimo-tts |
| `mimo-v2.5-tts-voicedesign` | XiaomiMimo | ✗ | ✗ | voicedesign, voice-design, mimo-voice-design |
| `mimo-v2.5-tts-voiceclone` | XiaomiMimo | ✗ | ✗ | voiceclone, voice-clone, mimo-voice-clone |
| `mimo-v2-tts` | XiaomiMimo | ✗ | ✗ | mimo-v2-speech |
| `deepseek/deepseek-v4-pro` | Novita | ✓ | ✓ | deepseek-v4-pro, novita-deepseek-v4-pro |
| `deepseek/deepseek-v4-flash` | Novita | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, novita-deepseek-v4-flash |
| `accounts/fireworks/models/deepseek-v4-pro` | Fireworks | ✓ | ✓ | deepseek-v4-pro, fireworks-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Pro` | Siliconflow | ✓ | ✓ | deepseek-v4-pro, deepseek-reasoner, deepseek-r1, siliconflow-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Siliconflow | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-v3, siliconflow-deepseek-v4-flash |
| `kimi-k2.7-code` | Moonshot | ✓ | ✓ | kimi, kimi-k2, kimi-k2.7, kimi-code, moonshot-kimi-k2.7-code |
| `kimi-k2.6` | Moonshot | ✓ | ✓ | moonshot-kimi-k2.6 |
| `deepseek-ai/DeepSeek-V4-Pro` | Sglang | ✓ | ✓ | deepseek-v4-pro, sglang-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Sglang | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, sglang-deepseek-v4-flash |
| `deepseek-ai/DeepSeek-V4-Pro` | Vllm | ✓ | ✓ | deepseek-v4-pro, vllm-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Vllm | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, vllm-deepseek-v4-flash |
| `deepseek-v4-flash` | Ollama | ✓ | ✓ | — (Ollama also accepts any name as-is) |
| `deepseek-ai/DeepSeek-V4-Pro` | Huggingface | ✓ | ✓ | deepseek-v4-pro, hf-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Huggingface | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, deepseek-reasoner, hf-deepseek-v4-flash |
| `deepseek-ai/DeepSeek-V4-Pro` | Together | ✓ | ✓ | deepseek-v4-pro, together-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Together | ✓ | ✓ | deepseek-v4-flash, deepseek-chat, together-deepseek-v4-flash |
| `gpt-5.5` | OpenaiCodex | ✓ | ✓ | codex-gpt-5.5, chatgpt-gpt-5.5 |
| `claude-opus-4-8` | Anthropic | ✓ | ✓ | opus, claude-opus |
| `claude-sonnet-4-6` | Anthropic | ✓ | ✓ | sonnet, claude-sonnet |
| `claude-haiku-4-5` | Anthropic | ✓ | ✗ | haiku, claude-haiku |
| `deepseek-v4-flash` | Openmodel | ✓ | ✓ | openmodel, openmodel-deepseek |
| `step-3.7-flash` | Stepfun | ✓ | ✗ | stepfun, stepflash |
| `MiniMax-M3` | Minimax | ✓ | ✓ | minimax, minimax-m3, minimax-m-3 |
| `MiniMax-M2.7` | Minimax | ✓ | ✓ | minimax-m2.7, minimax-m2-7, minimax-m-2.7, minimax-m-2-7 |
| `MiniMax-M2.7-highspeed` | Minimax | ✓ | ✓ | minimax-m2.7-highspeed (+ dash variants) |
| `MiniMax-M2.5` | Minimax | ✓ | ✓ | minimax-m2.5 (+ dash variants) |
| `MiniMax-M2.5-highspeed` | Minimax | ✓ | ✓ | minimax-m2.5-highspeed (+ dash variants) |
| `MiniMax-M2.1` | Minimax | ✓ | ✓ | minimax-m2.1 (+ dash variants) |
| `MiniMax-M2.1-highspeed` | Minimax | ✓ | ✓ | minimax-m2.1-highspeed (+ dash variants) |
| `MiniMax-M2` | Minimax | ✓ | ✓ | minimax-m2, minimax-m-2 |
| `deepseek-ai/DeepSeek-V4-Pro` | Deepinfra | ✓ | ✓ | deepseek-v4-pro, di-deepseek-v4-pro |
| `deepseek-ai/DeepSeek-V4-Flash` | Deepinfra | ✓ | ✓ | deepseek-v4-flash, di-deepseek-v4-flash |
| `fugu` | Sakana | ✓ | ✗ | sakana-fugu, sakana/fugu |
| `fugu-ultra-20260615` | Sakana | ✓ | ✓ | fugu-ultra, sakana-fugu-ultra |
| `LongCat-2.0` | LongCat | ✓ | ✓ | longcat, longcat-2.0 |
| `muse-spark-1.1` | Meta | ✓ | ✓ | muse-spark, muse |
| `grok-4.5` | Xai | ✓ | ✓ | grok, xai-grok-4.5 |
| `grok-4.3` | Xai | ✓ | ✓ | xai-grok-4.3 |
| `grok-build` | Xai | ✓ | ✓ | xai-grok-build |
| `grok-composer-2.5-fast` | Xai | ✓ | ✗ | xai-grok-composer |
| `grok-4.20-0309-reasoning` | Xai | ✓ | ✓ | xai-grok-reasoning |
| `grok-4.20-0309-non-reasoning` | Xai | ✓ | ✗ | xai-grok-fast |

Resolution order (`resolve()` at `lib.rs:987+`): Ollama passes names through
verbatim; a `provider_hint` narrows the search; Atlascloud/Arcee/XiaomiMimo
accept arbitrary ids for their provider; otherwise falls back to the hinted
provider's first model, ultimate default `deepseek-v4-pro`.

## 8. Fast-tier derivation contract

Auto routing and `model_strength = faster` use the provider-scoped family map
in `crates/tui/src/model_routing.rs`. A pair is valid only when the current
provider's catalog has both model ids; model-name similarity alone is not
enough. Current non-DeepSeek pairs are:

| Provider | Strong family | Fast sibling |
|---|---|---|
| OpenAI / OpenAI Codex | `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra` | `gpt-5.6-luna` |
| Anthropic | Claude Opus/Sonnet rows listed in §4.3 | `claude-haiku-4-5` |
| OpenRouter | Qwen 3.6, Claude, MiMo, Trinity, Kimi rows | provider-prefixed catalog sibling |
| Xiaomi MiMo | `mimo-v2.5-pro` | `mimo-v2.5` |
| Arcee | `trinity-large-thinking` / `trinity-large-preview` | `trinity-mini` |
| Moonshot | `kimi-k2.7-code` | `kimi-k2.6` |
| MiniMax | `MiniMax-M2.7` | `MiniMax-M2.7-highspeed` |
| OpenCode Go | `kimi-k3` | `kimi-k2.7-code` |

Already-fast rows and true singles return no further sibling. Local/custom
providers never inherit cloud families. New catalog multi-tier families must
add an explicit same-provider pair and a matrix test before they become an
Auto fast route.

## Appendix B — file map (where each fact lives)

| Fact | Primary file |
|---|---|
| Context window / max output / reasoning (static) | `crates/tui/src/models.rs:239-554` |
| Seeded metadata registry (future chokepoint) | `crates/tui/src/model_registry.rs` |
| Pricing (USD + DeepSeek CNY, time-aware rows) | `crates/tui/src/pricing.rs:112-303` |
| Bundled TUI catalog (31 entries, modalities) | `crates/tui/assets/model_catalog.bundled.json` |
| Bundled Models.dev seed (14 providers / 42 offerings) | `crates/config/assets/models_dev.bundled.json` |
| Live Models.dev cache (disk) | `~/.codewhale/catalog/models-dev-catalog.json` |
| Codex OAuth roster (disk, read-only) | `~/.codex/models_cache.json` |
| Provider descriptors (33) | `crates/config/src/provider.rs` |
| Provider base-URL/model constants | `crates/config/src/provider_defaults.rs` |
| Provider enum + parsing | `crates/config/src/provider_kind.rs`, `crates/tui/src/config.rs:42` |
| Catalog merge/precedence | `crates/config/src/catalog.rs`, `crates/tui/src/provider_lake.rs` |
| Live fetch/TTL/freshness | `crates/tui/src/models_dev_live.rs` |
| OAuth route limit override | `crates/tui/src/route_runtime.rs:33-51`, `crates/tui/src/codex_model_cache.rs` |
| Picker display precedence | `crates/tui/src/tui/model_picker.rs:1105-1155` |
| Agent-crate model×provider registry | `crates/agent/src/lib.rs:69-960` |
