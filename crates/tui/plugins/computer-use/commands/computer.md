---
description: Drive a computer's screen, mouse, and keyboard
usage: /computer [status|look|computers]
---

$ARGUMENTS

- With no arguments or `status`: report whether computer use is usable on
  the active computer — server reachable, platform, screen size, and which
  permissions or platform tools are missing (`request_access`) — without
  taking any action.
- With `look`: take one screenshot of the active computer and describe what
  is on screen.
- With `computers`: list registered computers (`computer_list`) and say which
  one is active; every tool also takes `computer` to switch.
- Anything else (clicking, typing, operating apps, recording) goes through
  the computer-use skill's observe-act-verify loop with per-action approval.
  A denied permission or a missing tool fails closed and is reported, never
  retried blindly.
