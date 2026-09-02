# TUI agent guidance

Scope: the terminal UI, its embedded runtime engine, and user-visible behavior.
Read the repository guidance first.

## UI contracts

- One owner per fact: mode, permission and live counts in the posture bar
  (`phase_strip.rs`, row 1 under the composer); model, context and the
  session metrics — cost, ttft, tok/s, output tokens — in the metrics line
  (`infoline.rs`, row 2); the roster and to-do in the work surface; receipts,
  the active row and the phase in the transcript. Key hints come from the
  `shell_key_routing` binding table, never from a string literal.
- Status-bar ink goes through `palette::grammar` (`docs/design/STATUS_BAR_COLOR_GRAMMAR.md`).
  Do not invent an eighth semantic or spend Failure red on non-failure chrome.
- Derive state from typed enums such as `ShellPhase` and `OceanTreatment`.
  Renderers must not infer state from English strings or invent lifecycle state.
- Keep settled output still. Motion is semantic, bounded, and fully disabled by
  reduced-motion settings.
- Route notices through the toast system, with typed level and lifetime; do not
  add new writes to the legacy `status_message` sink.
- Compact layouts remove chrome before content. Selectable rows need recorded
  hitboxes, visible focus, keyboard/mouse parity, and confirmation for
  destructive actions.
- User-visible prose uses `tr(locale, MessageId::...)`. Commands, key names, and
  glyphs are composed in code. Follow `locales/AGENTS.md` for string changes.

## Verification

Select the smallest evidence that answers the actual risk. Direct PTY or
terminal behavior is stronger evidence for visible UX than an assertion over
render internals. Use a focused existing test for safety, data integrity,
protocol, or a reproduced regression when useful. Do not add tests by default,
and do not require both full suites or workspace Clippy for an unrelated leaf
change. These are available release/cross-cutting gates, not per-edit ritual:

```sh
cargo test -p codewhale-tui --lib --locked
cargo test -p codewhale-tui --tests --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

`--lib` and `--tests` are disjoint; choose the target that owns the behavior.
Use both or the workspace gate only when the risk genuinely spans both.
`scripts/dev-test.sh <area|path> [filter]` prints and runs the fastest
targeted invocation for a source path (for example
`scripts/dev-test.sh crates/tui/src/elapsed.rs`). It uses `cargo nextest
run` when nextest is installed (`CODEWHALE_DEV_NEXTEST=0` forces libtest)
and applies `scripts/dev-cache.sh` so a new worktree gets an isolated
Cargo build-dir. For PTY
failures, reproduce the behavior directly before changing it. Script one input
at a time and capture after the UI settles. Choose the terminal sizes relevant
to the change from 40x12, 60x16, 80x24, 100x32, and 140x40; judge motion from
repeated frames, not a single screenshot. Remove inherited `NO_COLOR`,
`TERM=dumb`, and tmux motion overrides when they would invalidate the
observation.
