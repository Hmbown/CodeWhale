# Cost, Usage, and Route Honesty

CodeWhale's rule for money is simple: **it never invents a price.** If the
runtime cannot source a price for the route you are on, it says so instead of
guessing — no fabricated token rates, and no implicit "free" for local,
custom, or subscription routes.

## The five cost states

Every resolved route carries exactly one pricing state (`PricingSku` in
`crates/config/src/route/candidate.rs`), shown in the provider picker:

| State | Meaning | Shown as |
|:---|:---|:---|
| `Token` | Per-token pricing is sourced | `cost: $X/$Y mtok` |
| `SubscriptionQuota` | Subscription plan; quota usage, not dollars | `usage: subscription N%` |
| `AccountCredits` | Prepaid credit balance | `usage: $X credits` |
| `LocalOrNotApplicable` | Local runtime or route that isn't billed per token | `cost: local` |
| `UnknownOrStale` | No sourced price, or the sourced price aged out | provider-specific usage label, never a number |

Degradation is deliberate: an offering whose price can't be sourced resolves to
`UnknownOrStale`, and a partially-priced row (e.g. cache rates without
input/output rates) degrades to `UnknownOrStale` rather than displaying a
rate-less `Token` state.

## Where prices come from

- A bundled price table (`crates/tui/src/pricing.rs`) plus Models.dev catalog
  snapshots, with per-model user overrides. Every sourced price carries
  **provenance** (`ModelsDevBundled`, `ProviderLive`, `ProviderDocs`,
  `UserOverride`) and only live-sourced rows can go age-stale.
- DeepSeek rows carry the published CNY rates alongside USD; the DeepSeek
  balance endpoint is used for your **account balance** only — prices are
  never scraped from responses.
- Deliberate gaps: NVIDIA-NIM-hosted `deepseek-ai/*` rows are unpriced (so
  they can't masquerade as DeepSeek's own rates), and ChatGPT/Codex OAuth
  usage is shown **without** a spend estimate, because that plan exposes no
  authoritative dollar pricing to this runtime.
- Cost estimation refuses to produce a number if *any* token class you
  actually used has an unknown price.

Display currency: `cost_currency = "usd" | "cny"` in config (aliases `rmb`,
`yuan`); affects the footer, `/cost`, `/tokens`, and notification summaries.

## Commands

| Command | Shows |
|:---|:---|
| `/cost` | Approximate session total in your configured currency. That's all — estimates use provider usage telemetry when available and are explicitly labeled approximate. |
| `/tokens` | The real breakdown: active context vs. window, last API input/output tokens, cache hit/miss, cumulative tokens, approximate session cost, message counts, model. |
| `/cache` | Prefix-cache telemetry: per-turn hit/miss history; `inspect` (prompt-layer hashes and first divergence — never the prompt text); `stats` (prefix stability %, aggregate hit rate with a note under 80%); `zones`; `warmup`. |

The footer keeps running chips: `tok <total>`, session cost (with
`· saved <amount>` when the last turn had cache savings), `Cache: N% hit`,
`active ctx N%`, and your DeepSeek balance when available.

## Route honesty

The route resolver (`crates/config/src/route/resolver.rs`) is the single
authority on what model you're talking to, and it is deliberately boring:

- The provider comes **only** from your explicit selection — it is never
  inferred from a model-name prefix or a base URL.
- The model id you selected is preserved **verbatim** as the wire id sent to
  the provider. Asking a direct provider for a model it doesn't serve is
  rejected up front, not silently rerouted.
- There is structurally no prompt-content routing: the route request has no
  freeform text field, so nothing about your prompt can change the route.
- In Fleet runs, worker profiles cannot select or override providers —
  provider authority stays with the orchestrator ([FLEET.md](FLEET.md)).
- Auth diagnostics show the provider, base-URL authority, key source, and key
  fingerprint — never a secret value.

## What doesn't exist (yet)

Honest list: there are **no monetary spend limits, cost caps, or dollar-based
alarms**. The budget machinery that does exist (`route_budget`,
`context_budget`, `/goal` budgets) is denominated in tokens and time, not
money. Cost figures are estimates for orientation, not billing records — your
provider's dashboard is the source of truth for what you owe.

Related: [PROVIDERS.md](PROVIDERS.md) for route setup,
[MODEL_LAB.md](MODEL_LAB.md) for comparing routes,
[CONFIGURATION.md](CONFIGURATION.md) for `cost_currency` and the token
quantities reference.
