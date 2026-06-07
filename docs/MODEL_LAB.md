# Model Lab Roadmap

Model Lab is the planned open-model workbench for CodeWhale. The north star is
simple: CodeWhale should become the best terminal coding agent for open-source
and open-weight models across every provider that offers them. Model Lab is how
those models become discoverable, evaluable, routable, servable, and exportable
without weakening the current terminal-agent contract: local workspace control,
explicit provider auth, approval gates, and clear privacy boundaries.

This document is roadmap language. Some worksets below are roadmap-only.

## Implemented Today

- DeepSeek is the first-class default provider today, with `deepseek-v4-pro`,
  `deepseek-v4-flash`, streaming thinking blocks, Fin routing, `DEEPSEEK_*`
  environment variables, and `~/.deepseek` config compatibility.
- OpenRouter, Novita, Fireworks, NVIDIA NIM, AtlasCloud, Wanjie Ark, generic
  OpenAI-compatible endpoints, SGLang, vLLM, and Ollama are supported provider
  paths where their IDs appear in `/provider`, `codewhale --provider`, or
  `codewhale models`.
- Hugging Face Inference Providers are available through the
  OpenAI-compatible router at `https://router.huggingface.co/v1`. Select the
  route with `huggingface`, `hugging-face`, `hugging_face`, or `hf`; configure
  `HUGGINGFACE_API_KEY` or `HF_TOKEN` for auth.
- Model auto-routing chooses a concrete DeepSeek model and thinking level per
  turn. It is not a TUI mode.
- Fin is the fast `deepseek-v4-flash` thinking-off path for routing,
  summaries, cheap checks, RLM child calls, wakeup verification, and
  binary-completion checks.
- Self-hosted OpenAI-compatible endpoints can be used through SGLang, vLLM,
  Ollama, or the generic `openai` provider configuration.

## Still Planned

- Hugging Face Hub browsing, upload/export, model card, dataset, adapter,
  safetensors, or Jobs workflows.
- Native Unsloth, NeMo, or Arcee integrations.
- A dedicated Model Lab UI tab.
- Built-in benchmark suites, eval leaderboards, hosted observability, or
  training-infrastructure orchestration.

Until those land, use the provider paths above, MCP servers, or external
workflows explicitly configured by the user.

## Model Lab Principle

Model Lab should help users answer practical questions:

- Which model should handle this turn?
- Which open or open-weight model can I run locally or through a trusted
  provider?
- Which provider offers this model with the latency, price, context window,
  license, and privacy posture I need?
- What did this model cost, how did it perform, and what data left my machine?
- Can I reproduce, export, or self-host the route?

It should never hide provider boundaries, silently upload local artifacts, or
describe a model as available before CodeWhale can actually route to it.

## Hugging Face Workset

Implemented today:

- Hugging Face Inference Providers as an explicit OpenAI-compatible router
  provider, selected with `huggingface`, `hugging-face`, `hugging_face`, or
  `hf`.
- Model IDs are sent to the router exactly as selected, including
  org-prefixed Hugging Face model IDs.

Planned scope:

- Hub API auth and model discovery.
- Model cards, licenses, tags, safetensors metadata, adapters, and dataset
  links surfaced in a terminal-friendly way.
- Hugging Face Jobs as an optional remote execution path for user-approved
  experiments.

Non-goal for now: treating the router route as Hub browsing/export, or
inferring Hub upload/export auth from the inference-provider API key.

## Unsloth Workset

Planned scope:

- Fine-tuning recipes and adapter workflows for users who already own the data
  and compute path.
- Export guidance that keeps dataset, adapter, and checkpoint locations explicit.
- Compatibility notes for models that can return to local serving or a hosted
  OpenAI-compatible endpoint.

## NeMo Workset

Planned scope:

- Training and alignment workflow notes for users operating NVIDIA-centric
  infrastructure.
- Clear boundaries between NVIDIA NIM inference support that exists today and
  future NeMo training or customization workflows.

## Arcee Workset

Planned scope:

- Small-model routing and specialization experiments.
- Exportable routes that make it clear when a task is handled by a smaller
  model, Fin, or full DeepSeek reasoning.

## Serving Workset

Planned scope:

- Better local and private serving ergonomics for SGLang, vLLM, Ollama, and
  OpenAI-compatible gateways.
- Health checks, model listing, context-window metadata, and route validation.
- No silent network exposure: public endpoints must be configured explicitly.

## Eval Workset

Planned scope:

- Reproducible task suites for coding, review, docs, release checks, and
  long-context workflows.
- Side-by-side route comparisons where the exact model, provider, thinking
  level, prompt, and tool policy are captured.

## Observability Workset

Planned scope:

- Local-first traces for turn routing, tool calls, approvals, cost, cache
  behavior, and context pressure.
- Export rules that redact secrets and require explicit user action before data
  leaves the machine.

## Training Infra Workset

Planned scope:

- Recipes for dataset preparation, adapter training, artifact naming, and
  promotion into serving.
- Separation between local/private artifacts and anything published to a hub or
  registry.

## Privacy And Export Rules

- Local files, prompts, transcripts, traces, model outputs, eval results,
  adapters, datasets, and checkpoints should remain local unless the user
  explicitly chooses a provider or export destination.
- Provider auth must remain explicit. `DEEPSEEK_*`, OpenRouter,
  `HUGGINGFACE_API_KEY` / `HF_TOKEN`, and self-hosted credentials should not be
  inferred from unrelated config.
- Exportable artifacts should include provenance: source model, provider,
  route, tool policy, eval inputs, and redaction status.
- Public sharing, hosted telemetry, sponsorship badges, and external branding
  require maintainer approval.
