# Pro Plan Profile

Pro Plan is a config-gated routing profile, not part of the default `Tab` mode
cycle or `/mode` picker. Enable it with
`/config pro_plan_profile true --save`, then enter it with `/mode pro-plan`.
The user chooses the profile; then CodeWhale chooses the model route for each
phase. Planning and review stay on the stronger model while implementation uses
the faster model when available:

- Plan phase: use `deepseek-v4-pro` with the existing Plan mode prompt and
  read-only tool policy.
- Execute phase: use `deepseek-v4-flash` with Agent-mode tools and normal
  approvals, or temporary YOLO semantics when the user accepts the plan with
  auto-approval.
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

For implementation-like requests in Pro Plan's Plan phase, the TUI adds a
small turn-local instruction to use the existing Plan behavior and call
`update_plan` as the next step. The engine keeps that requirement active until
`update_plan` succeeds, so even text-parsed tool calls such as `read_file` are
blocked before the plan confirmation gate. Pure questions are not wrapped this
way, so normal Q&A does not pop a confirmation dialog.

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
- `Execute` after "Accept plan (YOLO)" uses `AppMode::Yolo` for that Pro Plan
  execution pass, but the visible mode stays Pro Plan so review still runs. If
  review requests follow-up changes, the next Execute pass returns to
  Agent-style approvals unless the user explicitly accepts with YOLO again.

After `Done`, the next user turn resets the router to a fresh Plan phase.

Model routes are provider-aware. If the active provider does not advertise a
usable `deepseek-v4-flash` route, the Execute phase falls back to the resolved
Pro model instead of sending an unavailable model id.

If a raw `AppMode::ProPlan` reaches the engine unexpectedly, it fails closed to
Plan-mode behavior: read-only registry, read-only sandbox, and Never approval.
