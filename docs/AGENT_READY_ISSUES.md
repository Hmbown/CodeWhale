# Agent-Ready Issues

CodeWhale's tracker is worked by humans and by autonomous agents. An issue is
**agent-ready** when a fresh agent — with a clone of `main`, shell/read/write
tools, and *no other context* — can execute it end-to-end and prove the result.
This document defines that standard for new issues, for triage reworks of
existing issues, and for maintainer replies on community threads.

The filing-time version of this standard is the
[Agent task issue form](../.github/ISSUE_TEMPLATE/agent-task.yml). This page
extends it to the rest of the tracker.

## Source of truth

- The **active milestone** decides what lane an issue is in. Version labels
  (`v0.9.2`, `v0.9.3`, …) are historical metadata; they never choose or change
  a milestone.
- The **issue body** is the executable spec. Refinements that arrive as
  comments get folded into the body during triage so the body never lies.
- Queue order within a lane (from `AGENTS.md`): release blockers, recently
  approved PRs, clean small PRs, blocked PRs with obvious fixes, safely
  harvestable dirty PRs, then larger architecture work.

## Required structure

```markdown
## Problem
2–6 sentences. What is wrong or missing, and why it matters now.

## Current evidence
Verified anchors and observed behavior, e.g.
`crates/tui/src/model_routing.rs::provider_router_candidates`.

## Scope
Numbered steps; one concrete action per step, file paths where known.

## Key files
One verified path per line. The executing agent reads these first.

## Acceptance criteria
Behavior-level `- [ ]` checkboxes. Every item must be testable.

## Verification
Exact commands, e.g.
`cargo test -p codewhale-tui --bin codewhale-tui --locked <filter>`.

## Out of scope
What this issue deliberately does not change.

## Related
Real issue/PR numbers only, each with one line on the boundary between them.
```

Epics additionally get a `## Phases` section: each phase sized as one
agent-executable slice with its own acceptance bullet. An epic without phases
is not agent-ready.

## Anchor discipline

- Every file path, symbol, config key, and command in an issue body must be
  verified against the current tree before it is written down. `rg`/`ls`
  first, then cite.
- If something cannot be located, write
  `(anchor not found — needs discovery)` rather than a guess. A wrong anchor
  costs an executing agent more than a missing one.
- Verification commands use real workspace package names
  (`codewhale-tui`, `codewhale-config`, `codewhale-protocol`, …) — confirm in
  the crate's `Cargo.toml`, not from memory.

## Reworking existing issues

- **Maintainer-authored issues**: restructure the body in place. Preserve
  every constraint and concrete fact from the original; fold in refinements
  from comments; end the body with a dated triage note, e.g.
  `_Triage note: body restructured for agent execution on YYYY-MM-DD; prior
  comment refinements folded in. Original wording preserved in edit history._`
- **Community-authored issues**: never rewrite the reporter's body. Post a
  maintainer comment carrying the same skeleton instead — status in the lane,
  what a fix looks like (anchored bullets + acceptance criteria), and the
  smallest set of asks that unblocks the issue.
- Apply the `agent-ready` label only when the body (or, for community issues,
  body + maintainer deconstruction comment) genuinely meets this standard.
  The label is a gate, not a wish.

## Community thread etiquette

- Open with specific thanks that references a real detail of the report —
  proof it was read. Vary phrasing across issues.
- State status honestly: milestone lane, what already shipped (cite the
  version, PR, or commit — only with evidence), and what is blocking.
- Never claim testing or reproduction that did not happen; never promise
  dates. "Queued in the v0.9.2 release lane" is the honest formulation.
- Reporters writing in Chinese (or another language) get the key points and
  asks translated at the end of the English reply.
- `needs-info` issues get exactly one crisp ask (typically
  `codewhale --version`, `codewhale doctor --json`, OS + terminal, minimal
  repro), plus a pointer that the stale policy in
  [ISSUE_TRIAGE.md](./ISSUE_TRIAGE.md) applies once a maintainer labels the
  issue `needs-info`.

## Why this exists

A deconstructed issue is cheap to execute and cheap to verify: the researcher
pays the discovery cost once, at triage time, instead of every executing agent
paying it again. When an issue is agent-ready, "pick up the next item in the
milestone" becomes a safe instruction for any contributor — human or agent.
