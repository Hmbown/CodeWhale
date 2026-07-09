# Model Alias Precedence

This document describes how model aliases (like `"deepseek-v4-pro"`) are resolved
when they collide across multiple providers in `crates/agent/src/lib.rs`.

## Resolution Algorithm

The `ModelRegistry` resolves a user-requested model name through these steps, in order:

1. **Provider hint + match.** If a `provider_hint` is supplied, the registry scans
   all models matching that provider. The first model whose canonical `id` or
   any alias matches the request wins.

2. **Provider-specific passthrough.** Certain providers (Atlascloud, Arcee,
   XiaomiMimo) allow any model id through via a passthrough function when the
   provider hint matches.

3. **Alias map lookup.** A case-insensitive alias map built from all canonical ids
   and aliases. The map uses `HashMap::entry(...).or_insert(idx)`, so the **first
   model registered with a given name wins**.

4. **Provider default fallback.** If no match, the first model for the hinted
   provider (or DeepSeek if no hint) is returned with `used_fallback = true`.

5. **Global default.** If the provider has no models, falls back to the very first
   model in the registry (DeepSeek `deepseek-v4-pro`).

## `"deepseek-v4-pro"` - Which Provider Wins?

Without a `provider_hint`, `"deepseek-v4-pro"` resolves to **DeepSeek**
(`ProviderKind::Deepseek`) because:

- The DeepSeek entry's canonical `id` is `"deepseek-v4-pro"` and it is the very
  first model inserted into the alias map (index 0).
- All subsequent entries that list `"deepseek-v4-pro"` as an alias are ignored
  (`or_insert` keeps the first value).

With a `provider_hint`, the resolution respects the hint. For example:
- `provider_hint = Some(NvidiaNim)` resolves to NvidiaNim's `deepseek-ai/deepseek-v4-pro`
- `provider_hint = Some(Atlascloud)` resolves to Atlascloud's `deepseek-ai/deepseek-v4-pro`
- `provider_hint = Some(Volcengine)` resolves to Volcengine's `DeepSeek-V4-Pro`

## Providers Registering `"deepseek-v4-pro"`

The alias `"deepseek-v4-pro"` is registered by these providers, in this order.
The first entry (DeepSeek) wins by default; the others require a provider hint.

| Order | Provider          | Canonical ID                    | Wins by default? |
|-------|-------------------|---------------------------------|------------------|
| 1     | **DeepSeek**      | `deepseek-v4-pro`               | Yes              |
| 2     | NvidiaNim         | `deepseek-ai/deepseek-v4-pro`   | No               |
| 3     | Atlascloud        | `deepseek-ai/deepseek-v4-pro`   | No               |
| 4     | Volcengine        | `DeepSeek-V4-Pro`               | No               |
| 5     | Openrouter        | `deepseek/deepseek-v4-pro`      | No               |
| 6     | Novita            | `deepseek-ai/deepseek-v4-pro`   | No               |
| 7     | Fireworks         | `deepseek-ai/deepseek-v4-pro`   | No               |
| 8     | Siliconflow       | `deepseek-ai/DeepSeek-V4-Pro`   | No               |
| 9     | Sglang            | `deepseek-ai/deepseek-v4-pro`   | No               |
| 10    | Vllm              | `deepseek-ai/deepseek-v4-pro`   | No               |
| 11    | Huggingface       | `deepseek-ai/deepseek-v4-pro`   | No               |
| 12    | Together          | `deepseek-ai/deepseek-v4-pro`   | No               |
| 13    | Deepinfra         | `deepseek-ai/deepseek-v4-pro`   | No               |

> **Note:** The Openai provider registers a model with canonical id `"deepseek-v4-pro"`
> but does **not** include `"deepseek-v4-pro"` in its aliases - it only exposes
> `"openai-compatible-deepseek-v4-pro"`. Therefore Openai's entry does not
> participate in the `"deepseek-v4-pro"` alias collision.

## Other Notable Alias Collisions

### `"deepseek-chat"`
Registered by: DeepSeek (canonical), NvidiaNim, Volcengine, Openrouter, Siliconflow.
**Winner by default:** DeepSeek (index win).

### `"deepseek-reasoner"`
Registered by: DeepSeek (alias), NvidiaNim, Openrouter, Siliconflow, WanjieArk (canonical).
**Winner by default:** DeepSeek (index win, via its alias on `deepseek-v4-flash`).

### `"deepseek-v4-flash"`
Registered by: DeepSeek (canonical), NvidiaNim, Openrouter, Atlascloud, Volcengine, Siliconflow, Novita, Fireworks, Sglang, Vllm, Huggingface, Together, Deepinfra.
**Winner by default:** DeepSeek (index win).

### `"glm-5.2"`
Registered by: Openrouter (canonical `z-ai/glm-5.2`), Zai (canonical `GLM-5.2`).
**Winner by default:** Openrouter (registered first).
