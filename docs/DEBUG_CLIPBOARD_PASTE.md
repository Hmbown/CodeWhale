# Diagnosing Clipboard Paste Issues

This document covers how to diagnose the context-menu paste and image paste
problems where **Ctrl+V works but right-click → Paste does nothing**.

## Why Ctrl+V works but the context menu doesn't

DeepSeek TUI has two completely separate paste paths:

| Path | Trigger | Mechanism |
|------|---------|-----------|
| **Bracketed paste** | Ctrl+V (most terminals) | Terminal delivers clipboard content directly as an `Event::Paste`. Bypasses the clipboard API entirely. |
| **Clipboard API** | Right-click → Paste, image paste | Calls `arboard` (pure-Rust clipboard) with `xclip` fallback on Linux. If both fail, paste silently does nothing. |

When Ctrl+V works but the context menu doesn't, the clipboard API path is
failing — most commonly because `arboard` cannot connect to your display
server (X11/Wayland).

## Step 1 — Run the doctor

After rebuilding with the diagnostic changes:

```
deepseek doctor
```

Scroll to the **Clipboard:** section near the bottom. You'll see one of:

- `✓ arboard clipboard available (context-menu paste should work)` — the API path is healthy.
- `✗ arboard clipboard unavailable: <error>` — this is the root cause. On Linux it will also check for the `xclip` fallback.

## Step 2 — Check the log file

After attempting a context-menu paste, check the TUI log:

```bash
cat ~/.deepseek/logs/tui-$(date +%Y-%m-%d).log | grep clipboard
```

You'll see entries like:

```
WARN clipboard: arboard clipboard unavailable — paste from context menu will not work
DEBUG clipboard: arboard unavailable (init failed) — skipping arboard path
DEBUG clipboard: trying xclip fallback
DEBUG clipboard: xclip fallback returned nothing
WARN clipboard: all read paths failed — no text, no image, no xclip
```

Each line tells you exactly which step failed.

## Step 3 — Run with debug logging

For maximum detail, set the log level to `debug` before launching:

```bash
RUST_LOG=debug deepseek
```

Then attempt a paste from the context menu and inspect the log again:

```bash
grep clipboard ~/.deepseek/logs/tui-$(date +%Y-%m-%d).log
```

This will show every arboard text/image read attempt, their error details,
and every xclip fallback attempt.

## Common fixes by platform

### Linux

Most clipboard failures on Linux come from missing X11/Wayland integration:

1. **Install xclip** (provides a fallback when arboard fails):
   ```bash
   sudo apt install xclip        # Debian/Ubuntu
   sudo dnf install xclip        # Fedora
   sudo pacman -S xclip          # Arch
   ```

2. **Wayland users**: arboard uses the `wl-clipboard` backend via `wlr-data-control`.
   Make sure your compositor supports it, or install `wl-clipboard`:
   ```bash
   sudo apt install wl-clipboard
   ```

3. **SSH / headless**: If you're running over SSH without X forwarding, neither
   arboard nor xclip can reach the clipboard. Ctrl+V will still work because
   the terminal handles bracketed paste locally.

4. **Docker / container**: Mount the X11 socket:
   ```bash
   docker run -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY=$DISPLAY ...
   ```

### macOS

arboard uses the native `NSPasteboard` API on macOS and rarely fails. If the
doctor reports an error, check that the terminal has Accessibility permissions
(System Settings → Privacy & Security → Accessibility).

### Windows

arboard uses the Win32 clipboard API. If it fails, check that no other
application has the clipboard locked (some clipboard managers hold exclusive
locks).

## What the status bar tells you

After the diagnostic changes, attempting a context-menu paste when the
clipboard is unavailable will show this in the status bar:

```
Clipboard empty or unavailable
```

If you see this, proceed to the doctor and log file steps above.

## How to verify the fix

1. Run `deepseek doctor` — the Clipboard section should show `✓ arboard clipboard available`.
2. Copy some text to your system clipboard.
3. Launch the TUI, right-click anywhere in the transcript area, and click **Paste**.
4. The text should appear in the composer.
5. Check the log — you should see:
   ```
   DEBUG clipboard: arboard returned text
   ```
   instead of the failure chain.
