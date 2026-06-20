## Summary

Validate terminal-safe voice shortcut and STT helper setup for the voice input feature shipping in v0.8.45/v0.8.46.

## Completed

- **`voice_input.rs`**: Added `diagnose_voice_setup()` async function that checks:
  - Missing/invalid `voice_input_command` config
  - Inexecutable binary (spawns `--version` as probe)
  - Permission/spawn failures with clear error messages
- **`voice_input.rs`**: Added `terminal_detection_tests` module with:
  - `chord_likely_reaches_tui()` heuristic function documenting known-consumed chords
  - Tests for common chords (Ctrl-K consumed, Ctrl-L/Ctrl-C safe)
  - Candidate shortcut test matrix (F2, F3, Alt-Space, Ctrl-] etc.)
- **`docs/VOICE_INPUT_TERMINALS.md`**: Terminal compatibility matrix documenting Ctrl-K behavior across 13 terminal emulators + STT helper setup checklist + recommended safe chords
- **`docs/KEYBINDINGS.md`**: Added Voice input section documenting the Ctrl-K caveat with cross-reference to the new terminal matrix doc

## Not completed (needs manual verification)

- **Actual terminal testing** on macOS Terminal.app, iTerm2, Ghostty, Warp, Windows Terminal, Linux terminals
- **Final default chord selection** — the matrix lists candidates but the final binding needs human verification
- **Manual QA checklist** for terminals that consume modifier keys
- **Compilation verification** — changes made to a feature branch (work/v0.8.45-flash) without Rust CI available

## Files changed

```
crates/tui/src/tui/voice_input.rs  | 110 +++++++++++++++++
docs/KEYBINDINGS.md                |  22 ++++
docs/VOICE_INPUT_TERMINALS.md      |  87 +++++++++++++
3 files changed, 219 insertions(+)
```
