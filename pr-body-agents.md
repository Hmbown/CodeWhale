## Summary

Auto-collapse completed sub-agents in the Agents sidebar panel. Non-running agents now show only a single line (label), freeing vertical space for active agents.

## Problem

Completed/failed/interrupted/cancelled sub-agents each occupied **2 lines** in the sidebar (label + detail line), wasting space that could be used for running agents or other content. With many agents, the sidebar became unnecessarily crowded.

## Solution

In `subagent_panel_lines()`, check the agent status before rendering the detail line. If the agent is not running (i.e. completed, failed, interrupted, cancelled), skip the detail line entirely and only render the single-line label.

**Before:**
```
✓ explore foo         ← 2 lines per agent
  abc123 · 3 steps · 12.3s
✗ build failed        ← 2 lines per agent
  def456 · 7 steps · 45.6s
● analysis running    ← 2 lines per agent
  ghi789 · 2 steps · 5.1s · parsing output...
```

**After:**
```
✓ explore foo         ← 1 line (collapsed)
✗ build failed        ← 1 line (collapsed)
● analysis running    ← 2 lines (expanded: label + detail)
  ghi789 · 2 steps · 5.1s · parsing output...
```

## File changed

`crates/tui/src/tui/sidebar.rs` — 5 lines added: `is_completed` check + early `continue` before the detail line.
