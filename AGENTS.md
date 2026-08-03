# Repository Agent Guidance

Durable rules only. Perishable lane state — branch, milestone, known flakes,
closed investigations — lives in `docs/ops/CURRENT.md`; read it, don't trust
memory of it.

## Intent is the artifact

Writing the code again is cheaper than recovering the code we wrote. Act on
that.

- **Rewriting any part of this project is always in scope**, up to the whole
  thing. Nothing is load-bearing by virtue of existing. Argue a rewrite on
  merit, not sunk cost.
- **Use git; do not be governed by it.** A branch 600 commits behind is a note
  describing something we once wanted, not a debt. Conflict count is a signal to
  rewrite, not a task list.
- **A stranded lane becomes an issue, not a merge.** State the intent, the
  behavior wanted, and evidence worth keeping; reference the dead branch for
  provenance; abandon the branch; rebuild from current `main`.
- **Verify before you rebuild.** Grep for the symbols and behavior — not the
  commit — to check whether `main` already does it. Re-landing landed work is
  the failure mode this ethos creates, and it is the one you own.

Limits: `main` stays protected and releases reproducible (never rewrite
published history, retag a shipped release, or force-push a shared ref);
contributor credit carries onto the rewrite; the do-not-delete guardrail below
still binds; and don't rewrite to avoid understanding.

Longer form: `docs/AGENT_ETHOS.md`.

## Build and test

Always before pushing: `cargo fmt`, then targeted tests for the area.

```sh
cargo test -p codewhale-config
cargo test -p codewhale-protocol
cargo test --workspace                                    # full gate
cargo build --release -p codewhale-cli -p codewhale-tui   # release build
```

Crate-specific commands live in that crate's `AGENTS.md`. Environment quirks
(Cursor Cloud, keyless providers, dispatcher siblings) live in
`docs/ENVIRONMENTS.md`.

Default branch is `main`. Committing directly to `main` is fine for release-lane
work — one reviewable concern per commit, with a real body. A fresh `codex/...`
branch or worktree is still right for an isolated or risky change.

Commit as **WIP** unless you actually verified the behavior — built the binary,
ran the test, reproduced the fix. "Fixed" without evidence is worse than an
honest WIP.

## Do-not-delete guardrail

These are actively imported and have been repeatedly misflagged as dead code;
deleting them broke the build. Verify consumers with `rg` before believing any
dead-code audit:

`tui/src/memory.rs`, `tui/src/context_budget.rs`, `tui/src/model_registry.rs`,
`tui/src/prompt_zones.rs`, `tui/src/tools/remember.rs`, and the entire
`config/src/route/` directory.

## Surfaces that exist today

Build only on these — removed machinery stays gone. The model-facing sub-agent
surface is **`agent` only**: the `agent_open`/`agent_eval`/`agent_close`/
`delegate_to_agent` variants, capacity/coherence/runtime-tag systems, lifecycle
tools, and runtime prompt/tag injection were all removed. The constitution
(`BASE_PROMPT` in `tui/src/prompts/text.rs`) is the sole base prompt.
Configurable sub-agent depth stays; add a new limit only when clearly needed,
and explain why.

## Stewardship

CodeWhale started as a DeepSeek-only harness; it is now about building the best
possible coding harness with an open-source community. Keep CodeWhale branding
and every model/provider first-class — none privileged.

- Community PRs, issues, repros, logs, and reviews are maintainer evidence, not
  queue noise. Review from code, tests, linked issues, comments, and checks.
- **Credit is CI-enforced.** `Co-authored-by` trailers are for human
  contributors only — `scripts/check-coauthor-trailers.py` rejects bot/tool ones
  (Claude, codex, cursor, `noreply@anthropic.com`). Use canonical identities
  from `.github/AUTHOR_MAP`; note agent assistance in a plain commit body.
- Keep gates warm and dry-run unless Hunter explicitly approves enforcement.
- Leave unrelated edits by other people or agents intact.

Full ethos: `docs/AGENT_ETHOS.md`. Issue triage standard:
`docs/AGENT_READY_ISSUES.md`. Release queue and harvest procedure:
`docs/RELEASE_QUEUE.md`.
