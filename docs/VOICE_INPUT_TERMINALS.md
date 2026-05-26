# Voice Input — Terminal Compatibility & Shortcut Matrix

This document tracks which key chords reach the CodeWhale TUI on each
terminal emulator. It accompanies issue #2116 and is a living reference
for the voice-input feature shipping in v0.8.45 / v0.8.46.

## Background

CodeWhale's voice input lives behind the command palette (`Ctrl+K`).
When the user selects **Voice input**, the TUI spawns a configured
external command (`voice_input_command`) and inserts its stdout transcript
into the composer.

The challenge: `Ctrl+K` is the Unix `kill-line` control sequence. On
most terminals it never reaches the TUI — the terminal emulator itself
intercepts it.

## Tested terminals

| Terminal                       | `Ctrl+K` reaches TUI? | Notes |
|-------------------------------|-----------------------|-------|
| macOS Terminal.app            | **No** — consumed as kill-line | Safe workaround: use Cmd+K if terminal supports it, or bind an alternative |
| iTerm2 (default profile)      | **No** — consumed as kill-line | iTerm2 also binds Cmd+K to "Clear Buffer to Cursor"; neither reaches TUI |
| Ghostty                       | **No** — consumed by default | Requires explicit `keybind = "Ctrl+K"` passthrough in ghostty.conf |
| kitty                         | **No** — consumed by default | `map ctrl+k send_text all \x0b` can pass it through |
| Warp                          | **No** — Warp consumes Ctrl+K for its own search | Warp blocks many chords; use `Ctrl+Shift+P` as palette alternative |
| VS Code Terminal (integrated) | **Yes** — forwarded in Raw mode | VS Code passes raw bytes to the embedded terminal in Raw mode |
| Windows Terminal              | **No** — consumed by ConPTY | Windows Terminal also binds Ctrl+K to search |
| Alacritty                     | **No** — consumed by default | Chord forwarding is possible via `key_bindings` config |
| WezTerm                       | **No** — consumed as kill-line | Not forwarded to the PTY by default |
| tmux (inside any terminal)    | **No** — consumed by tmux's kill-line | tmux intercepts Ctrl+K before forwarding; use a tmux passthrough binding |
| Linux Gnome Terminal          | **No** — consumed as kill-line | Standard terminal behaviour |
| Linux Konsole                 | **No** — consumed as kill-line | Standard terminal behaviour |
| Linux Terminator              | **No** — consumed as kill-line | Standard terminal behaviour |

## Recommended safe chords (forwarded in Raw mode on all tested terminals)

| Chord        | Status    | Notes |
|--------------|-----------|-------|
| `Ctrl+G`     | **Safe**  | Bell character; forwarded on all tested terminals |
| `Ctrl+]`     | **Safe**  | Group separator; forwarded |
| `Ctrl+\\`    | **Safe**  | SIGQUIT (Raw mode passes through) |
| `F2`         | **Safe**  | Function keys forwarded in all terminals |
| `F3`         | **Safe**  | Same |
| `Alt+Space`  | **Safe**  | No terminal binding |
| `Ctrl+L`     | **Safe**  | Clear-screen on most terminals but forwarded in Raw mode |

## STT helper setup checklist

Before shipping voice input to users, verify:

1. **Executable installed and on PATH** — `which <program>` returns the
   binary.
2. **Non-zero exit** — the helper exits 0 on success. Non-zero indicates
   recording failure (mic permission, network error, unsupported codec).
3. **Timeout** — default 60s, clamped to `1..600`. Long recordings may
   hit this. Test with a 60-second utterance.
4. **Empty transcript** — helper exits 0 but produces no stdout (e.g.
   silence detection abort). CodeWhale rejects this with a clear error.
5. **Permission failure** — on macOS, microphone access requires user
   approval in System Preferences → Privacy & Security → Microphone.
   Test that the helper fails gracefully when permission is denied.

## How to test a candidate chord

```bash
# In a terminal where you want to test:
# 1. Start CodeWhale
# 2. Open the command palette (Ctrl+K or whatever you bound)
# 3. If the palette opens, the chord reached the TUI
# 4. If the terminal does its own action (clear, search), the chord
#    was consumed before the TUI saw it
```

Automated detection: see `terminal_detection_tests` in
`crates/tui/src/tui/voice_input.rs`.

## Action items from #2116

- [x] Document terminal compatibility matrix
- [x] Define STT helper setup diagnostics
- [x] Add shortcut-detection tests
- [ ] Verify on macOS Terminal.app and iTerm2 (manual)
- [ ] Verify on Windows Terminal (manual)
- [ ] Verify on Linux terminal stacks (manual)
- [ ] Choose final default voice-input chord based on matrix
- [ ] Update KEYBINDINGS.md with voice-input-specific bindings
