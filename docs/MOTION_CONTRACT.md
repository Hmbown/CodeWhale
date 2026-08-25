# Motion contract

Central motion policy for the underwater TUI lives in
`crates/tui/src/tui/motion/`.

## Modes

| Mode | Decorative ambient | Status spinner | Streaming |
|------|--------------------|----------------|-----------|
| `Full` | yes | animated braille | steady ~60 FPS display clock (16 ms); catch-up is STAGED, not live — see note below |
| `Reduced` | no | static calm glyph | **same** display clock — not a slow typewriter; no catch-up bursts |
| `Still` | no | static chevron | state-change redraws; stream still coalesces on the display clock |

Provider SSE deltas are **input**, never animation timing.
`StreamDisplayClock` (`tui/streaming`) coalesces them; `FrameRequester`
coalesces decorative frame wakes. The main `ui` poll loop remains the only
`terminal.draw` emitter — do not add a competing animation loop.

## Integration

- Derive `MotionPolicy::from_settings(low_motion, fancy_animations, force_reduced)`.
- Spinners: prefer `MotionPolicy::spinner_glyph` / `spinner_presentation`; the
  frame table stays in `tui/spinner.rs`.
- Streaming: `stream_display_clock.set_allow_catch_up(policy.allows_catch_up_bursts())`.
- Working/phase chrome above the composer (TUI-DOG-008) must stay truthful under
  Reduced/Still — calm redraws, not decorative spin.

## Activity-shaped motion

Live work selects one row from `ActivityMotion` (`tui/ambient_life.rs`).
Chrome, the ocean clock, the streaming caret, and the completion cameo all
read that row.

Waiting-on-you pulses the diamond ink. Reduced/Still freeze decorative pulses.
Thinking turns no longer part the fish school; tool, computer-use, and
sub-agent work dart. `/loop` watchers keep a slow sonar pulse.

## One-shot phase transitions

- A successful turn records the first history index owned by that turn. Tool
  and agent receipts keep their final geometry and ordering while a bounded
  70 ms stagger briefly dims then settles each row. Reduced/Still skip the
  treatment and show the final receipts immediately.
- Ombre depth takes the typed `ShellPhase` as an input. Working leans subtly
  deeper, verification leans toward the live surface ink, and waiting,
  approval, and failure return the exact static base ramp.
- When an empty-water shell enters Working, fish follow one deterministic
  800 ms flee-and-return arc keyed to `turn_started_at`. It never loops;
  waiting, approval, stopped/error, and reduced-motion states remain still.
- These treatments never add/remove transcript rows, change hitboxes, or use
  provider delta timing as an animation clock.

## Honesty note: catch-up is staged, not wired

`note_delta_with_backlog` and the catch-up thresholds exist and are tested,
but every production drain site currently calls `note_delta` (queued = 1), so
Full-motion catch-up never actually fires and Full/Reduced stream at the same
steady clock. Do not describe catch-up as live behavior until the real queue
depth/oldest-age metrics are fed in at the `ui.rs` drain sites
(TUI-DOG-017 follow-up).

A second, unrelated "adaptive chunking" policy (`streaming/chunking.rs`, plus a
`LineBuffer` newline gate) was deleted in v0.9.4: it could only ever decide
"drain everything available", and both `LineBuffer` constructors bypassed the
gate. A commit beat now unconditionally flushes everything received since the
previous beat. Newline-boundary safety for partial code fences is owned by the
incremental markdown parser (`ParseState::commit_complete_lines`), which is
where it is actually in force.
