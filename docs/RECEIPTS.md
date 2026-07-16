# Runtime Receipts

This document sketches a future read-only receipt export for completed runtime
turns. It is a protocol note, not an implemented endpoint.

The goal is to let a local supervisor audit one completed turn without
screen-scraping the terminal transcript. A receipt should summarize the durable
runtime records that Codewhale already owns: thread metadata, turn status, turn
items, event sequence lineage, usage when available, approval decisions, and
side-effect boundaries.

## Non-Goals

A receipt is not a safety certification, provider compatibility certification,
or hosted attestation. It must not call providers, execute tools, write memory,
write project files, mutate runtime state, or expose API keys.

Receipts should not export raw chain-of-thought or private reasoning by default.
When reasoning custody is represented, use stable item ids, counts, hashes, or
explicit `unavailable` fields rather than raw hidden content.

## Candidate Surfaces

Potential local-only surfaces:

```text
codewhale receipt export --thread <thread_id> --turn <turn_id> --format json
GET /v1/threads/{thread_id}/turns/{turn_id}/receipt
```

Both surfaces should share the existing runtime API auth boundary. They should
only read persisted runtime records and append-only events.

## Review Receipts

`codewhale review --write-receipt` writes a local JSON receipt for the reviewed
diff under the Codewhale state directory (`review-receipts/`) unless
`--receipt-path <path>` is provided. This is a pre-push handoff artifact: it
records what diff was reviewed and what the review reported, without pushing,
tagging, opening a PR, or claiming to replace maintainer review.

The current receipt includes:

- `diff_fingerprint`: SHA-256 of the reviewed diff.
- `provider` and `model`: the routed review provider/model.
- `checks_run`: local checks attached to the receipt when available. Empty
  means no checks were attached; attached checks must report a passing status.
- `findings`: structured issue/suggestion counts and issue locations when the
  review output is structured.
- `unresolved_risk`: a conservative summary derived from unresolved findings.
- `review_content_sha256`: SHA-256 of the review text.

The receipt deliberately does not include the raw diff body. Re-run
`codewhale review --write-receipt` after changing the diff; reviewers should
compare the `diff_fingerprint` before reusing a receipt in a PR handoff.

`codewhale review --check-receipt` is the local pre-push gate. It does not call
a model; it compares the current diff fingerprint with a supplied receipt
(`--receipt-path <path>`) or the latest matching local receipt. The check exits
nonzero when the diff no longer matches, the receipt schema is unsupported, the
receipt has unresolved risk, or an attached check did not pass.

## Current Data Sources

The current runtime store already persists the core inputs a receipt builder
would need:

- `ThreadRecord`: model, workspace, mode, shell/trust/auto-approve flags,
  title, task linkage, and latest turn metadata.
- `TurnRecord`: turn status, input summary, timestamps, duration, usage, error,
  steer count, and item ids.
- `TurnItemRecord`: item kind, lifecycle status, summary, optional detail,
  metadata, artifact refs, and item timestamps.
- `RuntimeEventRecord`: thread id, turn id, item id, event name, JSON payload,
  timestamp, and monotonic `seq` values per runtime store.

Not every receipt field can be filled from those records today. If a provider or
store does not persist a value, the receipt should say `available: false` or
`unavailable`, not infer it from UI text.

## Draft Schema Shape

```json
{
  "schema_id": "codewhale.conformance-receipt/v0",
  "thread": {
    "id": "thr_...",
    "model": "deepseek-v4-pro",
    "mode": "agent",
    "auto_approve": false,
    "trust_mode": false,
    "allow_shell": false
  },
  "turn": {
    "id": "turn_...",
    "status": "completed",
    "started_at": "2026-06-02T01:00:00Z",
    "ended_at": "2026-06-02T01:00:12Z",
    "duration_ms": 12000
  },
  "reasoning_custody": {
    "raw_reasoning_exported": false,
    "available": false,
    "reason": "reasoning blocks are not persisted as receipt-ready records"
  },
  "tool_lineage": {
    "tool_call_count": 1,
    "tool_result_count": 1,
    "unmatched_tool_call_ids": [],
    "unmatched_tool_result_ids": []
  },
  "usage_evidence": {
    "available": true,
    "usage": {
      "prompt_tokens": 123,
      "completion_tokens": 45
    },
    "provider_cache_breakdown_available": false
  },
  "source_event_lineage": {
    "first_seq": 10,
    "last_seq": 42,
    "event_count": 33,
    "missing_event_ranges": []
  },
  "side_effect_boundary": {
    "approval_required_count": 1,
    "approval_allowed_count": 0,
    "approval_denied_count": 1,
    "command_execution_count": 0,
    "file_change_count": 0,
    "sandbox_denied_count": 0
  },
  "claim_ceiling": [
    "local_receipt_only",
    "not_safety_certification",
    "not_provider_compatibility_certification"
  ]
}
```

## Builder Rules

A receipt builder should be deterministic and conservative:

1. Load the thread and turn by id, then reject mismatched `thread_id` values.
2. Load only item ids referenced by the turn.
3. Read event records for the thread and filter by `turn_id`.
4. Preserve event sequence boundaries with `first_seq`, `last_seq`, and any
   detected gaps.
5. Count approval, command, file, sandbox, and tool events from typed records or
   known event names only.
6. Mark unavailable evidence explicitly instead of deriving it from free-form
   summaries.
7. Emit no raw tool output beyond existing item summaries unless a later schema
   adds a separate redaction policy.

## Incremental Implementation Path

The safest implementation path is:

1. Land this protocol note and settle field names/non-goals.
2. Add protocol structs and JSON snapshot fixtures for completed, failed, and
   approval-denied turns.
3. Add a pure builder over `ThreadRecord`, `TurnRecord`, `TurnItemRecord`, and
   `RuntimeEventRecord`.
4. Expose the local runtime API endpoint.
5. Add the CLI export command and optional validation mode.
