# Pro Plan Evaluation Tasks

This suite compares DeepSeek TUI's Pro Plan routing against single-model
baselines. It mirrors the Claude Code `opusplan` idea: use the strongest model
for planning/review and the cheaper/faster model for implementation.

## Variants

Run every task in a fresh session for each variant:

1. `pro-only`: normal Agent mode with `deepseek-v4-pro`.
2. `flash-only`: normal Agent mode with `deepseek-v4-flash`.
3. `pro-plan`: Pro Plan mode, using Pro for planning/review and Flash for
   execution.

For each run, record:

- `success`: task done and tests/checks pass.
- `wall_time`: from first prompt submit to final answer.
- `turns`: user-visible turns, including plan approval.
- `input_tokens` and `output_tokens`: from `/status` or session metadata.
- `reasoning_replay_tokens`: if reported.
- `tool_calls`: count total tools and failed tools.
- `approval_count`: number of user approvals requested.
- `files_changed`: from `git diff --name-only`.
- `diff_size`: `git diff --stat`.

## Tasks

### T1 Codebase Orientation

Goal: test planning/read-only exploration quality and token use.

Prompt:

```text
Explain how user messages flow from the TUI composer to the DeepSeek API
request. Identify the main files, the model-selection point, and where tool
results get appended. Do not edit files.
```

Success criteria:

- Names the relevant UI, app, engine, turn loop, and client paths.
- Explains where the model is selected.
- Makes no workspace edits.

### T2 Small Bugfix

Goal: measure whether Pro Plan overhead is worthwhile for a narrow edit.

Prompt:

```text
Find why `cargo test -p deepseek-tui pro_plan` would fail if a new AppMode
variant is missing from test-only footer styling, then fix the minimal match
branch and run the focused test.
```

Success criteria:

- Fix is minimal and scoped.
- Focused test passes.
- No unrelated formatting churn beyond `cargo fmt`.

### T3 Multi-File Mode Integration

Goal: stress architecture planning before implementation.

Prompt:

```text
Add a new TUI mode named `Audit` that behaves like Plan mode for tools and
permissions, but has a distinct label, `/mode audit` parsing, footer/header
display, and help text. Keep the implementation consistent with existing
mode patterns and add/adjust focused tests.
```

Success criteria:

- All AppMode matches compile.
- Audit uses read-only tool policy.
- `/mode audit`, config UI enum, footer/header/help paths are covered.
- Focused tests pass.

### T4 Review-Gated Refactor

Goal: test whether planning reduces rework on risky edits.

Prompt:

```text
Refactor ProPlan phase handling so UI side effects are separated from router
state mutation. Preserve behavior, reduce borrow-checker risk, and add tests
for Plan -> approval pending, Execute -> Review, Review approved -> Done.
```

Success criteria:

- Router remains pure/state-machine-like.
- UI borrow scopes are simple.
- Tests cover the phase transitions.
- `cargo check -p deepseek-tui` passes.

### T5 Test-First Bug

Goal: evaluate debugging and test discipline.

Prompt:

```text
Write a failing focused test showing that entering Pro Plan while auto model
is enabled restores the previous auto-model setting after leaving Pro Plan.
Then implement the fix and run the focused test.
```

Success criteria:

- Test fails before the fix or clearly captures the regression.
- Leaving Pro Plan restores prior `auto_model`.
- Focused test passes.

### T6 No-Code-Change Guardrail

Goal: ensure plan/review modes avoid unnecessary edits.

Prompt:

```text
Investigate whether Pro Plan review can accidentally write files. If the
current implementation already prevents writes, explain why and do not edit
the workspace. If it does not, propose a minimal fix but wait for approval.
```

Success criteria:

- Correctly identifies effective mode routing for Review.
- Makes no code changes if the guardrail is already present.
- Does not invent unnecessary refactors.

## Comparison Template

| Task | Variant | Success | Wall Time | Input Tokens | Output Tokens | Tool Calls | Failed Tools | Approvals | Files Changed | Notes |
| ---- | ------- | ------- | --------- | ------------ | ------------- | ---------- | ------------ | --------- | ------------- | ----- |
| T1 | pro-only | | | | | | | | | |
| T1 | flash-only | | | | | | | | | |
| T1 | pro-plan | | | | | | | | | |

## Expected Reading

Pro Plan should win most clearly on T3, T4, and T5: tasks with cross-file
reasoning, architecture choices, or test-first debugging. It may lose on T2
because the extra planning/review turn can cost more than it saves for a tiny
single-branch fix. T6 is a guardrail test: the best outcome is restraint, not
more code.

## Hard Suite

The small benchmark is useful for smoke testing, but it is too simple to show
the intelligence advantage of a Pro planning/review phase. Use this harder
suite when the goal is to compare task success, rework, and expensive-model
token share.

### H1 Exec ProPlan Parity

Prompt:

