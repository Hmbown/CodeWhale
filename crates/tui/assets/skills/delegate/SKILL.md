---
name: delegate
description: Strategic delegation for multi-step coding, research, or verification work. Use when a task can be split into parent reasoning plus focused sub-agent execution through agent_open, agent_eval, and agent_close.
metadata:
  short-description: Delegate focused work to sub-agents
---

# Delegate

Use sub-agents when they can do focused work in parallel while the parent keeps architectural judgment, integration, and final verification.

## Keep vs Delegate

Keep in the parent:

- Understanding the user's actual request and constraints
- Architecture, security, product, and release-risk decisions
- Cross-module integration
- Final review, test interpretation, and user-facing summary

Delegate to sub-agents:

- Read-only exploration over a bounded file set
- Mechanical edits with a clear file ownership boundary
- Focused test or lint runs
- Boilerplate generation from an explicit spec
- Independent checks that can run while parent work continues

Do not delegate tiny one-step tasks, ambiguous product decisions, destructive operations without a clear acceptance criterion, or final verification.

## Open Focused Sessions

Prefer `agent_open` for a named child session, then `agent_eval` to fetch or wait on its result. Open independent children together so they can run in parallel.

```json
{
  "name": "config_audit",
  "prompt": "Inspect crates/tui/src/config.rs and crates/tui/src/settings.rs for duplicate model-default logic. Return file/line findings only; do not edit files.",
  "type": "explore",
  "model": "deepseek-v4-flash",
  "cwd": "."
}
```

For code changes, give the child a precise write boundary and tell it not to revert unrelated edits:

```json
{
  "name": "docs_patch",
  "prompt": "Update only docs/configuration.md to document the new [statusline] keys. Match the surrounding style. Do not edit other files.",
  "type": "implementer",
  "model": "deepseek-v4-flash",
  "cwd": "."
}
```

Use `fork_context: true` only when the child genuinely needs the current conversation prefix. Leave it omitted for fresh, narrower context.

## Evaluate and Verify

Use `agent_eval` with `block: true` when you need the result before continuing:

```json
{
  "name": "docs_patch",
  "block": true,
  "timeout_ms": 120000
}
```

Use `block: false` for a quick status projection while other work continues.

Sub-agent outputs are self-reports. Re-check material claims before relying on them:

- Read changed files directly.
- Run the relevant tests locally.
- Inspect unexpected diffs before committing.
- Verify externally visible or destructive claims against source data.

Close sessions that are no longer useful with `agent_close`.

## Prompt Shape

A good delegation prompt includes:

- The exact task
- Files or modules owned by the child
- Files or behavior the child must not touch
- Expected output format
- Acceptance criteria

Weak prompt:

```text
Fix the settings bug.
```

Strong prompt:

```text
Own only crates/tui/src/settings.rs and its tests. Preserve existing config key names. Add a regression test showing that provider-specific API key changes do not restart DeepSeek onboarding. Return the changed paths and test command output.
```
