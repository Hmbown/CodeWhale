---
execute: .codewhale/skills/surf/scripts/surf.sh
description: "🌊 Ride the CodeWhale wave. Updates, builds, and tests the testbed."
---

# 🌊 Surf — Ride the CodeWhale Wave

This command runs the deterministic core of the Surf suite. It checks the current environment, updates the testbed if possible, and outputs a receipt.

## Behavior

| State | Action |
|---|---|
| **Empty directory** | Tells you to run `/surf setup` |
| **Clean testbed** | Pulls, builds, tests, writes receipt |
| **Dirty testbed** | Stops with a warning |
| **Unknown repo** | Stops with guidance |

## Sub-Commands

| Command | Action |
|---|---|
| `/surf` | Runs the main orchestrator |
| `/surf setup` | Clones the repo and initializes a testbed |

## Example

```text
/surf
🌊 Checking the wave...
🌊 Wave is clean. Riding...
...
✅ Surf complete.
📄 Receipt written: receipts/latest_receipt.json
```
