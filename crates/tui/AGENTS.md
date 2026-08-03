# crates/tui — agent guidance

Scope: the TUI, the runtime engine embedded in it, and everything a user sees.
Read the repo-root `AGENTS.md` first. Current flakes and known debt are in
`docs/ops/CURRENT.md`, not here.

## The shell grammar (do not regress it)

The default shell is the underwater system (`src/tui/underwater.rs`, `ocean.rs`,
`widgets/`, `views/`). Its contract:

- **One owner per fact.** Route/mode/permission/context live in the header;
  Tasks/To-do in the top strip; receipts and the single live row in the
  transcript; phase/cost/detail keys in the footer. Never restate a fact in a
  second place.
- **One live row.** Settled receipts are still; only the active row and the
  footer phase mark move. Decorative motion exists only in empty idle water and
  stops the instant the user types or anything needs attention.
- **Phase is typed.** `ShellPhase::from_app` derives idle/typing/working/
  waiting/approval/done/failed from real app state. Never invent state in a
  renderer; never compare English strings to detect state — use the enums.
- **Treatment is typed.** `OceanTreatment` (ombre/flat/classic) parses once from
  settings. Every treatment keeps ambient life; appearance and motion
  (`low_motion`, `fancy_animations`) are independent axes.
- **Footer notices go through the toast system** (`push_status_toast` /
  `active_status_toast`), never the legacy `status_message` sink: toasts carry
  level + TTL, errors hold sticky, acknowledgements expire.
- **Compact tiers shed chrome, not content.** At small sizes a room drops
  titles/captions/spacers before the object the user opened it to manipulate,
  and bodies budget from the footer's *wrapped* height (`wrapped_footer_lines` /
  `action_footer_lines`).
- **Rows are objects.** Anything selectable has a hitbox recorded at render
  time, keyboard + mouse parity, and visible focus. Destructive controls arm
  before they fire.

## Localization

Every user-visible string goes through `tr(locale, MessageId::…)` — no hardcoded
English in render paths. Glyphs (`▸ · ▾ ─`), key names (`Enter`, `Alt+?`), and
commands (`/fleet setup`) are composed in code, never embedded in translations.
Adding a string is a four-part change: see `locales/AGENTS.md`.

## Verification

```sh
cargo test -p codewhale-tui --bins --locked            # full unit suite
cargo test -p codewhale-tui --test qa_pty --locked     # PTY snapshots
cargo test -p codewhale-tui --test release_runtime_qa --locked
cargo test -p codewhale-tui --test terminal_matrix_qa --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Run clippy with `--all-targets`: `--bin` alone skips test targets and lets lints
reach CI.

Real-terminal QA gotchas, learned the hard way:

- The local tmux **server** may carry `NO_COLOR=1` and `TERM=dumb` from old VHS
  runs — launch panes with `env -u NO_COLOR` or all color QA silently lies. tmux
  also force-enables the low-motion overlay; prove full motion with
  `TMUX`/`TMUX_PANE` removed.
- Scripted PTY input: one Enter on the slash menu both accepts the highlighted
  match and runs it. A scripted second Enter lands *inside* whatever modal just
  opened. Send one key, wait, capture.
- Judge motion from repeated captures diffed over time, never single
  screenshots. Layout gates: 40x12, 60x16, 80x24, 100x32, 140x40.
- `CODEWHALE_TUI_DEBUG=1` writes per-frame diff sizes to
  `~/.codewhale/logs/tui-render.log`. Streaming should be tens of cells per
  frame; a multi-thousand-cell frame is only acceptable on a genuine layout
  transition.
