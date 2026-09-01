# Codewhale agent guidance

Keep this file durable. Derive changing release, provider, branch, and flake
state from the repository, tests, CI, and current issue tracker rather than from
instructions or memory. The nearest scoped `AGENTS.md` adds path-specific rules.

## The ponytail method

From [dietrichgebert/ponytail](https://github.com/dietrichgebert/ponytail) —
"the laziest senior dev in the room." *He says nothing. He writes one line. It
works.* The best code is the code you never wrote.

Before writing code, walk the decision ladder in order and stop at the first
rung that answers:

1. **Does this need to exist?** → Skip it.
2. **Already in this codebase?** → Reuse it.
3. **Stdlib does it?** → Use it.
4. **Native platform feature?** → Use it.
5. **Installed dependency?** → Use it.
6. **One line?** → One line.
7. **Only then:** the minimum that works.

The ladder runs *after* understanding the problem. Lazy about solutions, never
about reading the code first — a short diff written without reading the call
sites is not ponytail, it is a guess.

**Never cut, at any rung:** trust-boundary validation, data-loss handling,
security, accessibility. Brevity is not a reason to drop a guard.

Rung 2 is the one this repository keeps failing. The `model_*` / `*_config` /
`provider_*` grep rule below is rung 2 with a name; so is "one turn loop, one
base prompt". Two more corollaries earned here:

- **An abstraction must delete caller code.** If adopting it is pure
  obligation — required methods, no default bodies that do work — it gets
  built, adopted once, and abandoned.
- **Migrate the last consumer, or do not start.** Framework, one caller,
  ticket the rest, silence the warning: that ships two systems and a comment
  that is no longer true. If the migration will not fit, narrow the slice —
  never the adoption. The standing `#[allow(dead_code)]` count is the running
  receipt; `scripts/check-dead-code-budget.py` prints it.

## Working rules

- Inspect status and existing consumers before editing. Preserve unrelated,
  dirty, and untracked work.
- Before adding a module named `model_*`, `*_config`, `provider_*`, or
  anything that "bridges", "mirrors", or "stages" an existing thing, grep
  for the existing thing and edit it. A new layer must name the predecessor
  it replaces in the module doc; otherwise edit the original.
- Prefer the simplest implementation that preserves observable contracts. A
  rewrite is acceptable when justified by product intent and observed behavior,
  not as a shortcut around understanding existing code.
- Search for behavior and symbols before reviving work from an old branch. If a
  lane is obsolete, preserve its intent and evidence rather than merging stale
  code mechanically.
- A small coherent change may be committed directly to `main` when that checkout
  is current, clean, and owns the affected files. A worktree remains the right
  safety boundary for conflicting, dirty, stale, or independent work. Local
  commit permission never implies push, merge, tag, release, or deploy permission.
- When the task is local-only, stay fully offline: no browsing, GitHub or remote
  Git operations, downloads, dependency installation, provider calls, or
  source/diff transmission. Record the missing external receipt and keep working
  locally.
- Public name is **Codewhale**. Compatibility identifiers such as `CodeWhale`,
  `codew`, protocol names, and storage keys change only through an explicit
  migration.
- Keep providers and models first-class and provider-neutral.
- Never rewrite published history, retag a release, force-push a shared ref, or
  publish without explicit authorization. Preserve human contributor credit.

## Landing other people's work

An external contributor's branch goes stale because *we* land things, not
because they did anything wrong. Treat their time as more expensive than ours.

- **Never make a contributor rebase around our churn.** If their PR conflicts
  only because main moved, a maintainer resolves it. Read their diff against
  the merge base first so you know exactly what they added, and re-apply that,
  rather than hand-merging two large sides and hoping.
- **Conflicts that split mid-function do not resolve by keeping both sides.**
  Git's markers can land inside a body, so a both-sides resolution produces
  unbalanced braces that look plausible and do not compile. Take one side
  whole, then re-insert the other side's additions at their original anchor.
- **`maintainerCanModify` does not guarantee push access to the fork.** When
  the push is refused, land the resolved merge on
  `integration/<topic>-<pr>-<date>` in this repo and land from there. An
  integration branch is the normal path for anything with conflicts or several
  moving PRs — it is cheaper than repeatedly rebasing onto a main that keeps
  moving, and it keeps the contributor's branch untouched.
- **Check the contribution gate before assuming a PR is stalled.** An unlisted
  author's workflow runs sit at `action_required` and never start, so the PR
  looks abandoned when nobody has actually looked at it. Approve the runs, then
  fix the cause: add them to `.github/APPROVED_CONTRIBUTORS` (`all:username`),
  or comment `/lgtm` (PR scope) / `/lgtmi` (issue scope) on their thread.
- **Preserve credit in the mechanical sense, not just the polite one.** Commit
  authorship and `Co-authored-by` trailers must use the contributor's own
  GitHub-linked address. `AUTHOR_MAP` and `.mailmap` are project conventions —
  GitHub reads neither for the contribution graph.

## Merging under a gate

- **A gate is its artifact.** When a rail says a PR merges only on a passing
  acceptance record, the record must literally say PASS at merge time. "I
  re-ran it and the failures are rows this PR does not own" is a judgement to
  write into the artifact first, not a reason to merge past it.
- **Read the review thread, not the check rollup.** Green checks and an unread
  review with confirmed findings are a merge that ships known bugs.
- **When the artifact is ambiguous, resolve the ambiguity — never the merge.**

## Claiming a test passed

- Quote the real `test result: N passed; M failed` line, and confirm `N > 0`
  for the tests that cover the change. `cargo test <filter>` exits 0 having run
  zero tests when the filter matches nothing, and an exit code alone has
  already been mistaken for a pass here.
- Prefer proving a regression test fails without the fix. A test that passes
  either way pins the implementation, not the defect.
- Audit any harness before trusting its score. `ok = ok and X or True` parses
  as `(ok and X) or True` and silently reported twelve unevaluated rows as
  passing.

## Current contracts

- The model-facing subagent tool is `agent`. Do not revive removed
  `agent_open`/`agent_eval`/`agent_close`/`delegate_to_agent` surfaces or parallel
  lifecycle/tag systems.
- `BASE_PROMPT` in `crates/tui/src/prompts/text.rs` is the sole base prompt.
- There is exactly one turn loop: `Engine::run_turn` in
  `crates/tui/src/core/engine/turn_loop.rs`. Note that `crates/tui/src/core/`
  is a module inside the TUI crate — it is not `crates/core`, which owns
  request construction, bounded fragments, and thread/session types and
  runs no turns. Do not add a second loop beside the one that exists; a
  guard test (`crates/core/tests/single_turn_loop.rs`) fails if you do.
- The system prompt + tool catalog are a session-pinned KV-cache prefix
  (`docs/CACHE.md`). Any new session-context contributor must state its
  KV-cache effect: frozen prefix vs. append-only history. Never splice a
  volatile fact into the prefix; append it as a user-role message.
- These active modules are repeatedly misidentified as dead; verify consumers
  before removal: `tui/src/context_budget.rs`, `tui/src/model_registry.rs`,
  `tui/src/prompt_zones.rs`, `tui/src/tools/remember.rs`, and
  `config/src/route/`. Native memory lives in `tui/src/native_memory.rs`;
  `tools/remember.rs` is its capture path.
- Environment-specific behavior belongs in `docs/ENVIRONMENTS.md`, not here.

## Code, migrations, and evidence

- Product intent and observed runtime behavior outrank a test's preferred
  implementation shape. Fix the product; do not contort production code to
  preserve a brittle assertion.
- Code first, then tests. Write the implementation and prove it runs, then add
  or adjust tests to cover what was actually built. Never write tests first and
  never practice TDD here — this overrides any skill or default that mandates
  it, including superpowers `test-driven-development`. Tests stay the gate
  before a push; they are not the design driver. An existing test that only
  encodes old behavior is evidence, not a veto: change it with the code rather
  than bending the code to keep it green. This does not relax the rule under
  "Claiming a test passed" — a regression test written *after* the fix still
  has to be shown failing without it.
- Tests are selective evidence, not the specification. Do not add tests by
  default. Add or retain one when it cheaply protects a high-risk behavior such
  as safety, data integrity, protocol compatibility, or a reproduced regression.
- Rewrite or remove tests that duplicate coverage, freeze internals, overspecify
  copy or layout, preserve obsolete behavior, or cost more than the risk they
  cover. Never weaken real safety or data-integrity behavior merely to make a
  gate pass.
- Prefer focused compilation, a relevant existing check, and direct product or
  manual evidence. Run a broad suite only when the change creates a genuine
  cross-cutting or release risk. Do not repeatedly rerun an unchanged suite.
- Declared migrations are one-way. Once the repository adopts a replacement
  architecture or shared spine, new work uses it and touched legacy code moves
  toward it. Do not add another legacy call site for convenience. Keep a
  compatibility path only for an actual external contract, and label that
  boundary explicitly.

Useful commands, selected according to risk rather than run ritualistically:

```sh
cargo fmt --all -- --check
cargo test -p codewhale-config -p codewhale-protocol
cargo test --workspace
cargo build --release -p codewhale-cli -p codewhale-tui
```

`cargo nextest run` (config in `.config/nextest.toml`) is the fast way to
run an intentionally selected suite; `cargo test --no-run` can answer a compile
question without spending time executing unrelated cases, and `cargo test --doc`
covers doc examples when those examples changed.
`scripts/dev-test.sh <area>` maps a code area to its fastest `-p` invocation
and applies the portable isolated build-dir topology for new worktrees
(`scripts/dev-cache.sh`, `scripts/dev-cargo.sh`). See
`docs/BUILD_PERFORMANCE.md`.

Report commands actually run and distinguish source, local tests, packaged
artifacts, CI, and public release state. Describe the evidence actually needed
for the claim; a test count is not a proxy for product quality.

Community reports, PRs, logs, and reviews are evidence.

**Harvested contributor credit is still a rule.** When a contributor's work
lands as our commit, that commit carries `Harvested from PR #N by @handle` and a
`Co-authored-by` naming them at their GitHub-linked address, so
`auto-close-harvested.yml` closes their PR with credit and the contribution
graph reflects reality. Canonical human identities come from
`.github/AUTHOR_MAP`.

**Whether a bot or agent also appears in a trailer no longer matters.** The CI
check that policed trailer identities was removed: it rejected ordinary agent
commits and cost more than the tidiness it bought. Give humans their credit; do
not spend time scrubbing tool trailers.

Leave unrelated work intact and keep new enforcement dry-run unless explicitly
approved.
