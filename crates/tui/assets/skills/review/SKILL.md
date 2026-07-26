---
name: review
description: Read-only correctness review with actionable findings first, tight file/line evidence, severity, and a concise residual-risk summary.
invocation: model+user
aliases-for: code-review
---

# Review

## When to use
Use for correctness review of a diff, PR, or named change set.

## Non-goals
- Read-only by default. Do not edit unless asked.
- Do not bury findings under style nits.

## Workflow
1. Establish the change scope.
2. List findings by severity with file/line evidence.
3. Call out residual risk and missing tests.
4. End with a short merge/risk summary.