```text
Add non-interactive `exec` support for ProPlan so benchmarks can run the same
state-machine path as the TUI. It should support a Pro planning turn, accepted
plan handoff, Flash execution, and Pro review without requiring terminal key
input. Preserve existing `exec` behavior for Agent/Yolo and add focused tests.
```

Why it is hard:

- Requires understanding `main.rs`, `ui.rs`, `app.rs`, engine config, session
  persistence, and approval behavior.
- A shallow implementation can fake model routing while bypassing the actual
  ProPlan state machine.
- Success depends on tests plus real stream-json metadata.

Success criteria:

- `exec` has a documented way to select ProPlan.
- The same router phases are used by TUI and non-interactive runs.
- Metadata shows Pro for plan/review and Flash for execution.
- Existing Agent/Yolo exec tests still pass.

### H2 ProPlan Resume and Persistence

Prompt:

```text
Make ProPlan resumable across sessions. If the user exits after a generated
plan, resuming the session should restore the ProPlan phase, chosen models,
and pending approval state. Add tests around session save/load or the closest
existing persistence boundary.
```

Why it is hard:

- Requires locating the actual session snapshot format and deciding what should
  be persisted versus recomputed.
- Forces the model to reason about backward compatibility and migration.
- Flash-only is likely to make local fixes without preserving invariants.

Success criteria:

- Old sessions without ProPlan metadata still load.
- Resume after Plan returns to pending approval, not Execute.
- Resume after Execute returns to Review or Done according to saved state.
- Focused tests and `cargo check -p deepseek-tui` pass.

### H3 Permission Matrix Audit

Prompt:

```text
Audit and harden the permission behavior for Agent, Plan, Yolo, and ProPlan.
Create focused tests proving that ProPlan Plan and Review phases cannot write
files or run write-class tools, while Execute follows Agent-style approval.
Fix any bugs you find without broad refactors.
```

Why it is hard:

- Requires understanding mode-to-prompt routing, approval policy, sandbox
  setup, and tool classification.
- It has a security invariant, so a passing compile is not enough.
- The best solution may be a small fix after a broad read-only investigation.

Success criteria:

- Tests cover all mode/phase combinations.
- Plan and Review are read-only even when the outer mode is ProPlan.
- Execute uses the intended approval policy.
- No unrelated permissions become more permissive.

### H4 Streaming Tool Loop Recovery

Prompt:

```text
Find and fix a bug where an assistant response containing interleaved thinking,
text, and multiple tool_use blocks can cause a wrong ProPlan phase transition
or duplicated tool_result injection. Add a regression test using synthetic
content blocks.
```

Why it is hard:

- Requires reading the turn loop, event conversion, and UI TurnComplete hook.
- The failure mode is temporal and stateful rather than a single wrong branch.
- Good solutions need a minimal synthetic test instead of relying only on live
  API behavior.

Success criteria:

- Regression test fails without the fix.
- Multiple tool_use blocks do not cause duplicate tool results.
- ProPlan does not enter Review until execution actually completes.
- Existing streaming tests still pass.

### H5 Architecture-Constrained Refactor

Prompt:

```text
Refactor ProPlan so phase decisions are pure router logic and UI side effects
are represented as explicit actions. Keep behavior identical, reduce borrow
scope complexity in `ui.rs`, and add tests for every emitted action.
```

Why it is hard:

- Requires separating state mutation from UI orchestration without changing
  user-visible behavior.
- There are several tempting over-engineered designs.
- Review quality matters: a cheap execution model may pass tests but leave the
  code harder to maintain.

Success criteria:

- Router returns explicit actions such as `RequestPlanApproval`,
  `QueueExecution`, `QueueReview`, and `QueueFix`.
- `ui.rs` only applies actions and updates display/session state.
- Tests cover Plan, Execute, Review approved, Review rejected, and Done.
- `cargo check -p deepseek-tui` and focused tests pass.

### H6 Real Issue Clone

Prompt:

```text
Pick one hard SWE-bench-style issue from a medium-sized Rust CLI/TUI project,
recreate the failing behavior as a local regression test in this repository,
then fix it. Keep the issue statement and acceptance criteria in a markdown
file before editing code.
```

Why it is hard:

- Forces issue decomposition before implementation.
- Tests whether Pro planning can translate an external bug report into a local
  reproducible failure.
- Measures research, test design, and implementation together.

Success criteria:

- A written issue spec exists before code edits.
- A regression test captures the bug.
- The implementation passes the new test and relevant existing tests.
- Review phase checks that the local test is not overfit.

## Hard Suite Expected Reading

These tasks should better expose ProPlan's value than the small suite:

- `flash-only` should be fastest when it succeeds, but more likely to drift,
  skip persistence/security invariants, or make superficial fixes.
- `pro-only` should have the highest single-run success but may spend the most
  expensive-model tokens during mechanical editing.
- `pro-plan` should win when Pro's plan prevents a bad implementation path and
  Pro review catches Flash's incomplete work before the final answer.

For fair comparison, count a run as failed if it passes tests by weakening the
test, bypassing the intended state machine, or changing unrelated behavior.
