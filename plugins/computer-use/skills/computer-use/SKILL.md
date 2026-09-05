---
name: computer-use
description: See the screen and operate it. Use when the user asks to click, type, or check something visually on this machine.
---

# Computer use

You can see this machine's screen and operate its mouse and keyboard
through the `computer` MCP server. Work in a tight look-act-verify
loop and never act blind.

## The loop

1. **Look**: call `mcp_computer_screenshot` first. The image arrives as
   a vision block; read positions from it, never guess coordinates.
2. **Act**: exactly one action per turn — `mcp_computer_click`,
   `mcp_computer_type_text`, `mcp_computer_press_key`, or
   `mcp_computer_scroll`. Coordinates are pixels in the latest
   screenshot from the top-left, matching `mcp_computer_screen_size`;
   the server scales them to the display.
3. **Verify**: screenshot again after every action that should change
   something visible. If the screen did not change as expected, stop
   and report instead of retrying harder.

## Safety rules

- Every action tool asks for approval unless the session runs in
  Always-approve posture. Say what you are about to click or type
  before the approval prompt appears.
- Never type passwords, secrets, or one-time codes. If a login screen
  appears, stop and hand control back to the user.
- Never dismiss system permission dialogs (Accessibility, Screen
  Recording) yourself. If the server reports a missing permission,
  tell the user exactly which toggle to flip and stop.
- Prefer keyboard over mouse (Tab, arrows, Enter) — it is precise and
  does not depend on pixel coordinates.
- If a tool reports `unsupported_platform` or `missing_helper`, stop
  and relay the server's message verbatim.
