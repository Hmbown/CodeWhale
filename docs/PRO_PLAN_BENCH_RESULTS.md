# ProPlan Benchmark Results

Date: 2026-05-19

This benchmark compares three execution styles on small Rust repair tasks:

- `pro-only`: one automated `exec` turn with `deepseek-v4-pro`
- `flash-only`: one automated `exec` turn with `deepseek-v4-flash`
- `pro-plan-sim`: Pro planning turn, Flash execution turn, Pro review turn

The real interactive TUI ProPlan flow was verified separately. This benchmark uses `exec` to make token accounting repeatable. The ProPlan path simulates the same model split, because the TUI confirmation and approval flow is intentionally interactive.

## Tasks

| Task | Shape | Success check |
| --- | --- | --- |
| T1 mode alias | Single-file bugfix | `cargo test` passes |
| T2 phase router | Multi-file routing fix | `cargo test` passes |
| T3 setting parser | Parser edge cases | `cargo test` passes |

## First Run

| Task | pro-only | flash-only | pro-plan-sim |
| --- | ---: | ---: | ---: |
| T1 input/output | 159,444 / 1,056 | 136,458 / 1,066 | 213,296 / 2,118 |
| T1 success | pass | pass | pass |
| T2 input/output | 21,934 / 149 | 137,310 / 951 | 211,256 / 2,286 |
| T2 success | fail | pass | pass |
| T3 input/output | 166,232 / 2,220 | 139,390 / 1,427 | 201,794 / 3,467 |
| T3 success | pass | pass | pass |

The T2 `pro-only` first run failed before editing files. A retry with the same prompt and model completed successfully at 183,575 input tokens and 1,082 output tokens.

## Aggregate

| Variant | Success | Input tokens | Output tokens |
| --- | ---: | ---: | ---: |
| pro-only first run | 2/3 | 347,610 | 3,425 |
| pro-only with T2 retry | 3/3 | 509,251 | 4,358 |
| flash-only | 3/3 | 413,158 | 3,444 |
| pro-plan-sim | 3/3 | 626,346 | 7,871 |

## ProPlan Split

| Task | Plan, Pro | Execute, Flash | Review, Pro |
| --- | ---: | ---: | ---: |
| T1 input/output | 13,642 / 151 | 166,271 / 1,235 | 33,383 / 732 |
| T2 input/output | 57,420 / 873 | 120,667 / 766 | 33,169 / 647 |
| T3 input/output | 42,279 / 1,777 | 124,722 / 739 | 34,793 / 951 |

Across these three small tasks, ProPlan used more total tokens because it deliberately spends extra turns on planning and review. It did reduce the amount of Pro-model input compared with all-Pro execution: 214,686 Pro input tokens for ProPlan planning/review versus 509,251 Pro input tokens for the all-Pro retry-adjusted baseline. The tradeoff is that it added 411,660 Flash input tokens for execution.

## Takeaway

For tiny repairs, ProPlan is not token-cheaper in total. The overhead of plan plus review is larger than the savings from moving execution to Flash.

The mode becomes more plausible for larger tasks where the expensive model prevents wrong implementation direction and reviews a broader Flash-generated diff. The next benchmark should use larger, multi-step tasks with ambiguous architecture choices, because that is where Opus Plan-style routing is meant to pay for itself.

## Hard H3 Run

Task: permission matrix audit and hardening for Agent, Plan, Yolo, and ProPlan.

Prompt summary:

```text
Audit and harden the permission behavior for Agent, Plan, Yolo, and ProPlan.
Create focused tests proving that ProPlan Plan and Review phases cannot write
files or run write-class tools, while Execute follows Agent-style approval.
Fix any bugs you find without broad refactors.
```

Run directory:

```text
/private/tmp/deepseek-hard-h3-bench
```

| Variant | Checks | Input tokens | Output tokens | Files changed | Quality read |
| --- | ---: | ---: | ---: | ---: | --- |
| pro-only | pass | 8,132,723 | 24,151 | 4 | Most comprehensive, but very expensive and broader than necessary |
| flash-only | pass | 2,899,037 | 15,628 | 2 | Too shallow: tests phase mapping, not the actual permission matrix |
| pro-plan-sim | pass | 5,151,314 | 27,127 | 4 | Best tradeoff: engine-level tests plus smaller diff than pro-only |

ProPlan phase split:

| Phase | Model | Input tokens | Output tokens |
| --- | --- | ---: | ---: |
| Plan | deepseek-v4-pro | 1,240,285 | 11,653 |
| Execute | deepseek-v4-flash | 3,216,531 | 12,977 |
| Review | deepseek-v4-pro | 694,498 | 2,497 |

All three variants passed `cargo check -p deepseek-tui` and
`cargo test -p deepseek-tui pro_plan`. The checks alone overstate the Flash
result, because the Flash diff only added a helper mapping from ProPlan phase to
AppMode and tests for that mapping. It did not prove that write-class tools are
absent from the engine registry in Plan/Review, nor that Execute gets the
Agent-style tool policy.

The ProPlan diff added engine-level tests for Plan/Review read-only behavior and
Execute write-tool availability, plus a defense-in-depth fallback that treats raw
`AppMode::ProPlan` as read-only if any future path bypasses phase resolution.
That is closer to the requested security invariant while using about 37% fewer
input tokens than pro-only.
