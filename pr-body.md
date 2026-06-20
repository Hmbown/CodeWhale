## Summary

Add real-time incremental output display for shell execution commands. The TUI now displays shell output **while the command is still running**, instead of hiding all output until completion.

## Changes

- **`ExecCell`**: Added `live_output: Option<String>` field to store incremental output during execution
- **`history.rs`**: Modified render logic to show `live_output` when `output` is not yet available (priority: final output > live output > hints)
- **`app.rs`**: Added `poll_shell_progress()` method that polls `ShellManager` during idle frames and updates matching `ExecCell` entries
- **`ui.rs`**: Wired `poll_shell_progress()` into the event loop after `tick_quit_armed()`
- **All ExecCell construction sites**: Added `live_output: None` to 8 files

## How it works

1. During idle frames, the TUI polls `ShellManager.list_jobs()` for running shell processes
2. Running exec cells are matched to shell jobs by command prefix
3. Live output (tail of stdout/stderr) is written to `ExecCell.live_output`
4. The renderer displays it in the transcript immediately

## Files changed
```
crates/tui/src/tui/active_cell.rs  |  1 +
crates/tui/src/tui/app.rs          | 69 +++++++++++++++++++++++++++++
crates/tui/src/tui/history.rs      | 11 ++++
crates/tui/src/tui/sidebar.rs      |  5 ++
crates/tui/src/tui/tool_routing.rs |  2 +
crates/tui/src/tui/transcript.rs   |  1 +
crates/tui/src/tui/ui.rs           |  2 +
crates/tui/src/tui/ui/tests.rs     |  3 +
```
