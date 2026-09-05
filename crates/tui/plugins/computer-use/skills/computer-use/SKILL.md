---
name: computer-use
description: Full desktop control on macOS, Windows, Linux, and HarmonyOS — accessibility-first observation and actions with pixel fallback, screenshots, zoom, screen recording, and switching between registered computers as a default.
---

# Codewhale Computer Use

## Computers first

The plugin controls **computers**, not "the screen". `computer_list` shows the
registry; one computer is always **active**, and every tool acts on the active
computer unless given `computer`.

- Pass `computer: "<id>"` on any tool to act on (and stickily switch to) that
  computer. `computer_switch` changes the active computer without acting.
- `local` is the machine the plugin runs on. `ssh` computers run the bundled
  remote agent (pushed automatically at registration). `hdc` computers are
  HarmonyOS devices driven over hdc.
- Every receipt names the computer it happened on. Read it before continuing —
  never assume the action landed on the machine you meant.

## Core loop

Observe once, act once, then verify.

1. If readiness is unknown, call `request_access` once. It names missing
   permissions and missing tools per platform, and never pops dialogs.
2. `list_apps` shows running apps only. If the user names an app that is
   absent, call `open_application` once with the original user-provided name,
   copied character-for-character — including case, spaces, punctuation, and
   suffixes such as `app` or `.exe`. Do not translate, localize, normalize,
   shorten, or retry with guesses.
3. `get_app_state` returns a bounded accessibility tree (macOS AX / Windows
   UIA / Linux AT-SPI / HarmonyOS uitest) with element indices and a
   `state_id`. Start here, without a screenshot.
4. If the tree contains the target, act on the element: `perform_action`
   (AXPress/Invoke/click…), `set_value` for editable fields, element click.
   The element path is background-safe on macOS and UIA platforms.
5. Only when accessibility cannot express the target: `screenshot` (optionally
   `zoom` for small targets) and act with a coordinate target. Coordinates are
   pixels **in the latest returned raster** for that computer; the server maps
   them to screen points. After a new screenshot, old pixels are stale.
6. Verify with a fresh observation or a task oracle before claiming success.
   `action_sent: true` means it may already have happened — never replay.

## Choosing targets

- Element: `{"type":"element","state_id":"s-1","index":4}` — prefer this.
- Coordinate: `{"type":"coordinate","x":496,"y":331}` — pixels from the latest
  raster only; submit `x`/`y` unchanged, never transform them yourself.
- Never translate pixels into an element target; never invent `state_id`s.

## Raw input reality (read before clicking)

- macOS: element actions and `set_value` are background-safe. Raw pointer and
  keyboard events are posted at the target point and land on whatever is
  frontmost there — activate the app first for click/type flows, then act.
- Windows/Linux: raw input is foreground by nature; UIA/AT-SPI element actions
  are the precise path.
- HarmonyOS: `uitest` synthesizes touches; there is no hover or cursor.

## Keyboard

- macOS uses `cmd` (`cmd+c`), Linux/Windows use `ctrl` (`ctrl+c`).
- `key` for chords and repeats, `hold_key` for a duration, `type` for text.
- Prefer `set_value` on editable elements over typing.

## Recording

`recording_start` → work → `recording_stop` returns the finalized file path.
macOS uses `screencapture -v` (a receipt warning about Screen Recording
permission means the user must grant it once). Linux uses x11grab/wf-recorder,
Windows ffmpeg gdigrab, HarmonyOS snapshot-series (no native CLI recorder —
the receipt says so). `recording_status` / `recording_list` report bytes and
paths. Screenshots land in the same directory.

## Safety

- `stop_computer_control` is the kill switch; after it, actions fail closed
  for the session. Do not continue after it or after a denied permission.
- Never retry a refused action unchanged. Re-observe, choose a fresh target.
- If a permission is explicitly denied, tell the user which permission in
  which Settings pane, and end the turn. Do not promise later retries.

## Recipes

- **Screenshot** — optionally a computer id, display index, or `[x,y,w,h]`
  region; call `screenshot`; report path, size, computer/display. Black or
  empty capture means the host lacks Screen Recording permission (macOS):
  say so and stop.
- **Record** — `recording_start` (parse computer id, fps, display, duration
  or "record for 30s" → `durationSec` on macOS), then report id, path, mode.
  To stop, find the running id via `recording_list` and call `recording_stop`.
- **Switch computers** — `computer_list`; if asked to add: ssh `user@host`
  (agent is pushed automatically) or `hdc [target]` for a HarmonyOS device;
  otherwise show the registry and remind that any tool accepts `computer`.
- **Status** — `computer_list`, then `request_access` per computer; call out
  anything that will fail closed with the exact install hint from the receipt.
