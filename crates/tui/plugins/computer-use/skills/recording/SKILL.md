---
name: recording
description: Capture screen recordings and screenshots on any registered computer (macOS, Windows, Linux, HarmonyOS) and manage the recording library.
---

# Recording and screenshots

1. Pick the computer (`computer_list`, or pass `computer` — it switches).
2. `recording_start` with optional `display` (macOS/Linux), `region` (Linux/
   Windows), `fps` (Linux/Windows), `durationSec` (macOS auto-stop), and
   `intervalMs` (HarmonyOS snapshot cadence).
3. Do the work (or let the user do it).
4. `recording_stop` with the returned `id` → finalized file path + bytes.
5. `recording_list` shows everything saved; `recording_status` checks one.

Platform truths:

- **macOS**: `screencapture -v` per display; stop finalizes a .mov and, when
  ffmpeg is installed, a stream-copied .mp4 alongside. If a recording dies
  instantly, the terminal host is missing Screen Recording permission — tell
  the user, do not retry.
- **Windows**: ffmpeg gdigrab of the desktop or a region.
- **Linux X11**: ffmpeg x11grab (honors $DISPLAY, region, fps). Wayland:
  wf-recorder (per-output via CU_WAYLAND_OUTPUT).
- **HarmonyOS**: no native CLI recorder; the backend captures
  `snapshot_display` frames at `intervalMs` and muxes with ffmpeg on stop.
  The receipt labels the mode `snapshot-series` — never call it real-time.

Screenshots: `screenshot` returns the saved path and raster geometry; `zoom`
crops the latest raster when a target is too small to read.
