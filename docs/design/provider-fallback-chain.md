# Provider Fallback Chain — Design Document (#2574)

## Summary

Add an automatic provider fallback chain so that when the active provider
returns a non-recoverable error (429, selected 5xx, connection timeout),
CodeWhale switches to the next configured provider without interrupting
the user's workflow.

## Motivation

Currently, users must manually run `/provider` to switch when their
primary provider fails. This is especially disruptive during long-running
agentic tasks. A fallback chain keeps the agent working without user
intervention.

## Design

### Configuration

```toml
[providers]
active = "nvidia-nim"
fallback = ["deepseek", "openrouter"]

[providers.nvidia-nim]
api_key = "nvapi-..."
base_url = "https://integrate.api.nvidia.com/v1"
model = "meta/llama-4"

[providers.deepseek]
api_key = "$DEEPSEEK_API_KEY"
model = "deepseek-v4-pro"

[providers.openrouter]
api_key = "$OPENROUTER_API_KEY"
model = "deepseek/deepseek-v4-0324"
```

- `fallback` — ordered list of provider names to try
- `active` — the primary provider (existing `provider` key, renamed for clarity)

### Fallback triggers

| Error | Fallback? | Rationale |
|---|---|---|
| 429 (rate limit) | ✅ | Quota exhausted — swap key/provider |
| 502 / 503 / 504 | ✅ | Provider infrastructure issue |
| Connection timeout / DNS failure | ✅ | Network path broken |
| 401 / 403 | ❌ | Auth issue — no other provider will help |
| 400 (bad request) | ❌ | Client error — not provider-specific |
| Stream interrupted mid-content | ❌ | Already consumed partial response |

### Sequence

```
1. Try primary provider (nvidia-nim)
2. On fallback-eligible error → wait 1s → try fallback[0] (deepseek)
3. On fallback-eligible error → wait 1s → try fallback[1] (openrouter)
4. All exhausted → surface clear error to user
```

### Transcript / UI

- Status toast: `DeepSeek unavailable — switched to OpenRouter`
- Transcript marker: `[provider: nvidia-nim → deepseek]`
- `/provider` command shows current chain position: `deepseek (fallback #1)`
- Original (`active`) provider is remembered so user can `/provider reset` to go back

### Capability awareness

Before switching, the engine checks that the fallback provider supports
the current turn's needs:

| Capability | Check |
|---|---|
| Tools / function calling | Fallback provider must support tools |
| Reasoning effort | Must support same reasoning levels |
| Context length | Model must have ≥ current turn's token count |
| Vision | Must support image inputs if turn has images |

If no fallback provider meets capabilities, the error is surfaced directly.

### Retry integration

Existing `[retry]` settings apply per-provider **before** fallback triggers.
A provider gets `max_retries` attempts with `retry_delay` between them.
Only after retry exhaustion does fallback move to the next provider.

### Config schema validation

On startup, validate:
- Each `fallback` entry is a known provider
- No duplicate providers in chain
- Fallback providers have valid `api_key` (or env var set)
- Warn if fallback model has different capability profile

### Implementation plan (phased)

#### Phase 1 (draft PR #1): Config schema + validation

- Add `fallback` field to `ProvidersConfig`
- Startup validation of fallback chain
- Unit tests for config parsing

#### Phase 2 (draft PR #2): Engine fallback logic

- `try_with_fallback()` in `client.rs`
- Error classification (is fallback-eligible?)
- Saves original provider, pushes fallback status event
- Integration test with mock HTTP

#### Phase 3 (draft PR #3): UI feedback

- Status toast on switch
- Transcript marker
- `/provider` shows fallback position
- `/provider reset` command

### Rejected alternatives

- **Per-request model routing**: Too fine-grained; turns have state (system prompt, tools) that
  shouldn't change mid-turn
- **Weighted random selection**: Unpredictable billing; users need deterministic behavior
- **Sub-agent-level fallback**: Complicates sub-agent lifecycle for marginal gain

### Open questions

1. Should fallback persist across sessions or reset each launch?
   → **Reset each launch** (avoids silently staying on fallback forever)
2. Should `/compact` reset to primary provider?
   → **No** — compaction changes context, not provider
3. Tool call mid-turn: if tool call succeeds but next API call fails, do we fallback?
   → **Yes**, same turn can span providers as long as capabilities match
