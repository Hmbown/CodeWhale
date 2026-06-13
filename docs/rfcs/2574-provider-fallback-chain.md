# RFC: Provider Fallback Chain

**Issue:** #2574
**Reporter:** @hsdbeebou
**Design source:** #2581 by @idling11
**Status:** Draft for the v0.9 provider-routing lane
**Date:** 2026-06-04

## Problem

CodeWhale can store credentials and defaults for several providers, but a
running session uses one active provider route at a time. When that provider
hits a rate limit, temporary outage, or transport failure, the user must notice
the failure, run `/provider`, choose another route, and resubmit the turn.

That manual switch is especially disruptive during long-running agentic work.
A provider fallback chain can keep work moving, but it also changes billing
source, model behavior, tool support, context-window limits, and vendor
expectations. The design must make that switch explicit and capability-aware.

## Principles

- Fallback is opt-in. No provider switch happens unless the user configured a
  fallback chain.
- Billing and vendor changes are visible in the transcript and status UI.
- Normal retry policy runs before fallback.
- Fallback is allowed only before assistant content or tool calls have started
  streaming for the failing request.
- Fallback candidates must support the request shape for the current turn.
- Authentication, authorization, malformed request, and model-not-found errors
  do not silently switch providers by default.

## Proposed Config Shape

Keep the existing root `provider = "..."` setting as the primary route. Add an
ordered fallback list and a small policy section:

```toml
provider = "nvidia-nim"
fallback_providers = ["deepseek", "openrouter"]

[provider_fallback]
enabled = true
reset_on_new_session = true
```

Rules:

- `fallback_providers` is ordered and contains provider IDs already accepted by
  the provider parser.
- The primary provider is not repeated in the fallback list.
- Duplicate fallback providers are rejected.
- Missing credentials produce a startup warning and make that fallback entry
  inactive until credentials appear.
- If `provider_fallback.enabled` is absent, the presence of a non-empty
  `fallback_providers` list enables fallback.

## Fallback Eligibility

| Failure | Fallback by default? | Notes |
| --- | --- | --- |
| HTTP 429 | Yes | Rate limit or quota exhaustion on the active route. |
| HTTP 502, 503, 504 | Yes | Temporary upstream failure after normal retries. |
| Connect timeout / DNS failure | Yes | Transport path failed before content streamed. |
| HTTP 401 / 403 | No | Usually bad credentials or account permissions. |
| HTTP 400 | No | Usually client request shape or model parameter issue. |
| Model not found | No | Avoid silently switching model families unless a future policy explicitly opts in. |
| Stream interrupted after content | No | The transcript may already contain partial assistant content or tool-call deltas. |

The first implementation should classify errors centrally and expose tests for
each case before any fallback execution is wired into the turn loop.

## Capability Gate

Before switching to a fallback provider/model, CodeWhale checks that the
candidate can support the current request shape:

| Requirement | Gate |
| --- | --- |
| Tool calls | Candidate provider/model must support tool calling. |
| Reasoning effort | Candidate must support the requested thinking mode, or the switch is blocked. |
| Context size | Candidate context window must fit the estimated current request. |
| Image inputs | Candidate must support vision if the turn includes images. |
| Provider-specific headers | Candidate request must be rebuilt from that provider's own auth/base-url/header rules. |

If no fallback candidate passes the gate, CodeWhale surfaces the original
provider error with a clear "fallback chain exhausted or incompatible" note.

## Runtime Behavior

1. Build the request for the active provider.
2. Run existing retry policy for that provider.
3. If retries exhaust with a fallback-eligible failure and no assistant content
   has streamed, evaluate the next fallback provider.
4. Rebuild the request with the fallback provider's model, base URL, auth, and
   provider-specific headers.
5. Add a visible transcript marker and status event before the fallback request
   starts.
6. Continue through the chain until a provider succeeds, the chain is
   exhausted, or a non-eligible failure occurs.

Suggested transcript marker:

```text
[provider fallback: nvidia-nim -> deepseek, reason: rate_limit]
```

Suggested status text:

```text
NVIDIA NIM unavailable; switched to DeepSeek fallback
```

For multi-request turns, such as tool-call result follow-ups, fallback can be
considered for a later request only if that later request has not started
streaming assistant content yet. The transcript marker must identify that the
turn changed provider between requests.

## UI and Commands

- `/provider` should show the primary route and the current fallback position.
- `/provider reset` should return to the primary provider for future requests in
  the current session.
- The footer/statusline should surface the concrete provider/model that actually
  handled the latest request.
- Session receipts should record both attempted provider and successful
  provider so cost and debugging information stay truthful.

## Implementation Slices

1. Config schema and validation:
   - parse `fallback_providers` and `[provider_fallback]`
   - validate known providers, duplicates, missing credentials, and primary
     self-reference
   - document the config surface
2. Error classification:
   - define fallback-eligible error kinds
   - add unit tests for HTTP and transport failures
3. Request-shape capability gate:
   - evaluate tool, thinking, context, and image requirements
   - add tests for incompatible fallbacks
4. Fallback execution:
   - run retries per provider before moving to the next provider
   - rebuild auth/base-url/header state for each candidate
   - block fallback after partial streaming
5. UI/receipt integration:
   - status event
   - transcript marker
   - `/provider reset`
   - receipt fields for attempted and selected provider

## Non-goals

- No automatic cost optimization or weighted provider selection.
- No silent fallback when authentication or permissions fail.
- No fallback after partial assistant content or tool-call deltas have streamed.
- No provider/model capability downgrades without an explicit future policy.
- No sub-agent-specific fallback policy in the first implementation; sub-agents
  inherit the same configured fallback chain unless they are given an explicit
  provider/model override.

## Credit

This RFC is based on issue #2574 from @hsdbeebou and PR #2581 from @idling11.
The original PR head currently has no net file changes, so this document
preserves the useful design direction while tightening the v0.9 contract around
truthful provider routing, billing visibility, and capability checks.
