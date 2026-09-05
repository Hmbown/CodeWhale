# computer-use

Codewhale Computer Use — the `computer-use` plugin. One tool surface, four
platforms, and **switching between registered computers as a default**: every
tool accepts `computer`, and using a computer id sticks until you switch.

The bundle is an Agent Plugins v1 package (`plugin.json` + sibling `mcp.json`,
`commands/`, `skills/`): the Codewhale Engine discovers, reviews, installs, and
runs it. Nothing here writes to your Codewhale configuration.

| | |
|---|---|
| Platforms | macOS, Windows, Linux (X11 + Wayland), HarmonyOS (hdc devices) |
| Hosts | macOS, Windows, Linux (`when.os`); HarmonyOS is a target device, not a host |
| Transports | local process, ssh + bundled remote agent, hdc |
| Runtime deps | none (Node ≥ 20; platform tools probed at call time) |
| Tools | 38: observe, pointer, keyboard/text, semantic, clipboard, recording, computer registry |

## Frontier ability set

- **Observe & resolve** — `list_apps`, `list_windows`, `list_displays`,
  `switch_display`, `get_app_state` (accessibility/UIA/uitest tree with
  element indices + `state_id`), `screenshot` (display/region, raster-bound
  coordinates), `zoom` (close-up crop of the last raster), `cursor_position`,
  `open_application` (exact-name rule), `request_access` (fail-closed
  permission/capability probe).
- **Pointer** — left/double/triple/right/middle click, move, drag,
  down/up, scroll (4 directions).
- **Keyboard & text** — `type` (unicode), `key` (chords + repeat),
  `hold_key`, `set_value` (semantic, background-safe), `select_text`,
  `perform_action` (element's own actions: AXPress / UIA Invoke / AT-SPI / uitest).
- **Recording** — `recording_start/stop/status/list` (see below).
- **Computers** — `computer_list`, `computer_switch`, `computer_register`
  (ssh agent auto-push), `computer_remove`.
- **Safety** — `stop_computer_control` kill switch; permission probes that
  name the missing grant; receipts on every call naming the computer it
  happened on.

## Requirements

Tools and permissions are probed at call time; `request_access` reports what is
missing and every capability **fails closed naming the missing tool or
permission** — it never guesses and never half-acts.

- **macOS** — Accessibility + Screen Recording permission for the terminal
  app that hosts the Engine (System Settings → Privacy & Security).
  python3+pyobjc or cliclick improves cursor reads. ffmpeg optional (mp4 remux).
- **Windows** — PowerShell (built in); ffmpeg for recording.
- **Linux** — X11: xdotool, wmctrl, scrot or imagemagick, xclip; Wayland:
  grim, wtype, ydotool+ydotoold, wl-clipboard, wf-recorder; python3-pyatspi
  for the accessibility tree; ffmpeg for recording on X11.
- **HarmonyOS** — `hdc` on PATH with the device connected
  (`hdc list targets`); ffmpeg on the host for snapshot-series recordings.

## How the four platforms map

| Ability | macOS | Windows | Linux | HarmonyOS |
|---|---|---|---|---|
| Accessibility tree | AX via System Events (JXA) | UIAutomation | AT-SPI (pyatspi) | `uitest dumpLayout` |
| Raw input | CGEvent (JXA bridge) | user32 SendInput/mouse_event (PowerShell) | xdotool (X11) / ydotool+wtype (Wayland) | `uitest uiInput` |
| Screenshots | `screencapture` | .NET CopyFromScreen | scrot/import (X11), grim (Wayland) | `snapshot_display` |
| Recording | `screencapture -v` → .mov, ffmpeg remux to .mp4 | ffmpeg gdigrab | ffmpeg x11grab / wf-recorder | snapshot-series + ffmpeg mux |
| Clipboard | pbcopy/pbpaste | Get/Set-Clipboard | xclip/xsel, wl-clipboard | fail-closed (not exposed by hdc) |

## Remote computers (ssh)

```json
computer_register { "computer": "winbox", "transport": "ssh", "host": "winbox.lan", "user": "me" }
```

Registration pushes the self-contained agent (`agent.mjs` + `src/`) to
`~/.codewhale-cu/agent/` on the remote over scp, probes the remote platform
through it, and pins the result. Remote calls run
`node agent.mjs <base64 json>` — one JSON receipt line back. Only an
allow-listed tool set executes remotely; arguments travel as data, never as
shell. Requires publickey ssh (BatchMode) and Node ≥ 20 on the remote.

## HarmonyOS computers

```json
computer_register { "computer": "pad", "transport": "hdc" }
```

Drives the device over `hdc shell uitest ...` and `snapshot_display`. Element
targets come from `dumpLayout`; input is touch-synthesis (click / swipe /
inputText / keyEvent). Recording is honestly labeled `snapshot-series`
(frame captures muxed on stop) because HarmonyOS exposes no CLI screen
recorder.

## Development

The bundle lives at `crates/tui/plugins/computer-use` in the Codewhale repository and has
no dependencies to install; run its suites from that directory.

```bash
npm test          # unit + protocol tests (no GUI input performed)
npm run smoke     # live end-to-end against this machine (isolated state dirs,
                  # no clicks/typing into your session, no clipboard access)
```

Smoke receipts land in `receipts/` with per-check pass/fail and artifact
paths. Proven levels are separated: local live (this Mac: darwin) > mocked
transport (ssh protocol, harmony backend logic) > code-complete (win32/linux
paths, implemented to their documented tool interfaces but only verifiable on
those platforms).
