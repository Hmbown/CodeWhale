# Accessibility

Codewhale runs in a terminal, so the platform's own accessibility
stack (screen readers, magnifiers, terminal-level themes) does most
of the work. The TUI provides a small set of toggles that reduce
visual motion and density for screen-reader and low-motion users.

## Quick reference

| Toggle | Default | Effect |
| --- | --- | --- |
| `NO_ANIMATIONS=1` env var | unset | At startup, forces `low_motion = true` and `fancy_animations = false`. Overrides whatever's saved in `settings.toml`. |
| `CODEWHALE_ASCII_SAFE=1` env var | unset | Replaces decorative Unicode and box-drawing marks with narrow ASCII at the terminal backend. Labels, focus, state, and controls remain available. |
| `low_motion` setting | `false` | Freezes decorative and state animation without changing model text delivery. The footer water strip is controlled separately by `fancy_animations`. |
| `fancy_animations` setting | `true` | Enables expressive live-state chrome. Set to `false` to keep live-turn chrome still. The empty-state whale's 640 ms launch surface is decorative and stays still when this is off or `low_motion` is on. |
| `ocean_treatment` setting | `ombre` | Chooses the background appearance: `ombre` paints the state-reactive water column; `flat` uses the plain theme surface. Both keep the same state marks and idle ambient life; appearance is independent of motion settings. |
| `status_indicator` setting | `cw` | Static typographic header mark. Set to `dots` for the legacy animation, or `off` to hide it; `whale` is retired and normalizes to `cw`. |
| `calm_mode` setting | `true` | Collapses tool-output details by default and trims status messages. Useful for screen readers that announce every redraw. |
| `show_thinking` setting | `true` | Set to `false` to hide model `reasoning_content` blocks from the TUI presentation. Canonical session/replay receipts remain unchanged. |
| `thinking_default_expanded` setting | `false` | Set to `true` to expand visible thinking blocks initially. Space still collapses or expands the selected block. |
| `show_tool_details` setting | `false` | Set to `true` to expand tool calls inline; details remain available on demand either way. |
| `inline_diffs` setting | `full` | Use `summary` or `off` to reduce inline File-change density. Exact applied evidence remains available with Alt/Option+V in every mode. |

## Color contrast guarantees

The palette enforces WCAG contrast floors in two places, and this is what
the code actually guarantees — no more:

* **At draw time**, every text cell is lifted to a 4.5:1 contrast ratio
  against the surface it will actually render on (`enforce_cell_contrast` in
  the terminal backend). Frame chrome (borders, block glyphs) is not clamped,
  and community presets that own a full custom palette (Catppuccin, Tokyo
  Night, Dracula, Gruvbox, Claude, Matrix, Solarized Light, Terminal) are
  exempt from this draw-time pass because their authors tuned those pairs.
* **Per theme**, an audit (`theme_contrast_violations`) holds every
  selectable preset to the same floors: body, soft, and muted text at 4.5:1
  on every primary surface (including selection and error surfaces); hint and
  dim text at 3:1; status, warning, success, and info roles at 3:1 because
  they are redundant — every status also carries a glyph and a word label,
  so color is never the only channel. Diff foreground/background pairs are
  held to 3:1.
* The **Terminal** (transparent) theme is exempt by design: it paints
  `Color::Reset` surfaces and ANSI accents so the host terminal's own scheme
  shows through. Those colors are terminal-owned and cannot be measured, so
  the audit skips them rather than claiming a pass
  (`theme_uses_terminal_owned_surfaces` makes the exemption explicit).
* The **Grayscale** theme's "Color-minimal high contrast" tagline is
  enforced: its body text hierarchy clears 4.5:1 on every surface.
* The ASCII tier (`CODEWHALE_ASCII_SAFE=1`) keeps labels, focus, and state
  available without decorative glyphs, so the non-color redundancy above
  survives in the plainest rendering mode.

## Standard env-var surface

Set these in your shell profile so they apply to every session:

```bash
# Force low-motion + no fancy animations.
export NO_ANIMATIONS=1

# Force the terminal-safe ASCII rendering tier.
export CODEWHALE_ASCII_SAFE=1

# Optional: respect the wider terminal-color convention.
export NO_COLOR=1            # honored by the underlying ratatui backend
```

`NO_ANIMATIONS` accepts any of `1`, `true`, `yes`, or `on`
(case-insensitive). Any other value (including `0`, `false`, empty,
or unset) leaves your saved settings alone.

The override is applied once at startup. Changing the env var
mid-session has no effect — settings are only re-read on the next
launch.

## Configuring via `/config`

The same toggles are reachable from the command palette:

* `/config low_motion on --save`
* `/config fancy_animations off --save`
* `/config calm_mode on --save`
* `/config status_indicator off --save`

Settings written this way persist to `~/.codewhale/settings.toml` on new
installs, with legacy `~/.deepseek/settings.toml` and platform config-dir
settings kept as compatibility fallbacks.
The `NO_ANIMATIONS` env var still wins at startup if it's set, so
unsetting the env var is the way to honor your saved choice.

Tilix and Terminator sessions automatically start in low-motion mode because
those VTE-based terminals have reported visible redraw flicker during active
turns. You can still override the saved settings after launch if your terminal
version renders cleanly.

## Notes for screen-reader users

* `low_motion` slows the idle redraw loop to ~120ms per frame and freezes state
  markers without synthesizing or throttling model text. Combined with
  `calm_mode`, the redraw rate stays low enough that VoiceOver /
  Orca announcements track linearly with model output instead of
  re-reading the whole screen on each tick.
* The transcript is pure text — no images or canvas rendering — so
  any terminal that integrates with the platform's accessibility
  service (e.g. macOS Terminal.app, iTerm2, Ghostty, Windows
  Terminal) will pass the rendered content straight through.
* If you find a UI surface that still produces motion when
  `low_motion = true`, please file an issue against
  [`PRIOR: Screen-reader / accessibility flag`](https://github.com/Hmbown/CodeWhale/issues/450)
  with a screenshot or terminal recording.

## Related issues / history

* [#450](https://github.com/Hmbown/CodeWhale/issues/450) —
  documenting the existing flag, adding the `NO_ANIMATIONS`
  startup overlay, and writing this page.
* [#449](https://github.com/Hmbown/CodeWhale/issues/449) —
  footer statusline now uses the active theme's contrast pair
  instead of a bespoke palette.
