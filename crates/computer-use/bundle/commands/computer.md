---
name: computer
description: Operate the screen or an attached phone with the computer-use tools (vision model)
usage: /computer <task>
arguments: <task>
argument-hint: what to do on the screen, e.g. "open Safari and search for the weather"
---
Use the computer-use plugin to do this on the screen: $ARGUMENTS

Procedure:
1. Load the `computer-use:computer-use` skill and follow it.
2. If the active model cannot see images (computer_screenshot returns no picture you can read), delegate the task to the `computer-operator` agent profile with the `agent` tool and relay its report. Otherwise call the `computer_*` tools directly.
3. Start with `computer_info`, then `computer_screenshot`. Act one step at a time and verify each step from the screenshot the action returns.
4. Never type credentials, payment details, or confirm destructive dialogs without asking first. When done, summarize what changed on screen.
