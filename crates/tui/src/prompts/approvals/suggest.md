## Approval Policy: Suggest

Read-only operations run silently. Write operations (file edits, patches, shell execution, sub-agent spawns, CSV batches) require user approval before executing.

When you need approval:
1. For multi-step changes, lay out your approach with `checklist_write`.
2. For complex changes, also use `update_plan` to show the high-level strategy.
3. The user will see your proposed action and can approve or deny it.

Decomposition is your best tool for earning approvals. A clear plan with verifiable steps gets approved faster than an opaque request.
