# Codewhale agent guidance

Keep this file durable. Derive changing release, provider, branch, and flake
state from the repository, tests, CI, and current issue tracker rather than from
instructions or memory. The nearest scoped `AGENTS.md` adds path-specific rules.

## Working rules

- Inspect status and existing consumers before editing. Preserve unrelated,
  dirty, and untracked work.
- Prefer the simplest implementation that preserves observable contracts. A
  rewrite is acceptable when justified by behavior and tests, not as a shortcut
  around understanding existing code.
- Search for behavior and symbols before reviving work from an old branch. If a
  lane is obsolete, preserve its intent and evidence rather than merging stale
  code mechanically.
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

## Verification

Run formatting and focused tests for every change. Before a push, run the
relevant repository gate; release work requires the complete sequence:

```sh
cargo fmt --all -- --check
cargo test -p codewhale-config -p codewhale-protocol
cargo test --workspace
cargo build --release -p codewhale-cli -p codewhale-tui
```

`cargo nextest run` (config in `.config/nextest.toml`) is the fast way to
*run* those suites locally and in CI's Test job; `cargo test --no-run` and
`cargo test -p codewhale-tui --lib` remain the compile-time measurement and
the authoritative gate, and `cargo test --doc` covers what nextest skips.
`scripts/dev-test.sh <area>` maps a code area to its fastest `-p` invocation
and applies the portable isolated build-dir topology for new worktrees
(`scripts/dev-cache.sh`, `scripts/dev-cargo.sh`). See
`docs/BUILD_PERFORMANCE.md`.

Report commands actually run and distinguish source, local tests, packaged
artifacts, CI, and public release state. A commit is WIP until its claimed
behavior has direct evidence.

Community reports, PRs, logs, and reviews are evidence. Canonical human
identities come from `.github/AUTHOR_MAP`; `Co-authored-by` is for humans only.
Leave unrelated work intact and keep new enforcement dry-run unless explicitly
approved.
