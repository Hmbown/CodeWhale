# Pro Plan Mode

Pro Plan is a model-routing mode inspired by the public behavior of Claude
Code's plan-first workflow:

- Plan phase: use `deepseek-v4-pro` with the existing Plan mode prompt and
  read-only tool policy.
- Execute phase: use `deepseek-v4-flash` with Agent-mode tools and normal
  approvals.
- Review phase: use `deepseek-v4-pro` with Plan-mode read-only tools.

The mode intentionally reuses the existing Plan and Agent contracts instead of
inventing a separate prompt or permission system.

## State Flow

```text
Plan --user accepts plan--> Execute --explicit completion marker--> Review
Review --approved marker--> Done
Review --changes requested marker--> Execute
Execute --explicit replan marker--> Plan
```

Plan confirmation is shown only when the Plan phase actually creates plan
state, either through the existing `update_plan` tool path or an explicit
`<pro_plan plan_ready="true">` marker. Ordinary numbered answers are not enough
to trigger implementation.

The Review follow-up is queued only on the real `Execute -> Review` transition.
Remaining in Review does not enqueue another review request, which prevents
empty review loops after non-implementation conversations.

## Markers

Markers are control protocol, not user-facing prose:

- `<pro_plan plan_ready="true">`
- `<pro_plan execute_complete="true">`
- `<pro_plan review="approved">`
- `<pro_plan review="changes_requested">`
- `<pro_plan replan="true">`

Natural-language words like "review", "lgtm", "可以", or numbered lists are
not used as state-transition triggers.

## Fail-Closed Rules

Normal Pro Plan turns are resolved before dispatch:

- `Plan`, `Review`, and `Done` use `AppMode::Plan`.
- `Execute` uses `AppMode::Agent`.

If a raw `AppMode::ProPlan` reaches the engine unexpectedly, it fails closed to
Plan-mode behavior: read-only registry, read-only sandbox, and Never approval.
