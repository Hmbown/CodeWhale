# Workflow External Memory Cutline

This note resolves the v0.9.0 cutline for Aleph-style external memory in
Workflow. It is a design boundary, not a runtime implementation.

## Decision

External memory should be optional and explicit for v0.9.0. Normal CodeWhale
operation must not depend on it, and Workflow must not silently enable it for
long-running runs.

For v0.9.0, external memory can appear only as:

- an explicit workflow node whose inputs, outputs, scope, and permissions are
  visible in the typed Workflow IR;
- an optional plugin or skill-backed tool that the user enables deliberately;
- a documented experiment whose state can be inspected, cleared, and exported.

It should not be a hidden context substrate, a replacement for repo search, or a
default backing store for every workflow run.

## Layer Boundaries

External memory is separate from the existing memory and replay layers:

| Layer | Scope | v0.9.0 rule |
| --- | --- | --- |
| User memory | Small durable user preferences and facts surfaced by `/memory` | Opt-in, user-owned, not workflow evidence |
| Repo search / codemap | Derived repo structure and search results | Rebuildable from the workspace; not a memory log |
| ARMH/RLM memo | In-session working memory and exact-context memoization | Visible hit/miss telemetry; not durable replay evidence |
| TraceStore | Recorded workflow, branch, leaf, and control results | Source of deterministic replay; no live model calls during replay |
| Cached-main overlay | Promoted lessons after review and replay | Inspectable and reversible; never mutates Git main |
| External memory | Large local or plugin-backed data outside normal context | Explicit node/plugin only; visible state and clear/export required |

## Visibility Requirements

Any future external-memory implementation must show:

- when it is active;
- which workflow node or plugin owns it;
- where its state is stored;
- what repo or run scope it can read;
- whether it is included in replay, export, or promotion evidence;
- how to inspect, clear, pin, and export it.

The UI should treat this like an active context layer, not like invisible model
intuition. If a run cannot explain why a fact came from external memory, the
feature is not ready for default use.

## Permissions And Privacy

External memory must inherit the strictest relevant scope:

- it must not cross repo/workspace boundaries without explicit approval;
- project-local config must not silently enable broad external-memory reads;
- replay must record external-memory inputs as evidence or mark replay as
  unavailable/diverged;
- exports must make external-memory references visible without dumping private
  raw state by default.

## Deferred Work

The following remain out of scope for the v0.9.0 cutline:

- default-on Aleph-style memory for all Workflow runs;
- automatic promotion from external memory into cached-main overlay;
- hidden retrieval behind ordinary prompts;
- hosted or shared external-memory services;
- treating external memory as a substitute for TraceStore replay.

Future implementation should start with a read-only typed workflow node and a
mock replay fixture before adding any plugin-backed or live retrieval path.
