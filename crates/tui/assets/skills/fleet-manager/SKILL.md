---
name: fleet-manager
description: Use when managing, triaging, restarting, escalating, or summarizing Codewhale Pod runs and workers.
metadata:
  short-description: Triage Codewhale Pod runs
---

# Pod Manager

Use this skill when acting as a manager agent for Codewhale Pod runs.
Your job is to classify worker state, choose the narrowest safe typed action,
and leave a ledgered receipt or a safe escalation draft.

## Authority Boundary

- Prefer typed Pod surfaces over shell spelunking: `codewhale pod status`,
  `inspect`, `logs`, `artifacts`, `interrupt`, `restart`, `stop`, and the
  Runtime API endpoints.
- Do not read `.codewhale/fleet.jsonl`, host logs, or remote files directly
  unless the typed command or API is missing required evidence.
- Do not send Slack, webhook, PagerDuty, email, or chat messages unless the
  user or run config explicitly authorizes sending. Draft the message instead.
- Never include secrets, tokens, webhook URLs, routing keys, full prompts, or
  oversized logs in a summary or escalation.

## Triage Loop

1. Identify the run and worker from the user request, run receipt, or Pod
   status output. If no worker is named, start with `codewhale pod status`.
2. Inspect the worker with `codewhale pod inspect <worker-id>` or the matching
   Runtime API worker endpoint.
3. Review bounded evidence with `codewhale pod logs <worker-id>` and
   `codewhale pod artifacts <worker-id>`. Summarize artifact refs, not full
   payloads.
4. Classify the state before acting:
   - `transient failure`: transport error, timeout, stale heartbeat, host
     unavailable, or retryable provider/network failure.
   - `task failure`: worker completed the task but the result is wrong,
     missing required artifacts, or reports a domain error.
   - `verifier failure`: scorer/verifier failed or disagrees with the worker
     result.
   - `needs-human`: missing authority, unsafe secret boundary, destructive
     action, repeated restart exhaustion, ambiguous product decision, or
     conflict between artifacts and verifier.
5. Choose one typed action:
   - transient and retry budget remains: `codewhale pod restart <worker-id>`.
   - transient but unsafe to retry: draft escalation and mark needs-human.
   - task failure: preserve artifacts, summarize the failure, and avoid restart
     unless the task spec says retrying can produce new evidence.
   - verifier failure: inspect scorer inputs and artifacts, then escalate if the
     verifier cannot be corrected through a typed action.
   - needs-human: do not restart automatically; draft a concise escalation.
6. Record the result in the response: classification, action taken or drafted,
   evidence commands, artifact refs, and next owner.

## Restart vs Escalate

Restart only when all of these are true:

- the failure is likely transient,
- the task is idempotent or the run policy allows retry,
- retry budget remains,
- no secret, permission, or destructive action boundary is involved, and
- the previous attempt produced enough receipt data to explain the restart.

Escalate when any of these are true:

- restart budget is exhausted,
- the worker requests secrets or new authority,
- artifacts indicate data loss, corruption, or destructive side effects,
- the verifier and task result conflict in a way you cannot resolve from typed
  evidence,
- the same failure repeats after a restart, or
- a human product or release decision is required.

## Safe Escalation Draft

Use this shape for Slack/PagerDuty drafts. Keep logs to three short lines or an
artifact ref.

```text
Codewhale Pod needs attention
Run: <run-id>
Worker: <worker-id>
Task: <task-id or unknown>
Classification: <transient failure | task failure | verifier failure | needs-human>
Reason: <one sentence, no secrets>
Latest typed evidence: codewhale pod inspect <worker-id>; codewhale pod artifacts <worker-id>
Safe log excerpt: <3 lines max or "see artifact <ref>">
Requested decision: <restart approval | verifier review | task owner review | permission decision>
```

## Post-Run Receipt

End every Pod Manager response with a compact receipt:

```text
Pod receipt
Run: <run-id>
Workers checked: <count/list>
Classification: <state>
Action: <restart/interrupt/stop/escalation draft/no-op>
Ledger expectation: <typed action should be recorded | draft only, no send>
Artifacts reviewed: <refs>
Follow-up owner: <manager | task owner | human>
```
