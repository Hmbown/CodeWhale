---
description: Drive this machine's screen, mouse, and keyboard
usage: /computer [status|look]
---

$ARGUMENTS

- With no arguments or `status`: report whether the computer-use
  plugin is usable here — server reachable, platform supported,
  screen size — without taking any action.
- With `look`: take one screenshot and describe what is on screen.
- Anything else (clicking, typing, operating apps) goes through the
  computer-use skill's look-act-verify loop with per-action approval.
