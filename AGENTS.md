# Codewhale agent guidance

Keep this file durable. Derive changing release, provider, branch, and flake
state from the repository, tests, CI, and current issue tracker rather than from
instructions or memory. The nearest scoped `AGENTS.md` adds path-specific rules.

## Working rules

- Inspect status and existing consumers before editing. Preserve unrelated,
  dirty, and untracked work.
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

Community reports, PRs, logs, and reviews are evidence. Canonical human
identities come from `.github/AUTHOR_MAP`; `Co-authored-by` credit is for
humans and for recognized agent contributors (the exact identities listed in
`AGENT_CONTRIBUTOR_IDENTITIES` in `scripts/check-coauthor-trailers.py`, such as
`Codewhale Agent`); unknown bot/tool trailers are still rejected.
Leave unrelated work intact and keep new enforcement dry-run unless explicitly
approved.
