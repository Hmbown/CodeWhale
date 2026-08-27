# Motion ethos handoff — v0.9.12

Branch: `grok/v0912-omarchy-ethos-motion-20260827`
Base: `origin/main` (`a96ea6cb09cc464ea2e88f251c538c239d1fe9ad`)
Lane: isolated TUI motion polish. Does **not** duplicate PR #5643.

## What 5643 already does

PR #5643 (`codex/v0912-tui-polish-20260827`) converts `ocean_started_at` to
`Option<Instant>` and starts that clock when the empty ocean is shown, plus
MCP-login copy and composer vocabulary. This lane does **not** fork that
patch. `ocean_started_at` stays an `Instant` process-start clock.

## What this lane does

Omarchy (DHH / Hyprland) timing is infused, not copied. Codewhale keeps Blue
Stage water, the whale, gold current, and ombre depth. No workspace-slide
theater, no bouncing windows, no `rounding=0`.

### Welcome vs launch (the reported animation regression)

On `origin/main` the idle shine used process-start time, so the first visible
empty-ocean frame could already be parked in the 4 s caustic off-screen
window. Launch is full-canvas and returns before `ChatWidget`, so the whale
never paints there.

This lane:

- Adds `App::welcome_visible_since: Option<Instant>` — a dedicated
  occlusion-aware welcome clock, distinct from 5643's `ocean_started_at`
  Option change.
- Starts that clock only when decorative motion is on, the mark can draw,
  and launch/onboarding are **not** covering the ocean.
- Authored 640 ms whale surface + Codewhale letter-write land with
  **ease-out-quint from ~87% presence**, never from a vanishing point.
  Shine is one pass on that same clock, so it starts when the whale is
  actually on screen.
- Reduced / Still / `NO_ANIMATIONS` skip the clock and show the settled
  mark immediately.

### Ethos helpers (`crates/tui/src/tui/motion/ethos.rs`)

| Constant / curve | Value | Use |
| --- | --- | --- |
| `ease_out_quint` | `1-(1-t)^5` | arrivals, whale surface, wordmark |
| `ease_out_exit` | quadratic | exits, closer to linear |
| `FADE_MS` | 160 | almost-linear fade |
| `SURFACE_POP_MS` | 180 | picker/menu snappy settle |
| `SURFACE_EXIT_MS` | 120 | faster than entry |
| `SURFACE_POP_FROM` | 0.87 | never grow from 0 |
| `WELCOME_SURFACE_MS` | 640 | keep this duration |
| `RECEIPT_STAGGER_MS` | 70 | do not lengthen |
| `FISH_FLEE_MS` | 800 | do not lengthen |

`SettingsPickerController` records `opened_at` and exposes `settle_pop` /
`is_settling`. Hosts that already redraw can DIM until the pop lands.
Reduced/Still return 1.0 immediately. This lane does **not** add a
`ViewAction::Redraw` driver (event-loop change; follow-up).

## Tests

- `motion::ethos::tests::*`
- `underwater::startup_surface_tests::*` (land-from-87%, wordmark write,
  one-pass shine, launch/onboarding occlusion, Reduced/Still skip)
- `widgets::tests::idle_welcome_waits_behind_launch_then_starts_on_the_empty_ocean`
- `widgets::tests::idle_welcome_waits_behind_onboarding`
- `settings_picker::tests::picker_settle_pops_from_nearly_full_and_skips_when_still`
- existing 70 ms receipt + 800 ms fish-flee tests still pin those one-shots

## Remaining

- Live picker DIM needs a one-shot redraw while `is_settling` (no
  `ViewAction::Redraw` yet). Spatial memory is already correct: pickers do
  not slide.
- If 5643 merges first, keep `welcome_visible_since` for the 640 ms surface
  and let `ocean_started_at` stay whatever 5643 made it. Do not collapse
  the two clocks; occlusion and ambient start are different facts.
- Do not add workspace-slide, bounce, or Hyprland rounding. Do not lengthen
  receipt stagger or fish flee toward cinematic.
- `crates/tui/src/tui/motion/ethos.rs` is the chef's-choice default. New
  decorative timing should ask it rather than inventing another curve.
